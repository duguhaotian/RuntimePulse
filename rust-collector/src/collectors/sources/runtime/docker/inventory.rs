//! Docker inventory source.
//!
//! Collects Docker container and image inventory through the host Docker CLI.

use chrono::{DateTime, SecondsFormat, Utc};
use serde::Deserialize;
use serde_json::{json, Map};
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::process::Command;

use crate::collectors::core::config::CollectorConfig;
use crate::collectors::core::error::{CollectorError, Result};
use crate::collectors::core::model::{EventRecord, Metadata, PluginOutput};
use crate::collectors::sources::image::layer::{docker_image_metadata_rows, DockerImageCandidate};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct DockerInspectContainer {
    id: String,
    name: String,
    created: String,
    image: String,
    state: DockerState,
    config: DockerConfig,
    host_config: DockerHostConfig,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct DockerState {
    status: String,
    running: bool,
    #[serde(rename = "OOMKilled")]
    oom_killed: bool,
    error: String,
    started_at: String,
    finished_at: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct DockerConfig {
    image: String,
    #[serde(default)]
    labels: HashMap<String, String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct DockerHostConfig {
    runtime: String,
}

pub fn collect_docker_inventory(
    now: DateTime<Utc>,
    config: &CollectorConfig,
) -> Result<PluginOutput> {
    let ids = docker_container_ids()?;
    let containers = docker_inspect_containers(&ids)?;
    let ts = timestamp(now);
    let mut image_candidates = BTreeMap::new();
    let mut sandboxes = Vec::new();
    let mut events = Vec::new();

    for container in containers {
        let short_id = short_container_id(&container.id);
        let sandbox_id = docker_sandbox_id(&container.id);
        let image_ref = if container.config.image.is_empty() {
            container.image.clone()
        } else {
            container.config.image.clone()
        };
        let image_id = image_id_from_ref_or_digest(&image_ref, &container.image);
        let runtime_type = runtime_type_from_docker(&container.host_config.runtime);
        let status = sandbox_status_from_docker(&container.state);
        let workload_name = docker_workload_name(&container);
        let namespace = docker_namespace(&container);
        let created_at =
            normalize_docker_timestamp(&container.created).unwrap_or_else(|| ts.clone());
        let started_at = normalize_docker_timestamp(&container.state.started_at);
        let stopped_at = normalize_docker_timestamp(&container.state.finished_at);
        let startup_duration_ms = started_at
            .as_deref()
            .and_then(|started| duration_ms_between(&created_at, started))
            .unwrap_or(0.0);

        image_candidates
            .entry(image_id.clone())
            .or_insert_with(|| DockerImageCandidate {
                id: image_id.clone(),
                reference: image_ref.clone(),
                digest: container.image.clone(),
            });

        sandboxes.push(json!({
            "id": sandbox_id,
            "clusterId": config.cluster_id,
            "nodeId": config.node_id,
            "namespace": namespace,
            "workloadId": workload_name,
            "workloadName": workload_name,
            "imageId": image_id,
            "imageRef": image_ref,
            "runtimeType": runtime_type,
            "runtimeVersion": container.host_config.runtime,
            "status": status,
            "createdAt": created_at,
            "startedAt": started_at,
            "stoppedAt": stopped_at,
            "startupDurationMs": startup_duration_ms,
            "cpuAvg": 0,
            "memoryPeakBytes": 0,
            "labels": {
                "collector": "runtimepulse-rust-collector",
                "plugin": "docker",
                "scope": config.collection_scope,
            },
            "attributes": {
                "collector.scope": config.collection_scope,
                "docker.id": container.id,
                "docker.short_id": short_id,
                "docker.name": container.name.trim_start_matches('/'),
                "docker.status": container.state.status,
                "docker.oom_killed": container.state.oom_killed,
                "docker.error": container.state.error,
            }
        }));

        let mut attributes = Map::new();
        attributes.insert("plugin".to_string(), json!("docker"));
        attributes.insert("scope".to_string(), json!(config.collection_scope));
        attributes.insert("dockerId".to_string(), json!(container.id));
        attributes.insert("dockerStatus".to_string(), json!(container.state.status));

        events.push(EventRecord {
            id: format!("docker-{}-observed-{}", short_id, now.timestamp()),
            timestamp: ts.clone(),
            severity: if status == "failed" { "error" } else { "info" }.to_string(),
            event_type: "container".to_string(),
            event_name: "docker.container.observed".to_string(),
            message: format!("Docker container {workload_name} is {status}."),
            source: format!("runtimepulse-rust-collector/{}/docker", config.node_id),
            attributes,
            sandbox_id: Some(docker_sandbox_id(&container.id)),
            node_id: Some(config.node_id.clone()),
            runtime_type: Some(runtime_type.to_string()),
            reason: if container.state.oom_killed {
                Some("oom_killed".to_string())
            } else if !container.state.error.is_empty() {
                Some(container.state.error)
            } else {
                None
            },
        });
    }

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
                "kernelVersion": kernel_version().unwrap_or_else(|| "docker-observed".to_string()),
                "cpuCores": cpu_core_count().unwrap_or(0),
                "memoryBytes": memory_total_bytes().unwrap_or(0),
                "status": "ready",
                "labels": {
                    "collector": "runtimepulse-rust-collector",
                    "plugin": "docker",
                    "scope": config.collection_scope,
                }
            })],
            images: docker_image_metadata_rows(image_candidates.into_values().collect())?
                .into_values()
                .collect(),
            sandboxes,
        },
        metrics: Vec::new(),
        events,
        traces: Vec::new(),
        profiles: Vec::new(),
    })
}

fn docker_container_ids() -> Result<Vec<String>> {
    let output = Command::new("docker")
        .args(["ps", "-aq", "--no-trunc"])
        .output()?;

    if !output.status.success() {
        return Err(CollectorError::Plugin {
            plugin: "docker".to_string(),
            message: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        });
    }

    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(ToOwned::to_owned)
        .collect())
}

fn docker_inspect_containers(ids: &[String]) -> Result<Vec<DockerInspectContainer>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }

    let output = Command::new("docker").arg("inspect").args(ids).output()?;

    if !output.status.success() {
        return Err(CollectorError::Plugin {
            plugin: "docker".to_string(),
            message: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        });
    }

    Ok(serde_json::from_slice(&output.stdout)?)
}

fn short_container_id(id: &str) -> String {
    id.chars().take(12).collect()
}

fn docker_sandbox_id(id: &str) -> String {
    format!("docker-{}", short_container_id(id))
}

fn image_id_from_ref_or_digest(image_ref: &str, digest: &str) -> String {
    let source = if image_ref.is_empty() {
        digest
    } else {
        image_ref
    };
    format!("docker-image-{}", sanitize_id(source))
}

fn runtime_type_from_docker(runtime: &str) -> String {
    let normalized = runtime.to_ascii_lowercase();
    if normalized.contains("runsc") || normalized.contains("gvisor") {
        "gvisor".to_string()
    } else if normalized.contains("kata") {
        "kata".to_string()
    } else if normalized.contains("firecracker") {
        "firecracker".to_string()
    } else {
        "runc".to_string()
    }
}

fn sandbox_status_from_docker(state: &DockerState) -> String {
    let status = state.status.to_ascii_lowercase();
    if state.oom_killed || !state.error.is_empty() {
        "failed".to_string()
    } else if state.running || matches!(status.as_str(), "created" | "paused" | "restarting") {
        "running".to_string()
    } else {
        "stopped".to_string()
    }
}

fn docker_workload_name(container: &DockerInspectContainer) -> String {
    container
        .config
        .labels
        .get("com.docker.compose.service")
        .cloned()
        .or_else(|| {
            let name = container.name.trim_start_matches('/');
            if name.is_empty() {
                None
            } else {
                Some(name.to_string())
            }
        })
        .unwrap_or_else(|| short_container_id(&container.id))
}

fn docker_namespace(container: &DockerInspectContainer) -> String {
    container
        .config
        .labels
        .get("com.docker.compose.project")
        .cloned()
        .unwrap_or_else(|| "docker".to_string())
}

fn normalize_docker_timestamp(value: &str) -> Option<String> {
    if value.is_empty() || value.starts_with("0001-01-01T00:00:00") {
        return None;
    }

    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|time| timestamp(time.with_timezone(&Utc)))
}

fn duration_ms_between(start: &str, end: &str) -> Option<f64> {
    let start = DateTime::parse_from_rfc3339(start).ok()?;
    let end = DateTime::parse_from_rfc3339(end).ok()?;
    let duration = end.signed_duration_since(start);
    Some(duration.num_milliseconds().max(0) as f64)
}

fn cpu_core_count() -> Result<u64> {
    let cpuinfo = fs::read_to_string("/proc/cpuinfo")?;
    Ok(cpuinfo
        .lines()
        .filter(|line| line.starts_with("processor"))
        .count() as u64)
}

fn kernel_version() -> Option<String> {
    fs::read_to_string("/proc/sys/kernel/osrelease")
        .ok()
        .map(|value| value.trim().to_string())
}

fn memory_total_bytes() -> Result<u64> {
    let meminfo = fs::read_to_string("/proc/meminfo")?;
    Ok(meminfo_kib(&meminfo, "MemTotal").unwrap_or(0) * 1024)
}

fn meminfo_kib(meminfo: &str, key: &str) -> Option<u64> {
    meminfo.lines().find_map(|line| {
        let mut parts = line.split_whitespace();
        if parts.next()?.trim_end_matches(':') == key {
            parts.next()?.parse::<u64>().ok()
        } else {
            None
        }
    })
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
