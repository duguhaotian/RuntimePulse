//! containerd runtime inventory source.
//!
//! Talks to containerd through its gRPC API over the host Unix socket. The
//! host-agent is expected to run as root, so it can open the containerd socket
//! directly instead of shelling out to `ctr`.

use chrono::{DateTime, SecondsFormat, Utc};
use containerd_client::events::{
    ContainerCreate, ContainerDelete, ContainerUpdate, ContentCreate, ContentDelete,
    SnapshotCommit, SnapshotPrepare, SnapshotRemove, TaskCreate, TaskDelete, TaskExit, TaskOom,
    TaskPaused, TaskResumed, TaskStart,
};
use containerd_client::services::v1::{
    Container, GetContainerRequest, Image, Info, ListContainersRequest, ListContentRequest,
    ListImagesRequest, ListNamespacesRequest, SubscribeRequest,
};
use containerd_client::tonic::{Code, Request};
use containerd_client::{with_namespace, Client};
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, HashMap};
use std::env;
use std::path::PathBuf;

use crate::collectors::core::config::CollectorConfig;
use crate::collectors::core::error::{CollectorError, Result};
use crate::collectors::core::model::{EventRecord, Metadata, PluginOutput};

const DEFAULT_CONTAINERD_SOCKET: &str = "/run/containerd/containerd.sock";

#[derive(Clone, Debug)]
pub struct ContainerdEvent {
    pub namespace: String,
    pub action: String,
    pub container_id: String,
    pub image: Option<String>,
    pub runtime_name: Option<String>,
    pub labels: HashMap<String, String>,
    pub timestamp: DateTime<Utc>,
    pub exit_status: Option<u32>,
    pub pid: Option<u32>,
    pub topic: String,
}

#[derive(Clone, Debug)]
pub struct ContainerdImageEvent {
    pub namespace: String,
    pub action: String,
    pub image_ref: String,
    pub image_digest: String,
    pub content_digest: Option<String>,
    pub snapshot_key: Option<String>,
    pub snapshotter: Option<String>,
    pub bytes: Option<u64>,
    pub phase: String,
    pub timestamp: DateTime<Utc>,
    pub topic: String,
}

#[derive(Clone, Debug)]
pub enum ContainerdRuntimeEvent {
    Container(ContainerdEvent),
    Image(ContainerdImageEvent),
}

struct ContainerdSandboxIdentity {
    sandbox_id: String,
    namespace: String,
    workload_id: String,
    workload_name: String,
    runtime_sandbox_id: String,
    kubernetes_namespace: String,
    pod_name: String,
    container_name: String,
    pod_uid: String,
}

pub fn collect_containerd_inventory(
    now: DateTime<Utc>,
    config: &CollectorConfig,
) -> Result<PluginOutput> {
    let socket = containerd_socket_path();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_io()
        .build()?;
    runtime.block_on(collect_containerd_inventory_async(now, config, socket))
}

pub fn stream_containerd_events<F>(config: &CollectorConfig, mut on_event: F) -> Result<()>
where
    F: FnMut(ContainerdRuntimeEvent) -> Result<()>,
{
    let socket = containerd_socket_path();
    let namespace_filter = namespace_filter();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_io()
        .build()?;
    runtime.block_on(stream_containerd_events_async(
        config,
        socket,
        namespace_filter,
        |event| on_event(event),
    ))
}

pub fn output_from_runtime_event(
    event: ContainerdRuntimeEvent,
    config: &CollectorConfig,
) -> Result<Option<PluginOutput>> {
    match event {
        ContainerdRuntimeEvent::Container(event) => output_from_event(event, config),
        ContainerdRuntimeEvent::Image(event) => output_from_image_event(event, config),
    }
}

pub fn output_from_event(
    event: ContainerdEvent,
    config: &CollectorConfig,
) -> Result<Option<PluginOutput>> {
    let timestamp = timestamp(event.timestamp);
    let identity =
        containerd_identity_from_labels(&event.namespace, &event.container_id, &event.labels);
    let sandbox_id = identity.sandbox_id.clone();
    let workload_name = identity.workload_name.clone();
    let image_ref = event
        .image
        .clone()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "containerd/unknown:latest".to_string());
    let image_id = containerd_image_id(&event.namespace, &image_ref);
    let runtime_version = event
        .runtime_name
        .clone()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "containerd-events".to_string());
    let runtime_type = runtime_type_from_name(&runtime_version);
    let lifecycle_status = sandbox_status_from_action(&event.action, event.exit_status);
    let current = is_current_action(&event.action);
    let removed = is_removed_action(&event.action);

    let mut sandbox = json!({
        "id": sandbox_id,
        "clusterId": config.cluster_id,
        "nodeId": config.node_id,
        "namespace": identity.namespace,
        "workloadId": identity.workload_id,
        "workloadName": workload_name,
        "imageId": image_id,
        "imageRef": image_ref,
        "runtimeType": runtime_type,
        "runtimeVersion": runtime_version,
        "status": lifecycle_status,
        "createdAt": timestamp,
        "startupDurationMs": 0,
        "cpuAvg": 0,
        "memoryPeakBytes": 0,
        "labels": {
            "collector": "runtimepulse-rust-collector",
            "plugin": "containerd-events",
            "scope": config.collection_scope,
        },
        "attributes": {
            "collector.scope": config.collection_scope,
            "runtime.source": "containerd",
            "containerd.namespace": event.namespace,
            "containerd.id": event.container_id,
            "containerd.runtime_sandbox_id": identity.runtime_sandbox_id,
            "containerd.topic": event.topic,
            "containerd.action": event.action,
            "k8s.namespace": identity.kubernetes_namespace,
            "k8s.pod": identity.pod_name,
            "k8s.container": identity.container_name,
            "k8s.pod_uid": identity.pod_uid,
            "lifecycle.action": if removed { "destroy" } else { event.action.as_str() },
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

    let mut attributes = Map::new();
    attributes.insert("plugin".to_string(), json!("containerd-events"));
    attributes.insert("scope".to_string(), json!(config.collection_scope));
    attributes.insert("containerd.namespace".to_string(), json!(event.namespace));
    attributes.insert("containerd.id".to_string(), json!(event.container_id));
    attributes.insert(
        "containerd.runtime_sandbox_id".to_string(),
        json!(identity.runtime_sandbox_id),
    );
    attributes.insert("containerd.topic".to_string(), json!(event.topic));
    attributes.insert("containerd.action".to_string(), json!(event.action));
    attributes.insert(
        "k8s.namespace".to_string(),
        json!(identity.kubernetes_namespace),
    );
    attributes.insert("k8s.pod".to_string(), json!(identity.pod_name));
    attributes.insert("k8s.container".to_string(), json!(identity.container_name));
    attributes.insert("k8s.pod_uid".to_string(), json!(identity.pod_uid));
    if let Some(pid) = event.pid {
        attributes.insert("containerd.pid".to_string(), json!(pid));
    }
    if let Some(exit_status) = event.exit_status {
        attributes.insert("containerd.exitStatus".to_string(), json!(exit_status));
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
                    "plugin": "containerd-events",
                    "scope": config.collection_scope,
                }
            })],
            images: vec![image_row(
                &image_id,
                &image_ref,
                &format!("collector:{image_id}"),
                0,
                0,
                &event.namespace,
            )],
            sandboxes: vec![sandbox],
        },
        metrics: Vec::new(),
        events: vec![EventRecord {
            id: format!(
                "containerd-{}-{}-{}-{}",
                sanitize_id(&event.namespace),
                short_container_id(&event.container_id),
                sanitize_id(&event.action),
                event.timestamp.timestamp_micros()
            ),
            timestamp: timestamp.clone(),
            severity: event_severity(&event.action, event.exit_status).to_string(),
            event_type: "container".to_string(),
            event_name: format!("containerd.container.{}", event.action),
            message: format!(
                "containerd container {workload_name} emitted {} in namespace {}.",
                event.action, event.namespace
            ),
            source: format!(
                "runtimepulse-rust-collector/{}/containerd-events",
                config.node_id
            ),
            attributes,
            sandbox_id: Some(identity.sandbox_id),
            image_id: None,
            node_id: Some(config.node_id.clone()),
            runtime_type: Some(runtime_type),
            reason: event_reason(&event.action, event.exit_status),
        }],
        traces: Vec::new(),
        profiles: Vec::new(),
    }))
}

fn output_from_image_event(
    event: ContainerdImageEvent,
    config: &CollectorConfig,
) -> Result<Option<PluginOutput>> {
    let timestamp = timestamp(event.timestamp);
    let image_id = containerd_image_id(&event.namespace, &event.image_ref);
    let step_id = format!(
        "{}-{}-{}",
        image_id,
        sanitize_id(&event.phase),
        event
            .content_digest
            .as_deref()
            .or(event.snapshot_key.as_deref())
            .map(sanitize_id)
            .unwrap_or_else(|| event.timestamp.timestamp_micros().to_string())
    );
    let timeline_step = json!({
        "id": step_id,
        "name": image_stage_name(&event),
        "phase": event.phase,
        "durationMs": 0,
        "bytes": event.bytes,
        "timestamp": timestamp,
        "detail": image_stage_detail(&event),
    });

    let mut image = image_row(
        &image_id,
        &event.image_ref,
        &event.image_digest,
        event.bytes.unwrap_or(0),
        0,
        &event.namespace,
    );
    image["downloadTimeline"] = json!([timeline_step]);

    let mut attributes = Map::new();
    attributes.insert("plugin".to_string(), json!("containerd-events"));
    attributes.insert("scope".to_string(), json!(config.collection_scope));
    attributes.insert("containerd.namespace".to_string(), json!(event.namespace));
    attributes.insert("containerd.topic".to_string(), json!(event.topic));
    attributes.insert("image.id".to_string(), json!(image_id));
    attributes.insert("image.ref".to_string(), json!(event.image_ref));
    attributes.insert("image.digest".to_string(), json!(event.image_digest));
    attributes.insert("image.phase".to_string(), json!(event.phase));
    if let Some(content_digest) = &event.content_digest {
        attributes.insert(
            "containerd.content.digest".to_string(),
            json!(content_digest),
        );
    }
    if let Some(snapshot_key) = &event.snapshot_key {
        attributes.insert("containerd.snapshot.key".to_string(), json!(snapshot_key));
    }
    if let Some(snapshotter) = &event.snapshotter {
        attributes.insert("containerd.snapshotter".to_string(), json!(snapshotter));
    }
    if let Some(bytes) = event.bytes {
        attributes.insert("image.bytes".to_string(), json!(bytes));
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
                    "plugin": "containerd-events",
                    "scope": config.collection_scope,
                }
            })],
            images: vec![image],
            sandboxes: Vec::new(),
        },
        metrics: Vec::new(),
        events: vec![EventRecord {
            id: format!(
                "containerd-image-{}-{}-{}",
                sanitize_id(&event.namespace),
                sanitize_id(&event.action),
                event.timestamp.timestamp_micros()
            ),
            timestamp,
            severity: image_event_severity(&event.action).to_string(),
            event_type: "image".to_string(),
            event_name: format!("containerd.image.{}", event.action),
            message: format!(
                "containerd image {} observed {} in namespace {}.",
                event.image_ref, event.action, event.namespace
            ),
            source: format!(
                "runtimepulse-rust-collector/{}/containerd-events",
                config.node_id
            ),
            attributes,
            sandbox_id: None,
            image_id: Some(image_id),
            node_id: Some(config.node_id.clone()),
            runtime_type: None,
            reason: None,
        }],
        traces: Vec::new(),
        profiles: Vec::new(),
    }))
}

async fn collect_containerd_inventory_async(
    now: DateTime<Utc>,
    config: &CollectorConfig,
    socket: PathBuf,
) -> Result<PluginOutput> {
    let client = Client::from_path(socket)
        .await
        .map_err(|error| CollectorError::Plugin {
            plugin: "containerd".to_string(),
            message: error.to_string(),
        })?;
    let ts = timestamp(now);
    let namespaces = if let Some(namespaces) = namespace_filter() {
        namespaces
    } else {
        async_list_namespaces(&client).await?
    };
    let mut images = BTreeMap::new();
    let mut sandboxes = Vec::new();
    let mut events = Vec::new();

    for namespace in namespaces {
        let content = async_list_content(&client, &namespace)
            .await
            .unwrap_or_default();
        for image in async_list_images(&client, &namespace).await? {
            let row = image_row_from_containerd(&namespace, &image, &content);
            if let Some(id) = row.get("id").and_then(Value::as_str) {
                images.insert(id.to_string(), row);
            }
        }

        for container in async_list_containers(&client, &namespace).await? {
            let image_ref = if container.image.is_empty() {
                "containerd/unknown:latest".to_string()
            } else {
                container.image.clone()
            };
            let image_id = containerd_image_id(&namespace, &image_ref);
            let identity =
                containerd_identity_from_labels(&namespace, &container.id, &container.labels);
            let runtime_type = runtime_type_from_containerd(&container);
            let runtime_version = runtime_name_from_containerd(&container)
                .unwrap_or_else(|| "containerd".to_string());
            let workload_name = identity.workload_name.clone();
            let created_at = container
                .created_at
                .as_ref()
                .and_then(timestamp_from_prost)
                .unwrap_or_else(|| ts.clone());

            images.entry(image_id.clone()).or_insert_with(|| {
                image_row(
                    &image_id,
                    &image_ref,
                    "containerd:unknown",
                    0,
                    0,
                    &namespace,
                )
            });

            sandboxes.push(json!({
                "id": identity.sandbox_id,
                "clusterId": config.cluster_id,
                "nodeId": config.node_id,
                "namespace": identity.namespace,
                "workloadId": identity.workload_id,
                "workloadName": workload_name,
                "imageId": image_id,
                "imageRef": image_ref,
                "runtimeType": runtime_type,
                "runtimeVersion": runtime_version,
                "status": "running",
                "createdAt": created_at,
                "startupDurationMs": 0,
                "cpuAvg": 0,
                "memoryPeakBytes": 0,
                "labels": {
                    "collector": "runtimepulse-rust-collector",
                    "plugin": "containerd",
                    "scope": config.collection_scope,
                },
                "attributes": {
                    "collector.scope": config.collection_scope,
                    "runtime.source": "containerd",
                    "containerd.namespace": namespace,
                    "containerd.id": container.id,
                    "containerd.runtime_sandbox_id": identity.runtime_sandbox_id,
                    "containerd.snapshotter": container.snapshotter,
                    "containerd.snapshotKey": container.snapshot_key,
                    "containerd.sandbox": container.sandbox,
                    "k8s.namespace": identity.kubernetes_namespace,
                    "k8s.pod": identity.pod_name,
                    "k8s.container": identity.container_name,
                    "k8s.pod_uid": identity.pod_uid,
                }
            }));

            let mut attributes = Map::new();
            attributes.insert("plugin".to_string(), json!("containerd"));
            attributes.insert("scope".to_string(), json!(config.collection_scope));
            attributes.insert("containerd.namespace".to_string(), json!(namespace));
            attributes.insert("containerd.id".to_string(), json!(container.id));
            attributes.insert(
                "containerd.runtime_sandbox_id".to_string(),
                json!(identity.runtime_sandbox_id),
            );
            attributes.insert(
                "containerd.snapshotter".to_string(),
                json!(container.snapshotter),
            );
            attributes.insert(
                "k8s.namespace".to_string(),
                json!(identity.kubernetes_namespace),
            );
            attributes.insert("k8s.pod".to_string(), json!(identity.pod_name));
            attributes.insert("k8s.container".to_string(), json!(identity.container_name));
            attributes.insert("k8s.pod_uid".to_string(), json!(identity.pod_uid));

            events.push(EventRecord {
                id: format!(
                    "containerd-{}-{}-observed-{}",
                    sanitize_id(&namespace),
                    sanitize_id(&container.id),
                    now.timestamp()
                ),
                timestamp: ts.clone(),
                severity: "info".to_string(),
                event_type: "container".to_string(),
                event_name: "containerd.container.observed".to_string(),
                message: format!(
                    "containerd container {workload_name} is visible in namespace {namespace}."
                ),
                source: format!("runtimepulse-rust-collector/{}/containerd", config.node_id),
                attributes,
                sandbox_id: Some(identity.sandbox_id),
                image_id: None,
                node_id: Some(config.node_id.clone()),
                runtime_type: Some(runtime_type),
                reason: None,
            });
        }
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
                "status": "ready",
                "labels": {
                    "collector": "runtimepulse-rust-collector",
                    "plugin": "containerd",
                    "scope": config.collection_scope,
                }
            })],
            images: images.into_values().collect(),
            sandboxes,
        },
        metrics: Vec::new(),
        events,
        traces: Vec::new(),
        profiles: Vec::new(),
    })
}

async fn stream_containerd_events_async<F>(
    _config: &CollectorConfig,
    socket: PathBuf,
    namespace_filter: Option<Vec<String>>,
    mut on_event: F,
) -> Result<()>
where
    F: FnMut(ContainerdRuntimeEvent) -> Result<()>,
{
    let client = Client::from_path(socket)
        .await
        .map_err(|error| CollectorError::Plugin {
            plugin: "containerd-events".to_string(),
            message: error.to_string(),
        })?;
    let filters = namespace_filter
        .as_ref()
        .map(|namespaces| {
            namespaces
                .iter()
                .map(|namespace| format!("namespace=={namespace}"))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let mut stream = client
        .events()
        .subscribe(SubscribeRequest { filters })
        .await
        .map_err(containerd_events_status)?
        .into_inner();

    while let Some(envelope) = stream.message().await.map_err(containerd_events_status)? {
        let Some(event) =
            event_from_envelope(&client, envelope, namespace_filter.as_deref()).await?
        else {
            continue;
        };
        on_event(event)?;
    }

    Ok(())
}

async fn event_from_envelope(
    client: &Client,
    envelope: containerd_client::types::Envelope,
    namespace_filter: Option<&[String]>,
) -> Result<Option<ContainerdRuntimeEvent>> {
    if !namespace_allowed(&envelope.namespace, namespace_filter) {
        return Ok(None);
    }

    let timestamp = envelope
        .timestamp
        .as_ref()
        .and_then(|value| DateTime::from_timestamp(value.seconds, value.nanos as u32))
        .unwrap_or_else(Utc::now);
    let topic = envelope.topic.clone();
    let Some(mut payload) = envelope.event else {
        return Ok(None);
    };
    if !payload.type_url.starts_with('/') {
        payload.type_url.insert(0, '/');
    }

    let mut event = match topic.as_str() {
        "/containers/create" => {
            let payload: ContainerCreate = decode_containerd_event(&payload)?;
            Some(ContainerdRuntimeEvent::Container(ContainerdEvent {
                namespace: envelope.namespace,
                action: "create".to_string(),
                container_id: payload.id,
                image: Some(payload.image),
                runtime_name: payload.runtime.map(|runtime| runtime.name),
                labels: HashMap::new(),
                timestamp,
                exit_status: None,
                pid: None,
                topic,
            }))
        }
        "/containers/update" => {
            let payload: ContainerUpdate = decode_containerd_event(&payload)?;
            Some(ContainerdRuntimeEvent::Container(ContainerdEvent {
                namespace: envelope.namespace,
                action: "update".to_string(),
                container_id: payload.id,
                image: Some(payload.image),
                runtime_name: None,
                labels: HashMap::new(),
                timestamp,
                exit_status: None,
                pid: None,
                topic,
            }))
        }
        "/containers/delete" => {
            let payload: ContainerDelete = decode_containerd_event(&payload)?;
            Some(ContainerdRuntimeEvent::Container(ContainerdEvent {
                namespace: envelope.namespace,
                action: "delete".to_string(),
                container_id: payload.id,
                image: None,
                runtime_name: None,
                labels: HashMap::new(),
                timestamp,
                exit_status: None,
                pid: None,
                topic,
            }))
        }
        "/tasks/create" => {
            let payload: TaskCreate = decode_containerd_event(&payload)?;
            Some(ContainerdRuntimeEvent::Container(ContainerdEvent {
                namespace: envelope.namespace,
                action: "task_create".to_string(),
                container_id: payload.container_id,
                image: None,
                runtime_name: None,
                labels: HashMap::new(),
                timestamp,
                exit_status: None,
                pid: Some(payload.pid),
                topic,
            }))
        }
        "/tasks/start" => {
            let payload: TaskStart = decode_containerd_event(&payload)?;
            Some(ContainerdRuntimeEvent::Container(ContainerdEvent {
                namespace: envelope.namespace,
                action: "start".to_string(),
                container_id: payload.container_id,
                image: None,
                runtime_name: None,
                labels: HashMap::new(),
                timestamp,
                exit_status: None,
                pid: Some(payload.pid),
                topic,
            }))
        }
        "/tasks/exit" => {
            let payload: TaskExit = decode_containerd_event(&payload)?;
            Some(ContainerdRuntimeEvent::Container(ContainerdEvent {
                namespace: envelope.namespace,
                action: "exit".to_string(),
                container_id: payload.container_id,
                image: None,
                runtime_name: None,
                labels: HashMap::new(),
                timestamp,
                exit_status: Some(payload.exit_status),
                pid: Some(payload.pid),
                topic,
            }))
        }
        "/tasks/delete" => {
            let payload: TaskDelete = decode_containerd_event(&payload)?;
            Some(ContainerdRuntimeEvent::Container(ContainerdEvent {
                namespace: envelope.namespace,
                action: "task_delete".to_string(),
                container_id: payload.container_id,
                image: None,
                runtime_name: None,
                labels: HashMap::new(),
                timestamp,
                exit_status: Some(payload.exit_status),
                pid: Some(payload.pid),
                topic,
            }))
        }
        "/tasks/oom" => {
            let payload: TaskOom = decode_containerd_event(&payload)?;
            Some(ContainerdRuntimeEvent::Container(ContainerdEvent {
                namespace: envelope.namespace,
                action: "oom".to_string(),
                container_id: payload.container_id,
                image: None,
                runtime_name: None,
                labels: HashMap::new(),
                timestamp,
                exit_status: None,
                pid: None,
                topic,
            }))
        }
        "/tasks/paused" => {
            let payload: TaskPaused = decode_containerd_event(&payload)?;
            Some(ContainerdRuntimeEvent::Container(ContainerdEvent {
                namespace: envelope.namespace,
                action: "pause".to_string(),
                container_id: payload.container_id,
                image: None,
                runtime_name: None,
                labels: HashMap::new(),
                timestamp,
                exit_status: None,
                pid: None,
                topic,
            }))
        }
        "/tasks/resumed" => {
            let payload: TaskResumed = decode_containerd_event(&payload)?;
            Some(ContainerdRuntimeEvent::Container(ContainerdEvent {
                namespace: envelope.namespace,
                action: "resume".to_string(),
                container_id: payload.container_id,
                image: None,
                runtime_name: None,
                labels: HashMap::new(),
                timestamp,
                exit_status: None,
                pid: None,
                topic,
            }))
        }
        "/content/create" => {
            let payload: ContentCreate = decode_containerd_event(&payload)?;
            let (image_ref, image_digest) =
                image_identity_for_digest(client, &envelope.namespace, &payload.digest).await?;
            Some(ContainerdRuntimeEvent::Image(ContainerdImageEvent {
                namespace: envelope.namespace,
                action: "content_create".to_string(),
                image_ref,
                image_digest,
                content_digest: Some(payload.digest),
                snapshot_key: None,
                snapshotter: None,
                bytes: Some(payload.size.max(0) as u64),
                phase: "pull".to_string(),
                timestamp,
                topic,
            }))
        }
        "/content/delete" => {
            let payload: ContentDelete = decode_containerd_event(&payload)?;
            let (image_ref, image_digest) =
                image_identity_for_digest(client, &envelope.namespace, &payload.digest).await?;
            Some(ContainerdRuntimeEvent::Image(ContainerdImageEvent {
                namespace: envelope.namespace,
                action: "content_delete".to_string(),
                image_ref,
                image_digest,
                content_digest: Some(payload.digest),
                snapshot_key: None,
                snapshotter: None,
                bytes: None,
                phase: "pull".to_string(),
                timestamp,
                topic,
            }))
        }
        "/snapshot/prepare" => {
            let payload: SnapshotPrepare = decode_containerd_event(&payload)?;
            let image_ref = snapshot_image_ref(&envelope.namespace, &payload.key);
            Some(ContainerdRuntimeEvent::Image(ContainerdImageEvent {
                namespace: envelope.namespace,
                action: "snapshot_prepare".to_string(),
                image_ref,
                image_digest: format!("snapshot:{}", payload.key),
                content_digest: None,
                snapshot_key: Some(payload.key),
                snapshotter: Some(payload.snapshotter),
                bytes: None,
                phase: "unpack".to_string(),
                timestamp,
                topic,
            }))
        }
        "/snapshot/commit" => {
            let payload: SnapshotCommit = decode_containerd_event(&payload)?;
            let image_ref = snapshot_image_ref(&envelope.namespace, &payload.name);
            Some(ContainerdRuntimeEvent::Image(ContainerdImageEvent {
                namespace: envelope.namespace,
                action: "snapshot_commit".to_string(),
                image_ref,
                image_digest: format!("snapshot:{}", payload.name),
                content_digest: None,
                snapshot_key: Some(payload.name),
                snapshotter: Some(payload.snapshotter),
                bytes: None,
                phase: "snapshot".to_string(),
                timestamp,
                topic,
            }))
        }
        "/snapshot/remove" => {
            let payload: SnapshotRemove = decode_containerd_event(&payload)?;
            let image_ref = snapshot_image_ref(&envelope.namespace, &payload.key);
            Some(ContainerdRuntimeEvent::Image(ContainerdImageEvent {
                namespace: envelope.namespace,
                action: "snapshot_remove".to_string(),
                image_ref,
                image_digest: format!("snapshot:{}", payload.key),
                content_digest: None,
                snapshot_key: Some(payload.key),
                snapshotter: Some(payload.snapshotter),
                bytes: None,
                phase: "snapshot".to_string(),
                timestamp,
                topic,
            }))
        }
        _ => None,
    };

    if let Some(ContainerdRuntimeEvent::Container(event)) = event.as_mut() {
        if (event.image.is_none() || event.runtime_name.is_none() || event.labels.is_empty())
            && event.action != "delete"
        {
            if let Some(container) =
                async_get_container(client, &event.namespace, &event.container_id).await?
            {
                if event.image.is_none() && !container.image.is_empty() {
                    event.image = Some(container.image.clone());
                }
                if event.runtime_name.is_none() {
                    event.runtime_name = runtime_name_from_containerd(&container);
                }
                if event.labels.is_empty() {
                    event.labels = container.labels;
                }
            }
        }
    }

    Ok(event)
}

async fn async_list_namespaces(client: &Client) -> Result<Vec<String>> {
    let response = client
        .namespaces()
        .list(ListNamespacesRequest {
            filter: String::new(),
        })
        .await
        .map_err(containerd_status)?;
    Ok(response
        .into_inner()
        .namespaces
        .into_iter()
        .map(|namespace| namespace.name)
        .filter(|name| !name.is_empty())
        .collect())
}

async fn async_list_images(client: &Client, namespace: &str) -> Result<Vec<Image>> {
    let response = client
        .images()
        .list(with_namespace!(
            ListImagesRequest {
                filters: Vec::new()
            },
            namespace
        ))
        .await
        .map_err(containerd_status)?;
    Ok(response.into_inner().images)
}

async fn async_list_containers(client: &Client, namespace: &str) -> Result<Vec<Container>> {
    let response = client
        .containers()
        .list(with_namespace!(
            ListContainersRequest {
                filters: Vec::new()
            },
            namespace
        ))
        .await
        .map_err(containerd_status)?;
    Ok(response.into_inner().containers)
}

async fn async_list_content(client: &Client, namespace: &str) -> Result<Vec<Info>> {
    let mut stream = client
        .content()
        .list(with_namespace!(
            ListContentRequest {
                filters: Vec::new()
            },
            namespace
        ))
        .await
        .map_err(containerd_status)?
        .into_inner();
    let mut content = Vec::new();

    while let Some(response) = stream.message().await.map_err(containerd_status)? {
        content.extend(response.info);
    }

    Ok(content)
}

async fn async_get_container(
    client: &Client,
    namespace: &str,
    container_id: &str,
) -> Result<Option<Container>> {
    match client
        .containers()
        .get(with_namespace!(
            GetContainerRequest {
                id: container_id.to_string()
            },
            namespace
        ))
        .await
    {
        Ok(response) => Ok(response.into_inner().container),
        Err(status) if status.code() == Code::NotFound => Ok(None),
        Err(status) => Err(containerd_status(status)),
    }
}

async fn image_identity_for_digest(
    client: &Client,
    namespace: &str,
    digest: &str,
) -> Result<(String, String)> {
    for image in async_list_images(client, namespace)
        .await
        .unwrap_or_default()
    {
        if image_references_digest(&image, digest) {
            let image_digest = image
                .target
                .as_ref()
                .map(|target| target.digest.clone())
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| digest.to_string());
            return Ok((image.name, image_digest));
        }
    }

    Ok((format!("containerd-content:{digest}"), digest.to_string()))
}

fn image_references_digest(image: &Image, digest: &str) -> bool {
    image
        .target
        .as_ref()
        .is_some_and(|target| target.digest == digest)
        || image
            .target
            .as_ref()
            .is_some_and(|target| target.annotations.values().any(|value| value == digest))
}

fn image_row_from_containerd(namespace: &str, image: &Image, content: &[Info]) -> Value {
    let (digest, size_bytes, layer_count, layers) = image
        .target
        .as_ref()
        .map(|target| {
            let digest = if target.digest.is_empty() {
                "containerd:unknown".to_string()
            } else {
                target.digest.clone()
            };
            let refs = content_refs_for_target(target, content);
            let layer_refs = refs
                .iter()
                .filter(|item| is_containerd_layer_ref(&item.label))
                .collect::<Vec<_>>();
            let sized_refs = if layer_refs.is_empty() {
                refs.iter().collect::<Vec<_>>()
            } else {
                layer_refs
            };
            let size_bytes = sized_refs
                .iter()
                .map(|item| item.size_bytes)
                .sum::<u64>()
                .max(target.size.max(0) as u64);
            let layers = sized_refs
                .iter()
                .enumerate()
                .map(|(index, item)| content_layer_row(index, item))
                .collect::<Vec<_>>();
            (digest, size_bytes, layers.len() as u64, layers)
        })
        .unwrap_or_else(|| ("containerd:unknown".to_string(), 0, 0, Vec::new()));

    let mut row = image_row(
        &containerd_image_id(namespace, &image.name),
        &image.name,
        &digest,
        size_bytes,
        layer_count,
        namespace,
    );
    if !layers.is_empty() {
        row["layers"] = json!(layers);
    }
    row
}

#[derive(Clone, Debug)]
struct ContainerdContentRef {
    digest: String,
    label: String,
    size_bytes: u64,
}

fn content_refs_for_target(
    target: &containerd_client::types::Descriptor,
    content: &[Info],
) -> Vec<ContainerdContentRef> {
    let content_by_digest = content
        .iter()
        .map(|info| (info.digest.as_str(), info))
        .collect::<HashMap<_, _>>();
    let mut refs = BTreeMap::new();

    for (label, digest) in target
        .annotations
        .iter()
        .filter(|(key, _)| is_content_ref_key(key))
    {
        refs.insert(digest.clone(), label.clone());
    }

    if let Some(target_info) = content_by_digest.get(target.digest.as_str()) {
        for (label, digest) in target_info
            .labels
            .iter()
            .filter(|(key, _)| is_content_ref_key(key))
        {
            refs.insert(digest.clone(), label.clone());
        }
    }

    refs.into_iter()
        .map(|(digest, label)| {
            let size_bytes = content_by_digest
                .get(digest.as_str())
                .map(|info| info.size.max(0) as u64)
                .unwrap_or(0);
            ContainerdContentRef {
                digest,
                label,
                size_bytes,
            }
        })
        .collect()
}

fn is_content_ref_key(key: &str) -> bool {
    key.contains("containerd.io/gc.ref.content")
}

fn is_containerd_layer_ref(label: &str) -> bool {
    label.contains(".l.") || label.contains(".layer") || label.ends_with(".layer")
}

fn content_layer_row(index: usize, item: &ContainerdContentRef) -> Value {
    json!({
        "id": item.digest,
        "command": format!("containerd content {}", short_digest(&item.digest)),
        "sizeBytes": item.size_bytes,
        "blockSizeBytes": 0,
        "blockCount": 0,
        "requestedBlockCount": 0,
        "cacheHitBlockCount": 0,
        "localReadBytes": item.size_bytes,
        "remoteReadBytes": 0,
        "pullDurationMs": 0,
        "unpackDurationMs": 0,
        "attributes": {
            "containerd.content.label": item.label,
            "containerd.content.index": index,
        }
    })
}

fn image_row(
    id: &str,
    reference: &str,
    digest: &str,
    size_bytes: u64,
    layer_count: u64,
    namespace: &str,
) -> Value {
    json!({
        "id": id,
        "ref": reference,
        "digest": digest,
        "loadingMode": if reference.contains("nydus") || reference.contains("stargz") || reference.contains("overlaybd") { "lazy" } else { "eager" },
        "sizeBytes": size_bytes,
        "layerCount": layer_count,
        "attributes": {
            "containerd.namespace": namespace,
            "runtime.source": "containerd",
        }
    })
}

fn runtime_name_from_containerd(container: &Container) -> Option<String> {
    container.runtime.as_ref().and_then(|runtime| {
        if runtime.name.is_empty() {
            None
        } else {
            Some(runtime.name.clone())
        }
    })
}

fn runtime_type_from_containerd(container: &Container) -> String {
    let runtime = runtime_name_from_containerd(container)
        .unwrap_or_else(|| "runc".to_string())
        .to_ascii_lowercase();
    runtime_type_from_name(&runtime)
}

fn runtime_type_from_name(runtime: &str) -> String {
    let runtime = runtime.to_ascii_lowercase();
    if runtime.contains("runsc") || runtime.contains("gvisor") {
        "gvisor".to_string()
    } else if runtime.contains("kata") {
        "kata".to_string()
    } else if runtime.contains("firecracker") {
        "firecracker".to_string()
    } else {
        "runc".to_string()
    }
}

fn sandbox_status_from_action(action: &str, exit_status: Option<u32>) -> &'static str {
    match action {
        "start" | "resume" => "running",
        "oom" => "failed",
        "exit" | "task_delete" if exit_status.is_some_and(|code| code != 0) => "failed",
        "exit" | "task_delete" | "delete" | "pause" | "create" | "task_create" => "stopped",
        _ => "running",
    }
}

fn is_current_action(action: &str) -> bool {
    matches!(action, "start" | "resume")
}

fn is_removed_action(action: &str) -> bool {
    matches!(action, "delete")
}

fn event_severity(action: &str, exit_status: Option<u32>) -> &'static str {
    match action {
        "oom" => "error",
        "exit" | "task_delete" if exit_status.is_some_and(|code| code != 0) => "error",
        "exit" | "task_delete" | "delete" => "warning",
        _ => "info",
    }
}

fn event_reason(action: &str, exit_status: Option<u32>) -> Option<String> {
    match action {
        "oom" => Some("oom".to_string()),
        "exit" | "task_delete" => exit_status.and_then(|code| {
            if code == 0 {
                None
            } else {
                Some(format!("exit_code:{code}"))
            }
        }),
        _ => None,
    }
}

fn image_event_severity(action: &str) -> &'static str {
    match action {
        "content_delete" | "snapshot_remove" => "warning",
        _ => "info",
    }
}

fn image_stage_name(event: &ContainerdImageEvent) -> &'static str {
    match event.action.as_str() {
        "content_create" => "Content blob available",
        "content_delete" => "Content blob removed",
        "snapshot_prepare" => "Prepare snapshot",
        "snapshot_commit" => "Commit snapshot",
        "snapshot_remove" => "Remove snapshot",
        _ => "containerd image event",
    }
}

fn image_stage_detail(event: &ContainerdImageEvent) -> String {
    match event.action.as_str() {
        "content_create" => format!(
            "containerd content blob {} became available{}.",
            event.content_digest.as_deref().unwrap_or("unknown"),
            event
                .bytes
                .map(|bytes| format!(" with {bytes} bytes"))
                .unwrap_or_default()
        ),
        "content_delete" => format!(
            "containerd content blob {} was removed.",
            event.content_digest.as_deref().unwrap_or("unknown")
        ),
        "snapshot_prepare" | "snapshot_commit" | "snapshot_remove" => format!(
            "containerd snapshot {} via {}.",
            event.snapshot_key.as_deref().unwrap_or("unknown"),
            event
                .snapshotter
                .as_deref()
                .unwrap_or("unknown snapshotter")
        ),
        _ => "containerd image event observed.".to_string(),
    }
}

fn snapshot_image_ref(namespace: &str, key: &str) -> String {
    format!("containerd-snapshot:{namespace}:{key}")
}

fn namespace_allowed(namespace: &str, filter: Option<&[String]>) -> bool {
    filter
        .map(|namespaces| namespaces.iter().any(|item| item == namespace))
        .unwrap_or(true)
}

fn decode_containerd_event<M>(payload: &prost_types::Any) -> Result<M>
where
    M: Default + prost::Name + Sized,
{
    payload.to_msg().map_err(|error| CollectorError::Plugin {
        plugin: "containerd-events".to_string(),
        message: error.to_string(),
    })
}

fn containerd_identity_from_labels(
    namespace: &str,
    container_id: &str,
    labels: &HashMap<String, String>,
) -> ContainerdSandboxIdentity {
    let k8s_namespace = labels
        .get("io.kubernetes.pod.namespace")
        .cloned()
        .unwrap_or_default();
    let pod_name = labels
        .get("io.kubernetes.pod.name")
        .cloned()
        .unwrap_or_default();
    let container_name = labels
        .get("io.kubernetes.container.name")
        .cloned()
        .or_else(|| labels.get("com.docker.compose.service").cloned())
        .unwrap_or_else(|| short_container_id(container_id));
    let pod_uid = labels
        .get("io.kubernetes.pod.uid")
        .cloned()
        .unwrap_or_default();

    if namespace == "k8s.io" && !k8s_namespace.is_empty() && !pod_name.is_empty() {
        let sandbox_id = kubernetes_sandbox_id(&k8s_namespace, &pod_name, &container_name);
        let workload_name = if container_name == "POD" || container_name == "pod" {
            pod_name.clone()
        } else {
            format!("{pod_name}/{container_name}")
        };
        return ContainerdSandboxIdentity {
            sandbox_id,
            namespace: k8s_namespace.clone(),
            workload_id: pod_name.clone(),
            workload_name,
            runtime_sandbox_id: containerd_sandbox_id(namespace, container_id),
            kubernetes_namespace: k8s_namespace,
            pod_name,
            container_name,
            pod_uid,
        };
    }

    ContainerdSandboxIdentity {
        sandbox_id: containerd_sandbox_id(namespace, container_id),
        namespace: namespace.to_string(),
        workload_id: container_name.clone(),
        workload_name: container_name,
        runtime_sandbox_id: containerd_sandbox_id(namespace, container_id),
        kubernetes_namespace: k8s_namespace,
        pod_name,
        container_name: labels
            .get("io.kubernetes.container.name")
            .cloned()
            .unwrap_or_default(),
        pod_uid,
    }
}

fn kubernetes_sandbox_id(namespace: &str, pod: &str, container: &str) -> String {
    format!(
        "k8s-{}-{}-{}",
        sanitize_id(namespace),
        sanitize_id(pod),
        sanitize_id(container)
    )
}

fn namespace_filter() -> Option<Vec<String>> {
    let value = env::var("RUNTIMEPULSE_CONTAINERD_NAMESPACES").ok()?;
    let namespaces = value
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    if namespaces.is_empty() {
        None
    } else {
        Some(namespaces)
    }
}

fn containerd_socket_path() -> PathBuf {
    PathBuf::from(
        env::var("RUNTIMEPULSE_CONTAINERD_SOCKET")
            .unwrap_or_else(|_| DEFAULT_CONTAINERD_SOCKET.to_string()),
    )
}

fn containerd_status(error: containerd_client::tonic::Status) -> CollectorError {
    CollectorError::Plugin {
        plugin: "containerd".to_string(),
        message: error.to_string(),
    }
}

fn containerd_events_status(error: containerd_client::tonic::Status) -> CollectorError {
    CollectorError::Plugin {
        plugin: "containerd-events".to_string(),
        message: error.to_string(),
    }
}

fn timestamp_from_prost(value: &prost_types::Timestamp) -> Option<String> {
    DateTime::from_timestamp(value.seconds, value.nanos as u32).map(timestamp)
}

fn containerd_sandbox_id(namespace: &str, container_id: &str) -> String {
    format!(
        "containerd-{}-{}",
        sanitize_id(namespace),
        sanitize_id(container_id)
    )
}

fn containerd_image_id(namespace: &str, image_ref: &str) -> String {
    format!(
        "containerd-image-{}-{}",
        sanitize_id(namespace),
        sanitize_id(image_ref)
    )
}

fn short_container_id(id: &str) -> String {
    id.chars().take(12).collect()
}

fn short_digest(digest: &str) -> String {
    digest
        .rsplit_once(':')
        .map(|(_, value)| value)
        .unwrap_or(digest)
        .chars()
        .take(12)
        .collect()
}

fn sanitize_id(value: &str) -> String {
    let sanitized = value
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
