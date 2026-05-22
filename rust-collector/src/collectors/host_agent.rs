//! Host-agent runtime.
//!
//! Runs host collectors in one process and decouples collection from outlet
//! HTTP calls with a bounded queue and a batch sender.

use chrono::{DateTime, SecondsFormat, Utc};
use reqwest::blocking::Client;
use serde_json::{json, Map};
use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, SyncSender, TrySendError};
use std::sync::Arc;
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant};

use crate::collectors::adapters::command::CommandPlugin;
use crate::collectors::adapters::http::HttpPlugin;
use crate::collectors::core::config::CollectorConfig;
use crate::collectors::core::error::{CollectorError, Result};
use crate::collectors::core::model::{Metadata, MetricSample, PluginOutput, TraceSpan};
use crate::collectors::core::plugin::CollectorPlugin;
use crate::collectors::core::report::node_metric;
use crate::collectors::outlet::sender::send_local_report;
use crate::collectors::sources::image::cache::ImageCachePlugin;
use crate::collectors::sources::image::download::output_from_event as image_output_from_event;
use crate::collectors::sources::kubernetes::metrics::KubernetesMetricsPlugin;
use crate::collectors::sources::node::cgroupfs::CgroupfsPlugin;
use crate::collectors::sources::node::procfs::ProcfsPlugin;
use crate::collectors::sources::node::psi::PsiPlugin;
use crate::collectors::sources::runtime::containerd::{
    collect_containerd_inventory, collect_containerd_task_targets, containerd_image_id_from_ref,
    output_from_runtime_event as containerd_output_from_event, runtime_type_from_containerd_name,
    sandbox_identity_from_containerd_event, stream_containerd_events, ContainerdEvent,
    ContainerdRuntimeEvent,
};
use crate::collectors::sources::runtime::docker::events::{
    merge_output, stream_docker_events, DockerEvent,
};
use crate::collectors::sources::runtime::docker::inventory::collect_docker_inventory;
use crate::collectors::sources::runtime::docker::lifecycle::output_from_event as lifecycle_output_from_event;
use crate::collectors::sources::runtime::kubelet::{
    output_from_cri_event, stream_cri_events, CriEvent,
};
use crate::collectors::sources::sandbox::cgroupfs::{
    ContainerdSandboxCgroupTarget, DockerSandboxCgroupfsPlugin,
};
use crate::collectors::sources::sandbox::manager::{
    active_docker_ids_snapshot, apply_lifecycle_output, docker_active_ids_from_inventory,
    ActiveDockerIds,
};

type ActiveContainerdTargets = Arc<Mutex<HashMap<String, ContainerdSandboxCgroupTarget>>>;

const DEFAULT_QUEUE_CAPACITY: usize = 512;
const DEFAULT_FLUSH_MS: u64 = 1000;
const DEFAULT_SPOOL_DIR: &str = "/tmp/runtimepulse/host-agent-spool";
const DEFAULT_SPOOL_MAX_FILES: usize = 256;
const MAX_REPORTS_PER_BATCH: usize = 64;
const DEFAULT_HOST_AGENT_SOURCES: &[&str] = &[
    "procfs",
    "psi",
    "cgroupfs",
    "docker-inventory",
    "docker-events",
    "docker-sandbox-cgroupfs",
    "image-cache",
];

#[derive(Clone, Debug)]
struct HostAgentSources {
    procfs: bool,
    psi: bool,
    cgroupfs: bool,
    docker_inventory: bool,
    containerd_inventory: bool,
    docker_events: bool,
    containerd_events: bool,
    kubelet_events: bool,
    kubernetes_metrics: bool,
    docker_sandbox_cgroupfs: bool,
    containerd_sandbox_cgroupfs: bool,
    image_cache: bool,
    command: bool,
    http: bool,
}

#[derive(Default)]
struct HostAgentStats {
    queue_depth: AtomicI64,
    enqueued_reports: AtomicU64,
    dropped_reports: AtomicU64,
    collect_errors: AtomicU64,
    sent_batches: AtomicU64,
    sent_reports: AtomicU64,
    failed_batches: AtomicU64,
    spooled_batches: AtomicU64,
    replayed_batches: AtomicU64,
    spool_files: AtomicU64,
    source_stats: Mutex<Vec<HostAgentSourceStat>>,
    event_stream_stats: Mutex<Vec<HostAgentEventStreamStat>>,
}

#[derive(Clone, Debug)]
struct HostAgentSourceStat {
    source: String,
    duration_ms: f64,
    success: bool,
    errors_total: u64,
}

#[derive(Clone, Debug)]
struct HostAgentEventStreamStat {
    stream: String,
    enabled: bool,
    running: bool,
    events_total: u64,
    errors_total: u64,
    restarts_total: u64,
    last_event_at: Option<DateTime<Utc>>,
}

pub fn run_host_agent(mut config: CollectorConfig) -> Result<()> {
    config.collection_scope = "host".to_string();
    let sources = HostAgentSources::from_env()?;
    log_host_agent_sources(&sources);

    let queue_capacity = env_usize("RUNTIMEPULSE_HOST_AGENT_QUEUE_CAPACITY")
        .unwrap_or(DEFAULT_QUEUE_CAPACITY)
        .max(1);
    let flush_interval = Duration::from_millis(
        env_u64("RUNTIMEPULSE_HOST_AGENT_FLUSH_MS")
            .unwrap_or(DEFAULT_FLUSH_MS)
            .max(100),
    );
    let spool = HostAgentSpool::from_env()?;
    let (tx, rx) = mpsc::sync_channel::<PluginOutput>(queue_capacity);
    let stats = Arc::new(HostAgentStats::default());
    stats
        .spool_files
        .store(spool.file_count().unwrap_or(0) as u64, Ordering::Relaxed);
    initialize_event_stream_stats(&stats, &sources);

    let sender_config = config.clone();
    let sender_stats = Arc::clone(&stats);
    let sender =
        thread::spawn(move || run_sender(sender_config, rx, flush_interval, sender_stats, spool));

    let active_docker_ids = if sources.docker_sandbox_cgroupfs {
        Some(docker_active_ids_from_inventory()?)
    } else {
        None
    };
    let active_containerd_targets = if sources.containerd_sandbox_cgroupfs {
        Some(active_containerd_targets_from_inventory()?)
    } else {
        None
    };

    let docker_event_thread = if !config.once && sources.needs_docker_event_stream() {
        let event_tx = tx.clone();
        let event_config = config.clone();
        let event_sources = sources.clone();
        let event_active_docker_ids = active_docker_ids.clone();
        let event_stats = Arc::clone(&stats);
        Some(thread::spawn(move || {
            run_docker_event_worker(
                event_config,
                event_tx,
                event_sources,
                event_active_docker_ids,
                event_stats,
            )
        }))
    } else {
        None
    };

    let containerd_event_thread = if !config.once && sources.containerd_events {
        let event_tx = tx.clone();
        let event_config = config.clone();
        let event_active_containerd_targets = active_containerd_targets.clone();
        let event_stats = Arc::clone(&stats);
        Some(thread::spawn(move || {
            run_containerd_event_worker(
                event_config,
                event_tx,
                event_active_containerd_targets,
                event_stats,
            )
        }))
    } else {
        None
    };

    let kubelet_event_thread = if !config.once && sources.kubelet_events {
        let event_tx = tx.clone();
        let event_config = config.clone();
        let event_stats = Arc::clone(&stats);
        Some(thread::spawn(move || {
            run_kubelet_event_worker(event_config, event_tx, event_stats)
        }))
    } else {
        None
    };

    let mut procfs = ProcfsPlugin::new();
    let mut psi = PsiPlugin::new();
    let mut cgroupfs = CgroupfsPlugin::new(config.cgroup_root.clone(), config.cgroup_max_entries);
    let mut docker_cgroupfs = DockerSandboxCgroupfsPlugin::new(config.cgroup_root.clone());
    let mut image_cache = ImageCachePlugin::new(config.image_cache_report_path.clone());
    let mut kubernetes_metrics = if sources.kubernetes_metrics {
        Some(KubernetesMetricsPlugin::from_env().ok_or_else(|| {
            CollectorError::Config(
                "kubernetes-metrics source requires RUNTIMEPULSE_PROMETHEUS_URL".to_string(),
            )
        })?)
    } else {
        None
    };
    let mut adapter_plugins = build_host_adapter_plugins(&config, &sources)?;

    loop {
        let started = Instant::now();
        collect_periodic(
            &config,
            &sources,
            &tx,
            &mut procfs,
            &mut psi,
            &mut cgroupfs,
            &mut docker_cgroupfs,
            &mut image_cache,
            kubernetes_metrics.as_mut(),
            &mut adapter_plugins,
            active_docker_ids.as_ref(),
            active_containerd_targets.as_ref(),
            &stats,
        );

        if config.once {
            drop(tx);
            let _ = sender.join().map_err(|_| CollectorError::Plugin {
                plugin: "host-agent".to_string(),
                message: "host-agent sender thread panicked".to_string(),
            })?;
            if let Some(event_thread) = docker_event_thread {
                event_thread.join().unwrap_or_else(|_| {
                    Err(CollectorError::Plugin {
                        plugin: "host-agent".to_string(),
                        message: "host-agent docker event thread panicked".to_string(),
                    })
                })?;
            }
            if let Some(event_thread) = containerd_event_thread {
                event_thread.join().unwrap_or_else(|_| {
                    Err(CollectorError::Plugin {
                        plugin: "host-agent".to_string(),
                        message: "host-agent containerd event thread panicked".to_string(),
                    })
                })?;
            }
            if let Some(event_thread) = kubelet_event_thread {
                event_thread.join().unwrap_or_else(|_| {
                    Err(CollectorError::Plugin {
                        plugin: "host-agent".to_string(),
                        message: "host-agent kubelet event thread panicked".to_string(),
                    })
                })?;
            }
            return Ok(());
        }

        let elapsed = started.elapsed();
        if config.interval > elapsed {
            thread::sleep(config.interval - elapsed);
        }
    }
}

fn collect_periodic(
    config: &CollectorConfig,
    sources: &HostAgentSources,
    tx: &SyncSender<PluginOutput>,
    procfs: &mut ProcfsPlugin,
    psi: &mut PsiPlugin,
    cgroupfs: &mut CgroupfsPlugin,
    docker_cgroupfs: &mut DockerSandboxCgroupfsPlugin,
    image_cache: &mut ImageCachePlugin,
    kubernetes_metrics: Option<&mut KubernetesMetricsPlugin>,
    adapter_plugins: &mut [Box<dyn CollectorPlugin>],
    active_docker_ids: Option<&ActiveDockerIds>,
    active_containerd_targets: Option<&ActiveContainerdTargets>,
    stats: &Arc<HostAgentStats>,
) {
    let now = Utc::now();
    if sources.procfs {
        collect_source("host-procfs", tx, stats, || procfs.collect(now, config));
    }
    if sources.psi {
        collect_source("host-psi", tx, stats, || psi.collect(now, config));
    }
    if sources.cgroupfs {
        collect_source("host-cgroupfs", tx, stats, || cgroupfs.collect(now, config));
    }
    if sources.docker_inventory {
        collect_source("host-docker", tx, stats, || {
            collect_docker_inventory(now, config)
        });
    }
    if sources.containerd_inventory {
        collect_source("host-containerd", tx, stats, || {
            collect_containerd_inventory(now, config)
        });
    }
    if sources.docker_sandbox_cgroupfs {
        collect_source("host-docker-cgroupfs", tx, stats, || {
            active_docker_ids
                .map(active_docker_ids_snapshot)
                .transpose()
                .and_then(|docker_ids| {
                    docker_cgroupfs.collect_for_docker_ids(
                        now,
                        config,
                        docker_ids.as_deref().unwrap_or_default(),
                    )
                })
        });
    }
    if sources.containerd_sandbox_cgroupfs {
        collect_source("host-containerd-cgroupfs", tx, stats, || {
            let targets = active_containerd_targets
                .map(active_containerd_targets_snapshot)
                .transpose()?
                .unwrap_or_default();
            docker_cgroupfs.collect_for_containerd_targets(now, config, &targets)
        });
    }
    if sources.image_cache {
        collect_source("host-image-cache", tx, stats, || {
            image_cache.collect(now, config)
        });
    }
    if let Some(kubernetes_metrics) = kubernetes_metrics {
        collect_source("host-kubernetes-metrics", tx, stats, || {
            kubernetes_metrics.collect(now, config)
        });
    }
    for plugin in adapter_plugins {
        let source = format!("host-adapter-{}", plugin.name());
        collect_source(&source, tx, stats, || plugin.collect(now, config));
    }
    enqueue_report(
        "host-agent-self",
        tx,
        host_agent_stats_output(now, config, stats),
        stats,
    );
}

fn collect_source<F>(
    source: &str,
    tx: &SyncSender<PluginOutput>,
    stats: &Arc<HostAgentStats>,
    collect: F,
) where
    F: FnOnce() -> Result<PluginOutput>,
{
    let started = Instant::now();
    let result = collect();
    let duration_ms = started.elapsed().as_secs_f64() * 1000.0;
    let success = result.is_ok();
    remember_source_stat(
        stats,
        HostAgentSourceStat {
            source: source.to_string(),
            duration_ms,
            success,
            errors_total: 0,
        },
    );
    enqueue_collected(source, tx, result, stats);
}

fn run_docker_event_worker(
    config: CollectorConfig,
    tx: SyncSender<PluginOutput>,
    sources: HostAgentSources,
    active_docker_ids: Option<ActiveDockerIds>,
    stats: Arc<HostAgentStats>,
) -> Result<()> {
    if config.once {
        return Ok(());
    }

    mark_event_stream_running(&stats, "docker-events", true);
    let mut startup_trace_tracker = DockerStartupTraceTracker::default();
    let result = stream_docker_events(&config, |event| {
        mark_event_stream_event(&stats, "docker-events", docker_event_timestamp(&event));
        let trace_output = startup_trace_tracker.output_from_event(&event, &config);
        if let Some(output) =
            docker_event_output(event, &config, &sources, active_docker_ids.as_ref())?
        {
            enqueue_report("docker-events", &tx, output, &stats);
        }
        if let Some(output) = trace_output {
            enqueue_report("docker-startup-trace", &tx, output, &stats);
        }
        Ok(())
    });
    mark_event_stream_running(&stats, "docker-events", false);
    if result.is_err() {
        mark_event_stream_error(&stats, "docker-events");
    }
    result
}

fn run_containerd_event_worker(
    config: CollectorConfig,
    tx: SyncSender<PluginOutput>,
    active_containerd_targets: Option<ActiveContainerdTargets>,
    stats: Arc<HostAgentStats>,
) -> Result<()> {
    if config.once {
        return Ok(());
    }

    mark_event_stream_running(&stats, "containerd-events", true);
    let mut startup_trace_tracker = ContainerdStartupTraceTracker::default();
    let result = stream_containerd_events(&config, |event| {
        mark_event_stream_event(
            &stats,
            "containerd-events",
            containerd_event_timestamp(&event),
        );
        let trace_output = startup_trace_tracker.output_from_event(&event, &config);
        if let Some(active) = active_containerd_targets.as_ref() {
            apply_containerd_runtime_event(active, &event);
        }
        if let Some(output) = containerd_output_from_event(event, &config)? {
            enqueue_report("containerd-events", &tx, output, &stats);
        }
        if let Some(output) = trace_output {
            enqueue_report("containerd-startup-trace", &tx, output, &stats);
        }
        Ok(())
    });
    mark_event_stream_running(&stats, "containerd-events", false);
    if result.is_err() {
        mark_event_stream_error(&stats, "containerd-events");
    }
    result
}

fn run_kubelet_event_worker(
    config: CollectorConfig,
    tx: SyncSender<PluginOutput>,
    stats: Arc<HostAgentStats>,
) -> Result<()> {
    mark_event_stream_running(&stats, "kubelet-events", true);
    let result = stream_cri_events(&config, |event| {
        mark_event_stream_event(&stats, "kubelet-events", cri_event_timestamp(&event));
        if let Some(output) = output_from_cri_event(event, &config) {
            enqueue_report("kubelet-events", &tx, output, &stats);
        }
        Ok(())
    });
    mark_event_stream_running(&stats, "kubelet-events", false);
    if result.is_err() {
        mark_event_stream_error(&stats, "kubelet-events");
    }
    result
}

fn run_sender(
    config: CollectorConfig,
    rx: mpsc::Receiver<PluginOutput>,
    flush_interval: Duration,
    stats: Arc<HostAgentStats>,
    spool: HostAgentSpool,
) -> Result<()> {
    let client = Client::new();
    let mut pending = PluginOutput::default();
    let mut pending_count = 0_usize;

    loop {
        match rx.recv_timeout(flush_interval) {
            Ok(output) => {
                decrement_queue_depth(&stats);
                merge_plugin_output(&mut pending, output);
                pending_count += 1;
                while pending_count < MAX_REPORTS_PER_BATCH {
                    match rx.try_recv() {
                        Ok(output) => {
                            decrement_queue_depth(&stats);
                            merge_plugin_output(&mut pending, output);
                            pending_count += 1;
                        }
                        Err(_) => break,
                    }
                }
                if pending_count >= MAX_REPORTS_PER_BATCH {
                    flush_pending(
                        &client,
                        &config,
                        &mut pending,
                        &mut pending_count,
                        &stats,
                        &spool,
                    )?;
                }
            }
            Err(RecvTimeoutError::Timeout) => {
                flush_pending(
                    &client,
                    &config,
                    &mut pending,
                    &mut pending_count,
                    &stats,
                    &spool,
                )?;
            }
            Err(RecvTimeoutError::Disconnected) => {
                flush_pending(
                    &client,
                    &config,
                    &mut pending,
                    &mut pending_count,
                    &stats,
                    &spool,
                )?;
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
    stats: &HostAgentStats,
    spool: &HostAgentSpool,
) -> Result<()> {
    replay_spooled_reports(client, config, stats, spool);

    if *pending_count == 0 || !has_plugin_output_payload(pending) {
        *pending = PluginOutput::default();
        *pending_count = 0;
        return Ok(());
    }

    match send_local_report(client, &config.local_report_url, pending) {
        Ok(()) => {
            let report_count = *pending_count;
            stats.sent_batches.fetch_add(1, Ordering::Relaxed);
            stats
                .sent_reports
                .fetch_add(report_count as u64, Ordering::Relaxed);
            println!(
                "{}",
                json!({
                    "level": "info",
                    "message": "host_agent_batch_report_accepted",
                    "url": config.local_report_url,
                    "reports": report_count,
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
            let report_count = *pending_count;
            stats.failed_batches.fetch_add(1, Ordering::Relaxed);
            match spool.write(pending, report_count) {
                Ok(Some(path)) => {
                    stats.spooled_batches.fetch_add(1, Ordering::Relaxed);
                    stats
                        .spool_files
                        .store(spool.file_count().unwrap_or(0) as u64, Ordering::Relaxed);
                    *pending = PluginOutput::default();
                    *pending_count = 0;
                    eprintln!(
                        "{}",
                        json!({
                            "level": "warning",
                            "message": "host_agent_batch_spooled",
                            "path": path.display().to_string(),
                        })
                    );
                }
                Ok(None) => {}
                Err(spool_error) => eprintln!(
                    "{}",
                    json!({
                        "level": "error",
                        "message": "host_agent_batch_spool_failed",
                        "error": spool_error.to_string(),
                    })
                ),
            }
            eprintln!(
                "{}",
                json!({
                    "level": "error",
                    "message": "host_agent_batch_report_failed",
                    "error": error.to_string(),
                    "queuedReports": report_count,
                })
            );
            Ok(())
        }
    }
}

fn replay_spooled_reports(
    client: &Client,
    config: &CollectorConfig,
    stats: &HostAgentStats,
    spool: &HostAgentSpool,
) {
    let paths = match spool.pending_paths() {
        Ok(paths) => paths,
        Err(error) => {
            eprintln!(
                "{}",
                json!({
                    "level": "error",
                    "message": "host_agent_spool_scan_failed",
                    "error": error.to_string(),
                })
            );
            return;
        }
    };

    stats
        .spool_files
        .store(paths.len() as u64, Ordering::Relaxed);

    for path in paths {
        let report = match spool.read(&path) {
            Ok(report) => report,
            Err(error) => {
                eprintln!(
                    "{}",
                    json!({
                        "level": "error",
                        "message": "host_agent_spool_read_failed",
                        "path": path.display().to_string(),
                        "error": error.to_string(),
                    })
                );
                continue;
            }
        };

        match send_local_report(client, &config.local_report_url, &report.output) {
            Ok(()) => {
                if let Err(error) = fs::remove_file(&path) {
                    eprintln!(
                        "{}",
                        json!({
                            "level": "error",
                            "message": "host_agent_spool_remove_failed",
                            "path": path.display().to_string(),
                            "error": error.to_string(),
                        })
                    );
                    continue;
                }
                stats.replayed_batches.fetch_add(1, Ordering::Relaxed);
                stats.sent_batches.fetch_add(1, Ordering::Relaxed);
                stats
                    .sent_reports
                    .fetch_add(report.report_count as u64, Ordering::Relaxed);
                stats
                    .spool_files
                    .store(spool.file_count().unwrap_or(0) as u64, Ordering::Relaxed);
                println!(
                    "{}",
                    json!({
                        "level": "info",
                        "message": "host_agent_spool_replayed",
                        "path": path.display().to_string(),
                        "reports": report.report_count,
                    })
                );
            }
            Err(error) => {
                stats.failed_batches.fetch_add(1, Ordering::Relaxed);
                eprintln!(
                    "{}",
                    json!({
                        "level": "error",
                        "message": "host_agent_spool_replay_failed",
                        "path": path.display().to_string(),
                        "error": error.to_string(),
                    })
                );
                break;
            }
        }
    }
}

fn enqueue_collected(
    source: &str,
    tx: &SyncSender<PluginOutput>,
    result: Result<PluginOutput>,
    stats: &Arc<HostAgentStats>,
) {
    match result {
        Ok(output) => enqueue_report(source, tx, output, stats),
        Err(error) => {
            stats.collect_errors.fetch_add(1, Ordering::Relaxed);
            eprintln!(
                "{}",
                json!({
                    "level": "error",
                    "message": "host_agent_collect_failed",
                    "source": source,
                    "error": error.to_string(),
                })
            )
        }
    }
}

fn enqueue_report(
    source: &str,
    tx: &SyncSender<PluginOutput>,
    output: PluginOutput,
    stats: &Arc<HostAgentStats>,
) {
    if !has_plugin_output_payload(&output) {
        return;
    }

    match tx.try_send(output) {
        Ok(()) => {
            stats.enqueued_reports.fetch_add(1, Ordering::Relaxed);
            stats.queue_depth.fetch_add(1, Ordering::Relaxed);
        }
        Err(TrySendError::Full(_)) => {
            stats.dropped_reports.fetch_add(1, Ordering::Relaxed);
            eprintln!(
                "{}",
                json!({
                    "level": "warning",
                    "message": "host_agent_queue_full",
                    "source": source,
                })
            )
        }
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

fn decrement_queue_depth(stats: &HostAgentStats) {
    let _ = stats
        .queue_depth
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
            Some(value.saturating_sub(1))
        });
}

fn remember_source_stat(stats: &HostAgentStats, next: HostAgentSourceStat) {
    let Ok(mut source_stats) = stats.source_stats.lock() else {
        return;
    };
    if let Some(existing) = source_stats
        .iter_mut()
        .find(|item| item.source == next.source)
    {
        existing.duration_ms = next.duration_ms;
        existing.success = next.success;
        if !next.success {
            existing.errors_total = existing.errors_total.saturating_add(1);
        }
    } else {
        source_stats.push(HostAgentSourceStat {
            errors_total: if next.success { 0 } else { 1 },
            ..next
        });
    }
}

fn source_stats_snapshot(stats: &HostAgentStats) -> Vec<HostAgentSourceStat> {
    stats
        .source_stats
        .lock()
        .map(|source_stats| source_stats.clone())
        .unwrap_or_default()
}

fn initialize_event_stream_stats(stats: &HostAgentStats, sources: &HostAgentSources) {
    let mut rows = stats
        .event_stream_stats
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    rows.clear();
    rows.push(HostAgentEventStreamStat {
        stream: "docker-events".to_string(),
        enabled: sources.needs_docker_event_stream(),
        running: false,
        events_total: 0,
        errors_total: 0,
        restarts_total: 0,
        last_event_at: None,
    });
    rows.push(HostAgentEventStreamStat {
        stream: "containerd-events".to_string(),
        enabled: sources.containerd_events,
        running: false,
        events_total: 0,
        errors_total: 0,
        restarts_total: 0,
        last_event_at: None,
    });
    rows.push(HostAgentEventStreamStat {
        stream: "kubelet-events".to_string(),
        enabled: sources.kubelet_events,
        running: false,
        events_total: 0,
        errors_total: 0,
        restarts_total: 0,
        last_event_at: None,
    });
}

fn mark_event_stream_running(stats: &HostAgentStats, stream: &str, running: bool) {
    update_event_stream_stat(stats, stream, |row| {
        if running && !row.running {
            row.restarts_total += 1;
        }
        row.running = running;
    });
}

fn mark_event_stream_event(stats: &HostAgentStats, stream: &str, timestamp: DateTime<Utc>) {
    update_event_stream_stat(stats, stream, |row| {
        row.events_total += 1;
        row.last_event_at = Some(timestamp);
    });
}

fn mark_event_stream_error(stats: &HostAgentStats, stream: &str) {
    update_event_stream_stat(stats, stream, |row| {
        row.errors_total += 1;
    });
}

fn update_event_stream_stat<F>(stats: &HostAgentStats, stream: &str, update: F)
where
    F: FnOnce(&mut HostAgentEventStreamStat),
{
    let mut rows = stats
        .event_stream_stats
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(row) = rows.iter_mut().find(|row| row.stream == stream) {
        update(row);
    }
}

fn event_stream_stats_snapshot(stats: &HostAgentStats) -> Vec<HostAgentEventStreamStat> {
    stats
        .event_stream_stats
        .lock()
        .map(|event_stream_stats| event_stream_stats.clone())
        .unwrap_or_default()
}

fn host_agent_stats_output(
    now: chrono::DateTime<Utc>,
    config: &CollectorConfig,
    stats: &HostAgentStats,
) -> PluginOutput {
    let timestamp = now.to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    let attributes = metric_attributes();
    let mut metrics = vec![
        self_metric(
            &timestamp,
            "host_agent.queue.depth",
            stats.queue_depth.load(Ordering::Relaxed) as f64,
            "count",
            &config.node_id,
            &attributes,
        ),
        self_metric(
            &timestamp,
            "host_agent.reports.enqueued_total",
            stats.enqueued_reports.load(Ordering::Relaxed) as f64,
            "count",
            &config.node_id,
            &attributes,
        ),
        self_metric(
            &timestamp,
            "host_agent.reports.dropped_total",
            stats.dropped_reports.load(Ordering::Relaxed) as f64,
            "count",
            &config.node_id,
            &attributes,
        ),
        self_metric(
            &timestamp,
            "host_agent.collect.errors_total",
            stats.collect_errors.load(Ordering::Relaxed) as f64,
            "count",
            &config.node_id,
            &attributes,
        ),
        self_metric(
            &timestamp,
            "host_agent.sender.batches_sent_total",
            stats.sent_batches.load(Ordering::Relaxed) as f64,
            "count",
            &config.node_id,
            &attributes,
        ),
        self_metric(
            &timestamp,
            "host_agent.sender.batches_failed_total",
            stats.failed_batches.load(Ordering::Relaxed) as f64,
            "count",
            &config.node_id,
            &attributes,
        ),
        self_metric(
            &timestamp,
            "host_agent.sender.reports_sent_total",
            stats.sent_reports.load(Ordering::Relaxed) as f64,
            "count",
            &config.node_id,
            &attributes,
        ),
        self_metric(
            &timestamp,
            "host_agent.spool.files",
            stats.spool_files.load(Ordering::Relaxed) as f64,
            "count",
            &config.node_id,
            &attributes,
        ),
        self_metric(
            &timestamp,
            "host_agent.spool.batches_spooled_total",
            stats.spooled_batches.load(Ordering::Relaxed) as f64,
            "count",
            &config.node_id,
            &attributes,
        ),
        self_metric(
            &timestamp,
            "host_agent.spool.batches_replayed_total",
            stats.replayed_batches.load(Ordering::Relaxed) as f64,
            "count",
            &config.node_id,
            &attributes,
        ),
    ];

    metrics.push(node_metric(
        &timestamp,
        "host_agent.up",
        1.0,
        "state",
        "collector",
        &config.node_id,
    ));

    for source in source_stats_snapshot(stats) {
        let mut attributes = metric_attributes();
        attributes.insert("collector.source".to_string(), json!(source.source));
        metrics.push(self_metric(
            &timestamp,
            "host_agent.source.collect.duration_ms",
            source.duration_ms,
            "ms",
            &config.node_id,
            &attributes,
        ));
        metrics.push(self_metric(
            &timestamp,
            "host_agent.source.collect.success",
            if source.success { 1.0 } else { 0.0 },
            "state",
            &config.node_id,
            &attributes,
        ));
        metrics.push(self_metric(
            &timestamp,
            "host_agent.source.collect.errors_total",
            source.errors_total as f64,
            "count",
            &config.node_id,
            &attributes,
        ));
    }

    for stream in event_stream_stats_snapshot(stats) {
        let mut attributes = metric_attributes();
        attributes.insert("collector.event_stream".to_string(), json!(stream.stream));
        metrics.push(self_metric(
            &timestamp,
            "host_agent.event_stream.enabled",
            if stream.enabled { 1.0 } else { 0.0 },
            "state",
            &config.node_id,
            &attributes,
        ));
        metrics.push(self_metric(
            &timestamp,
            "host_agent.event_stream.running",
            if stream.running { 1.0 } else { 0.0 },
            "state",
            &config.node_id,
            &attributes,
        ));
        metrics.push(self_metric(
            &timestamp,
            "host_agent.event_stream.events_total",
            stream.events_total as f64,
            "count",
            &config.node_id,
            &attributes,
        ));
        metrics.push(self_metric(
            &timestamp,
            "host_agent.event_stream.errors_total",
            stream.errors_total as f64,
            "count",
            &config.node_id,
            &attributes,
        ));
        metrics.push(self_metric(
            &timestamp,
            "host_agent.event_stream.restarts_total",
            stream.restarts_total as f64,
            "count",
            &config.node_id,
            &attributes,
        ));
        metrics.push(self_metric(
            &timestamp,
            "host_agent.event_stream.last_event_age_seconds",
            stream
                .last_event_at
                .map(|last_event_at| (now - last_event_at).num_seconds().max(0) as f64)
                .unwrap_or(0.0),
            "seconds",
            &config.node_id,
            &attributes,
        ));
    }

    PluginOutput {
        metadata: Metadata {
            clusters: vec![json!({
                "id": config.cluster_id,
                "name": config.cluster_id,
                "environment": "collector"
            })],
            nodes: vec![json!({
                "id": config.node_id,
                "clusterId": config.cluster_id,
                "name": config.node_id,
                "status": "ready",
                "labels": {
                    "collector": "runtimepulse-rust-collector",
                    "plugin": "host-agent",
                    "scope": config.collection_scope,
                }
            })],
            images: Vec::new(),
            sandboxes: Vec::new(),
        },
        metrics,
        events: Vec::new(),
        traces: Vec::new(),
        profiles: Vec::new(),
    }
}

fn metric_attributes() -> Map<String, serde_json::Value> {
    let mut attributes = Map::new();
    attributes.insert("plugin".to_string(), json!("host-agent"));
    attributes.insert("component".to_string(), json!("host-agent"));
    attributes
}

fn self_metric(
    timestamp: &str,
    name: &str,
    value: f64,
    unit: &str,
    node_id: &str,
    attributes: &Map<String, serde_json::Value>,
) -> MetricSample {
    let mut metric = node_metric(timestamp, name, value, unit, "collector", node_id);
    metric.attributes = Some(attributes.clone());
    metric
}

fn docker_event_output(
    event: DockerEvent,
    config: &CollectorConfig,
    sources: &HostAgentSources,
    active_docker_ids: Option<&ActiveDockerIds>,
) -> Result<Option<PluginOutput>> {
    let mut combined = PluginOutput::default();

    if sources.docker_events || active_docker_ids.is_some() {
        if let Some(output) = lifecycle_output_from_event(event.clone(), config)? {
            if let Some(active_docker_ids) = active_docker_ids {
                apply_lifecycle_output(active_docker_ids, &output)?;
            }
            if sources.docker_events {
                merge_plugin_output(&mut combined, output);
            }
        }
    }

    if sources.docker_events {
        if let Some(output) = image_output_from_event(event, config)? {
            merge_plugin_output(&mut combined, output);
        }
    }

    if has_plugin_output_payload(&combined) {
        Ok(Some(combined))
    } else {
        Ok(None)
    }
}

fn build_host_adapter_plugins(
    config: &CollectorConfig,
    sources: &HostAgentSources,
) -> Result<Vec<Box<dyn CollectorPlugin>>> {
    let mut plugins: Vec<Box<dyn CollectorPlugin>> = Vec::new();

    if sources.command {
        if config.command_plugins.is_empty() {
            return Err(CollectorError::Config(
                "host-agent command source requires RUNTIMEPULSE_COMMAND_PLUGIN_CMD or indexed RUNTIMEPULSE_COMMAND_PLUGIN_<N>_CMD".to_string(),
            ));
        }
        for command in &config.command_plugins {
            plugins.push(Box::new(CommandPlugin {
                name: command.name.clone(),
                command: command.command.clone(),
                timeout: command.timeout,
            }));
        }
    }

    if sources.http {
        if config.http_plugins.is_empty() {
            return Err(CollectorError::Config(
                "host-agent http source requires RUNTIMEPULSE_HTTP_PLUGIN_URL or indexed RUNTIMEPULSE_HTTP_PLUGIN_<N>_URL".to_string(),
            ));
        }
        for http in &config.http_plugins {
            plugins.push(Box::new(HttpPlugin {
                name: http.name.clone(),
                url: http.url.clone(),
                client: Client::new(),
                timeout: http.timeout,
            }));
        }
    }

    Ok(plugins)
}

fn merge_plugin_output(target: &mut PluginOutput, output: PluginOutput) {
    merge_output(target, output);
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

fn active_containerd_targets_from_inventory() -> Result<ActiveContainerdTargets> {
    let mut targets = HashMap::new();

    for task in collect_containerd_task_targets()? {
        let event = ContainerdEvent {
            namespace: task.namespace.clone(),
            action: "task_create".to_string(),
            container_id: task.container_id.clone(),
            image: Some(task.image_ref),
            runtime_name: Some(task.runtime_name),
            labels: task.labels,
            timestamp: Utc::now(),
            exit_status: None,
            pid: Some(task.pid),
            topic: "inventory/tasks".to_string(),
        };
        targets.insert(
            containerd_event_key(&event.namespace, &event.container_id),
            containerd_target_from_event(&event, u64::from(task.pid)),
        );
    }

    Ok(Arc::new(Mutex::new(targets)))
}

fn active_containerd_targets_snapshot(
    active_targets: &ActiveContainerdTargets,
) -> Result<Vec<ContainerdSandboxCgroupTarget>> {
    Ok(active_targets
        .lock()
        .map_err(|_| CollectorError::Plugin {
            plugin: "containerd-sandbox-cgroupfs".to_string(),
            message: "active containerd target set lock poisoned".to_string(),
        })?
        .values()
        .cloned()
        .collect())
}

fn apply_containerd_runtime_event(
    active_targets: &ActiveContainerdTargets,
    event: &ContainerdRuntimeEvent,
) {
    let ContainerdRuntimeEvent::Container(event) = event else {
        return;
    };
    let key = containerd_event_key(&event.namespace, &event.container_id);

    if matches!(
        event.action.as_str(),
        "exit" | "task_delete" | "delete" | "oom"
    ) {
        if let Ok(mut targets) = active_targets.lock() {
            targets.remove(&key);
        }
        return;
    }

    if !matches!(event.action.as_str(), "task_create" | "start" | "resume") {
        return;
    }

    let Some(pid) = event.pid.filter(|pid| *pid > 0).map(u64::from) else {
        return;
    };

    let target = containerd_target_from_event(event, pid);
    if let Ok(mut targets) = active_targets.lock() {
        targets.insert(key, target);
    }
}

fn containerd_target_from_event(
    event: &ContainerdEvent,
    pid: u64,
) -> ContainerdSandboxCgroupTarget {
    let identity = sandbox_identity_from_containerd_event(event);
    let image_ref = event
        .image
        .clone()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "containerd/unknown:latest".to_string());
    let runtime_version = event
        .runtime_name
        .clone()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "containerd".to_string());

    ContainerdSandboxCgroupTarget {
        sandbox_id: identity.sandbox_id,
        containerd_id: event.container_id.clone(),
        containerd_namespace: event.namespace.clone(),
        workload_name: identity.workload_name,
        namespace: identity.namespace,
        image_id: containerd_image_id_from_ref(&event.namespace, &image_ref),
        image_ref,
        runtime_type: runtime_type_from_containerd_name(&runtime_version),
        runtime_version,
        pid,
    }
}

#[derive(Default)]
struct DockerStartupTraceTracker {
    pending: HashMap<String, DockerCreateEvent>,
}

#[derive(Clone)]
struct DockerCreateEvent {
    image_ref: String,
    name: String,
    timestamp: DateTime<Utc>,
}

impl DockerStartupTraceTracker {
    fn output_from_event(
        &mut self,
        event: &DockerEvent,
        config: &CollectorConfig,
    ) -> Option<PluginOutput> {
        if event.event_type != "container" {
            return None;
        }

        let action = docker_event_action(event);
        let container_id = docker_event_container_id(event)?;

        match action {
            "create" => {
                self.pending.insert(
                    container_id,
                    DockerCreateEvent {
                        image_ref: docker_event_image_ref(event),
                        name: docker_event_container_name(event),
                        timestamp: docker_event_timestamp(event),
                    },
                );
                None
            }
            "start" | "restart" | "unpause" => {
                let started_at = docker_event_timestamp(event);
                let create = self.pending.remove(&container_id)?;
                Some(docker_startup_trace_output(
                    config,
                    &container_id,
                    &create,
                    started_at,
                ))
            }
            "die" | "kill" | "oom" | "destroy" => {
                self.pending.remove(&container_id);
                None
            }
            _ => None,
        }
    }
}

fn docker_startup_trace_output(
    config: &CollectorConfig,
    container_id: &str,
    create: &DockerCreateEvent,
    started_at: DateTime<Utc>,
) -> PluginOutput {
    let short_id = short_container_id(container_id);
    let sandbox_id = format!("docker-{short_id}");
    let start_time = create.timestamp;
    let end_time = if started_at > start_time {
        started_at
    } else {
        start_time + chrono::Duration::milliseconds(1)
    };
    let duration_ms = (end_time - start_time).num_milliseconds().max(1) as f64;

    let mut attributes = Map::new();
    attributes.insert("plugin".to_string(), json!("docker-startup-trace"));
    attributes.insert("docker.id".to_string(), json!(container_id));
    attributes.insert("docker.short_id".to_string(), json!(short_id));
    attributes.insert("docker.name".to_string(), json!(create.name));
    attributes.insert("image.ref".to_string(), json!(create.image_ref));
    attributes.insert("scope".to_string(), json!(config.collection_scope));

    PluginOutput {
        metadata: Metadata::default(),
        metrics: Vec::new(),
        events: Vec::new(),
        traces: vec![TraceSpan {
            trace_id: format!("docker-startup-{short_id}"),
            span_id: format!("docker-startup-{short_id}-container-startup"),
            span_name: "container.startup".to_string(),
            start_time: timestamp(start_time),
            end_time: timestamp(end_time),
            duration_ms,
            status: "ok".to_string(),
            attributes,
            sandbox_id: Some(sandbox_id),
            image_id: None,
            parent_span_id: None,
        }],
        profiles: Vec::new(),
    }
}

#[derive(Default)]
struct ContainerdStartupTraceTracker {
    pending: HashMap<String, ContainerdCreateEvent>,
}

#[derive(Clone)]
struct ContainerdCreateEvent {
    image_ref: String,
    labels: HashMap<String, String>,
    namespace: String,
    runtime_name: String,
    timestamp: DateTime<Utc>,
}

impl ContainerdStartupTraceTracker {
    fn output_from_event(
        &mut self,
        event: &ContainerdRuntimeEvent,
        config: &CollectorConfig,
    ) -> Option<PluginOutput> {
        let ContainerdRuntimeEvent::Container(event) = event else {
            return None;
        };

        let key = containerd_event_key(&event.namespace, &event.container_id);
        match event.action.as_str() {
            "create" | "task_create" => {
                let existing = self.pending.get(&key);
                let timestamp = existing
                    .map(|pending| pending.timestamp.min(event.timestamp))
                    .unwrap_or(event.timestamp);
                self.pending.insert(
                    key,
                    ContainerdCreateEvent {
                        image_ref: event
                            .image
                            .clone()
                            .filter(|value| !value.is_empty())
                            .or_else(|| existing.map(|pending| pending.image_ref.clone()))
                            .unwrap_or_else(|| "containerd/unknown:latest".to_string()),
                        labels: if event.labels.is_empty() {
                            existing
                                .map(|pending| pending.labels.clone())
                                .unwrap_or_default()
                        } else {
                            event.labels.clone()
                        },
                        namespace: event.namespace.clone(),
                        runtime_name: event
                            .runtime_name
                            .clone()
                            .filter(|value| !value.is_empty())
                            .or_else(|| existing.map(|pending| pending.runtime_name.clone()))
                            .unwrap_or_else(|| "containerd".to_string()),
                        timestamp,
                    },
                );
                None
            }
            "start" | "resume" => {
                let started_at = event.timestamp;
                let create = self.pending.remove(&key)?;
                Some(containerd_startup_trace_output(
                    config,
                    &event.container_id,
                    &create,
                    started_at,
                ))
            }
            "exit" | "task_delete" | "delete" | "oom" => {
                self.pending.remove(&key);
                None
            }
            _ => None,
        }
    }
}

fn containerd_startup_trace_output(
    config: &CollectorConfig,
    container_id: &str,
    create: &ContainerdCreateEvent,
    started_at: DateTime<Utc>,
) -> PluginOutput {
    let short_id = short_container_id(container_id);
    let namespace_id = sanitize_id(&create.namespace);
    let identity = sandbox_identity_from_containerd_event(&ContainerdEvent {
        namespace: create.namespace.clone(),
        action: "start".to_string(),
        container_id: container_id.to_string(),
        image: Some(create.image_ref.clone()),
        runtime_name: Some(create.runtime_name.clone()),
        labels: create.labels.clone(),
        timestamp: started_at,
        exit_status: None,
        pid: None,
        topic: "startup-trace".to_string(),
    });
    let sandbox_id = identity.sandbox_id;
    let start_time = create.timestamp;
    let end_time = if started_at > start_time {
        started_at
    } else {
        start_time + chrono::Duration::milliseconds(1)
    };
    let duration_ms = (end_time - start_time).num_milliseconds().max(1) as f64;

    let mut attributes = Map::new();
    attributes.insert("plugin".to_string(), json!("containerd-startup-trace"));
    attributes.insert("containerd.id".to_string(), json!(container_id));
    attributes.insert("containerd.short_id".to_string(), json!(short_id));
    attributes.insert("containerd.namespace".to_string(), json!(create.namespace));
    attributes.insert("containerd.runtime".to_string(), json!(create.runtime_name));
    attributes.insert("image.ref".to_string(), json!(create.image_ref));
    attributes.insert(
        "k8s.namespace".to_string(),
        json!(identity.kubernetes_namespace),
    );
    attributes.insert("k8s.pod".to_string(), json!(identity.pod_name));
    attributes.insert("k8s.container".to_string(), json!(identity.container_name));
    attributes.insert("k8s.pod_uid".to_string(), json!(identity.pod_uid));
    attributes.insert("scope".to_string(), json!(config.collection_scope));

    PluginOutput {
        metadata: Metadata::default(),
        metrics: Vec::new(),
        events: Vec::new(),
        traces: vec![TraceSpan {
            trace_id: format!("containerd-startup-{namespace_id}-{short_id}"),
            span_id: format!("containerd-startup-{namespace_id}-{short_id}-container-startup"),
            span_name: "container.startup".to_string(),
            start_time: timestamp(start_time),
            end_time: timestamp(end_time),
            duration_ms,
            status: "ok".to_string(),
            attributes,
            sandbox_id: Some(sandbox_id),
            image_id: None,
            parent_span_id: None,
        }],
        profiles: Vec::new(),
    }
}

fn containerd_event_key(namespace: &str, container_id: &str) -> String {
    format!("{namespace}/{container_id}")
}

fn docker_event_action(event: &DockerEvent) -> &str {
    if event.action.is_empty() {
        event.status.as_str()
    } else {
        event.action.as_str()
    }
}

fn docker_event_container_id(event: &DockerEvent) -> Option<String> {
    let id = if event.actor.id.is_empty() {
        &event.id
    } else {
        &event.actor.id
    };
    (!id.is_empty()).then(|| id.to_string())
}

fn docker_event_container_name(event: &DockerEvent) -> String {
    event
        .actor
        .attributes
        .get("name")
        .cloned()
        .filter(|value| !value.is_empty())
        .or_else(|| docker_event_container_id(event).map(|id| short_container_id(&id)))
        .unwrap_or_else(|| "docker-container".to_string())
}

fn docker_event_image_ref(event: &DockerEvent) -> String {
    event
        .actor
        .attributes
        .get("image")
        .cloned()
        .filter(|value| !value.is_empty())
        .or_else(|| (!event.image.is_empty()).then(|| event.image.clone()))
        .unwrap_or_else(|| "docker/unknown:latest".to_string())
}

fn docker_event_timestamp(event: &DockerEvent) -> DateTime<Utc> {
    if event.time_nano > 0 {
        let secs = event.time_nano / 1_000_000_000;
        let nanos = (event.time_nano % 1_000_000_000) as u32;
        if let Some(time) = DateTime::from_timestamp(secs, nanos) {
            return time;
        }
    }
    if event.time > 0 {
        if let Some(time) = DateTime::from_timestamp(event.time, 0) {
            return time;
        }
    }
    Utc::now()
}

fn containerd_event_timestamp(event: &ContainerdRuntimeEvent) -> DateTime<Utc> {
    match event {
        ContainerdRuntimeEvent::Container(event) => event.timestamp,
        ContainerdRuntimeEvent::Image(event) => event.timestamp,
    }
}

fn cri_event_timestamp(event: &CriEvent) -> DateTime<Utc> {
    if event.created_at > 1_000_000_000_000_000_000 {
        let secs = event.created_at / 1_000_000_000;
        let nanos = (event.created_at % 1_000_000_000) as u32;
        if let Some(time) = DateTime::from_timestamp(secs, nanos) {
            return time;
        }
    }
    if event.created_at > 1_000_000_000 {
        if let Some(time) = DateTime::from_timestamp(event.created_at, 0) {
            return time;
        }
    }
    Utc::now()
}

fn short_container_id(id: &str) -> String {
    id.chars().take(12).collect()
}

fn sanitize_id(value: &str) -> String {
    let sanitized = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .to_string();
    if sanitized.is_empty() {
        "unknown".to_string()
    } else {
        sanitized
    }
}

fn timestamp(time: DateTime<Utc>) -> String {
    time.to_rfc3339_opts(SecondsFormat::Millis, true)
}

fn env_u64(name: &str) -> Option<u64> {
    env::var(name).ok()?.parse::<u64>().ok()
}

fn env_usize(name: &str) -> Option<usize> {
    env::var(name).ok()?.parse::<usize>().ok()
}

#[derive(Clone, Debug)]
struct HostAgentSpool {
    dir: PathBuf,
    max_files: usize,
}

#[derive(serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct SpooledReport {
    spooled_at: String,
    report_count: usize,
    output: PluginOutput,
}

impl HostAgentSpool {
    fn from_env() -> Result<Self> {
        let dir = PathBuf::from(
            env::var("RUNTIMEPULSE_HOST_AGENT_SPOOL_DIR")
                .unwrap_or_else(|_| DEFAULT_SPOOL_DIR.to_string()),
        );
        let max_files = env_usize("RUNTIMEPULSE_HOST_AGENT_SPOOL_MAX_FILES")
            .unwrap_or(DEFAULT_SPOOL_MAX_FILES)
            .max(1);
        fs::create_dir_all(&dir)?;
        Ok(Self { dir, max_files })
    }

    fn write(&self, output: &PluginOutput, report_count: usize) -> Result<Option<PathBuf>> {
        if !has_plugin_output_payload(output) {
            return Ok(None);
        }

        self.prune()?;
        let spooled = SpooledReport {
            spooled_at: Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            report_count,
            output: clone_output(output)?,
        };
        let filename = format!(
            "{}-{}.json",
            Utc::now().timestamp_millis(),
            std::process::id()
        );
        let final_path = self.dir.join(filename);
        let tmp_path = final_path.with_extension("json.tmp");
        fs::write(&tmp_path, serde_json::to_vec(&spooled)?)?;
        fs::rename(&tmp_path, &final_path)?;
        Ok(Some(final_path))
    }

    fn read(&self, path: &PathBuf) -> Result<SpooledReport> {
        Ok(serde_json::from_slice(&fs::read(path)?)?)
    }

    fn pending_paths(&self) -> Result<Vec<PathBuf>> {
        let mut paths = fs::read_dir(&self.dir)?
            .filter_map(std::result::Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.extension().and_then(|value| value.to_str()) == Some("json"))
            .collect::<Vec<_>>();
        paths.sort();
        Ok(paths)
    }

    fn file_count(&self) -> Result<usize> {
        Ok(self.pending_paths()?.len())
    }

    fn prune(&self) -> Result<()> {
        let paths = self.pending_paths()?;
        if paths.len() < self.max_files {
            return Ok(());
        }

        let remove_count = paths.len() - self.max_files + 1;
        for path in paths.into_iter().take(remove_count) {
            fs::remove_file(path)?;
        }
        Ok(())
    }
}

fn clone_output(output: &PluginOutput) -> Result<PluginOutput> {
    Ok(serde_json::from_value(serde_json::to_value(output)?)?)
}

impl HostAgentSources {
    fn from_env() -> Result<Self> {
        let names = env::var("RUNTIMEPULSE_HOST_AGENT_SOURCES")
            .ok()
            .map(|value| parse_source_list(&value))
            .unwrap_or_else(|| {
                DEFAULT_HOST_AGENT_SOURCES
                    .iter()
                    .map(|item| item.to_string())
                    .collect()
            });

        let mut sources = Self {
            procfs: false,
            psi: false,
            cgroupfs: false,
            docker_inventory: false,
            containerd_inventory: false,
            docker_events: false,
            containerd_events: false,
            kubelet_events: false,
            kubernetes_metrics: false,
            docker_sandbox_cgroupfs: false,
            containerd_sandbox_cgroupfs: false,
            image_cache: false,
            command: false,
            http: false,
        };

        for name in names {
            match name.as_str() {
                "procfs" | "host-procfs" => sources.procfs = true,
                "psi" | "host-psi" => sources.psi = true,
                "cgroupfs" | "host-cgroupfs" => sources.cgroupfs = true,
                "docker" | "docker-inventory" | "host-docker" => sources.docker_inventory = true,
                "containerd" | "containerd-inventory" | "host-containerd" => {
                    sources.containerd_inventory = true;
                }
                "docker-events" | "host-docker-events" => sources.docker_events = true,
                "containerd-events" | "host-containerd-events" => sources.containerd_events = true,
                "kubelet-events" | "host-kubelet-events" | "cri-events" | "host-cri-events" => {
                    sources.kubelet_events = true;
                }
                "kubernetes-metrics"
                | "k8s-metrics"
                | "prometheus-metrics"
                | "host-kubernetes-metrics" => {
                    sources.kubernetes_metrics = true;
                }
                "docker-sandbox-cgroupfs" | "host-docker-cgroupfs" | "sandbox-cgroupfs" => {
                    sources.docker_sandbox_cgroupfs = true;
                }
                "containerd-sandbox-cgroupfs" | "host-containerd-cgroupfs" => {
                    sources.containerd_sandbox_cgroupfs = true;
                    sources.containerd_events = true;
                }
                "image-cache" | "host-image-cache" | "snapshotter-cache" => {
                    sources.image_cache = true;
                }
                "command" | "host-command" | "adapter-command" => sources.command = true,
                "http" | "host-http" | "adapter-http" => sources.http = true,
                other => {
                    return Err(CollectorError::Config(format!(
                        "unknown host-agent source: {other}"
                    )));
                }
            }
        }

        Ok(sources)
    }

    fn needs_docker_event_stream(&self) -> bool {
        self.docker_events || self.docker_sandbox_cgroupfs
    }

    fn enabled_names(&self) -> Vec<&'static str> {
        let mut names = Vec::new();
        if self.procfs {
            names.push("procfs");
        }
        if self.psi {
            names.push("psi");
        }
        if self.cgroupfs {
            names.push("cgroupfs");
        }
        if self.docker_inventory {
            names.push("docker-inventory");
        }
        if self.containerd_inventory {
            names.push("containerd-inventory");
        }
        if self.docker_events {
            names.push("docker-events");
        }
        if self.containerd_events {
            names.push("containerd-events");
        }
        if self.kubelet_events {
            names.push("kubelet-events");
        }
        if self.kubernetes_metrics {
            names.push("kubernetes-metrics");
        }
        if self.docker_sandbox_cgroupfs {
            names.push("docker-sandbox-cgroupfs");
        }
        if self.containerd_sandbox_cgroupfs {
            names.push("containerd-sandbox-cgroupfs");
        }
        if self.image_cache {
            names.push("image-cache");
        }
        if self.command {
            names.push("command");
        }
        if self.http {
            names.push("http");
        }
        names
    }
}

fn parse_source_list(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .filter(|item| *item != "none")
        .map(ToOwned::to_owned)
        .collect()
}

fn log_host_agent_sources(sources: &HostAgentSources) {
    println!(
        "{}",
        json!({
            "level": "info",
            "message": "host_agent_sources_enabled",
            "sources": sources.enabled_names(),
            "sandboxSampling": if sources.docker_sandbox_cgroupfs {
                "docker-active-set"
            } else {
                "disabled"
            },
        })
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn containerd_startup_trace_uses_kubernetes_sandbox_identity() {
        let config = CollectorConfig {
            ingest_url: "http://query-api/api/ingest/batch".to_string(),
            node_id: "node-a".to_string(),
            cluster_id: "cluster-a".to_string(),
            interval: Duration::from_secs(1),
            local_report_addr: "127.0.0.1:9091".to_string(),
            local_report_url: "http://127.0.0.1:9091/api/local/ingest".to_string(),
            collection_scope: "host".to_string(),
            once: true,
            cgroup_root: PathBuf::from("/sys/fs/cgroup"),
            cgroup_max_entries: 200,
            image_cache_report_path: None,
            plugins: Vec::new(),
            command_plugins: Vec::new(),
            http_plugins: Vec::new(),
        };

        let mut labels = HashMap::new();
        labels.insert(
            "io.kubernetes.pod.namespace".to_string(),
            "default".to_string(),
        );
        labels.insert(
            "io.kubernetes.pod.name".to_string(),
            "runtimepulse-demo".to_string(),
        );
        labels.insert(
            "io.kubernetes.container.name".to_string(),
            "app".to_string(),
        );
        labels.insert(
            "io.kubernetes.pod.uid".to_string(),
            "runtimepulse-demo-uid".to_string(),
        );

        let create = ContainerdCreateEvent {
            image_ref: "docker.io/library/nginx:latest".to_string(),
            labels,
            namespace: "k8s.io".to_string(),
            runtime_name: "io.containerd.runc.v2".to_string(),
            timestamp: Utc::now(),
        };

        let output =
            containerd_startup_trace_output(&config, "runtimepulse-ctrd-demo", &create, Utc::now());

        assert_eq!(output.traces.len(), 1);
        let span = &output.traces[0];
        assert_eq!(
            span.sandbox_id.as_deref(),
            Some("k8s-default-runtimepulse-demo-app")
        );
        assert_eq!(
            span.attributes
                .get("k8s.pod")
                .and_then(|value| value.as_str()),
            Some("runtimepulse-demo")
        );
    }
}
