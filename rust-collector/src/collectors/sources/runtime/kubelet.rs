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
use crate::collectors::sources::runtime::containerd::runtime_type_from_containerd_name;

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
    #[serde(
        default,
        alias = "timestamp",
        deserialize_with = "deserialize_i64_string"
    )]
    pub created_at: i64,
    #[serde(default, alias = "image")]
    pub image_ref: String,
    #[serde(default)]
    pub labels: HashMap<String, String>,
    #[serde(default)]
    pub metadata: HashMap<String, String>,
    #[serde(default)]
    pub annotations: HashMap<String, String>,
    #[serde(default, alias = "runtime_handler")]
    pub runtime_handler: String,
}

fn deserialize_i64_string<'de, D>(deserializer: D) -> std::result::Result<i64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Value::deserialize(deserializer)?;
    if let Some(number) = value.as_i64() {
        return Ok(number);
    }
    if let Some(text) = value.as_str() {
        return text.parse::<i64>().map_err(serde::de::Error::custom);
    }
    Ok(0)
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
    let action = cri_event_action(&event);
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
    let runtime_id = runtime_object_id(&first_non_empty(&[
        container_id.as_str(),
        runtime_sandbox_id.as_str(),
    ]));
    let short_id = short_id(&runtime_id);
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
    let sandbox_id = kubernetes_sandbox_id(&namespace, &pod_name, &container_name)
        .unwrap_or_else(|| format!("k8s-{short_id}"));
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
    let runtime_handler = cri_event_runtime_handler(&event);
    let runtime_type = cri_event_runtime_type(&event);
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
    attributes.insert("cri.runtime_handler".to_string(), json!(runtime_handler));
    attributes.insert("runtime.type".to_string(), json!(runtime_type));
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
        "runtimeType": runtime_type,
        "runtimeVersion": runtime_handler,
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
            "cri.runtime_handler": runtime_handler,
            "cri.short_id": short_id,
            "k8s.namespace": namespace,
            "k8s.pod": pod_name,
            "k8s.container": container_name,
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
        source: None,
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
            runtime_type: Some(runtime_type),
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

    let value = serde_json::from_str::<Value>(line)?;
    if event_needs_normalization(&value) {
        return Ok(Some(serde_json::from_value(normalize_event_value(value))?));
    }

    match serde_json::from_value::<CriEvent>(value.clone()) {
        Ok(event) => Ok(Some(event)),
        Err(_) => Ok(Some(serde_json::from_value(normalize_event_value(value))?)),
    }
}

fn event_needs_normalization(value: &Value) -> bool {
    value.as_object().is_some_and(|object| {
        object.contains_key("target")
            || object.contains_key("podSandboxStatus")
            || object.contains_key("containerEventType")
    })
}

fn normalize_event_value(value: Value) -> Value {
    let Some(object) = value.as_object() else {
        return value;
    };

    if (object.contains_key("containerId") || object.contains_key("sandboxId"))
        && !event_needs_normalization(&value)
    {
        return Value::Object(object.clone());
    }

    let mut normalized = object.clone();
    if let Some(Value::Object(target)) = object.get("target") {
        copy_if_missing(&mut normalized, target, "id", "containerId");
        copy_if_missing(&mut normalized, target, "podSandboxId", "sandboxId");
        copy_if_missing(&mut normalized, target, "image", "image");
        copy_string_map_if_present(&mut normalized, target, "labels");
        copy_string_map_if_present(&mut normalized, target, "metadata");
    }
    if let Some(Value::Object(status)) = object.get("podSandboxStatus") {
        copy_if_missing(&mut normalized, status, "id", "sandboxId");
        copy_if_missing(&mut normalized, status, "id", "containerId");
        copy_if_missing(&mut normalized, status, "createdAt", "createdAt");
        copy_if_missing(&mut normalized, status, "runtimeHandler", "runtimeHandler");
        if let Some(Value::String(state)) = status.get("state") {
            normalized.insert("type".to_string(), json!(state));
        }
        copy_string_map_if_present(&mut normalized, status, "labels");
        copy_string_map_if_present(&mut normalized, status, "metadata");
    }
    if let Some(Value::String(event_type)) = object.get("containerEventType") {
        normalized.insert(
            "reason".to_string(),
            json!(normalize_crictl_container_event_type(event_type)),
        );
        if object.contains_key("podSandboxStatus") {
            let state = object
                .get("podSandboxStatus")
                .and_then(Value::as_object)
                .and_then(|status| status.get("state"))
                .and_then(Value::as_str)
                .unwrap_or("");
            normalized.insert(
                "type".to_string(),
                json!(normalize_crictl_sandbox_event_type(event_type, state)),
            );
        }
    }
    Value::Object(normalized)
}

fn normalize_crictl_container_event_type(value: &str) -> &str {
    match value {
        "CONTAINER_CREATED_EVENT" => "CONTAINER_CREATED",
        "CONTAINER_STARTED_EVENT" => "CONTAINER_STARTED",
        "CONTAINER_STOPPED_EVENT" => "CONTAINER_STOPPED",
        "CONTAINER_DELETED_EVENT" => "CONTAINER_DELETED",
        other => other.trim_end_matches("_EVENT"),
    }
}

fn normalize_crictl_sandbox_event_type<'a>(event_type: &str, state: &'a str) -> &'a str {
    match event_type {
        "CONTAINER_CREATED_EVENT" => "SANDBOX_CREATED",
        "CONTAINER_STARTED_EVENT" => "SANDBOX_READY",
        "CONTAINER_STOPPED_EVENT" => {
            if state.is_empty() {
                "SANDBOX_NOTREADY"
            } else {
                state
            }
        }
        "CONTAINER_DELETED_EVENT" => "SANDBOX_DELETED",
        _ => state,
    }
}

fn copy_string_map_if_present(
    target: &mut serde_json::Map<String, Value>,
    source: &serde_json::Map<String, Value>,
    key: &str,
) {
    let Some(Value::Object(values)) = source.get(key) else {
        return;
    };

    let string_values = values
        .iter()
        .filter_map(|(entry_key, entry_value)| {
            entry_value
                .as_str()
                .map(|entry_text| (entry_key.clone(), Value::String(entry_text.to_string())))
        })
        .collect::<serde_json::Map<String, Value>>();
    target.insert(key.to_string(), Value::Object(string_values));
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

pub fn cri_event_action(event: &CriEvent) -> &str {
    event_action(event)
}

pub fn cri_event_timestamp(event: &CriEvent) -> DateTime<Utc> {
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

pub fn cri_event_runtime_sandbox_id(event: &CriEvent) -> String {
    first_non_empty(&[
        event.sandbox_id.as_str(),
        label(event, "io.kubernetes.cri.sandbox-id"),
        label(event, "io.kubernetes.sandbox.id"),
        label(event, "io.kubernetes.pod.uid"),
        label(event, "KubernetesPodUID"),
        event.container_id.as_str(),
    ])
}

pub fn cri_event_runtime_object_id(event: &CriEvent) -> String {
    runtime_object_id(&first_non_empty(&[
        event.container_id.as_str(),
        event.sandbox_id.as_str(),
        label(event, "io.kubernetes.cri.sandbox-id"),
    ]))
}

pub fn cri_event_runtime_handler(event: &CriEvent) -> String {
    first_non_empty(&[
        event.runtime_handler.as_str(),
        label(event, "io.kubernetes.cri.runtime-handler"),
        label(event, "io.kubernetes.runtime.handler"),
        label(event, "runtimeHandler"),
        "runc",
    ])
}

pub fn cri_event_runtime_type(event: &CriEvent) -> String {
    runtime_type_from_containerd_name(&cri_event_runtime_handler(event))
}

pub fn cri_event_stable_sandbox_id(event: &CriEvent) -> Option<String> {
    let runtime_id = runtime_object_id(&cri_event_runtime_sandbox_id(event));
    if runtime_id.is_empty() {
        return None;
    }

    let namespace = first_non_empty(&[
        label(event, "io.kubernetes.pod.namespace"),
        label(event, "KubernetesPodNamespace"),
    ]);
    let pod_name = first_non_empty(&[
        label(event, "io.kubernetes.pod.name"),
        label(event, "KubernetesPodName"),
        metadata(event, "name"),
    ]);
    let container_name = first_non_empty(&[
        label(event, "io.kubernetes.container.name"),
        label(event, "KubernetesContainerName"),
        pod_name.as_str(),
    ]);

    if !namespace.is_empty() && !pod_name.is_empty() && !container_name.is_empty() {
        kubernetes_sandbox_id(&namespace, &pod_name, &container_name)
    } else {
        Some(format!("cri-{}", sanitize_id(&runtime_id)))
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
    cri_event_timestamp(event).to_rfc3339_opts(SecondsFormat::Millis, true)
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

fn runtime_object_id(value: &str) -> String {
    let trimmed = value.trim();
    let without_scheme = trimmed
        .split_once("://")
        .map(|(_, suffix)| suffix)
        .unwrap_or(trimmed);
    without_scheme
        .rsplit('/')
        .next()
        .unwrap_or(without_scheme)
        .to_string()
}

fn kubernetes_sandbox_id(namespace: &str, pod: &str, container: &str) -> Option<String> {
    let namespace = sanitize_id(namespace);
    let pod = sanitize_id(pod);
    let container = sanitize_id(container);
    if namespace.is_empty() || pod.is_empty() || container.is_empty() {
        None
    } else {
        Some(format!("k8s-{namespace}-{pod}-{container}"))
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::time::Duration;

    #[test]
    fn cri_event_uses_kubernetes_sandbox_identity_when_labels_exist() {
        let config = test_config();
        let event = CriEvent {
            container_id: "containerd://runtimepulse-demo-container".to_string(),
            sandbox_id: "runtimepulse-demo-pod-uid".to_string(),
            event_type: "CONTAINER_STARTED".to_string(),
            reason: String::new(),
            created_at: 1_779_415_900_000_000_000,
            image_ref: "docker.io/library/nginx:latest".to_string(),
            labels: HashMap::from([
                (
                    "io.kubernetes.pod.namespace".to_string(),
                    "default".to_string(),
                ),
                (
                    "io.kubernetes.pod.name".to_string(),
                    "runtimepulse-demo".to_string(),
                ),
                (
                    "io.kubernetes.container.name".to_string(),
                    "app".to_string(),
                ),
            ]),
            metadata: HashMap::new(),
            annotations: HashMap::new(),
            runtime_handler: String::new(),
        };

        let output = output_from_cri_event(event, &config).expect("lifecycle output");
        assert_eq!(output.metadata.sandboxes.len(), 1);
        assert_eq!(
            output.metadata.sandboxes[0]
                .get("id")
                .and_then(serde_json::Value::as_str),
            Some("k8s-default-runtimepulse-demo-app")
        );
        assert_eq!(
            output.events[0].sandbox_id.as_deref(),
            Some("k8s-default-runtimepulse-demo-app")
        );
        assert_eq!(
            output.metadata.sandboxes[0]
                .get("attributes")
                .and_then(|attributes| attributes.get("cri.short_id"))
                .and_then(serde_json::Value::as_str),
            Some("runtimepulse")
        );
    }

    #[test]
    fn parses_crictl_pod_sandbox_status_event() {
        let config = test_config();
        let content = r#"{
          "containerId": "sandboxabcdef1234567890",
          "containerEventType": "CONTAINER_STARTED_EVENT",
          "createdAt": "1779846067309998570",
          "podSandboxStatus": {
            "id": "sandboxabcdef1234567890",
            "metadata": {
              "name": "runtimepulse-cri-demo",
              "uid": "runtimepulse-cri-demo-uid",
              "namespace": "default",
              "attempt": 1
            },
            "state": "SANDBOX_READY",
            "runtimeHandler": "kata",
            "createdAt": "1779846067274109932",
            "labels": {
              "io.kubernetes.container.name": "POD",
              "io.kubernetes.pod.name": "runtimepulse-cri-demo",
              "io.kubernetes.pod.namespace": "default",
              "io.kubernetes.pod.uid": "runtimepulse-cri-demo-uid"
            }
          }
        }"#;

        let event = event_from_line(content).unwrap().expect("event");
        assert_eq!(event.sandbox_id, "sandboxabcdef1234567890");
        assert_eq!(event.container_id, "sandboxabcdef1234567890");
        assert_eq!(cri_event_action(&event), "SANDBOX_READY");
        assert_eq!(cri_event_runtime_handler(&event), "kata");
        assert_eq!(cri_event_runtime_type(&event), "kata");
        assert_eq!(
            event.metadata.get("name").map(String::as_str),
            Some("runtimepulse-cri-demo")
        );

        let output = output_from_cri_event(event, &config).expect("lifecycle output");
        assert_eq!(
            output.metadata.sandboxes[0]
                .get("id")
                .and_then(serde_json::Value::as_str),
            Some("k8s-default-runtimepulse-cri-demo-pod")
        );
        assert_eq!(
            output.metadata.sandboxes[0]
                .get("runtimeType")
                .and_then(serde_json::Value::as_str),
            Some("kata")
        );
    }

    #[test]
    fn maps_crictl_created_event_to_sandbox_created() {
        let content = r#"{
          "containerId": "sandboxcreated123",
          "containerEventType": "CONTAINER_CREATED_EVENT",
          "createdAt": "1779846067309998570",
          "podSandboxStatus": {
            "id": "sandboxcreated123",
            "metadata": {"name": "runtimepulse-cri-demo", "namespace": "default", "uid": "uid", "attempt": 1},
            "state": "SANDBOX_READY",
            "createdAt": "1779846067274109932",
            "labels": {
              "io.kubernetes.container.name": "POD",
              "io.kubernetes.pod.name": "runtimepulse-cri-demo",
              "io.kubernetes.pod.namespace": "default",
              "io.kubernetes.pod.uid": "uid"
            }
          }
        }"#;

        let event = event_from_line(content).unwrap().expect("event");
        assert_eq!(cri_event_action(&event), "SANDBOX_CREATED");
        assert_eq!(event.reason, "CONTAINER_CREATED");
    }

    fn test_config() -> CollectorConfig {
        CollectorConfig {
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
            image_cache_report_command: None,
            image_cache_report_command_timeout: Duration::from_secs(5),
            profile_report_path: None,
            diagnostic_report_path: None,
            diagnostic_report_command: None,
            diagnostic_report_command_timeout: Duration::from_secs(1),
            perf_report_path: None,
            perf_script_path: None,
            perf_script_command: None,
            perf_script_command_timeout: Duration::from_secs(1),
            perf_folded_path: None,
            ebpf_report_path: None,
            ebpf_folded_path: None,
            perf_profile_command: None,
            ebpf_profile_command: None,
            profile_command_timeout: Duration::from_secs(1),
            plugins: Vec::new(),
            command_plugins: Vec::new(),
            http_plugins: Vec::new(),
        }
    }
}
