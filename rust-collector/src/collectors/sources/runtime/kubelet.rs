//! Kubelet / CRI runtime event source.
//!
//! The first implementation consumes CRI-compatible JSON lines from a command
//! such as `crictl events --output json`. Keeping the command configurable lets
//! local deployments use crictl today and swap in a native CRI client later.

use chrono::{DateTime, SecondsFormat, Utc};
use serde::Deserialize;
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::env;
use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};

use crate::collectors::core::config::CollectorConfig;
use crate::collectors::core::error::{CollectorError, Result};
use crate::collectors::core::model::{EventRecord, Metadata, PluginOutput};

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CriEvent {
    #[serde(default, alias = "container_id", alias = "containerID")]
    pub container_id: String,
    #[serde(
        default,
        alias = "sandbox_id",
        alias = "pod_sandbox_id",
        alias = "podSandboxID"
    )]
    pub sandbox_id: String,
    #[serde(default, alias = "type")]
    pub event_type: String,
    #[serde(default, alias = "reason")]
    pub reason: String,
    #[serde(default, alias = "timestamp")]
    pub created_at: i64,
    #[serde(default, alias = "image")]
    pub image_ref: String,
    #[serde(default)]
    pub labels: HashMap<String, String>,
    #[serde(default)]
    pub metadata: HashMap<String, String>,
    #[serde(default)]
    pub annotations: HashMap<String, String>,
}

pub fn stream_cri_events<F>(_config: &CollectorConfig, mut on_event: F) -> Result<()>
where
    F: FnMut(CriEvent) -> Result<()>,
{
    let command = cri_events_command();
    let mut child = Command::new("sh")
        .arg("-lc")
        .arg(&command)
        .stdout(Stdio::piped())
        .spawn()
        .map_err(|error| CollectorError::Plugin {
            plugin: "kubelet-events".to_string(),
            message: format!("failed to start CRI event command `{command}`: {error}"),
        })?;
    let stdout = child.stdout.take().ok_or_else(|| CollectorError::Plugin {
        plugin: "kubelet-events".to_string(),
        message: "CRI event command did not expose stdout".to_string(),
    })?;

    for line in BufReader::new(stdout).lines() {
        if let Some(event) = event_from_line(&line?)? {
            on_event(event)?;
        }
    }

    let status = child.wait()?;
    if !status.success() {
        return Err(CollectorError::Plugin {
            plugin: "kubelet-events".to_string(),
            message: format!("CRI event command exited with status {status}"),
        });
    }

    Ok(())
}

pub fn output_from_cri_event(event: CriEvent, config: &CollectorConfig) -> Option<PluginOutput> {
    let action = event_action(&event);
    if !is_lifecycle_action(action) {
        return None;
    }

    let timestamp = event_timestamp(&event);
    let runtime_sandbox_id = first_non_empty(&[
        event.sandbox_id.as_str(),
        label(&event, "io.kubernetes.pod.uid"),
        label(&event, "KubernetesPodUID"),
        event.container_id.as_str(),
    ]);
    if runtime_sandbox_id.is_empty() {
        return None;
    }

    let container_id = event.container_id.clone();
    let short_id = short_id(&first_non_empty(&[
        container_id.as_str(),
        runtime_sandbox_id.as_str(),
    ]));
    let sandbox_id = format!("k8s-{short_id}");
    let namespace = first_non_empty(&[
        label(&event, "io.kubernetes.pod.namespace"),
        label(&event, "KubernetesPodNamespace"),
        "kubernetes",
    ]);
    let pod_name = first_non_empty(&[
        label(&event, "io.kubernetes.pod.name"),
        label(&event, "KubernetesPodName"),
        metadata(&event, "name"),
        short_id.as_str(),
    ]);
    let container_name = first_non_empty(&[
        label(&event, "io.kubernetes.container.name"),
        label(&event, "KubernetesContainerName"),
        pod_name.as_str(),
    ]);
    let workload_name = if container_name == pod_name {
        pod_name.clone()
    } else {
        format!("{pod_name}/{container_name}")
    };
    let image_ref = if event.image_ref.is_empty() {
        first_non_empty(&[label(&event, "image"), "kubernetes/unknown:latest"])
    } else {
        event.image_ref.clone()
    };
    let image_id = image_id_from_ref(&image_ref);
    let status = sandbox_status_from_action(action);
    let current = matches!(status, "running");
    let removed = matches!(action, "CONTAINER_DELETED" | "SANDBOX_DELETED" | "REMOVE");
    let severity = event_severity(action);

    let mut attributes = Map::new();
    attributes.insert("plugin".to_string(), json!("kubelet-events"));
    attributes.insert("scope".to_string(), json!(config.collection_scope));
    attributes.insert("cri.action".to_string(), json!(action));
    attributes.insert("cri.reason".to_string(), json!(event.reason));
    attributes.insert("cri.container_id".to_string(), json!(container_id));
    attributes.insert("cri.sandbox_id".to_string(), json!(event.sandbox_id));
    attributes.insert("k8s.namespace".to_string(), json!(namespace));
    attributes.insert("k8s.pod".to_string(), json!(pod_name));
    attributes.insert("k8s.container".to_string(), json!(container_name));
    attributes.insert("k8s.annotations".to_string(), json!(event.annotations));

    let mut sandbox = json!({
        "id": sandbox_id,
        "clusterId": config.cluster_id,
        "nodeId": config.node_id,
        "namespace": namespace,
        "workloadId": pod_name,
        "workloadName": workload_name,
        "imageId": image_id,
        "imageRef": image_ref,
        "runtimeType": "kubernetes",
        "runtimeVersion": "cri-events",
        "status": status,
        "createdAt": timestamp,
        "startupDurationMs": 0,
        "cpuAvg": 0,
        "memoryPeakBytes": 0,
        "labels": {
            "collector": "runtimepulse-rust-collector",
            "plugin": "kubelet-events",
            "scope": config.collection_scope,
        },
        "attributes": {
            "collector.scope": config.collection_scope,
            "runtime.source": "cri",
            "cri.container_id": container_id,
            "cri.sandbox_id": event.sandbox_id,
            "lifecycle.action": action,
            "lifecycle.current": current,
            "lifecycle.removed": removed,
        }
    });

    if current {
        sandbox["startedAt"] = json!(timestamp);
    } else if status == "stopped" || status == "failed" {
        sandbox["stoppedAt"] = json!(timestamp);
    }
    if removed {
        sandbox["removedAt"] = json!(timestamp);
    }

    Some(PluginOutput {
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
                    "plugin": "kubelet-events",
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
                "cri-{}-{}-{}",
                short_id,
                sanitize_id(action),
                event.created_at
            ),
            timestamp: timestamp.clone(),
            severity: severity.to_string(),
            event_type: "container".to_string(),
            event_name: format!("cri.container.{}", action.to_ascii_lowercase()),
            message: format!("CRI container {workload_name} emitted {action}."),
            source: format!(
                "runtimepulse-rust-collector/{}/kubelet-events",
                config.node_id
            ),
            attributes,
            sandbox_id: Some(sandbox_id),
            image_id: None,
            node_id: Some(config.node_id.clone()),
            runtime_type: Some("kubernetes".to_string()),
            reason: if event.reason.is_empty() {
                None
            } else {
                Some(event.reason)
            },
        }],
        traces: Vec::new(),
        profiles: Vec::new(),
    })
}

fn cri_events_command() -> String {
    env::var("RUNTIMEPULSE_CRI_EVENTS_CMD")
        .unwrap_or_else(|_| "crictl events --output json".to_string())
}

fn event_from_line(line: &str) -> Result<Option<CriEvent>> {
    let line = line.trim();
    if line.is_empty() {
        return Ok(None);
    }

    if let Ok(event) = serde_json::from_str::<CriEvent>(line) {
        return Ok(Some(event));
    }

    let value = serde_json::from_str::<Value>(line)?;
    Ok(Some(serde_json::from_value(normalize_event_value(value))?))
}

fn normalize_event_value(value: Value) -> Value {
    let Some(object) = value.as_object() else {
        return value;
    };

    if object.contains_key("containerId") || object.contains_key("sandboxId") {
        return Value::Object(object.clone());
    }

    let mut normalized = object.clone();
    if let Some(Value::Object(target)) = object.get("target") {
        copy_if_missing(&mut normalized, target, "id", "containerId");
        copy_if_missing(&mut normalized, target, "podSandboxId", "sandboxId");
        copy_if_missing(&mut normalized, target, "image", "image");
        if let Some(labels) = target.get("labels") {
            normalized.insert("labels".to_string(), labels.clone());
        }
        if let Some(metadata) = target.get("metadata") {
            normalized.insert("metadata".to_string(), metadata.clone());
        }
    }
    Value::Object(normalized)
}

fn copy_if_missing(
    target: &mut serde_json::Map<String, Value>,
    source: &serde_json::Map<String, Value>,
    source_key: &str,
    target_key: &str,
) {
    if !target.contains_key(target_key) {
        if let Some(value) = source.get(source_key) {
            target.insert(target_key.to_string(), value.clone());
        }
    }
}

fn event_action(event: &CriEvent) -> &str {
    if event.event_type.is_empty() {
        event.reason.as_str()
    } else {
        event.event_type.as_str()
    }
}

fn is_lifecycle_action(action: &str) -> bool {
    matches!(
        action,
        "CONTAINER_CREATED"
            | "CONTAINER_STARTED"
            | "CONTAINER_STOPPED"
            | "CONTAINER_DELETED"
            | "SANDBOX_CREATED"
            | "SANDBOX_READY"
            | "SANDBOX_NOTREADY"
            | "SANDBOX_DELETED"
            | "CREATE"
            | "START"
            | "STOP"
            | "REMOVE"
            | "DIE"
    )
}

fn sandbox_status_from_action(action: &str) -> &'static str {
    match action {
        "CONTAINER_STARTED" | "SANDBOX_READY" | "START" => "running",
        "DIE" => "failed",
        "CONTAINER_STOPPED" | "CONTAINER_DELETED" | "SANDBOX_NOTREADY" | "SANDBOX_DELETED"
        | "STOP" | "REMOVE" | "CONTAINER_CREATED" | "SANDBOX_CREATED" | "CREATE" => "stopped",
        _ => "running",
    }
}

fn event_severity(action: &str) -> &'static str {
    match action {
        "DIE" | "SANDBOX_NOTREADY" => "warning",
        "CONTAINER_STOPPED" | "CONTAINER_DELETED" | "SANDBOX_DELETED" | "STOP" | "REMOVE" => {
            "warning"
        }
        _ => "info",
    }
}

fn event_timestamp(event: &CriEvent) -> String {
    if event.created_at > 1_000_000_000_000_000_000 {
        let secs = event.created_at / 1_000_000_000;
        let nanos = (event.created_at % 1_000_000_000) as u32;
        if let Some(time) = DateTime::from_timestamp(secs, nanos) {
            return time.to_rfc3339_opts(SecondsFormat::Millis, true);
        }
    }
    if event.created_at > 1_000_000_000 {
        if let Some(time) = DateTime::from_timestamp(event.created_at, 0) {
            return time.to_rfc3339_opts(SecondsFormat::Millis, true);
        }
    }
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

fn first_non_empty(values: &[&str]) -> String {
    values
        .iter()
        .map(|value| value.trim())
        .find(|value| !value.is_empty())
        .unwrap_or("")
        .to_string()
}

fn label<'a>(event: &'a CriEvent, key: &str) -> &'a str {
    event.labels.get(key).map(String::as_str).unwrap_or("")
}

fn metadata<'a>(event: &'a CriEvent, key: &str) -> &'a str {
    event.metadata.get(key).map(String::as_str).unwrap_or("")
}

fn short_id(value: &str) -> String {
    value.chars().take(12).collect::<String>()
}

fn image_id_from_ref(reference: &str) -> String {
    let normalized = reference
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .to_string();
    format!("cri-image-{}", normalized)
}

fn sanitize_id(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .to_string()
}
