//! CRI/containerd startup call-chain report source.
//!
//! This source ingests real reports produced by an external uprobe/eBPF or
//! runtime-instrumentation exporter. RuntimePulse does not attach uprobes here;
//! it normalizes the exporter's JSON/JSONL output into trace spans, metrics,
//! and observation events. This keeps the host-agent usable before versioned
//! containerd/OCI probe profiles are implemented in-tree.

use chrono::{DateTime, SecondsFormat, Utc};
use serde::Deserialize;
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::env;
use std::fs;
#[cfg(unix)]
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use crate::collectors::core::config::CollectorConfig;
use crate::collectors::core::error::{CollectorError, Result};
use crate::collectors::core::model::{
    EventRecord, Metadata, MetricSample, PluginOutput, TraceSpan,
};
use crate::collectors::core::plugin::CollectorPlugin;

const DEFAULT_TIMEOUT_MS: u64 = 1000;

pub struct StartupCallchainPlugin {
    path: Option<PathBuf>,
    command: Option<String>,
    timeout: Duration,
}

#[derive(Default, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StartupCallchainReport {
    id: Option<String>,
    #[serde(alias = "trace_id")]
    trace_id: Option<String>,
    #[serde(default, alias = "sandbox_id")]
    sandbox_id: String,
    #[serde(
        default,
        alias = "cri_sandbox_id",
        alias = "podSandboxId",
        alias = "pod_sandbox_id"
    )]
    cri_sandbox_id: String,
    #[serde(default, alias = "containerd_id", alias = "containerdContainerId")]
    containerd_id: String,
    #[serde(
        default,
        alias = "namespace",
        alias = "k8s_namespace",
        alias = "podNamespace"
    )]
    k8s_namespace: String,
    #[serde(default, alias = "pod_name", alias = "podName")]
    pod_name: String,
    #[serde(default, alias = "container_name", alias = "containerName")]
    container_name: String,
    #[serde(default, alias = "pod_uid", alias = "podUid")]
    pod_uid: String,
    timestamp: Option<String>,
    source: Option<String>,
    #[serde(alias = "runtime_type")]
    runtime_type: Option<String>,
    #[serde(alias = "runtime_handler")]
    runtime_handler: Option<String>,
    status: Option<String>,
    #[serde(alias = "start_time")]
    start_time: Option<String>,
    #[serde(alias = "end_time")]
    end_time: Option<String>,
    #[serde(alias = "duration_ms", alias = "duration")]
    duration_ms: Option<f64>,
    attributes: Option<Map<String, Value>>,
    summary: Option<Map<String, Value>>,
    #[serde(default, alias = "events", alias = "uprobeEvents", alias = "rawEvents")]
    raw_events: Vec<UprobeEventReport>,
    #[serde(default, alias = "stages")]
    spans: Vec<StartupStageReport>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UprobeEventReport {
    #[serde(default, alias = "event", alias = "type", alias = "phase")]
    event_type: String,
    #[serde(alias = "timestamp_ns", alias = "timeNs")]
    timestamp_ns: Option<u64>,
    timestamp: Option<String>,
    #[serde(alias = "duration_ms", alias = "duration")]
    duration_ms: Option<f64>,
    #[serde(alias = "request_id", alias = "correlationId", alias = "callId")]
    request_id: Option<String>,
    #[serde(alias = "function", alias = "symbol", alias = "probe", alias = "name")]
    function_name: Option<String>,
    #[serde(alias = "sandbox_id")]
    sandbox_id: Option<String>,
    #[serde(
        alias = "cri_sandbox_id",
        alias = "podSandboxId",
        alias = "pod_sandbox_id"
    )]
    cri_sandbox_id: Option<String>,
    #[serde(alias = "containerd_id", alias = "containerdContainerId")]
    containerd_id: Option<String>,
    #[serde(alias = "namespace", alias = "k8s_namespace", alias = "podNamespace")]
    k8s_namespace: Option<String>,
    #[serde(alias = "pod_name", alias = "podName")]
    pod_name: Option<String>,
    #[serde(alias = "container_name", alias = "containerName")]
    container_name: Option<String>,
    #[serde(alias = "pod_uid", alias = "podUid")]
    pod_uid: Option<String>,
    #[serde(alias = "runtime_type")]
    runtime_type: Option<String>,
    #[serde(alias = "runtime_handler")]
    runtime_handler: Option<String>,
    status: Option<String>,
    role: Option<String>,
    binary: Option<String>,
    command: Option<String>,
    #[serde(alias = "pid")]
    process_id: Option<u64>,
    #[serde(alias = "ppid")]
    parent_process_id: Option<u64>,
    argv: Option<Vec<String>>,
    env: Option<Map<String, Value>>,
    #[serde(alias = "cni_plugin")]
    cni_plugin: Option<String>,
    #[serde(alias = "cni_command")]
    cni_command: Option<String>,
    #[serde(alias = "cni_container_id")]
    cni_container_id: Option<String>,
    #[serde(alias = "netns")]
    cni_netns: Option<String>,
    #[serde(alias = "oci_runtime")]
    oci_runtime: Option<String>,
    #[serde(alias = "oci_operation")]
    oci_operation: Option<String>,
    #[serde(alias = "bundle")]
    oci_bundle: Option<String>,
    attributes: Option<Map<String, Value>>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StartupStageReport {
    #[serde(alias = "span_id")]
    span_id: Option<String>,
    #[serde(alias = "span_name", alias = "name", alias = "stage")]
    span_name: String,
    #[serde(alias = "parent_span_id")]
    parent_span_id: Option<String>,
    #[serde(alias = "sandbox_id")]
    sandbox_id: Option<String>,
    status: Option<String>,
    #[serde(alias = "start_time")]
    start_time: Option<String>,
    #[serde(alias = "end_time")]
    end_time: Option<String>,
    #[serde(alias = "duration_ms", alias = "duration")]
    duration_ms: Option<f64>,
    attributes: Option<Map<String, Value>>,
    role: Option<String>,
    binary: Option<String>,
    command: Option<String>,
    #[serde(alias = "pid")]
    process_id: Option<u64>,
    #[serde(alias = "ppid")]
    parent_process_id: Option<u64>,
    argv: Option<Vec<String>>,
    env: Option<Map<String, Value>>,
    #[serde(alias = "cni_plugin")]
    cni_plugin: Option<String>,
    #[serde(alias = "cni_command")]
    cni_command: Option<String>,
    #[serde(alias = "cni_container_id")]
    cni_container_id: Option<String>,
    #[serde(alias = "netns")]
    cni_netns: Option<String>,
    #[serde(alias = "oci_runtime")]
    oci_runtime: Option<String>,
    #[serde(alias = "oci_operation")]
    oci_operation: Option<String>,
    #[serde(alias = "bundle")]
    oci_bundle: Option<String>,
}

enum ParsedCallchainReport {
    RuntimePulse(PluginOutput),
    Lightweight(StartupCallchainReport),
}

impl StartupCallchainPlugin {
    pub fn from_env() -> Self {
        Self {
            path: env_path("RUNTIMEPULSE_STARTUP_CALLCHAIN_REPORT_PATH"),
            command: env_string("RUNTIMEPULSE_STARTUP_CALLCHAIN_REPORT_CMD"),
            timeout: Duration::from_millis(
                env_u64("RUNTIMEPULSE_STARTUP_CALLCHAIN_REPORT_TIMEOUT_MS")
                    .unwrap_or(DEFAULT_TIMEOUT_MS)
                    .max(100),
            ),
        }
    }
}

impl CollectorPlugin for StartupCallchainPlugin {
    fn name(&self) -> &str {
        "startup-callchain"
    }

    fn collect(&mut self, now: DateTime<Utc>, config: &CollectorConfig) -> Result<PluginOutput> {
        let mut output = PluginOutput::default();

        if let Some(path) = self.path.clone() {
            let content = match fs::read_to_string(&path) {
                Ok(content) => content,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
                Err(error) => return Err(error.into()),
            };
            if !content.trim().is_empty() {
                merge_plugin_output(
                    &mut output,
                    startup_callchain_output_from_content(&content, now, config)?,
                );
            }
        }

        if let Some(command) = self.command.clone() {
            let content = run_report_command(&command, self.timeout)?;
            if !content.trim().is_empty() {
                merge_plugin_output(
                    &mut output,
                    startup_callchain_output_from_content(&content, now, config)?,
                );
            }
        }

        Ok(output)
    }
}

pub fn startup_callchain_output_from_content(
    content: &str,
    now: DateTime<Utc>,
    config: &CollectorConfig,
) -> Result<PluginOutput> {
    let mut output = PluginOutput::default();
    let fallback_timestamp = timestamp(now);

    for report in parse_reports(content)? {
        match report {
            ParsedCallchainReport::RuntimePulse(runtimepulse_output) => {
                merge_plugin_output(&mut output, runtimepulse_output);
            }
            ParsedCallchainReport::Lightweight(report) => {
                merge_plugin_output(
                    &mut output,
                    output_from_lightweight_report(report, &fallback_timestamp, config),
                );
            }
        }
    }

    if has_payload(&output) && output.metadata.nodes.is_empty() {
        output.metadata.nodes.push(json!({
            "id": config.node_id,
            "clusterId": config.cluster_id,
            "name": config.node_id,
            "status": "ready",
            "labels": {
                "collector": "runtimepulse-rust-collector",
                "plugin": "startup-callchain",
                "scope": config.collection_scope,
            }
        }));
    }

    Ok(output)
}

fn parse_reports(content: &str) -> Result<Vec<ParsedCallchainReport>> {
    let trimmed = content.trim();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }

    if let Ok(value) = serde_json::from_str::<Value>(trimmed) {
        return parse_report_value(value);
    }

    let values = trimmed
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(serde_json::from_str::<Value>)
        .collect::<std::result::Result<Vec<_>, _>>()?;
    if values.iter().all(is_uprobe_event_value) {
        return parse_report_value(Value::Array(values));
    }

    let mut reports = Vec::new();
    for value in values {
        reports.extend(parse_report_value(value)?);
    }
    Ok(reports)
}

fn parse_report_value(value: Value) -> Result<Vec<ParsedCallchainReport>> {
    if is_runtimepulse_output_value(&value) {
        return Ok(vec![ParsedCallchainReport::RuntimePulse(
            serde_json::from_value(value)?,
        )]);
    }

    if let Some(items) = value.get("reports").and_then(Value::as_array) {
        return items
            .iter()
            .cloned()
            .map(|item| {
                Ok(ParsedCallchainReport::Lightweight(serde_json::from_value(
                    item,
                )?))
            })
            .collect();
    }

    if value.get("events").is_some() && value.get("spans").is_none() {
        let report = lightweight_report_from_event_container(value)?;
        return Ok(vec![ParsedCallchainReport::Lightweight(report)]);
    }

    if let Some(items) = value.as_array() {
        if items.iter().all(is_uprobe_event_value) {
            return Ok(vec![ParsedCallchainReport::Lightweight(
                lightweight_report_from_event_array(items.clone())?,
            )]);
        }
        return items
            .iter()
            .cloned()
            .map(|item| {
                Ok(ParsedCallchainReport::Lightweight(serde_json::from_value(
                    item,
                )?))
            })
            .collect();
    }

    Ok(vec![ParsedCallchainReport::Lightweight(
        serde_json::from_value(value)?,
    )])
}

fn lightweight_report_from_event_container(mut value: Value) -> Result<StartupCallchainReport> {
    let mut report = StartupCallchainReport::default();
    if let Some(object) = value.as_object_mut() {
        if let Some(events) = object.remove("events") {
            report.raw_events = serde_json::from_value(events)?;
        }
        overlay_report_fields(&mut report, object);
    }
    Ok(report)
}

fn lightweight_report_from_event_array(items: Vec<Value>) -> Result<StartupCallchainReport> {
    Ok(StartupCallchainReport {
        raw_events: items
            .into_iter()
            .map(serde_json::from_value)
            .collect::<std::result::Result<Vec<UprobeEventReport>, _>>()?,
        ..StartupCallchainReport::default()
    })
}

fn is_uprobe_event_value(value: &Value) -> bool {
    value.as_object().is_some_and(|object| {
        object.contains_key("eventType")
            || object.contains_key("event_type")
            || object.contains_key("event")
            || object.contains_key("phase")
            || object.contains_key("probe")
            || object.contains_key("function")
            || object.contains_key("symbol")
    })
}

fn overlay_report_fields(report: &mut StartupCallchainReport, object: &Map<String, Value>) {
    report.id = value_string_from_keys(object, &["id"]).or_else(|| report.id.clone());
    report.trace_id = value_string_from_keys(object, &["traceId", "trace_id"])
        .or_else(|| report.trace_id.clone());
    report.sandbox_id =
        value_string_from_keys(object, &["sandboxId", "sandbox_id"]).unwrap_or_default();
    report.cri_sandbox_id = value_string_from_keys(
        object,
        &[
            "criSandboxId",
            "cri_sandbox_id",
            "podSandboxId",
            "pod_sandbox_id",
        ],
    )
    .unwrap_or_default();
    report.containerd_id = value_string_from_keys(
        object,
        &["containerdId", "containerd_id", "containerdContainerId"],
    )
    .unwrap_or_default();
    report.k8s_namespace =
        value_string_from_keys(object, &["namespace", "k8s_namespace", "podNamespace"])
            .unwrap_or_default();
    report.pod_name = value_string_from_keys(object, &["podName", "pod_name"]).unwrap_or_default();
    report.container_name =
        value_string_from_keys(object, &["containerName", "container_name"]).unwrap_or_default();
    report.pod_uid = value_string_from_keys(object, &["podUid", "pod_uid"]).unwrap_or_default();
    report.timestamp =
        value_string_from_keys(object, &["timestamp"]).or_else(|| report.timestamp.clone());
    report.source = value_string_from_keys(object, &["source"]).or_else(|| report.source.clone());
    report.runtime_type = value_string_from_keys(object, &["runtimeType", "runtime_type"])
        .or_else(|| report.runtime_type.clone());
    report.runtime_handler = value_string_from_keys(object, &["runtimeHandler", "runtime_handler"])
        .or_else(|| report.runtime_handler.clone());
    report.status = value_string_from_keys(object, &["status"]).or_else(|| report.status.clone());
    report.start_time = value_string_from_keys(object, &["startTime", "start_time"])
        .or_else(|| report.start_time.clone());
    report.end_time = value_string_from_keys(object, &["endTime", "end_time"])
        .or_else(|| report.end_time.clone());
    report.duration_ms = value_f64_from_keys(object, &["durationMs", "duration_ms", "duration"])
        .or(report.duration_ms);
    report.attributes = object
        .get("attributes")
        .and_then(Value::as_object)
        .cloned()
        .or_else(|| report.attributes.clone());
    report.summary = object
        .get("summary")
        .and_then(Value::as_object)
        .cloned()
        .or_else(|| report.summary.clone());
}

fn is_runtimepulse_output_value(value: &Value) -> bool {
    value.get("traces").is_some()
        || value.get("metrics").is_some()
        || value.get("profiles").is_some()
        || value.get("metadata").is_some()
        || value
            .get("events")
            .and_then(Value::as_array)
            .and_then(|items| items.first())
            .and_then(Value::as_object)
            .is_some_and(|event| {
                (event.contains_key("eventName") || event.contains_key("event_name"))
                    && (event.contains_key("eventType") || event.contains_key("event_type"))
                    && event.contains_key("id")
            })
}

fn output_from_lightweight_report(
    mut report: StartupCallchainReport,
    fallback_timestamp: &str,
    config: &CollectorConfig,
) -> PluginOutput {
    apply_event_report_defaults(&mut report, fallback_timestamp);
    let sandbox_id = stable_sandbox_id(&report);
    let runtime_sandbox_id = first_non_empty([
        report.cri_sandbox_id.as_str(),
        report.sandbox_id.as_str(),
        report.containerd_id.as_str(),
    ]);
    let explicit_trace_id = report
        .trace_id
        .as_deref()
        .is_some_and(|value| !value.trim().is_empty());
    let trace_id = report
        .trace_id
        .clone()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| format!("cri-containerd-startup-{}", sanitize_id(&sandbox_id)));
    let cri_startup_root_span_id = format!("{trace_id}-e2e");
    let root_span_id = if explicit_trace_id {
        format!("{trace_id}-root")
    } else {
        format!("{trace_id}-callchain")
    };
    let status = report.status.as_deref().unwrap_or("ok").to_string();
    let runtime_type = report
        .runtime_type
        .clone()
        .filter(|value| !value.trim().is_empty() && value != "unknown")
        .or_else(|| {
            report
                .runtime_handler
                .as_ref()
                .map(|handler| runtime_type_from_name(handler))
        })
        .unwrap_or_else(|| "unknown".to_string());
    let source = report
        .source
        .clone()
        .unwrap_or_else(|| "startup-callchain".to_string());
    let mut base_attributes = report.attributes.clone().unwrap_or_default();
    base_attributes.insert("plugin".to_string(), json!("startup-callchain"));
    base_attributes.insert("scope".to_string(), json!(config.collection_scope));
    base_attributes.insert("startup.trace.source".to_string(), json!(source));
    base_attributes.insert("runtime.type".to_string(), json!(runtime_type));
    base_attributes.insert("cri.sandbox_id".to_string(), json!(runtime_sandbox_id));
    base_attributes.insert("containerd.id".to_string(), json!(report.containerd_id));
    base_attributes.insert("k8s.namespace".to_string(), json!(report.k8s_namespace));
    base_attributes.insert("k8s.pod".to_string(), json!(report.pod_name));
    base_attributes.insert("k8s.container".to_string(), json!(report.container_name));
    base_attributes.insert("k8s.pod_uid".to_string(), json!(report.pod_uid));
    if let Some(handler) = &report.runtime_handler {
        base_attributes.insert("runtime.handler".to_string(), json!(handler));
    }
    if let Some(summary) = &report.summary {
        base_attributes.insert("startup.summary".to_string(), json!(summary));
    }
    if !explicit_trace_id {
        base_attributes.insert(
            "startup.trace.joined_trace_id".to_string(),
            json!("cri-containerd-startup"),
        );
    }

    let normalized_events = normalize_uprobe_events(&report);
    report.spans.extend(normalized_events.spans.clone());

    let mut traces = Vec::new();
    if let Some(root_span) = root_span_from_report(
        &report,
        &sandbox_id,
        &trace_id,
        &root_span_id,
        (!explicit_trace_id).then_some(cri_startup_root_span_id.as_str()),
        &status,
        &runtime_type,
        base_attributes.clone(),
    ) {
        traces.push(root_span);
    }

    let default_parent = if traces.is_empty() {
        (!explicit_trace_id).then_some(cri_startup_root_span_id.clone())
    } else {
        Some(root_span_id.clone())
    };
    for stage in &report.spans {
        if let Some(span) = span_from_stage(
            stage,
            &trace_id,
            default_parent.as_deref(),
            &sandbox_id,
            &runtime_type,
            base_attributes.clone(),
        ) {
            traces.push(span);
        }
    }

    let observed_at = report
        .timestamp
        .clone()
        .or_else(|| report.end_time.clone())
        .unwrap_or_else(|| fallback_timestamp.to_string());
    let mut metrics = metrics_from_summary(
        report.summary.as_ref(),
        &observed_at,
        &sandbox_id,
        &runtime_type,
        &base_attributes,
        config,
    );
    metrics.extend(metrics_from_spans(
        &report.spans,
        &observed_at,
        &sandbox_id,
        &runtime_type,
        &base_attributes,
        config,
    ));
    metrics.extend(metrics_from_uprobe_normalization(
        &normalized_events,
        &observed_at,
        &sandbox_id,
        &runtime_type,
        &base_attributes,
        config,
    ));
    if let Some(duration_ms) = report.duration_ms {
        metrics.push(metric(
            &observed_at,
            "sandbox.startup.callchain_duration_ms",
            duration_ms,
            "ms",
            &sandbox_id,
            &runtime_type,
            &base_attributes,
            config,
        ));
    }

    let mut metadata = Metadata::default();
    metadata.sandboxes.push(json!({
        "id": sandbox_id,
        "clusterId": config.cluster_id,
        "nodeId": config.node_id,
        "namespace": if report.k8s_namespace.trim().is_empty() { "collector" } else { report.k8s_namespace.as_str() },
        "workloadId": first_non_empty([report.pod_uid.as_str(), report.pod_name.as_str(), sandbox_id.as_str()]),
        "workloadName": first_non_empty([report.pod_name.as_str(), sandbox_id.as_str()]),
        "imageRef": "collector/startup-callchain:unknown",
        "runtimeType": runtime_type,
        "runtimeVersion": report.runtime_handler.as_deref().unwrap_or(runtime_type.as_str()),
        "status": if status == "ok" { "running" } else { "failed" },
        "createdAt": report.start_time.as_deref().unwrap_or(&observed_at),
        "startedAt": report.end_time.as_deref(),
        "startupDurationMs": report.duration_ms.unwrap_or(0.0),
        "labels": {
            "collector": "runtimepulse-rust-collector",
            "plugin": "startup-callchain",
            "runtime": runtime_type,
        },
        "attributes": base_attributes.clone(),
    }));

    let mut event_attributes = base_attributes.clone();
    if normalized_events.event_count > 0 {
        event_attributes.insert(
            "startup.uprobe.event_count".to_string(),
            json!(normalized_events.event_count),
        );
        event_attributes.insert(
            "startup.uprobe.span_count".to_string(),
            json!(normalized_events.span_count),
        );
        event_attributes.insert(
            "startup.uprobe.matched_event_count".to_string(),
            json!(normalized_events.matched_event_count),
        );
        event_attributes.insert(
            "startup.uprobe.unmatched_event_count".to_string(),
            json!(normalized_events.unmatched_event_count()),
        );
    }
    let event_severity = if status == "ok" && normalized_events.unmatched_event_count() == 0 {
        "info"
    } else {
        "warning"
    };

    PluginOutput {
        source: None,
        metadata,
        metrics,
        events: vec![EventRecord {
            id: format!(
                "startup-callchain-{}-{}",
                sanitize_id(report.id.as_deref().unwrap_or(&trace_id)),
                sanitize_id(&observed_at)
            ),
            timestamp: observed_at,
            severity: event_severity.to_string(),
            event_type: "startup".to_string(),
            event_name: "startup.callchain.observed".to_string(),
            message: format!(
                "Startup call-chain report observed {} spans for {}.",
                traces.len(),
                sandbox_id
            ),
            source: format!(
                "runtimepulse-rust-collector/{}/startup-callchain",
                config.node_id
            ),
            attributes: event_attributes,
            sandbox_id: Some(sandbox_id),
            image_id: None,
            node_id: Some(config.node_id.clone()),
            runtime_type: Some(runtime_type),
            reason: None,
        }],
        traces,
        profiles: Vec::new(),
    }
}

fn runtime_type_from_name(value: &str) -> String {
    let value = value.to_ascii_lowercase();
    if value.contains("runsc") || value.contains("gvisor") {
        "gvisor".to_string()
    } else if value.contains("kata") {
        "kata".to_string()
    } else if value.contains("firecracker") {
        "firecracker".to_string()
    } else {
        "runc".to_string()
    }
}

fn stable_sandbox_id(report: &StartupCallchainReport) -> String {
    if !report.k8s_namespace.trim().is_empty()
        && !report.pod_name.trim().is_empty()
        && !report.container_name.trim().is_empty()
    {
        return format!(
            "k8s-{}-{}-{}",
            sanitize_id(&report.k8s_namespace),
            sanitize_id(&report.pod_name),
            sanitize_id(&report.container_name)
        );
    }

    first_non_empty([
        report.sandbox_id.as_str(),
        report.cri_sandbox_id.as_str(),
        report.containerd_id.as_str(),
        report.pod_uid.as_str(),
    ])
    .to_string()
}

fn apply_event_report_defaults(report: &mut StartupCallchainReport, fallback_timestamp: &str) {
    if report.raw_events.is_empty() {
        return;
    }

    if report.sandbox_id.trim().is_empty() {
        if let Some(value) =
            first_event_string(&report.raw_events, |event| event.sandbox_id.as_deref())
        {
            report.sandbox_id = value;
        }
    }
    if report.cri_sandbox_id.trim().is_empty() {
        if let Some(value) =
            first_event_string(&report.raw_events, |event| event.cri_sandbox_id.as_deref())
        {
            report.cri_sandbox_id = value;
        }
    }
    if report.containerd_id.trim().is_empty() {
        if let Some(value) =
            first_event_string(&report.raw_events, |event| event.containerd_id.as_deref())
        {
            report.containerd_id = value;
        }
    }
    if report.k8s_namespace.trim().is_empty() {
        if let Some(value) =
            first_event_string(&report.raw_events, |event| event.k8s_namespace.as_deref())
        {
            report.k8s_namespace = value;
        }
    }
    if report.pod_name.trim().is_empty() {
        if let Some(value) =
            first_event_string(&report.raw_events, |event| event.pod_name.as_deref())
        {
            report.pod_name = value;
        }
    }
    if report.container_name.trim().is_empty() {
        if let Some(value) =
            first_event_string(&report.raw_events, |event| event.container_name.as_deref())
        {
            report.container_name = value;
        }
    }
    if report.pod_uid.trim().is_empty() {
        if let Some(value) =
            first_event_string(&report.raw_events, |event| event.pod_uid.as_deref())
        {
            report.pod_uid = value;
        }
    }
    if report.runtime_type.is_none() {
        report.runtime_type =
            first_event_string(&report.raw_events, |event| event.runtime_type.as_deref());
    }
    if report.runtime_handler.is_none() {
        report.runtime_handler =
            first_event_string(&report.raw_events, |event| event.runtime_handler.as_deref());
    }
    if report.start_time.is_none() {
        report.start_time = earliest_event_timestamp(&report.raw_events)
            .or_else(|| Some(fallback_timestamp.to_string()));
    }
    if report.end_time.is_none() {
        report.end_time =
            latest_event_timestamp(&report.raw_events).or_else(|| report.start_time.clone());
    }
    if report.duration_ms.is_none() {
        report.duration_ms = report
            .start_time
            .as_deref()
            .zip(report.end_time.as_deref())
            .and_then(|(start, end)| duration_between(start, end));
    }
}

fn first_event_string<F>(events: &[UprobeEventReport], getter: F) -> Option<String>
where
    F: Fn(&UprobeEventReport) -> Option<&str>,
{
    events
        .iter()
        .filter_map(getter)
        .map(str::trim)
        .find(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn earliest_event_timestamp(events: &[UprobeEventReport]) -> Option<String> {
    events
        .iter()
        .filter_map(event_timestamp)
        .min_by_key(|timestamp| parse_time(timestamp).map(|time| time.timestamp_millis()))
}

fn latest_event_timestamp(events: &[UprobeEventReport]) -> Option<String> {
    events
        .iter()
        .filter_map(event_timestamp)
        .max_by_key(|timestamp| parse_time(timestamp).map(|time| time.timestamp_millis()))
}

#[derive(Default)]
struct PendingUprobeEvent {
    event: Option<UprobeEventReport>,
    started_at: Option<String>,
}

#[derive(Clone, Default)]
struct UprobeNormalization {
    spans: Vec<StartupStageReport>,
    event_count: u64,
    matched_event_count: u64,
    span_count: u64,
}

impl UprobeNormalization {
    fn unmatched_event_count(&self) -> u64 {
        self.event_count.saturating_sub(self.matched_event_count)
    }

    fn pairing_ratio(&self) -> f64 {
        if self.event_count == 0 {
            1.0
        } else {
            self.matched_event_count as f64 / self.event_count as f64
        }
    }
}

fn normalize_uprobe_events(report: &StartupCallchainReport) -> UprobeNormalization {
    if report.raw_events.is_empty() {
        return UprobeNormalization::default();
    }

    let mut pending = HashMap::<String, PendingUprobeEvent>::new();
    let mut normalized = UprobeNormalization {
        event_count: report.raw_events.len() as u64,
        ..UprobeNormalization::default()
    };
    let mut events = report.raw_events.iter().collect::<Vec<_>>();
    events.sort_by_key(|event| {
        event_timestamp(event)
            .and_then(|value| parse_time(&value).map(|time| time.timestamp_millis()))
            .unwrap_or_default()
    });

    for event in events {
        let kind = normalized_event_kind(&event.event_type);
        let key = event_correlation_key(event);
        match kind.as_str() {
            "enter" | "start" => {
                pending.insert(
                    key,
                    PendingUprobeEvent {
                        event: Some(event.clone()),
                        started_at: event_timestamp(event),
                    },
                );
            }
            "exit" | "end" | "return" => {
                if let Some(started) = pending.remove(&key) {
                    if let Some(span) = span_from_uprobe_pair(
                        started.event.as_ref(),
                        event,
                        started.started_at.as_deref(),
                        report,
                    ) {
                        normalized.spans.push(span);
                        normalized.matched_event_count += 2;
                    }
                } else if let Some(span) = span_from_uprobe_pair(None, event, None, report) {
                    normalized.spans.push(span);
                    normalized.matched_event_count += 1;
                }
            }
            "span" | "complete" | "event" | "" => {
                if let Some(span) = span_from_uprobe_pair(None, event, None, report) {
                    normalized.spans.push(span);
                    normalized.matched_event_count += 1;
                }
            }
            _ => {
                if let Some(span) = span_from_uprobe_pair(None, event, None, report) {
                    normalized.spans.push(span);
                    normalized.matched_event_count += 1;
                }
            }
        }
    }

    attach_parent_spans_by_process_tree(&mut normalized.spans);
    normalized.span_count = normalized.spans.len() as u64;
    normalized
}

fn attach_parent_spans_by_process_tree(spans: &mut [StartupStageReport]) {
    let mut spans_by_pid = HashMap::<u64, Vec<usize>>::new();
    for (idx, span) in spans.iter().enumerate() {
        if let Some(pid) = span.process_id {
            spans_by_pid.entry(pid).or_default().push(idx);
        }
    }

    for idx in 0..spans.len() {
        if spans[idx].parent_span_id.is_some() {
            continue;
        }
        let Some(ppid) = spans[idx].parent_process_id else {
            continue;
        };
        let Some(parent_idx) = choose_parent_span(spans, idx, spans_by_pid.get(&ppid)) else {
            continue;
        };
        if let Some(parent_span_id) = spans[parent_idx].span_id.clone() {
            spans[idx].parent_span_id = Some(parent_span_id.clone());
            if let Some(attributes) = spans[idx].attributes.as_mut() {
                attributes.insert("startup.parent.pid".to_string(), json!(ppid));
                attributes.insert("startup.parent.span_id".to_string(), json!(parent_span_id));
            }
        }
    }
}

fn choose_parent_span(
    spans: &[StartupStageReport],
    child_idx: usize,
    candidates: Option<&Vec<usize>>,
) -> Option<usize> {
    let child_start = spans[child_idx]
        .start_time
        .as_deref()
        .and_then(parse_time)?;
    let child_end = spans[child_idx].end_time.as_deref().and_then(parse_time);

    candidates?
        .iter()
        .copied()
        .filter(|candidate_idx| *candidate_idx != child_idx)
        .filter_map(|candidate_idx| {
            let parent = &spans[candidate_idx];
            let parent_start = parent.start_time.as_deref().and_then(parse_time)?;
            if parent_start > child_start {
                return None;
            }
            if let Some(parent_end) = parent.end_time.as_deref().and_then(parse_time) {
                if let Some(child_end) = child_end {
                    if parent_end < child_end {
                        return None;
                    }
                }
            }
            let delta_ms = (child_start - parent_start).num_milliseconds().abs();
            Some((candidate_idx, delta_ms))
        })
        .min_by_key(|(_, delta_ms)| *delta_ms)
        .map(|(candidate_idx, _)| candidate_idx)
}

fn span_from_uprobe_pair(
    enter: Option<&UprobeEventReport>,
    exit: &UprobeEventReport,
    entered_at: Option<&str>,
    report: &StartupCallchainReport,
) -> Option<StartupStageReport> {
    let start_time = entered_at.map(ToOwned::to_owned).or_else(|| {
        value_string_from_event(
            exit,
            &["startTime", "start_time", "startTimestamp", "beginTime"],
        )
    });
    let end_time = event_timestamp(exit).or_else(|| {
        value_string_from_event(exit, &["endTime", "end_time", "endTimestamp", "finishTime"])
    });
    let duration_ms = exit
        .duration_ms
        .or_else(|| enter.and_then(|event| event.duration_ms))
        .or_else(|| {
            value_f64_from_event(
                exit,
                &[
                    "durationMs",
                    "duration_ms",
                    "duration",
                    "latencyMs",
                    "latency_ms",
                ],
            )
        })
        .or_else(|| {
            start_time
                .as_deref()
                .zip(end_time.as_deref())
                .and_then(|(start, end)| duration_between(start, end))
        });

    let (start_time, end_time) = match (start_time, end_time, duration_ms) {
        (Some(start), Some(end), _) => (Some(start), Some(end)),
        (Some(start), None, Some(duration)) => parse_time(&start).map(|time| {
            (
                Some(start),
                Some(timestamp(
                    time + chrono::Duration::milliseconds(duration.max(1.0) as i64),
                )),
            )
        })?,
        (None, Some(end), Some(duration)) => parse_time(&end).map(|time| {
            (
                Some(timestamp(
                    time - chrono::Duration::milliseconds(duration.max(1.0) as i64),
                )),
                Some(end),
            )
        })?,
        _ => (None, None),
    };

    let start_time = start_time?;
    let end_time = end_time?;
    let function_name = exit
        .function_name
        .as_deref()
        .or_else(|| enter.and_then(|event| event.function_name.as_deref()))
        .unwrap_or("");
    let binary = exit
        .binary
        .clone()
        .or_else(|| enter.and_then(|event| event.binary.clone()))
        .or_else(|| value_string_from_event(exit, &["process.binary", "binaryPath", "exe"]));
    let command = exit
        .command
        .clone()
        .or_else(|| enter.and_then(|event| event.command.clone()))
        .or_else(|| value_string_from_event(exit, &["process.command", "cmd"]));
    let binary_name = binary
        .as_deref()
        .or(command.as_deref())
        .map(binary_basename)
        .unwrap_or("")
        .to_ascii_lowercase();
    let role = exit
        .role
        .clone()
        .or_else(|| enter.and_then(|event| event.role.clone()))
        .or_else(|| infer_stage_role(function_name, &binary_name));
    let span_name = infer_span_name(function_name, role.as_deref(), &binary_name, exit, enter);
    let sandbox_id = Some(stable_sandbox_id(report));

    let mut attributes = Map::new();
    attributes.insert("startup.event.kind".to_string(), json!("uprobe"));
    attributes.insert(
        "startup.event.correlation_key".to_string(),
        json!(event_correlation_key(exit)),
    );
    if !function_name.is_empty() {
        attributes.insert("uprobe.function".to_string(), json!(function_name));
    }
    if let Some(request_id) = exit
        .request_id
        .as_deref()
        .or_else(|| enter.and_then(|event| event.request_id.as_deref()))
    {
        attributes.insert("uprobe.request_id".to_string(), json!(request_id));
    }
    merge_event_attributes(&mut attributes, enter);
    merge_event_attributes(&mut attributes, Some(exit));

    Some(StartupStageReport {
        span_id: exit
            .request_id
            .as_deref()
            .or_else(|| enter.and_then(|event| event.request_id.as_deref()))
            .map(|request_id| {
                format!(
                    "uprobe-{}-{}",
                    sanitize_id(request_id),
                    sanitize_id(&span_name)
                )
            }),
        span_name,
        parent_span_id: None,
        sandbox_id,
        status: exit
            .status
            .clone()
            .or_else(|| enter.and_then(|event| event.status.clone())),
        start_time: Some(start_time),
        end_time: Some(end_time),
        duration_ms,
        attributes: Some(attributes),
        role,
        binary,
        command,
        process_id: exit
            .process_id
            .or_else(|| enter.and_then(|event| event.process_id)),
        parent_process_id: exit
            .parent_process_id
            .or_else(|| enter.and_then(|event| event.parent_process_id)),
        argv: exit
            .argv
            .clone()
            .or_else(|| enter.and_then(|event| event.argv.clone())),
        env: exit
            .env
            .clone()
            .or_else(|| enter.and_then(|event| event.env.clone())),
        cni_plugin: exit
            .cni_plugin
            .clone()
            .or_else(|| enter.and_then(|event| event.cni_plugin.clone())),
        cni_command: exit
            .cni_command
            .clone()
            .or_else(|| enter.and_then(|event| event.cni_command.clone())),
        cni_container_id: exit
            .cni_container_id
            .clone()
            .or_else(|| enter.and_then(|event| event.cni_container_id.clone())),
        cni_netns: exit
            .cni_netns
            .clone()
            .or_else(|| enter.and_then(|event| event.cni_netns.clone())),
        oci_runtime: exit
            .oci_runtime
            .clone()
            .or_else(|| enter.and_then(|event| event.oci_runtime.clone())),
        oci_operation: exit
            .oci_operation
            .clone()
            .or_else(|| enter.and_then(|event| event.oci_operation.clone())),
        oci_bundle: exit
            .oci_bundle
            .clone()
            .or_else(|| enter.and_then(|event| event.oci_bundle.clone())),
    })
}

fn normalized_event_kind(value: &str) -> String {
    match value.to_ascii_lowercase().as_str() {
        "enter" | "entry" | "start" | "begin" => "enter".to_string(),
        "exit" | "return" | "ret" | "end" | "finish" => "exit".to_string(),
        "span" | "complete" | "completed" => "span".to_string(),
        other => other.to_string(),
    }
}

fn event_correlation_key(event: &UprobeEventReport) -> String {
    if let Some(request_id) = event
        .request_id
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        return format!("request:{}", request_id.trim());
    }
    let function = event
        .function_name
        .as_deref()
        .unwrap_or("unknown-function")
        .trim();
    let pid = event
        .process_id
        .map(|value| value.to_string())
        .unwrap_or_else(|| "unknown-pid".to_string());
    let sandbox = event
        .sandbox_id
        .as_deref()
        .or(event.cri_sandbox_id.as_deref())
        .or(event.containerd_id.as_deref())
        .unwrap_or("unknown-sandbox")
        .trim();
    format!("function:{function}:pid:{pid}:sandbox:{sandbox}")
}

fn event_timestamp(event: &UprobeEventReport) -> Option<String> {
    event
        .timestamp
        .clone()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| event.timestamp_ns.map(timestamp_from_unix_nanos))
}

fn timestamp_from_unix_nanos(value: u64) -> String {
    let secs = (value / 1_000_000_000) as i64;
    let nanos = (value % 1_000_000_000) as u32;
    DateTime::<Utc>::from_timestamp(secs, nanos)
        .map(timestamp)
        .unwrap_or_else(|| timestamp(Utc::now()))
}

fn infer_stage_role(function_name: &str, binary_name: &str) -> Option<String> {
    let function = function_name.to_ascii_lowercase();
    if function.contains("runpodsandbox")
        || function.contains("run_pod_sandbox")
        || function.contains("run-pod-sandbox")
    {
        return Some("cri".to_string());
    }
    if is_helper_binary(binary_name) {
        return Some("helper".to_string());
    }
    if function.contains("cni")
        || function.contains("setuppodnetwork")
        || is_likely_cni_plugin_binary(binary_name)
    {
        return Some("cni".to_string());
    }
    if function.contains("kata") || (binary_name.contains("kata") && binary_name != "kata-runtime") {
        return Some("kata".to_string());
    }
    if function.contains("oci")
        || function.contains("runc")
        || matches!(binary_name, "runc" | "crun" | "kata-runtime" | "runsc")
    {
        return Some("oci".to_string());
    }
    if !binary_name.is_empty() {
        return Some("exec".to_string());
    }
    None
}

fn infer_span_name(
    function_name: &str,
    role: Option<&str>,
    binary_name: &str,
    exit: &UprobeEventReport,
    enter: Option<&UprobeEventReport>,
) -> String {
    let explicit_name = value_string_from_event(exit, &["spanName", "span_name", "stage", "name"])
        .or_else(|| {
            enter.and_then(|event| {
                value_string_from_event(event, &["spanName", "span_name", "stage", "name"])
            })
        });
    if let Some(name) = explicit_name.filter(|value| !value.trim().is_empty()) {
        return name;
    }
    let function = function_name.to_ascii_lowercase();
    if function.contains("runpodsandbox")
        || function.contains("run_pod_sandbox")
        || function.contains("run-pod-sandbox")
    {
        return "cri.run_pod_sandbox".to_string();
    }
    if function.contains("setuppodnetwork")
        || function.contains("cnisetup")
        || function.contains("go-cni")
    {
        return "cni.setup".to_string();
    }
    if function.contains("ocishimcreate") {
        return "oci.shim.create".to_string();
    }
    if function.contains("ocishimstart") {
        return "oci.shim.start".to_string();
    }
    if function.contains("ocirunccreate") {
        return "oci.runc.create".to_string();
    }
    if function.contains("ociruncstart") {
        return "oci.runc.start".to_string();
    }
    if function.contains("katashimcreate") {
        return "kata.shim.create".to_string();
    }
    if function.contains("katashimstart") {
        return "kata.shim.start".to_string();
    }
    if function.contains("katasandboxcreate") {
        return "kata.sandbox.create".to_string();
    }
    if function.contains("katasandboxstart") {
        return "kata.sandbox.start".to_string();
    }
    if let Some(role) = role {
        match role {
            "cni" => {
                return format!(
                    "cni.plugin.{}",
                    if binary_name.is_empty() {
                        "unknown"
                    } else {
                        binary_name
                    }
                )
            }
            "oci" => {
                return format!(
                    "oci.{}",
                    if binary_name.is_empty() {
                        "runtime"
                    } else {
                        binary_name
                    }
                )
            }
            "exec" | "helper" => {
                return format!(
                    "process.exec.{}",
                    if binary_name.is_empty() {
                        "unknown"
                    } else {
                        binary_name
                    }
                )
            }
            _ => {}
        }
    }
    if !function_name.trim().is_empty() {
        format!("uprobe.{}", sanitize_metric_key(function_name))
    } else {
        "uprobe.event".to_string()
    }
}

fn merge_event_attributes(attributes: &mut Map<String, Value>, event: Option<&UprobeEventReport>) {
    let Some(event) = event else {
        return;
    };
    if let Some(extra) = &event.attributes {
        attributes.extend(extra.clone());
    }
    if let Some(value) = &event.cri_sandbox_id {
        attributes.insert("cri.sandbox_id".to_string(), json!(value));
    }
    if let Some(value) = &event.containerd_id {
        attributes.insert("containerd.id".to_string(), json!(value));
    }
    if let Some(value) = &event.k8s_namespace {
        attributes.insert("k8s.namespace".to_string(), json!(value));
    }
    if let Some(value) = &event.pod_name {
        attributes.insert("k8s.pod".to_string(), json!(value));
    }
    if let Some(value) = &event.container_name {
        attributes.insert("k8s.container".to_string(), json!(value));
    }
    if let Some(value) = &event.pod_uid {
        attributes.insert("k8s.pod_uid".to_string(), json!(value));
    }
}

fn value_string_from_event(event: &UprobeEventReport, keys: &[&str]) -> Option<String> {
    let attributes = event.attributes.as_ref()?;
    keys.iter()
        .filter_map(|key| value_string_from_keys(attributes, &[*key]))
        .next()
}

fn value_string_from_keys(object: &Map<String, Value>, keys: &[&str]) -> Option<String> {
    for key in keys {
        if let Some(value) = object.get(*key) {
            if let Some(text) = value.as_str() {
                if !text.trim().is_empty() {
                    return Some(text.trim().to_string());
                }
            } else if value.is_number() || value.is_boolean() {
                return Some(value.to_string());
            }
        }
    }
    None
}

fn value_f64_from_keys(object: &Map<String, Value>, keys: &[&str]) -> Option<f64> {
    for key in keys {
        let Some(value) = object.get(*key) else {
            continue;
        };
        if let Some(number) = value.as_f64() {
            return Some(number);
        }
        if let Some(text) = value.as_str().and_then(|text| text.parse::<f64>().ok()) {
            return Some(text);
        }
    }
    None
}

fn value_f64_from_event(event: &UprobeEventReport, keys: &[&str]) -> Option<f64> {
    let attributes = event.attributes.as_ref()?;
    for key in keys {
        let Some(value) = attributes.get(*key) else {
            continue;
        };
        if let Some(number) = value.as_f64() {
            return Some(number);
        }
        if let Some(text) = value.as_str().and_then(|text| text.parse::<f64>().ok()) {
            return Some(text);
        }
    }
    None
}

fn is_likely_cni_plugin_binary(binary_name: &str) -> bool {
    binary_name.starts_with("bridge")
        || matches!(
            binary_name,
            "loopback"
                | "portmap"
                | "firewall"
                | "host-local"
                | "bandwidth"
                | "tuning"
                | "calico"
                | "calico-ipam"
                | "cilium-cni"
                | "multus"
                | "flannel"
                | "weave-net"
        )
}

fn root_span_from_report(
    report: &StartupCallchainReport,
    sandbox_id: &str,
    trace_id: &str,
    root_span_id: &str,
    parent_span_id: Option<&str>,
    status: &str,
    runtime_type: &str,
    attributes: Map<String, Value>,
) -> Option<TraceSpan> {
    let (start, end, duration_ms) = resolve_times(
        report.start_time.as_deref(),
        report.end_time.as_deref(),
        report.duration_ms,
    )?;
    Some(TraceSpan {
        trace_id: trace_id.to_string(),
        span_id: root_span_id.to_string(),
        span_name: "sandbox.startup.callchain".to_string(),
        start_time: start,
        end_time: end,
        duration_ms,
        status: status.to_string(),
        attributes,
        sandbox_id: Some(sandbox_id.to_string()),
        image_id: None,
        runtime_type: Some(runtime_type.to_string()),
        parent_span_id: parent_span_id.map(ToOwned::to_owned),
    })
}

fn span_from_stage(
    stage: &StartupStageReport,
    trace_id: &str,
    default_parent: Option<&str>,
    fallback_sandbox_id: &str,
    runtime_type: &str,
    mut attributes: Map<String, Value>,
) -> Option<TraceSpan> {
    let (start, end, duration_ms) = resolve_times(
        stage.start_time.as_deref(),
        stage.end_time.as_deref(),
        stage.duration_ms,
    )?;
    merge_stage_attributes(&mut attributes, stage, runtime_type);
    let span_id = stage.span_id.clone().unwrap_or_else(|| {
        format!(
            "{trace_id}-{}-{}",
            sanitize_id(&stage.span_name),
            sanitize_id(&start)
        )
    });
    Some(TraceSpan {
        trace_id: trace_id.to_string(),
        span_id,
        span_name: stage.span_name.clone(),
        start_time: start,
        end_time: end,
        duration_ms,
        status: stage.status.clone().unwrap_or_else(|| "ok".to_string()),
        attributes,
        sandbox_id: Some(
            stage
                .sandbox_id
                .clone()
                .filter(|value| !value.trim().is_empty())
                .unwrap_or_else(|| fallback_sandbox_id.to_string()),
        ),
        image_id: None,
        runtime_type: Some(runtime_type.to_string()),
        parent_span_id: stage
            .parent_span_id
            .clone()
            .or_else(|| default_parent.map(ToOwned::to_owned)),
    })
}

fn merge_stage_attributes(
    attributes: &mut Map<String, Value>,
    stage: &StartupStageReport,
    runtime_type: &str,
) {
    if let Some(extra) = &stage.attributes {
        attributes.extend(extra.clone());
    }
    attributes.insert("runtime.type".to_string(), json!(runtime_type));
    if let Some(role) = &stage.role {
        attributes.insert("process.role".to_string(), json!(role));
    }
    if let Some(binary) = &stage.binary {
        attributes.insert("process.binary".to_string(), json!(binary));
    }
    if let Some(command) = &stage.command {
        attributes.insert("process.command".to_string(), json!(command));
    }
    if let Some(pid) = stage.process_id {
        attributes.insert("process.pid".to_string(), json!(pid));
    }
    if let Some(ppid) = stage.parent_process_id {
        attributes.insert("process.ppid".to_string(), json!(ppid));
    }
    if let Some(argv) = &stage.argv {
        attributes.insert("process.argv".to_string(), json!(argv));
    }
    if let Some(env) = &stage.env {
        attributes.insert("process.env".to_string(), json!(env));
    }
    let span_name = stage.span_name.to_ascii_lowercase();
    let binary = stage
        .binary
        .as_deref()
        .or_else(|| stage.command.as_deref())
        .unwrap_or("");
    let binary_name = binary_basename(binary).to_ascii_lowercase();
    let cni_plugin = cni_plugin_name(stage, &span_name, &binary_name);
    if !cni_plugin.is_empty() {
        attributes.insert("cni.plugin".to_string(), json!(cni_plugin));
    }
    if let Some(command) = &stage.cni_command {
        attributes.insert("cni.command".to_string(), json!(command));
    }
    if let Some(container_id) = &stage.cni_container_id {
        attributes.insert("cni.container_id".to_string(), json!(container_id));
    }
    if let Some(netns) = &stage.cni_netns {
        attributes.insert("cni.netns".to_string(), json!(netns));
    }
    if let Some(runtime) = &stage.oci_runtime {
        attributes.insert("oci.runtime".to_string(), json!(runtime));
    }
    if let Some(operation) = &stage.oci_operation {
        attributes.insert("oci.operation".to_string(), json!(operation));
    }
    if let Some(bundle) = &stage.oci_bundle {
        attributes.insert("oci.bundle".to_string(), json!(bundle));
    }
}

fn resolve_times(
    start: Option<&str>,
    end: Option<&str>,
    duration_ms: Option<f64>,
) -> Option<(String, String, f64)> {
    match (start, end, duration_ms) {
        (Some(start), Some(end), duration_ms) => Some((
            start.to_string(),
            end.to_string(),
            duration_ms.unwrap_or_else(|| duration_between(start, end).unwrap_or(1.0)),
        )),
        (Some(start), None, Some(duration_ms)) => {
            let start_at = parse_time(start)?;
            let end_at = start_at + chrono::Duration::milliseconds(duration_ms.max(1.0) as i64);
            Some((start.to_string(), timestamp(end_at), duration_ms.max(1.0)))
        }
        (None, Some(end), Some(duration_ms)) => {
            let end_at = parse_time(end)?;
            let start_at = end_at - chrono::Duration::milliseconds(duration_ms.max(1.0) as i64);
            Some((timestamp(start_at), end.to_string(), duration_ms.max(1.0)))
        }
        _ => None,
    }
}

fn duration_between(start: &str, end: &str) -> Option<f64> {
    let start = parse_time(start)?;
    let end = parse_time(end)?;
    Some((end - start).num_milliseconds().max(1) as f64)
}

fn parse_time(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|value| value.with_timezone(&Utc))
}

fn metrics_from_summary(
    summary: Option<&Map<String, Value>>,
    timestamp: &str,
    sandbox_id: &str,
    runtime_type: &str,
    attributes: &Map<String, Value>,
    config: &CollectorConfig,
) -> Vec<MetricSample> {
    let Some(summary) = summary else {
        return Vec::new();
    };
    summary
        .iter()
        .filter_map(|(key, value)| value.as_f64().map(|number| (key, number)))
        .map(|(key, value)| {
            metric(
                timestamp,
                &format!("sandbox.startup.{}", sanitize_metric_key(key)),
                value,
                metric_unit(key),
                sandbox_id,
                runtime_type,
                attributes,
                config,
            )
        })
        .collect()
}

#[derive(Default)]
struct CniPluginMetrics {
    count: u64,
    duration_ms: f64,
}

#[derive(Default)]
struct SpanDerivedMetrics {
    cni_duration_ms: f64,
    cni_plugin_count: BTreeSet<String>,
    cni_plugins: BTreeMap<String, CniPluginMetrics>,
    oci_duration_ms: f64,
    oci_call_count: u64,
    binary_exec_count: u64,
    binary_exec_duration_ms: f64,
    helper_binary_count: u64,
    helper_binary_duration_ms: f64,
    iptables_count: u64,
    iptables_duration_ms: f64,
    nft_count: u64,
    nft_duration_ms: f64,
    ip_count: u64,
    ip_duration_ms: f64,
    tc_count: u64,
    tc_duration_ms: f64,
    kata_duration_ms: f64,
}

fn metrics_from_spans(
    spans: &[StartupStageReport],
    timestamp: &str,
    sandbox_id: &str,
    runtime_type: &str,
    attributes: &Map<String, Value>,
    config: &CollectorConfig,
) -> Vec<MetricSample> {
    let mut derived = SpanDerivedMetrics::default();

    for span in spans {
        let duration = span_duration_ms(span).unwrap_or(0.0);
        let span_name = span.span_name.to_ascii_lowercase();
        let binary = span
            .binary
            .as_deref()
            .or_else(|| span.command.as_deref())
            .unwrap_or("");
        let binary_name = binary_basename(binary).to_ascii_lowercase();
        let role = span.role.as_deref().unwrap_or("").to_ascii_lowercase();

        let is_cni = is_cni_span(span, &span_name, &binary_name);
        let is_oci = is_oci_span(span, &span_name, &binary_name);
        let is_kata = is_kata_span(&span_name, &binary_name);

        if is_cni {
            derived.cni_duration_ms += duration;
            let plugin = cni_plugin_name(span, &span_name, &binary_name);
            if !plugin.is_empty() {
                derived.cni_plugin_count.insert(plugin.clone());
                let plugin_stats = derived.cni_plugins.entry(plugin).or_default();
                plugin_stats.count += 1;
                plugin_stats.duration_ms += duration;
            }
        }

        if is_oci {
            derived.oci_duration_ms += duration;
            derived.oci_call_count += 1;
        }

        if is_kata {
            derived.kata_duration_ms += duration;
        }

        if !binary_name.is_empty()
            || span.process_id.is_some()
            || role == "exec"
            || is_cni
            || is_oci
        {
            derived.binary_exec_count += 1;
            derived.binary_exec_duration_ms += duration;
        }

        if is_helper_binary(&binary_name) {
            derived.helper_binary_count += 1;
            derived.helper_binary_duration_ms += duration;
        }

        match binary_name.as_str() {
            "iptables" | "iptables-restore" | "ip6tables" | "ip6tables-restore" => {
                derived.iptables_count += 1;
                derived.iptables_duration_ms += duration;
            }
            "nft" => {
                derived.nft_count += 1;
                derived.nft_duration_ms += duration;
            }
            "ip" => {
                derived.ip_count += 1;
                derived.ip_duration_ms += duration;
            }
            "tc" => {
                derived.tc_count += 1;
                derived.tc_duration_ms += duration;
            }
            _ => {}
        }
    }

    let mut metrics = Vec::new();
    push_metric_if_positive(
        &mut metrics,
        timestamp,
        "sandbox.startup.cni_duration_ms",
        derived.cni_duration_ms,
        "ms",
        sandbox_id,
        runtime_type,
        attributes,
        config,
    );
    push_metric_if_positive(
        &mut metrics,
        timestamp,
        "sandbox.startup.cni_plugin_count",
        derived.cni_plugin_count.len() as f64,
        "count",
        sandbox_id,
        runtime_type,
        attributes,
        config,
    );
    for (plugin_name, stats) in &derived.cni_plugins {
        push_cni_plugin_metric_if_positive(
            &mut metrics,
            timestamp,
            plugin_name,
            stats.count as f64,
            stats.duration_ms,
            sandbox_id,
            runtime_type,
            attributes,
            config,
        );
    }
    push_metric_if_positive(
        &mut metrics,
        timestamp,
        "sandbox.startup.oci_duration_ms",
        derived.oci_duration_ms,
        "ms",
        sandbox_id,
        runtime_type,
        attributes,
        config,
    );
    push_metric_if_positive(
        &mut metrics,
        timestamp,
        "sandbox.startup.oci_call_count",
        derived.oci_call_count as f64,
        "count",
        sandbox_id,
        runtime_type,
        attributes,
        config,
    );
    push_metric_if_positive(
        &mut metrics,
        timestamp,
        "sandbox.startup.binary_exec_count",
        derived.binary_exec_count as f64,
        "count",
        sandbox_id,
        runtime_type,
        attributes,
        config,
    );
    push_metric_if_positive(
        &mut metrics,
        timestamp,
        "sandbox.startup.binary_exec_duration_ms",
        derived.binary_exec_duration_ms,
        "ms",
        sandbox_id,
        runtime_type,
        attributes,
        config,
    );
    push_metric_if_positive(
        &mut metrics,
        timestamp,
        "sandbox.startup.helper_binary_count",
        derived.helper_binary_count as f64,
        "count",
        sandbox_id,
        runtime_type,
        attributes,
        config,
    );
    push_metric_if_positive(
        &mut metrics,
        timestamp,
        "sandbox.startup.helper_binary_duration_ms",
        derived.helper_binary_duration_ms,
        "ms",
        sandbox_id,
        runtime_type,
        attributes,
        config,
    );
    push_metric_if_positive(
        &mut metrics,
        timestamp,
        "sandbox.startup.iptables_count",
        derived.iptables_count as f64,
        "count",
        sandbox_id,
        runtime_type,
        attributes,
        config,
    );
    push_metric_if_positive(
        &mut metrics,
        timestamp,
        "sandbox.startup.iptables_duration_ms",
        derived.iptables_duration_ms,
        "ms",
        sandbox_id,
        runtime_type,
        attributes,
        config,
    );
    push_metric_if_positive(
        &mut metrics,
        timestamp,
        "sandbox.startup.nft_count",
        derived.nft_count as f64,
        "count",
        sandbox_id,
        runtime_type,
        attributes,
        config,
    );
    push_metric_if_positive(
        &mut metrics,
        timestamp,
        "sandbox.startup.nft_duration_ms",
        derived.nft_duration_ms,
        "ms",
        sandbox_id,
        runtime_type,
        attributes,
        config,
    );
    push_metric_if_positive(
        &mut metrics,
        timestamp,
        "sandbox.startup.ip_count",
        derived.ip_count as f64,
        "count",
        sandbox_id,
        runtime_type,
        attributes,
        config,
    );
    push_metric_if_positive(
        &mut metrics,
        timestamp,
        "sandbox.startup.ip_duration_ms",
        derived.ip_duration_ms,
        "ms",
        sandbox_id,
        runtime_type,
        attributes,
        config,
    );
    push_metric_if_positive(
        &mut metrics,
        timestamp,
        "sandbox.startup.tc_count",
        derived.tc_count as f64,
        "count",
        sandbox_id,
        runtime_type,
        attributes,
        config,
    );
    push_metric_if_positive(
        &mut metrics,
        timestamp,
        "sandbox.startup.tc_duration_ms",
        derived.tc_duration_ms,
        "ms",
        sandbox_id,
        runtime_type,
        attributes,
        config,
    );
    push_metric_if_positive(
        &mut metrics,
        timestamp,
        "sandbox.startup.kata_duration_ms",
        derived.kata_duration_ms,
        "ms",
        sandbox_id,
        runtime_type,
        attributes,
        config,
    );

    metrics
}

fn metrics_from_uprobe_normalization(
    normalized: &UprobeNormalization,
    timestamp: &str,
    sandbox_id: &str,
    runtime_type: &str,
    attributes: &Map<String, Value>,
    config: &CollectorConfig,
) -> Vec<MetricSample> {
    if normalized.event_count == 0 {
        return Vec::new();
    }

    let mut metric_attributes = attributes.clone();
    metric_attributes.insert("startup.metric.kind".to_string(), json!("uprobe_quality"));
    let mut metrics = Vec::new();
    metrics.push(metric(
        timestamp,
        "sandbox.startup.uprobe_event_count",
        normalized.event_count as f64,
        "count",
        sandbox_id,
        runtime_type,
        &metric_attributes,
        config,
    ));
    metrics.push(metric(
        timestamp,
        "sandbox.startup.uprobe_matched_event_count",
        normalized.matched_event_count as f64,
        "count",
        sandbox_id,
        runtime_type,
        &metric_attributes,
        config,
    ));
    metrics.push(metric(
        timestamp,
        "sandbox.startup.uprobe_span_count",
        normalized.span_count as f64,
        "count",
        sandbox_id,
        runtime_type,
        &metric_attributes,
        config,
    ));
    metrics.push(metric(
        timestamp,
        "sandbox.startup.uprobe_unmatched_event_count",
        normalized.unmatched_event_count() as f64,
        "count",
        sandbox_id,
        runtime_type,
        &metric_attributes,
        config,
    ));
    metrics.push(metric(
        timestamp,
        "sandbox.startup.uprobe_pairing_ratio",
        normalized.pairing_ratio(),
        "ratio",
        sandbox_id,
        runtime_type,
        &metric_attributes,
        config,
    ));
    metrics
}

fn push_cni_plugin_metric_if_positive(
    metrics: &mut Vec<MetricSample>,
    timestamp: &str,
    plugin_name: &str,
    count: f64,
    duration_ms: f64,
    sandbox_id: &str,
    runtime_type: &str,
    attributes: &Map<String, Value>,
    config: &CollectorConfig,
) {
    if count <= 0.0 && duration_ms <= 0.0 {
        return;
    }

    let mut plugin_attributes = attributes.clone();
    plugin_attributes.insert("cni.plugin".to_string(), json!(plugin_name));
    plugin_attributes.insert(
        "startup.metric.kind".to_string(),
        json!("cni_plugin_breakdown"),
    );
    let metric_prefix = format!(
        "sandbox.startup.cni.plugin.{}",
        sanitize_metric_key(plugin_name)
    );
    push_metric_if_positive(
        metrics,
        timestamp,
        &format!("{metric_prefix}_count"),
        count,
        "count",
        sandbox_id,
        runtime_type,
        &plugin_attributes,
        config,
    );
    push_metric_if_positive(
        metrics,
        timestamp,
        &format!("{metric_prefix}_duration_ms"),
        duration_ms,
        "ms",
        sandbox_id,
        runtime_type,
        &plugin_attributes,
        config,
    );
}

fn push_metric_if_positive(
    metrics: &mut Vec<MetricSample>,
    timestamp: &str,
    name: &str,
    value: f64,
    unit: &str,
    sandbox_id: &str,
    runtime_type: &str,
    attributes: &Map<String, Value>,
    config: &CollectorConfig,
) {
    if value > 0.0 {
        metrics.push(metric(
            timestamp,
            name,
            value,
            unit,
            sandbox_id,
            runtime_type,
            attributes,
            config,
        ));
    }
}

fn span_duration_ms(span: &StartupStageReport) -> Option<f64> {
    if let Some(duration) = span.duration_ms {
        return Some(duration.max(1.0));
    }
    duration_between(span.start_time.as_deref()?, span.end_time.as_deref()?)
}

fn is_cni_span(span: &StartupStageReport, span_name: &str, binary_name: &str) -> bool {
    if span
        .role
        .as_deref()
        .is_some_and(|role| role.eq_ignore_ascii_case("helper"))
        || is_helper_binary(binary_name)
    {
        return false;
    }

    span_name.starts_with("cni.")
        || span.cni_plugin.is_some()
        || span.cni_command.is_some()
        || span.cni_container_id.is_some()
        || span
            .env
            .as_ref()
            .is_some_and(|env| env.contains_key("CNI_COMMAND"))
        || cni_plugin_from_span_name(span_name, binary_name) != ""
}

fn is_oci_span(span: &StartupStageReport, span_name: &str, binary_name: &str) -> bool {
    span_name.starts_with("oci.")
        || span.oci_runtime.is_some()
        || span.oci_operation.is_some()
        || matches!(binary_name, "runc" | "crun" | "kata-runtime" | "runsc")
}

fn is_kata_span(span_name: &str, binary_name: &str) -> bool {
    span_name.starts_with("kata.")
        || binary_name.contains("kata")
        || matches!(
            binary_name,
            "qemu-system-x86_64" | "qemu-system-aarch64" | "cloud-hypervisor" | "firecracker"
        )
}

fn is_helper_binary(binary_name: &str) -> bool {
    matches!(
        binary_name,
        "iptables"
            | "iptables-restore"
            | "ip6tables"
            | "ip6tables-restore"
            | "nft"
            | "ip"
            | "ipset"
            | "tc"
            | "mount"
            | "umount"
            | "modprobe"
    )
}

fn cni_plugin_name(span: &StartupStageReport, span_name: &str, binary_name: &str) -> String {
    if is_helper_binary(binary_name) {
        return String::new();
    }
    if let Some(plugin) = span
        .cni_plugin
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        return plugin.to_string();
    }
    if !binary_name.is_empty()
        && (span_name.starts_with("cni.")
            || span.cni_command.is_some()
            || span.cni_container_id.is_some()
            || span
                .env
                .as_ref()
                .is_some_and(|env| env.contains_key("CNI_COMMAND")))
    {
        return binary_name.to_string();
    }
    cni_plugin_from_span_name(span_name, binary_name)
}

fn cni_plugin_from_span_name(span_name: &str, binary_name: &str) -> String {
    if let Some(plugin) = span_name.strip_prefix("cni.plugin.") {
        return plugin.to_string();
    }
    if binary_name.starts_with("bridge")
        || matches!(
            binary_name,
            "loopback" | "portmap" | "firewall" | "host-local" | "bandwidth" | "tuning"
        )
    {
        return binary_name.to_string();
    }
    String::new()
}

fn binary_basename(binary: &str) -> &str {
    binary
        .split_whitespace()
        .next()
        .unwrap_or(binary)
        .rsplit('/')
        .next()
        .unwrap_or(binary)
}

fn metric(
    timestamp: &str,
    name: &str,
    value: f64,
    unit: &str,
    sandbox_id: &str,
    runtime_type: &str,
    attributes: &Map<String, Value>,
    config: &CollectorConfig,
) -> MetricSample {
    MetricSample {
        timestamp: timestamp.to_string(),
        name: name.to_string(),
        value,
        unit: Some(unit.to_string()),
        group: Some("startup".to_string()),
        sandbox_id: Some(sandbox_id.to_string()),
        node_id: Some(config.node_id.clone()),
        image_id: None,
        runtime_type: Some(runtime_type.to_string()),
        attributes: Some(attributes.clone()),
    }
}

fn run_report_command(command: &str, timeout: Duration) -> Result<String> {
    let mut child_command = Command::new("sh");
    child_command
        .arg("-lc")
        .arg(command)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    child_command.process_group(0);
    let mut child = child_command.spawn()?;

    let started = Instant::now();
    loop {
        if child.try_wait()?.is_some() {
            let output = child.wait_with_output()?;
            if !output.status.success() {
                return Err(CollectorError::Plugin {
                    plugin: "startup-callchain".to_string(),
                    message: format!(
                        "startup call-chain command exited with status {:?}: {}",
                        output.status.code(),
                        String::from_utf8_lossy(&output.stderr).trim()
                    ),
                });
            }
            return Ok(String::from_utf8_lossy(&output.stdout).to_string());
        }

        if started.elapsed() >= timeout {
            kill_child_tree(&mut child);
            let _ = child.wait();
            return Err(CollectorError::Plugin {
                plugin: "startup-callchain".to_string(),
                message: format!(
                    "startup call-chain command timed out after {} ms",
                    timeout.as_millis()
                ),
            });
        }

        thread::sleep(Duration::from_millis(20));
    }
}

fn kill_child_tree(child: &mut std::process::Child) {
    #[cfg(unix)]
    {
        let group = format!("-{}", child.id());
        let _ = Command::new("kill").args(["-TERM", &group]).status();
        thread::sleep(Duration::from_millis(50));
        let _ = Command::new("kill").args(["-KILL", &group]).status();
        return;
    }

    #[allow(unreachable_code)]
    {
        let _ = child.kill();
    }
}

fn merge_plugin_output(target: &mut PluginOutput, output: PluginOutput) {
    target.metadata.clusters.extend(output.metadata.clusters);
    target.metadata.nodes.extend(output.metadata.nodes);
    target.metadata.images.extend(output.metadata.images);
    target.metadata.sandboxes.extend(output.metadata.sandboxes);
    target.metrics.extend(output.metrics);
    target.events.extend(output.events);
    target.traces.extend(output.traces);
    target.profiles.extend(output.profiles);
}

fn has_payload(output: &PluginOutput) -> bool {
    !output.metadata.clusters.is_empty()
        || !output.metadata.nodes.is_empty()
        || !output.metadata.images.is_empty()
        || !output.metadata.sandboxes.is_empty()
        || !output.metrics.is_empty()
        || !output.events.is_empty()
        || !output.traces.is_empty()
        || !output.profiles.is_empty()
}

fn env_path(name: &str) -> Option<PathBuf> {
    env_string(name).map(PathBuf::from)
}

fn env_string(name: &str) -> Option<String> {
    env::var(name).ok().filter(|value| !value.trim().is_empty())
}

fn env_u64(name: &str) -> Option<u64> {
    env::var(name).ok()?.parse::<u64>().ok()
}

fn timestamp(time: DateTime<Utc>) -> String {
    time.to_rfc3339_opts(SecondsFormat::Millis, true)
}

fn metric_unit(key: &str) -> &'static str {
    let key = key.to_ascii_lowercase();
    if key.contains("duration") || key.contains("latency") || key.ends_with("ms") {
        "ms"
    } else if key.contains("count") || key.ends_with("total") {
        "count"
    } else {
        "1"
    }
}

fn first_non_empty<'a, I>(values: I) -> &'a str
where
    I: IntoIterator<Item = &'a str>,
{
    values
        .into_iter()
        .map(str::trim)
        .find(|value| !value.is_empty())
        .unwrap_or("")
}

fn sanitize_metric_key(value: &str) -> String {
    let mut output = String::new();
    let mut previous_lowercase = false;
    for character in value.chars() {
        if character.is_ascii_uppercase() {
            if previous_lowercase {
                output.push('_');
            }
            output.push(character.to_ascii_lowercase());
            previous_lowercase = false;
        } else if character.is_ascii_alphanumeric() {
            output.push(character.to_ascii_lowercase());
            previous_lowercase = true;
        } else if !output.ends_with('_') {
            output.push('_');
            previous_lowercase = false;
        }
    }
    output.trim_matches('_').to_string()
}

fn sanitize_id(value: &str) -> String {
    let sanitized = value
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
    if sanitized.is_empty() {
        "unknown".to_string()
    } else {
        sanitized
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_startup_callchain_report_into_trace_and_metrics() {
        let config = test_config();
        let content = r#"
        {
          "id": "runpod-a",
          "traceId": "trace-runpod-a",
          "sandboxId": "cri-sandbox-a",
          "runtimeType": "runc",
          "runtimeHandler": "runc",
          "startTime": "2026-05-26T01:00:00.000Z",
          "endTime": "2026-05-26T01:00:01.000Z",
          "durationMs": 1000,
          "summary": {"cniDurationMs": 250},
          "spans": [
            {
              "spanName": "cri.run_pod_sandbox",
              "startTime": "2026-05-26T01:00:00.000Z",
              "durationMs": 1000
            },
            {
              "spanName": "cni.add",
              "startTime": "2026-05-26T01:00:00.100Z",
              "durationMs": 100,
              "binary": "/opt/cni/bin/bridge",
              "cniCommand": "ADD",
              "cniContainerId": "cri-sandbox-a"
            },
            {
              "spanName": "oci.runc.create",
              "startTime": "2026-05-26T01:00:00.400Z",
              "durationMs": 80,
              "ociRuntime": "runc",
              "ociOperation": "create",
              "bundle": "/run/containerd/io.containerd.runtime.v2.task/k8s.io/cri-sandbox-a"
            },
            {
              "spanName": "process.exec.iptables",
              "startTime": "2026-05-26T01:00:00.500Z",
              "durationMs": 25,
              "binary": "/usr/sbin/iptables",
              "pid": 4321
            }
          ]
        }
        "#;

        let output = startup_callchain_output_from_content(content, Utc::now(), &config).unwrap();
        assert_eq!(output.traces.len(), 5);
        assert_eq!(output.traces[0].span_name, "sandbox.startup.callchain");
        assert!(output
            .traces
            .iter()
            .any(|span| span.span_name == "cni.add"
                && span.attributes["cni.plugin"] == json!("bridge")));
        assert!(output.metrics.iter().any(|metric| {
            metric.name == "sandbox.startup.cni_duration_ms" && metric.value == 250.0
        }));
        assert!(output.metrics.iter().any(|metric| {
            metric.name == "sandbox.startup.cni.plugin.bridge_duration_ms" && metric.value == 100.0
        }));
        assert!(output.metrics.iter().any(|metric| {
            metric.name == "sandbox.startup.cni.plugin.bridge_count" && metric.value == 1.0
        }));
        assert!(output.metrics.iter().any(|metric| {
            metric.name == "sandbox.startup.oci_duration_ms" && metric.value == 80.0
        }));
        assert!(output.metrics.iter().any(|metric| {
            metric.name == "sandbox.startup.iptables_count" && metric.value == 1.0
        }));
        assert!(output.metrics.iter().any(|metric| {
            metric.name == "sandbox.startup.binary_exec_count" && metric.value == 3.0
        }));
        assert_eq!(output.events[0].event_name, "startup.callchain.observed");
        assert_eq!(output.metadata.nodes.len(), 1);
    }

    #[test]
    fn defaults_to_cri_containerd_startup_trace_id_when_missing() {
        let config = test_config();
        let content = r#"
        {
          "sandboxId": "cri-sandbox-a",
          "runtimeType": "kata",
          "startTime": "2026-05-26T01:00:00.000Z",
          "endTime": "2026-05-26T01:00:01.000Z",
          "spans": [{
            "spanName": "kata.agent.connect",
            "startTime": "2026-05-26T01:00:00.800Z",
            "durationMs": 50
          }]
        }
        "#;

        let output = startup_callchain_output_from_content(content, Utc::now(), &config).unwrap();
        assert!(output
            .traces
            .iter()
            .all(|span| span.trace_id == "cri-containerd-startup-cri-sandbox-a"));
        assert_eq!(
            output.traces[0].parent_span_id.as_deref(),
            Some("cri-containerd-startup-cri-sandbox-a-e2e")
        );
        assert_eq!(
            output.traces[1].parent_span_id.as_deref(),
            Some("cri-containerd-startup-cri-sandbox-a-callchain")
        );
    }

    #[test]
    fn derives_kubernetes_sandbox_identity_for_trace_join() {
        let config = test_config();
        let content = r#"
        {
          "criSandboxId": "sandboxabcdef1234567890",
          "containerdId": "sandboxabcdef1234567890",
          "namespace": "default",
          "podName": "runtimepulse-demo",
          "containerName": "POD",
          "podUid": "runtimepulse-demo-uid",
          "runtimeType": "runc",
          "startTime": "2026-05-26T01:00:00.000Z",
          "durationMs": 100,
          "spans": [{
            "spanName": "cni.plugin.loopback",
            "startTime": "2026-05-26T01:00:00.010Z",
            "durationMs": 10
          }]
        }
        "#;

        let output = startup_callchain_output_from_content(content, Utc::now(), &config).unwrap();
        assert!(output.traces.iter().all(
            |span| span.trace_id == "cri-containerd-startup-k8s-default-runtimepulse-demo-pod"
        ));
        assert_eq!(
            output.traces[0].sandbox_id.as_deref(),
            Some("k8s-default-runtimepulse-demo-pod")
        );
        assert_eq!(
            output.traces[0].attributes["cri.sandbox_id"],
            json!("sandboxabcdef1234567890")
        );
        assert_eq!(
            output.events[0].sandbox_id.as_deref(),
            Some("k8s-default-runtimepulse-demo-pod")
        );
    }

    #[test]
    fn normalizes_uprobe_enter_exit_events_into_startup_callchain() {
        let config = test_config();
        let content = r#"
        {
          "source": "uprobe-exporter",
          "events": [
            {
              "eventType": "enter",
              "requestId": "runpod-1",
              "function": "RunPodSandbox",
              "timestamp": "2026-05-26T01:00:00.000Z",
              "criSandboxId": "sandboxabcdef1234567890",
              "namespace": "default",
              "podName": "demo",
              "containerName": "POD",
              "runtimeType": "kata",
              "runtimeHandler": "kata"
            },
            {
              "eventType": "exit",
              "requestId": "runpod-1",
              "function": "RunPodSandbox",
              "timestamp": "2026-05-26T01:00:01.000Z",
              "status": "ok"
            },
            {
              "eventType": "enter",
              "requestId": "cni-bridge-1",
              "function": "libc.execve",
              "timestamp": "2026-05-26T01:00:00.100Z",
              "binary": "/opt/cni/bin/bridge",
              "cniCommand": "ADD",
              "cniContainerId": "sandboxabcdef1234567890"
            },
            {
              "eventType": "exit",
              "requestId": "cni-bridge-1",
              "function": "libc.execve",
              "timestamp": "2026-05-26T01:00:00.400Z",
              "binary": "/opt/cni/bin/bridge",
              "cniCommand": "ADD",
              "cniContainerId": "sandboxabcdef1234567890"
            },
            {
              "eventType": "enter",
              "requestId": "oci-runc-1",
              "function": "oci.runtime.create",
              "timestamp": "2026-05-26T01:00:00.500Z",
              "binary": "/usr/bin/kata-runtime",
              "ociRuntime": "kata-runtime",
              "ociOperation": "create",
              "bundle": "/run/containerd/io.containerd.runtime.v2.task/k8s.io/sandboxabcdef1234567890"
            },
            {
              "eventType": "exit",
              "requestId": "oci-runc-1",
              "function": "oci.runtime.create",
              "timestamp": "2026-05-26T01:00:00.700Z",
              "binary": "/usr/bin/kata-runtime",
              "ociRuntime": "kata-runtime",
              "ociOperation": "create"
            }
          ]
        }
        "#;

        let output = startup_callchain_output_from_content(content, Utc::now(), &config).unwrap();

        assert!(output.traces.iter().all(|span| {
            span.trace_id == "cri-containerd-startup-k8s-default-demo-pod"
                && span.sandbox_id.as_deref() == Some("k8s-default-demo-pod")
        }));
        assert!(output
            .traces
            .iter()
            .any(|span| { span.span_name == "cri.run_pod_sandbox" && span.duration_ms == 1000.0 }));
        assert!(output.traces.iter().any(|span| {
            span.span_name == "cni.plugin.bridge"
                && span.duration_ms == 300.0
                && span.attributes["cni.plugin"] == json!("bridge")
        }));
        assert!(output
            .traces
            .iter()
            .any(|span| { span.span_name == "oci.kata-runtime" && span.duration_ms == 200.0 }));
        assert!(output.metrics.iter().any(|metric| {
            metric.name == "sandbox.startup.cni.plugin.bridge_duration_ms" && metric.value == 300.0
        }));
        assert!(output.metrics.iter().any(|metric| {
            metric.name == "sandbox.startup.oci_duration_ms" && metric.value == 200.0
        }));
        assert!(output.metrics.iter().any(|metric| {
            metric.name == "sandbox.startup.kata_duration_ms" && metric.value == 200.0
        }));
        assert!(output.metrics.iter().any(|metric| {
            metric.name == "sandbox.startup.uprobe_event_count" && metric.value == 6.0
        }));
        assert!(output.metrics.iter().any(|metric| {
            metric.name == "sandbox.startup.uprobe_unmatched_event_count" && metric.value == 0.0
        }));
        assert!(output.metrics.iter().any(|metric| {
            metric.name == "sandbox.startup.uprobe_pairing_ratio" && metric.value == 1.0
        }));
        assert_eq!(output.events[0].severity, "info");
        assert_eq!(
            output.metadata.sandboxes[0]["id"],
            json!("k8s-default-demo-pod")
        );
    }

    #[test]
    fn keeps_cni_helper_execs_out_of_cni_plugin_metrics() {
        let config = test_config();
        let content = r#"
        {
          "sandboxId": "sandbox-cni-helper",
          "runtimeType": "runc",
          "events": [
            {
              "eventType": "enter",
              "requestId": "cni-bridge",
              "function": "execve",
              "timestamp": "2026-05-26T01:00:00.000Z",
              "binary": "/opt/cni/bin/bridge",
              "cniCommand": "ADD",
              "cniContainerId": "sandbox-cni-helper"
            },
            {
              "eventType": "exit",
              "requestId": "cni-bridge",
              "function": "execve",
              "timestamp": "2026-05-26T01:00:00.100Z",
              "binary": "/opt/cni/bin/bridge",
              "cniCommand": "ADD",
              "cniContainerId": "sandbox-cni-helper"
            },
            {
              "eventType": "enter",
              "requestId": "iptables-child",
              "function": "execve",
              "timestamp": "2026-05-26T01:00:00.020Z",
              "binary": "/usr/sbin/iptables",
              "role": "helper",
              "cniCommand": "ADD",
              "cniContainerId": "sandbox-cni-helper",
              "env": {"CNI_COMMAND": "ADD"}
            },
            {
              "eventType": "exit",
              "requestId": "iptables-child",
              "function": "execve",
              "timestamp": "2026-05-26T01:00:00.050Z",
              "binary": "/usr/sbin/iptables",
              "role": "helper",
              "cniCommand": "ADD",
              "cniContainerId": "sandbox-cni-helper",
              "env": {"CNI_COMMAND": "ADD"}
            }
          ]
        }
        "#;

        let output = startup_callchain_output_from_content(content, Utc::now(), &config).unwrap();

        assert!(output.traces.iter().any(|span| {
            span.span_name == "cni.plugin.bridge"
                && span.duration_ms == 100.0
                && span.attributes["cni.plugin"] == json!("bridge")
        }));
        assert!(output.traces.iter().any(|span| {
            span.span_name == "process.exec.iptables"
                && span.duration_ms == 30.0
                && span.attributes["process.role"] == json!("helper")
        }));
        assert!(output.metrics.iter().any(|metric| {
            metric.name == "sandbox.startup.cni_duration_ms" && metric.value == 100.0
        }));
        assert!(output.metrics.iter().any(|metric| {
            metric.name == "sandbox.startup.cni.plugin.bridge_duration_ms" && metric.value == 100.0
        }));
        assert!(!output
            .metrics
            .iter()
            .any(|metric| { metric.name == "sandbox.startup.cni.plugin.iptables_duration_ms" }));
        assert!(output.metrics.iter().any(|metric| {
            metric.name == "sandbox.startup.helper_binary_duration_ms" && metric.value == 30.0
        }));
        assert!(output.metrics.iter().any(|metric| {
            metric.name == "sandbox.startup.iptables_duration_ms" && metric.value == 30.0
        }));
    }

    #[test]
    fn links_child_exec_spans_to_parent_process_span() {
        let config = test_config();
        let content = r#"
        {
          "sandboxId": "sandbox-proc-tree",
          "runtimeType": "runc",
          "events": [
            {
              "eventType": "enter",
              "requestId": "cni-bridge",
              "function": "execve",
              "timestamp": "2026-05-26T01:00:00.000Z",
              "binary": "/opt/cni/bin/bridge",
              "pid": 100,
              "ppid": 1,
              "cniCommand": "ADD",
              "cniContainerId": "sandbox-proc-tree"
            },
            {
              "eventType": "enter",
              "requestId": "iptables-child",
              "function": "execve",
              "timestamp": "2026-05-26T01:00:00.020Z",
              "binary": "/usr/sbin/iptables",
              "role": "helper",
              "pid": 102,
              "ppid": 100,
              "cniCommand": "ADD",
              "cniContainerId": "sandbox-proc-tree"
            },
            {
              "eventType": "exit",
              "requestId": "iptables-child",
              "function": "execve",
              "timestamp": "2026-05-26T01:00:00.050Z",
              "binary": "/usr/sbin/iptables",
              "role": "helper",
              "pid": 102,
              "ppid": 100,
              "cniCommand": "ADD",
              "cniContainerId": "sandbox-proc-tree"
            },
            {
              "eventType": "exit",
              "requestId": "cni-bridge",
              "function": "execve",
              "timestamp": "2026-05-26T01:00:00.100Z",
              "binary": "/opt/cni/bin/bridge",
              "pid": 100,
              "ppid": 1,
              "cniCommand": "ADD",
              "cniContainerId": "sandbox-proc-tree"
            }
          ]
        }
        "#;

        let output = startup_callchain_output_from_content(content, Utc::now(), &config).unwrap();
        let bridge = output
            .traces
            .iter()
            .find(|span| span.span_name == "cni.plugin.bridge")
            .expect("bridge span");
        let iptables = output
            .traces
            .iter()
            .find(|span| span.span_name == "process.exec.iptables")
            .expect("iptables span");

        assert_eq!(
            iptables.parent_span_id.as_deref(),
            Some(bridge.span_id.as_str())
        );
        assert_eq!(iptables.attributes["startup.parent.pid"], json!(100));
        assert_eq!(
            iptables.attributes["startup.parent.span_id"],
            json!(bridge.span_id)
        );
    }

    #[test]
    fn flags_unmatched_uprobe_events_for_exporter_quality() {
        let config = test_config();
        let content = r#"
        {
          "sandboxId": "sandbox-quality",
          "runtimeType": "runc",
          "events": [
            {
              "eventType": "enter",
              "requestId": "runpod-quality",
              "function": "RunPodSandbox",
              "timestamp": "2026-05-26T01:00:00.000Z"
            },
            {
              "eventType": "exit",
              "requestId": "cni-exit-only",
              "function": "execve",
              "timestamp": "2026-05-26T01:00:00.120Z",
              "durationMs": 20,
              "binary": "/opt/cni/bin/bridge",
              "cniCommand": "ADD"
            }
          ]
        }
        "#;

        let output = startup_callchain_output_from_content(content, Utc::now(), &config).unwrap();

        assert_eq!(output.events[0].severity, "warning");
        assert_eq!(
            output.events[0].attributes["startup.uprobe.unmatched_event_count"],
            json!(1)
        );
        assert!(output.metrics.iter().any(|metric| {
            metric.name == "sandbox.startup.uprobe_event_count" && metric.value == 2.0
        }));
        assert!(output.metrics.iter().any(|metric| {
            metric.name == "sandbox.startup.uprobe_matched_event_count" && metric.value == 1.0
        }));
        assert!(output.metrics.iter().any(|metric| {
            metric.name == "sandbox.startup.uprobe_unmatched_event_count" && metric.value == 1.0
        }));
        assert!(output.metrics.iter().any(|metric| {
            metric.name == "sandbox.startup.uprobe_pairing_ratio" && metric.value == 0.5
        }));
    }

    #[test]
    fn callchain_spans_carry_runtime_type_from_handler() {
        let config = test_config();
        let content = r#"{
          "id":"handler-only",
          "sandboxId":"handler-sandbox",
          "runtimeHandler":"kata",
          "startTime":"2026-05-26T01:00:00.000Z",
          "endTime":"2026-05-26T01:00:00.100Z",
          "spans":[{
            "spanName":"oci.kata-runtime",
            "startTime":"2026-05-26T01:00:00.010Z",
            "endTime":"2026-05-26T01:00:00.090Z",
            "binary":"/usr/bin/kata-runtime"
          }]
        }"#;

        let output = startup_callchain_output_from_content(content, Utc::now(), &config)
            .expect("callchain output");
        assert_eq!(output.metadata.sandboxes[0]["runtimeType"], "kata");
        assert!(output
            .traces
            .iter()
            .all(|span| span.runtime_type.as_deref() == Some("kata")));
    }

    #[test]
    fn groups_jsonl_uprobe_events_into_one_callchain_report() {
        let config = test_config();
        let content = r#"
{"eventType":"enter","requestId":"runpod-jsonl","function":"RunPodSandbox","timestamp":"2026-05-26T01:00:00.000Z","sandboxId":"sandbox-jsonl","runtimeType":"runc"}
{"eventType":"exit","requestId":"runpod-jsonl","function":"RunPodSandbox","timestamp":"2026-05-26T01:00:00.500Z"}
        "#;

        let output = startup_callchain_output_from_content(content, Utc::now(), &config).unwrap();

        assert_eq!(output.metadata.sandboxes.len(), 1);
        assert!(output.traces.iter().any(|span| {
            span.span_name == "cri.run_pod_sandbox"
                && span.trace_id == "cri-containerd-startup-sandbox-jsonl"
                && span.duration_ms == 500.0
        }));
    }

    #[test]
    fn accepts_runtimepulse_plugin_output() {
        let config = test_config();
        let content = r#"{
          "traces": [{
            "traceId": "trace-a",
            "spanId": "span-a",
            "spanName": "cri.run_pod_sandbox",
            "startTime": "2026-05-26T01:00:00.000Z",
            "endTime": "2026-05-26T01:00:00.010Z",
            "durationMs": 10,
            "status": "ok",
            "attributes": {},
            "sandboxId": "sandbox-a"
          }]
        }"#;

        let output = startup_callchain_output_from_content(content, Utc::now(), &config).unwrap();
        assert_eq!(output.traces.len(), 1);
        assert_eq!(output.traces[0].trace_id, "trace-a");
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
