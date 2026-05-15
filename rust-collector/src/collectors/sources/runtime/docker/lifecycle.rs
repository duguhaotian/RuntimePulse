//! Docker lifecycle source.
//!
//! Observes Docker container lifecycle events and converts them into
//! RuntimePulse sandbox events. The sandbox sampler manager will later consume
//! the same event stream to start/stop per-sandbox samplers.

use chrono::{DateTime, SecondsFormat, Utc};
use serde::Deserialize;
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};

use crate::collectors::core::config::CollectorConfig;
use crate::collectors::core::error::{CollectorError, Result};
use crate::collectors::core::model::{EventRecord, Metadata, PluginOutput};

#[derive(Debug, Deserialize)]
struct DockerLifecycleEvent {
    #[serde(default, rename = "Type")]
    event_type: String,
    #[serde(default, rename = "Action")]
    action: String,
    #[serde(default)]
    status: String,
    #[serde(default)]
    id: String,
    #[serde(default, rename = "from")]
    image: String,
    #[serde(default, rename = "Actor")]
    actor: DockerEventActor,
    #[serde(default)]
    scope: String,
    #[serde(default)]
    time: i64,
    #[serde(default, rename = "timeNano")]
    time_nano: i64,
}

#[derive(Debug, Default, Deserialize)]
struct DockerEventActor {
    #[serde(default, rename = "ID")]
    id: String,
    #[serde(default, rename = "Attributes")]
    attributes: HashMap<String, String>,
}

pub fn collect_recent_docker_lifecycle(
    now: DateTime<Utc>,
    config: &CollectorConfig,
) -> Result<PluginOutput> {
    let since = now.timestamp() - config.interval.as_secs() as i64;
    let until = now.timestamp() + 1;
    let output = docker_events_command()
        .args(["--since", &since.to_string(), "--until", &until.to_string()])
        .output()?;

    if !output.status.success() {
        return Err(CollectorError::Plugin {
            plugin: "docker-events".to_string(),
            message: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        });
    }

    let mut combined = empty_output(config);
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        if let Some(output) = output_from_line(line, config)? {
            merge_output(&mut combined, output);
        }
    }

    Ok(combined)
}

pub fn stream_docker_lifecycle<F>(config: &CollectorConfig, mut on_output: F) -> Result<()>
where
    F: FnMut(PluginOutput) -> Result<()>,
{
    let mut child = docker_events_command().stdout(Stdio::piped()).spawn()?;
    let stdout = child.stdout.take().ok_or_else(|| CollectorError::Plugin {
        plugin: "docker-events".to_string(),
        message: "docker events did not expose stdout".to_string(),
    })?;

    for line in BufReader::new(stdout).lines() {
        let line = line?;
        if let Some(output) = output_from_line(&line, config)? {
            on_output(output)?;
        }
    }

    let status = child.wait()?;
    if !status.success() {
        return Err(CollectorError::Plugin {
            plugin: "docker-events".to_string(),
            message: format!("docker events exited with status {status}"),
        });
    }

    Ok(())
}

fn docker_events_command() -> Command {
    let mut command = Command::new("docker");
    command.args([
        "events",
        "--filter",
        "type=container",
        "--format",
        "{{json .}}",
    ]);
    command
}

fn output_from_line(line: &str, config: &CollectorConfig) -> Result<Option<PluginOutput>> {
    let line = line.trim();
    if line.is_empty() {
        return Ok(None);
    }

    let event = serde_json::from_str::<DockerLifecycleEvent>(line)?;
    if event.event_type != "container" || !is_lifecycle_action(event_action(&event)) {
        return Ok(None);
    }

    Ok(Some(output_from_event(event, config)))
}

fn output_from_event(event: DockerLifecycleEvent, config: &CollectorConfig) -> PluginOutput {
    let timestamp = event_timestamp(&event);
    let container_id = if event.actor.id.is_empty() {
        event.id.clone()
    } else {
        event.actor.id.clone()
    };
    let short_id = short_container_id(&container_id);
    let sandbox_id = docker_sandbox_id(&container_id);
    let image_ref = event
        .actor
        .attributes
        .get("image")
        .cloned()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| {
            if event.image.is_empty() {
                "docker/unknown:latest".to_string()
            } else {
                event.image.clone()
            }
        });
    let image_id = image_id_from_ref(&image_ref);
    let name = event
        .actor
        .attributes
        .get("name")
        .cloned()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| short_id.clone());
    let action = event_action(&event);
    let lifecycle_status = sandbox_status_from_event(action, &event.actor.attributes);
    let current = is_current_action(action);
    let removed = is_removed_action(action);
    let severity = event_severity(action, &event.actor.attributes);

    let mut attributes = Map::new();
    attributes.insert("plugin".to_string(), json!("docker-events"));
    attributes.insert("scope".to_string(), json!(config.collection_scope));
    attributes.insert("dockerId".to_string(), json!(container_id));
    attributes.insert("dockerShortId".to_string(), json!(short_id));
    attributes.insert("dockerAction".to_string(), json!(action));
    attributes.insert("dockerEventType".to_string(), json!(event.event_type));
    attributes.insert("dockerScope".to_string(), json!(event.scope));
    for key in ["exitCode", "signal", "name", "image"] {
        if let Some(value) = event.actor.attributes.get(key) {
            attributes.insert(format!("docker.{key}"), json!(value));
        }
    }

    let mut sandbox = json!({
        "id": sandbox_id,
        "clusterId": config.cluster_id,
        "nodeId": config.node_id,
        "namespace": "docker",
        "workloadId": name,
        "workloadName": name,
        "imageId": image_id,
        "imageRef": image_ref,
        "runtimeType": "runc",
        "runtimeVersion": "docker-events",
        "status": lifecycle_status,
        "createdAt": timestamp,
        "startupDurationMs": 0,
        "cpuAvg": 0,
        "memoryPeakBytes": 0,
        "labels": {
            "collector": "runtimepulse-rust-collector",
            "plugin": "docker-events",
            "scope": config.collection_scope,
        },
        "attributes": {
            "collector.scope": config.collection_scope,
            "docker.id": container_id,
            "docker.short_id": short_id,
            "docker.action": action,
            "lifecycle.action": action,
            "lifecycle.current": current,
            "lifecycle.removed": removed,
        }
    });

    if lifecycle_status == "running" {
        sandbox["startedAt"] = json!(timestamp);
    } else if lifecycle_status == "stopped" || lifecycle_status == "failed" {
        sandbox["stoppedAt"] = json!(timestamp);
    }
    if removed {
        sandbox["removedAt"] = json!(timestamp);
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
                    "plugin": "docker-events",
                    "scope": config.collection_scope,
                }
            })],
            images: vec![json!({
                "id": image_id,
                "ref": image_ref,
                "digest": format!("collector:{image_id}"),
                "loadingMode": "eager",
                "sizeBytes": 0,
                "layerCount": 0
            })],
            sandboxes: vec![sandbox],
        },
        metrics: Vec::new(),
        events: vec![EventRecord {
            id: format!(
                "docker-{}-{}-{}",
                short_id,
                sanitize_id(action),
                event.time_nano
            ),
            timestamp: timestamp.clone(),
            severity: severity.to_string(),
            event_type: "container".to_string(),
            event_name: format!("docker.container.{action}"),
            message: format!("Docker container {name} emitted {action}."),
            source: format!(
                "runtimepulse-rust-collector/{}/docker-events",
                config.node_id
            ),
            attributes,
            sandbox_id: Some(docker_sandbox_id(&container_id)),
            node_id: Some(config.node_id.clone()),
            runtime_type: Some("runc".to_string()),
            reason: event_reason(action, &event.actor.attributes),
        }],
        traces: Vec::new(),
        profiles: Vec::new(),
    }
}

fn empty_output(config: &CollectorConfig) -> PluginOutput {
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
                    "plugin": "docker-events",
                    "scope": config.collection_scope,
                }
            })],
            images: Vec::new(),
            sandboxes: Vec::new(),
        },
        metrics: Vec::new(),
        events: Vec::new(),
        traces: Vec::new(),
        profiles: Vec::new(),
    }
}

fn merge_output(target: &mut PluginOutput, output: PluginOutput) {
    extend_unique_by_id(&mut target.metadata.clusters, output.metadata.clusters);
    extend_unique_by_id(&mut target.metadata.nodes, output.metadata.nodes);
    extend_unique_by_id(&mut target.metadata.images, output.metadata.images);
    extend_unique_by_id(&mut target.metadata.sandboxes, output.metadata.sandboxes);
    target.metrics.extend(output.metrics);
    target.events.extend(output.events);
    target.traces.extend(output.traces);
    target.profiles.extend(output.profiles);
}

fn extend_unique_by_id(target: &mut Vec<Value>, rows: Vec<Value>) {
    for row in rows {
        let Some(id) = row.get("id").and_then(Value::as_str) else {
            target.push(row);
            continue;
        };
        if let Some(existing) = target
            .iter_mut()
            .find(|item| item.get("id").and_then(Value::as_str) == Some(id))
        {
            *existing = row;
        } else {
            target.push(row);
        }
    }
}

fn event_action(event: &DockerLifecycleEvent) -> &str {
    if event.action.is_empty() {
        event.status.as_str()
    } else {
        event.action.as_str()
    }
}

fn is_lifecycle_action(action: &str) -> bool {
    matches!(
        action,
        "create"
            | "start"
            | "restart"
            | "pause"
            | "unpause"
            | "stop"
            | "die"
            | "kill"
            | "oom"
            | "destroy"
    )
}

fn sandbox_status_from_event(action: &str, attributes: &HashMap<String, String>) -> &'static str {
    match action {
        "start" | "restart" | "unpause" => "running",
        "oom" | "kill" => "failed",
        "die" if exit_code(attributes) != Some(0) => "failed",
        "stop" | "die" | "destroy" | "pause" | "create" => "stopped",
        _ => "running",
    }
}

fn is_current_action(action: &str) -> bool {
    matches!(action, "start" | "restart" | "unpause")
}

fn is_removed_action(action: &str) -> bool {
    matches!(action, "destroy")
}

fn event_severity(action: &str, attributes: &HashMap<String, String>) -> &'static str {
    match action {
        "oom" | "kill" => "error",
        "die" if exit_code(attributes) != Some(0) => "error",
        "die" | "stop" | "destroy" => "warning",
        _ => "info",
    }
}

fn event_reason(action: &str, attributes: &HashMap<String, String>) -> Option<String> {
    match action {
        "oom" => Some("oom".to_string()),
        "kill" => attributes
            .get("signal")
            .map(|signal| format!("signal:{signal}"))
            .or_else(|| Some("killed".to_string())),
        "die" => exit_code(attributes).and_then(|code| {
            if code == 0 {
                None
            } else {
                Some(format!("exit_code:{code}"))
            }
        }),
        _ => None,
    }
}

fn exit_code(attributes: &HashMap<String, String>) -> Option<i32> {
    attributes.get("exitCode")?.parse::<i32>().ok()
}

fn event_timestamp(event: &DockerLifecycleEvent) -> String {
    if event.time_nano > 0 {
        let secs = event.time_nano / 1_000_000_000;
        let nanos = (event.time_nano % 1_000_000_000) as u32;
        if let Some(time) = DateTime::from_timestamp(secs, nanos) {
            return timestamp(time);
        }
    }
    if event.time > 0 {
        if let Some(time) = DateTime::from_timestamp(event.time, 0) {
            return timestamp(time);
        }
    }
    timestamp(Utc::now())
}

fn short_container_id(id: &str) -> String {
    id.chars().take(12).collect()
}

fn docker_sandbox_id(id: &str) -> String {
    format!("docker-{}", short_container_id(id))
}

fn image_id_from_ref(image_ref: &str) -> String {
    format!("docker-image-{}", sanitize_id(image_ref))
}

fn sanitize_id(value: &str) -> String {
    value
        .chars()
        .map(|char| {
            if char.is_ascii_alphanumeric() {
                char.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .to_string()
}

fn timestamp(time: DateTime<Utc>) -> String {
    time.to_rfc3339_opts(SecondsFormat::Millis, true)
}
