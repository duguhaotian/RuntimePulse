//! Image download source.
//!
//! Wraps `docker pull` when the user wants RuntimePulse to observe an eager
//! image download. Docker CLI does not expose stable resolve/verify/unpack
//! sub-stage timings, so this source records the real pull command duration as
//! one `pull` stage instead of fabricating finer-grained stages.

use chrono::{DateTime, SecondsFormat, Utc};
use serde_json::{json, Map, Value};
use std::process::Command;
use std::time::Instant;

use crate::collectors::core::config::CollectorConfig;
use crate::collectors::core::error::{CollectorError, Result};
use crate::collectors::core::model::{EventRecord, Metadata, PluginOutput};
use crate::collectors::sources::image::layer::{
    docker_image_id_from_ref_or_digest, docker_image_metadata_row, DockerImageCandidate,
};

pub fn pull_docker_image(
    image_ref: &str,
    now: DateTime<Utc>,
    config: &CollectorConfig,
) -> Result<PluginOutput> {
    if image_ref.trim().is_empty() {
        return Err(CollectorError::Config(
            "host-docker-pull requires an image reference".to_string(),
        ));
    }

    let started = Instant::now();
    let output = Command::new("docker").args(["pull", image_ref]).output()?;
    let duration_ms = started.elapsed().as_secs_f64() * 1000.0;

    if !output.status.success() {
        return Err(CollectorError::Plugin {
            plugin: "docker-image-download".to_string(),
            message: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        });
    }

    let image_id = docker_image_id_from_ref_or_digest(image_ref, image_ref);
    let mut image = docker_image_metadata_row(&DockerImageCandidate {
        id: image_id.clone(),
        reference: image_ref.to_string(),
        digest: image_ref.to_string(),
    })?;
    attach_download_timeline(&mut image, &image_id, image_ref, duration_ms);

    let ts = timestamp(now);
    let mut attributes = Map::new();
    attributes.insert("plugin".to_string(), json!("docker-image-download"));
    attributes.insert("scope".to_string(), json!(config.collection_scope));
    attributes.insert("image.id".to_string(), json!(image_id));
    attributes.insert("image.ref".to_string(), json!(image_ref));
    attributes.insert("durationMs".to_string(), json!(duration_ms));

    Ok(PluginOutput {
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
                    "plugin": "docker-image-download",
                    "scope": config.collection_scope,
                }
            })],
            images: vec![image],
            sandboxes: Vec::new(),
        },
        metrics: Vec::new(),
        events: vec![EventRecord {
            id: format!(
                "docker-image-pull-{}-{}",
                sanitize_id(image_ref),
                now.timestamp()
            ),
            timestamp: ts,
            severity: "info".to_string(),
            event_type: "image".to_string(),
            event_name: "docker.image.pull.completed".to_string(),
            message: format!("Docker image {image_ref} pull completed."),
            source: format!(
                "runtimepulse-rust-collector/{}/docker-image-download",
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
    })
}

fn attach_download_timeline(image: &mut Value, image_id: &str, image_ref: &str, duration_ms: f64) {
    let size_bytes = image
        .get("sizeBytes")
        .and_then(Value::as_u64)
        .filter(|value| *value > 0);

    if let Some(object) = image.as_object_mut() {
        object.insert(
            "downloadTimeline".to_string(),
            json!([{
                "id": format!("{image_id}-docker-pull"),
                "name": "Docker pull",
                "phase": "pull",
                "durationMs": duration_ms,
                "bytes": size_bytes,
                "detail": format!("Measured elapsed time of `docker pull {image_ref}` on the host."),
            }]),
        );
    }
}

fn timestamp(time: DateTime<Utc>) -> String {
    time.to_rfc3339_opts(SecondsFormat::Millis, true)
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
