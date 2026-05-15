//! Sandbox sampler managers.
//!
//! Managers reconcile already-running sandboxes at startup, keep periodic
//! sampler output flowing, and report runtime lifecycle events that update the
//! current sandbox set.

use chrono::Utc;
use reqwest::blocking::Client;
use serde_json::json;
use std::collections::HashSet;
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::Instant;

use crate::collectors::core::config::CollectorConfig;
use crate::collectors::core::error::{CollectorError, Result};
use crate::collectors::core::model::PluginOutput;
use crate::collectors::outlet::sender::send_local_report;
use crate::collectors::sources::runtime::docker::lifecycle::{
    collect_recent_docker_lifecycle, stream_docker_lifecycle,
};
use crate::collectors::sources::sandbox::cgroupfs::{
    docker_running_container_ids, DockerSandboxCgroupfsPlugin,
};

pub fn run_docker_sandbox_agent(mut config: CollectorConfig) -> Result<()> {
    config.collection_scope = "host".to_string();

    let client = Client::new();
    let mut sampler = DockerSandboxCgroupfsPlugin::new(config.cgroup_root.clone());
    let active_docker_ids = Arc::new(Mutex::new(
        docker_running_container_ids()?
            .into_iter()
            .collect::<HashSet<_>>(),
    ));

    if config.once {
        let docker_ids = active_docker_ids_snapshot(&active_docker_ids)?;
        collect_and_send_sandbox_snapshot(&client, &config, &mut sampler, &docker_ids)?;
        collect_and_send_recent_lifecycle(&client, &config, Some(&active_docker_ids))?;
        return Ok(());
    }

    collect_and_send_recent_lifecycle(&client, &config, Some(&active_docker_ids))?;

    let (lifecycle_result_tx, lifecycle_result_rx) = mpsc::channel();
    let lifecycle_config = config.clone();
    let lifecycle_client = client.clone();
    let lifecycle_active_docker_ids = Arc::clone(&active_docker_ids);
    thread::spawn(move || {
        let result = stream_docker_lifecycle(&lifecycle_config, |output| {
            apply_lifecycle_output(&lifecycle_active_docker_ids, &output)?;
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

        let docker_ids = active_docker_ids_snapshot(&active_docker_ids)?;
        collect_and_send_sandbox_snapshot(&client, &config, &mut sampler, &docker_ids)?;

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
    docker_ids: &[String],
) -> Result<()> {
    let output = sampler.collect_for_docker_ids(Utc::now(), config, docker_ids)?;
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

fn collect_and_send_recent_lifecycle(
    client: &Client,
    config: &CollectorConfig,
    active_docker_ids: Option<&Arc<Mutex<HashSet<String>>>>,
) -> Result<()> {
    let output = collect_recent_docker_lifecycle(Utc::now(), config)?;
    if output.metadata.sandboxes.is_empty() && output.events.is_empty() {
        return Ok(());
    }

    if let Some(active_docker_ids) = active_docker_ids {
        apply_lifecycle_output(active_docker_ids, &output)?;
    }
    send_local_report(client, &config.local_report_url, &output)?;
    log_lifecycle_report(config, &output);
    Ok(())
}

fn active_docker_ids_snapshot(
    active_docker_ids: &Arc<Mutex<HashSet<String>>>,
) -> Result<Vec<String>> {
    let mut ids = active_docker_ids
        .lock()
        .map_err(|_| CollectorError::Plugin {
            plugin: "docker-sandbox-agent".to_string(),
            message: "active docker id set lock poisoned".to_string(),
        })?
        .iter()
        .cloned()
        .collect::<Vec<_>>();
    ids.sort();
    Ok(ids)
}

fn apply_lifecycle_output(
    active_docker_ids: &Arc<Mutex<HashSet<String>>>,
    output: &PluginOutput,
) -> Result<()> {
    let mut active_docker_ids = active_docker_ids
        .lock()
        .map_err(|_| CollectorError::Plugin {
            plugin: "docker-sandbox-agent".to_string(),
            message: "active docker id set lock poisoned".to_string(),
        })?;

    for sandbox in &output.metadata.sandboxes {
        let Some(attributes) = sandbox
            .get("attributes")
            .and_then(|value| value.as_object())
        else {
            continue;
        };
        let Some(docker_id) = attributes.get("docker.id").and_then(|value| value.as_str()) else {
            continue;
        };
        let Some(action) = attributes
            .get("lifecycle.action")
            .and_then(|value| value.as_str())
        else {
            continue;
        };

        if lifecycle_action_is_running(action) {
            active_docker_ids.insert(docker_id.to_string());
        } else if lifecycle_action_stops_sampling(action) {
            active_docker_ids.remove(docker_id);
        }
    }

    Ok(())
}

fn lifecycle_action_is_running(action: &str) -> bool {
    matches!(action, "start" | "restart" | "unpause")
}

fn lifecycle_action_stops_sampling(action: &str) -> bool {
    matches!(
        action,
        "pause" | "stop" | "die" | "kill" | "oom" | "destroy"
    )
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
