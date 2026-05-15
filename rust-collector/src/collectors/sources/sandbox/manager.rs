//! Sandbox sampler managers.
//!
//! Managers reconcile already-running sandboxes at startup, keep periodic
//! sampler output flowing, and report runtime lifecycle events that update the
//! current sandbox set.

use chrono::Utc;
use reqwest::blocking::Client;
use serde_json::json;
use std::sync::mpsc;
use std::thread;
use std::time::Instant;

use crate::collectors::core::config::CollectorConfig;
use crate::collectors::core::error::{CollectorError, Result};
use crate::collectors::core::model::PluginOutput;
use crate::collectors::core::plugin::CollectorPlugin;
use crate::collectors::outlet::sender::send_local_report;
use crate::collectors::sources::runtime::docker::lifecycle::{
    collect_recent_docker_lifecycle, stream_docker_lifecycle,
};
use crate::collectors::sources::sandbox::cgroupfs::DockerSandboxCgroupfsPlugin;

pub fn run_docker_sandbox_agent(mut config: CollectorConfig) -> Result<()> {
    config.collection_scope = "host".to_string();

    let client = Client::new();
    let mut sampler = DockerSandboxCgroupfsPlugin::new(config.cgroup_root.clone());

    if config.once {
        collect_and_send_sandbox_snapshot(&client, &config, &mut sampler)?;
        collect_and_send_recent_lifecycle(&client, &config)?;
        return Ok(());
    }

    let (lifecycle_result_tx, lifecycle_result_rx) = mpsc::channel();
    let lifecycle_config = config.clone();
    let lifecycle_client = client.clone();
    thread::spawn(move || {
        let result = stream_docker_lifecycle(&lifecycle_config, |output| {
            send_local_report(
                &lifecycle_client,
                &lifecycle_config.local_report_url,
                &output,
            )?;
            log_lifecycle_report(&lifecycle_config, &output);
            Ok(())
        });
        let _ = lifecycle_result_tx.send(result);
    });

    loop {
        let started = Instant::now();

        if let Ok(result) = lifecycle_result_rx.try_recv() {
            return result.and_then(|_| {
                Err(CollectorError::Plugin {
                    plugin: "docker-sandbox-agent".to_string(),
                    message: "docker lifecycle stream exited".to_string(),
                })
            });
        }

        collect_and_send_sandbox_snapshot(&client, &config, &mut sampler)?;

        let elapsed = started.elapsed();
        if config.interval > elapsed {
            thread::sleep(config.interval - elapsed);
        }
    }
}

fn collect_and_send_sandbox_snapshot(
    client: &Client,
    config: &CollectorConfig,
    sampler: &mut DockerSandboxCgroupfsPlugin,
) -> Result<()> {
    let output = sampler.collect(Utc::now(), config)?;
    send_local_report(client, &config.local_report_url, &output)?;
    println!(
        "{}",
        json!({
            "level": "info",
            "message": "docker_sandbox_agent_snapshot_accepted",
            "url": config.local_report_url,
            "sandboxes": output.metadata.sandboxes.len(),
            "metrics": output.metrics.len(),
            "events": output.events.len(),
        })
    );
    Ok(())
}

fn collect_and_send_recent_lifecycle(client: &Client, config: &CollectorConfig) -> Result<()> {
    let output = collect_recent_docker_lifecycle(Utc::now(), config)?;
    if output.metadata.sandboxes.is_empty() && output.events.is_empty() {
        return Ok(());
    }

    send_local_report(client, &config.local_report_url, &output)?;
    log_lifecycle_report(config, &output);
    Ok(())
}

fn log_lifecycle_report(config: &CollectorConfig, output: &PluginOutput) {
    println!(
        "{}",
        json!({
            "level": "info",
            "message": "docker_sandbox_agent_lifecycle_accepted",
            "url": config.local_report_url,
            "sandboxes": output.metadata.sandboxes.len(),
            "events": output.events.len(),
        })
    );
}
