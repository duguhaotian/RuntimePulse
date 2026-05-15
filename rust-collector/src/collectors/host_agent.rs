//! Host-agent runtime.
//!
//! Runs host collectors in one process and decouples collection from outlet
//! HTTP calls with a bounded queue and a batch sender.

use chrono::Utc;
use reqwest::blocking::Client;
use serde_json::json;
use std::env;
use std::sync::mpsc::{self, RecvTimeoutError, SyncSender, TrySendError};
use std::thread;
use std::time::{Duration, Instant};

use crate::collectors::core::config::CollectorConfig;
use crate::collectors::core::error::{CollectorError, Result};
use crate::collectors::core::model::PluginOutput;
use crate::collectors::core::plugin::CollectorPlugin;
use crate::collectors::outlet::sender::send_local_report;
use crate::collectors::sources::image::download::output_from_event as image_output_from_event;
use crate::collectors::sources::node::cgroupfs::CgroupfsPlugin;
use crate::collectors::sources::node::procfs::ProcfsPlugin;
use crate::collectors::sources::runtime::docker::events::{stream_docker_events, DockerEvent};
use crate::collectors::sources::runtime::docker::inventory::collect_docker_inventory;
use crate::collectors::sources::runtime::docker::lifecycle::output_from_event as lifecycle_output_from_event;
use crate::collectors::sources::sandbox::cgroupfs::DockerSandboxCgroupfsPlugin;

const DEFAULT_QUEUE_CAPACITY: usize = 512;
const DEFAULT_FLUSH_MS: u64 = 1000;
const MAX_REPORTS_PER_BATCH: usize = 64;

pub fn run_host_agent(mut config: CollectorConfig) -> Result<()> {
    config.collection_scope = "host".to_string();

    let queue_capacity = env_usize("RUNTIMEPULSE_HOST_AGENT_QUEUE_CAPACITY")
        .unwrap_or(DEFAULT_QUEUE_CAPACITY)
        .max(1);
    let flush_interval = Duration::from_millis(
        env_u64("RUNTIMEPULSE_HOST_AGENT_FLUSH_MS")
            .unwrap_or(DEFAULT_FLUSH_MS)
            .max(100),
    );
    let (tx, rx) = mpsc::sync_channel::<PluginOutput>(queue_capacity);

    let sender_config = config.clone();
    let sender = thread::spawn(move || run_sender(sender_config, rx, flush_interval));

    let event_tx = tx.clone();
    let event_config = config.clone();
    let event_thread = thread::spawn(move || run_docker_event_worker(event_config, event_tx));

    let mut procfs = ProcfsPlugin::new();
    let mut cgroupfs = CgroupfsPlugin::new(config.cgroup_root.clone(), config.cgroup_max_entries);
    let mut docker_cgroupfs = DockerSandboxCgroupfsPlugin::new(config.cgroup_root.clone());

    loop {
        let started = Instant::now();
        collect_periodic(
            &config,
            &tx,
            &mut procfs,
            &mut cgroupfs,
            &mut docker_cgroupfs,
        );

        if config.once {
            drop(tx);
            let _ = sender.join().map_err(|_| CollectorError::Plugin {
                plugin: "host-agent".to_string(),
                message: "host-agent sender thread panicked".to_string(),
            })?;
            return event_thread.join().unwrap_or_else(|_| {
                Err(CollectorError::Plugin {
                    plugin: "host-agent".to_string(),
                    message: "host-agent docker event thread panicked".to_string(),
                })
            });
        }

        let elapsed = started.elapsed();
        if config.interval > elapsed {
            thread::sleep(config.interval - elapsed);
        }
    }
}

fn collect_periodic(
    config: &CollectorConfig,
    tx: &SyncSender<PluginOutput>,
    procfs: &mut ProcfsPlugin,
    cgroupfs: &mut CgroupfsPlugin,
    docker_cgroupfs: &mut DockerSandboxCgroupfsPlugin,
) {
    let now = Utc::now();
    enqueue_collected("host-procfs", tx, procfs.collect(now, config));
    enqueue_collected("host-cgroupfs", tx, cgroupfs.collect(now, config));
    enqueue_collected("host-docker", tx, collect_docker_inventory(now, config));
    enqueue_collected(
        "host-docker-cgroupfs",
        tx,
        docker_cgroupfs.collect(now, config),
    );
}

fn run_docker_event_worker(config: CollectorConfig, tx: SyncSender<PluginOutput>) -> Result<()> {
    if config.once {
        return Ok(());
    }

    stream_docker_events(&config, |event| {
        if let Some(output) = docker_event_output(event, &config)? {
            enqueue_report("docker-events", &tx, output);
        }
        Ok(())
    })
}

fn run_sender(
    config: CollectorConfig,
    rx: mpsc::Receiver<PluginOutput>,
    flush_interval: Duration,
) -> Result<()> {
    let client = Client::new();
    let mut pending = PluginOutput::default();
    let mut pending_count = 0_usize;

    loop {
        match rx.recv_timeout(flush_interval) {
            Ok(output) => {
                merge_plugin_output(&mut pending, output);
                pending_count += 1;
                while pending_count < MAX_REPORTS_PER_BATCH {
                    match rx.try_recv() {
                        Ok(output) => {
                            merge_plugin_output(&mut pending, output);
                            pending_count += 1;
                        }
                        Err(_) => break,
                    }
                }
                if pending_count >= MAX_REPORTS_PER_BATCH {
                    flush_pending(&client, &config, &mut pending, &mut pending_count)?;
                }
            }
            Err(RecvTimeoutError::Timeout) => {
                flush_pending(&client, &config, &mut pending, &mut pending_count)?;
            }
            Err(RecvTimeoutError::Disconnected) => {
                flush_pending(&client, &config, &mut pending, &mut pending_count)?;
                return Ok(());
            }
        }
    }
}

fn flush_pending(
    client: &Client,
    config: &CollectorConfig,
    pending: &mut PluginOutput,
    pending_count: &mut usize,
) -> Result<()> {
    if *pending_count == 0 || !has_plugin_output_payload(pending) {
        *pending = PluginOutput::default();
        *pending_count = 0;
        return Ok(());
    }

    match send_local_report(client, &config.local_report_url, pending) {
        Ok(()) => {
            println!(
                "{}",
                json!({
                    "level": "info",
                    "message": "host_agent_batch_report_accepted",
                    "url": config.local_report_url,
                    "reports": pending_count,
                    "sandboxes": pending.metadata.sandboxes.len(),
                    "images": pending.metadata.images.len(),
                    "metrics": pending.metrics.len(),
                    "events": pending.events.len(),
                })
            );
            *pending = PluginOutput::default();
            *pending_count = 0;
            Ok(())
        }
        Err(error) => {
            eprintln!(
                "{}",
                json!({
                    "level": "error",
                    "message": "host_agent_batch_report_failed",
                    "error": error.to_string(),
                    "queuedReports": pending_count,
                })
            );
            Ok(())
        }
    }
}

fn enqueue_collected(source: &str, tx: &SyncSender<PluginOutput>, result: Result<PluginOutput>) {
    match result {
        Ok(output) => enqueue_report(source, tx, output),
        Err(error) => eprintln!(
            "{}",
            json!({
                "level": "error",
                "message": "host_agent_collect_failed",
                "source": source,
                "error": error.to_string(),
            })
        ),
    }
}

fn enqueue_report(source: &str, tx: &SyncSender<PluginOutput>, output: PluginOutput) {
    if !has_plugin_output_payload(&output) {
        return;
    }

    match tx.try_send(output) {
        Ok(()) => {}
        Err(TrySendError::Full(_)) => eprintln!(
            "{}",
            json!({
                "level": "warning",
                "message": "host_agent_queue_full",
                "source": source,
            })
        ),
        Err(TrySendError::Disconnected(_)) => eprintln!(
            "{}",
            json!({
                "level": "error",
                "message": "host_agent_queue_disconnected",
                "source": source,
            })
        ),
    }
}

fn docker_event_output(
    event: DockerEvent,
    config: &CollectorConfig,
) -> Result<Option<PluginOutput>> {
    let mut combined = PluginOutput::default();
    if let Some(output) = lifecycle_output_from_event(event.clone(), config)? {
        merge_plugin_output(&mut combined, output);
    }
    if let Some(output) = image_output_from_event(event, config)? {
        merge_plugin_output(&mut combined, output);
    }

    if has_plugin_output_payload(&combined) {
        Ok(Some(combined))
    } else {
        Ok(None)
    }
}

fn merge_plugin_output(target: &mut PluginOutput, output: PluginOutput) {
    target.metadata.clusters.extend(output.metadata.clusters);
    target.metadata.nodes.extend(output.metadata.nodes);
    target.metadata.images.extend(output.metadata.images);
    target.metadata.sandboxes.extend(output.metadata.sandboxes);
    target.metrics.extend(output.metrics);
    target.events.extend(output.events);
    target.traces.extend(output.traces);
    target.profiles.extend(output.profiles);
}

fn has_plugin_output_payload(output: &PluginOutput) -> bool {
    !output.metadata.clusters.is_empty()
        || !output.metadata.nodes.is_empty()
        || !output.metadata.images.is_empty()
        || !output.metadata.sandboxes.is_empty()
        || !output.metrics.is_empty()
        || !output.events.is_empty()
        || !output.traces.is_empty()
        || !output.profiles.is_empty()
}

fn env_u64(name: &str) -> Option<u64> {
    env::var(name).ok()?.parse::<u64>().ok()
}

fn env_usize(name: &str) -> Option<usize> {
    env::var(name).ok()?.parse::<usize>().ok()
}
