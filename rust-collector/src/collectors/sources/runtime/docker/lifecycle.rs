//! Docker container lifecycle handler.
//!
//! Converts parsed Docker container events into RuntimePulse sandbox lifecycle
//! output. The raw Docker event stream is owned by `runtime/docker/events.rs`.

use chrono::{DateTime, SecondsFormat, Utc};
use serde_json::{json, Map};
use std::collections::HashMap;

use crate::collectors::core::config::CollectorConfig;
use crate::collectors::core::error::Result;
use crate::collectors::core::model::{EventRecord, Metadata, PluginOutput};
use crate::collectors::sources::runtime::docker::events::DockerEvent;

pub fn output_from_event(
    event: DockerEvent,
    config: &CollectorConfig,
) -> Result<Option<PluginOutput>> {
    if event.event_type != "container" || !is_lifecycle_action(event_action(&event)) {
        return Ok(None);
    }

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

    Ok(Some(PluginOutput {
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
    }))
}

fn event_action(event: &DockerEvent) -> &str {
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

fn event_timestamp(event: &DockerEvent) -> String {
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
