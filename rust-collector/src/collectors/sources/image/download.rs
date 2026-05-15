//! Docker image event handler.
//!
//! Converts parsed Docker image events into RuntimePulse image observations.
//! It does not execute pulls itself; pull timelines should be derived from
//! runtime/snapshotter events when those systems expose stage timing.

use chrono::{DateTime, SecondsFormat, Utc};
use serde_json::{json, Map, Value};

use crate::collectors::core::config::CollectorConfig;
use crate::collectors::core::error::Result;
use crate::collectors::core::model::{EventRecord, Metadata, PluginOutput};
use crate::collectors::sources::image::layer::{
    docker_image_id_from_ref_or_digest, docker_image_metadata_row, DockerImageCandidate,
};
use crate::collectors::sources::runtime::docker::events::DockerEvent;

pub fn output_from_event(
    event: DockerEvent,
    config: &CollectorConfig,
) -> Result<Option<PluginOutput>> {
    if event.event_type != "image" || !is_image_action(event_action(&event)) {
        return Ok(None);
    }

    let image_ref = image_ref_from_event(&event);
    let image_digest = if event.actor.id.is_empty() {
        event.id.clone()
    } else {
        event.actor.id.clone()
    };
    let image_id = docker_image_id_from_ref_or_digest(&image_ref, &image_digest);
    let image = docker_image_metadata_row(&DockerImageCandidate {
        id: image_id.clone(),
        reference: image_ref.clone(),
        digest: image_digest.clone(),
    })
    .unwrap_or_else(|_| fallback_image_row(&image_id, &image_ref, &image_digest));
    let action = event_action(&event);
    let timestamp = event_timestamp(&event);

    let mut attributes = Map::new();
    attributes.insert("plugin".to_string(), json!("docker-image-events"));
    attributes.insert("scope".to_string(), json!(config.collection_scope));
    attributes.insert("image.id".to_string(), json!(image_id));
    attributes.insert("image.ref".to_string(), json!(image_ref));
    attributes.insert("image.digest".to_string(), json!(image_digest));
    attributes.insert("dockerAction".to_string(), json!(action));
    attributes.insert("dockerEventType".to_string(), json!(event.event_type));
    attributes.insert("dockerScope".to_string(), json!(event.scope));
    for (key, value) in &event.actor.attributes {
        attributes.insert(format!("docker.{key}"), json!(value));
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
                    "plugin": "docker-image-events",
                    "scope": config.collection_scope,
                }
            })],
            images: vec![image],
            sandboxes: Vec::new(),
        },
        metrics: Vec::new(),
        events: vec![EventRecord {
            id: format!(
                "docker-image-{}-{}-{}",
                sanitize_id(&image_ref),
                sanitize_id(action),
                event.time_nano
            ),
            timestamp,
            severity: image_event_severity(action).to_string(),
            event_type: "image".to_string(),
            event_name: format!("docker.image.{action}"),
            message: format!("Docker image {image_ref} emitted {action}."),
            source: format!(
                "runtimepulse-rust-collector/{}/docker-image-events",
                config.node_id
            ),
            attributes,
            sandbox_id: None,
            node_id: Some(config.node_id.clone()),
            runtime_type: None,
            reason: None,
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

fn is_image_action(action: &str) -> bool {
    matches!(
        action,
        "pull" | "push" | "tag" | "untag" | "delete" | "import" | "load" | "save"
    )
}

fn image_ref_from_event(event: &DockerEvent) -> String {
    event
        .actor
        .attributes
        .get("name")
        .cloned()
        .filter(|value| !value.is_empty())
        .or_else(|| {
            if event.image.is_empty() {
                None
            } else {
                Some(event.image.clone())
            }
        })
        .or_else(|| {
            if event.id.is_empty() {
                None
            } else {
                Some(event.id.clone())
            }
        })
        .unwrap_or_else(|| "docker/unknown:latest".to_string())
}

fn fallback_image_row(image_id: &str, image_ref: &str, image_digest: &str) -> Value {
    json!({
        "id": image_id,
        "ref": image_ref,
        "digest": if image_digest.is_empty() { format!("collector:{image_id}") } else { image_digest.to_string() },
        "loadingMode": "eager",
        "sizeBytes": 0,
        "layerCount": 0
    })
}

fn image_event_severity(action: &str) -> &'static str {
    match action {
        "delete" | "untag" => "warning",
        _ => "info",
    }
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
