//! gVisor sandbox report source.
//!
//! Reads JSON/JSONL exported by real runsc/gVisor tooling or sidecar exporters
//! and normalizes sandbox/kernel observations into RuntimePulse records. The
//! collector does not fabricate state; without a configured report file it emits
//! an empty output.

use chrono::{DateTime, SecondsFormat, Utc};
use serde::Deserialize;
use serde_json::{json, Map, Value};
use std::env;
use std::fs;
use std::path::PathBuf;

use crate::collectors::core::config::CollectorConfig;
use crate::collectors::core::error::Result;
use crate::collectors::core::model::{
    EventRecord, Metadata, MetricSample, PluginOutput, TraceSpan,
};
use crate::collectors::core::plugin::CollectorPlugin;
use crate::collectors::core::report::metric;
use crate::collectors::sources::image::layer::docker_image_id_from_ref_or_digest;

pub struct GvisorSandboxPlugin {
    path: Option<PathBuf>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GvisorReport {
    timestamp: Option<String>,
    source: Option<String>,
    #[serde(default)]
    sandboxes: Vec<GvisorSandboxReport>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GvisorSandboxReport {
    id: Option<String>,
    sandbox_id: Option<String>,
    name: Option<String>,
    workload_id: Option<String>,
    workload_name: Option<String>,
    namespace: Option<String>,
    image_id: Option<String>,
    image_ref: Option<String>,
    image_digest: Option<String>,
    status: Option<String>,
    runtime_version: Option<String>,
    platform: Option<String>,
    sandbox_pid: Option<u64>,
    gofer_pid: Option<u64>,
    sentry_pid: Option<u64>,
    gofer_pids: Option<Vec<u64>>,
    sentry_pids: Option<Vec<u64>>,
    boot_time_ms: Option<f64>,
    syscall_count: Option<u64>,
    syscall_latency_ms: Option<f64>,
    gofer_io_bytes: Option<u64>,
    faults: Option<u64>,
    created_at: Option<String>,
    started_at: Option<String>,
    stopped_at: Option<String>,
    labels: Option<Map<String, Value>>,
    attributes: Option<Map<String, Value>>,
}

enum ParsedGvisorReport {
    RuntimePulse(PluginOutput),
    Lightweight(GvisorReport),
}

impl GvisorSandboxPlugin {
    pub fn new(path: Option<PathBuf>) -> Self {
        Self { path }
    }

    pub fn from_env() -> Self {
        Self::new(env_path("RUNTIMEPULSE_GVISOR_REPORT_PATH"))
    }
}

impl CollectorPlugin for GvisorSandboxPlugin {
    fn name(&self) -> &str {
        "gvisor"
    }

    fn collect(&mut self, now: DateTime<Utc>, config: &CollectorConfig) -> Result<PluginOutput> {
        let Some(path) = self.path.clone() else {
            return Ok(PluginOutput::default());
        };
        let content = match fs::read_to_string(&path) {
            Ok(content) => content,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(PluginOutput::default());
            }
            Err(error) => return Err(error.into()),
        };

        gvisor_output_from_content(&content, now, config)
    }
}

pub fn gvisor_output_from_content(
    content: &str,
    now: DateTime<Utc>,
    config: &CollectorConfig,
) -> Result<PluginOutput> {
    let fallback_timestamp = timestamp(now);
    let mut output = PluginOutput::default();

    for report in parse_reports(content)? {
        match report {
            ParsedGvisorReport::RuntimePulse(runtimepulse_output) => {
                merge_plugin_output(&mut output, runtimepulse_output);
            }
            ParsedGvisorReport::Lightweight(report) => {
                merge_plugin_output(
                    &mut output,
                    output_from_lightweight_report(report, &fallback_timestamp, config),
                );
            }
        }
    }

    Ok(output)
}

fn parse_reports(content: &str) -> Result<Vec<ParsedGvisorReport>> {
    let trimmed = content.trim();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }

    if trimmed.starts_with('[') {
        if let Ok(rows) = serde_json::from_str::<Vec<GvisorSandboxReport>>(trimmed) {
            return Ok(vec![ParsedGvisorReport::Lightweight(GvisorReport {
                timestamp: None,
                source: None,
                sandboxes: rows,
            })]);
        }
        return Ok(serde_json::from_str::<Vec<GvisorReport>>(trimmed)?
            .into_iter()
            .map(ParsedGvisorReport::Lightweight)
            .collect());
    }

    if trimmed.starts_with('{') {
        return parse_single(trimmed).map(|report| vec![report]);
    }

    let mut reports = Vec::new();
    for line in trimmed
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
    {
        reports.push(parse_single(line)?);
    }
    Ok(reports)
}

fn parse_single(value: &str) -> Result<ParsedGvisorReport> {
    if let Ok(output) = serde_json::from_str::<PluginOutput>(value) {
        if has_plugin_output_payload(&output) {
            return Ok(ParsedGvisorReport::RuntimePulse(output));
        }
    }
    if let Ok(report) = serde_json::from_str::<GvisorReport>(value) {
        if !report.sandboxes.is_empty() {
            return Ok(ParsedGvisorReport::Lightweight(report));
        }
    }
    let row = serde_json::from_str::<GvisorSandboxReport>(value)?;
    Ok(ParsedGvisorReport::Lightweight(GvisorReport {
        timestamp: None,
        source: None,
        sandboxes: vec![row],
    }))
}

fn output_from_lightweight_report(
    report: GvisorReport,
    fallback_timestamp: &str,
    config: &CollectorConfig,
) -> PluginOutput {
    let report_timestamp = report
        .timestamp
        .unwrap_or_else(|| fallback_timestamp.to_string());
    let source = report.source.unwrap_or_else(|| "gvisor".to_string());
    let mut output = PluginOutput {
        source: Some(source.clone()),
        metadata: Metadata::default(),
        metrics: Vec::new(),
        events: Vec::new(),
        traces: Vec::new(),
        profiles: Vec::new(),
    };

    if !report.sandboxes.is_empty() {
        output.metadata.clusters.push(json!({
            "id": config.cluster_id,
            "name": config.cluster_id,
            "environment": "collector"
        }));
        output.metadata.nodes.push(json!({
            "id": config.node_id,
            "clusterId": config.cluster_id,
            "name": config.node_id,
            "status": "ready",
            "labels": {
                "collector": "runtimepulse-rust-collector",
                "plugin": "gvisor",
                "scope": config.collection_scope,
            }
        }));
    }

    for (index, sandbox) in report.sandboxes.into_iter().enumerate() {
        append_sandbox_output(
            &mut output,
            sandbox,
            index,
            &report_timestamp,
            &source,
            config,
        );
    }

    output
}

fn append_sandbox_output(
    output: &mut PluginOutput,
    sandbox: GvisorSandboxReport,
    index: usize,
    timestamp: &str,
    source: &str,
    config: &CollectorConfig,
) {
    let sandbox_id = sandbox
        .sandbox_id
        .clone()
        .or_else(|| sandbox.id.clone())
        .unwrap_or_else(|| format!("gvisor-{}-{}", sanitize_id(timestamp), index));
    let image_ref = sandbox
        .image_ref
        .clone()
        .unwrap_or_else(|| "gvisor/unknown:latest".to_string());
    let image_digest = sandbox
        .image_digest
        .clone()
        .unwrap_or_else(|| format!("collector:{}", sanitize_id(&image_ref)));
    let image_id = sandbox
        .image_id
        .clone()
        .unwrap_or_else(|| docker_image_id_from_ref_or_digest(&image_ref, &image_digest));
    let status = sandbox
        .status
        .clone()
        .unwrap_or_else(|| "running".to_string());
    let runtime_version = sandbox.runtime_version.clone();
    let labels = sandbox.labels.clone().unwrap_or_default();
    let mut attributes = sandbox.attributes.clone().unwrap_or_default();
    attributes.insert("collector.plugin".to_string(), json!("gvisor"));
    attributes.insert("runtime.source".to_string(), json!(source));
    attributes.insert("runtime.type".to_string(), json!("gvisor"));
    if let Some(platform) = &sandbox.platform {
        attributes.insert("gvisor.platform".to_string(), json!(platform));
    }
    if let Some(pid) = sandbox.sandbox_pid {
        attributes.insert("gvisor.sandboxPid".to_string(), json!(pid));
    }
    if let Some(pid) = sandbox.gofer_pid {
        attributes.insert("gvisor.goferPid".to_string(), json!(pid));
    }
    if let Some(pid) = sandbox.sentry_pid {
        attributes.insert("gvisor.sentryPid".to_string(), json!(pid));
    }
    if let Some(pids) = &sandbox.gofer_pids {
        attributes.insert("gvisor.goferPids".to_string(), json!(pids));
    }
    if let Some(pids) = &sandbox.sentry_pids {
        attributes.insert("gvisor.sentryPids".to_string(), json!(pids));
    }

    output.metadata.images.push(json!({
        "id": image_id,
        "ref": image_ref,
        "digest": image_digest,
        "loadingMode": "runtime-observed",
        "attributes": {
            "collector.plugin": "gvisor",
            "runtime.source": source,
        }
    }));
    output.metadata.sandboxes.push(json!({
        "id": sandbox_id,
        "clusterId": config.cluster_id,
        "nodeId": config.node_id,
        "namespace": sandbox.namespace.unwrap_or_else(|| "default".to_string()),
        "workloadId": sandbox.workload_id.unwrap_or_else(|| sandbox_id.clone()),
        "workloadName": sandbox.workload_name.or(sandbox.name).unwrap_or_else(|| sandbox_id.clone()),
        "imageId": image_id,
        "imageRef": image_ref,
        "runtimeType": "gvisor",
        "runtimeVersion": runtime_version,
        "status": normalize_status(&status),
        "createdAt": sandbox.created_at,
        "startedAt": sandbox.started_at,
        "stoppedAt": sandbox.stopped_at,
        "startupDurationMs": sandbox.boot_time_ms,
        "cpuAvg": 0.0,
        "memoryPeakBytes": 0,
        "labels": labels,
        "attributes": attributes,
    }));

    push_sandbox_metric(
        &mut output.metrics,
        timestamp,
        &config.node_id,
        &sandbox_id,
        "sandbox.runtime.boot_time_ms",
        sandbox.boot_time_ms,
        "ms",
        source,
    );
    push_sandbox_metric(
        &mut output.metrics,
        timestamp,
        &config.node_id,
        &sandbox_id,
        "sandbox.runtime.syscalls_total",
        sandbox.syscall_count.map(|value| value as f64),
        "calls",
        source,
    );
    push_sandbox_metric(
        &mut output.metrics,
        timestamp,
        &config.node_id,
        &sandbox_id,
        "sandbox.runtime.syscall_latency_ms",
        sandbox.syscall_latency_ms,
        "ms",
        source,
    );
    push_sandbox_metric(
        &mut output.metrics,
        timestamp,
        &config.node_id,
        &sandbox_id,
        "sandbox.runtime.gofer_io_bytes",
        sandbox.gofer_io_bytes.map(|value| value as f64),
        "bytes",
        source,
    );
    push_sandbox_metric(
        &mut output.metrics,
        timestamp,
        &config.node_id,
        &sandbox_id,
        "sandbox.runtime.faults_total",
        sandbox.faults.map(|value| value as f64),
        "faults",
        source,
    );

    output.events.push(sandbox_event(
        timestamp,
        &config.node_id,
        &sandbox_id,
        &status,
        source,
        sandbox.platform.as_deref(),
    ));

    if let Some(duration_ms) = sandbox.boot_time_ms.filter(|value| *value > 0.0) {
        output.traces.push(boot_span(
            timestamp,
            &sandbox_id,
            sandbox.platform.as_deref(),
            duration_ms,
            source,
        ));
    }
}

fn push_sandbox_metric(
    metrics: &mut Vec<MetricSample>,
    timestamp: &str,
    node_id: &str,
    sandbox_id: &str,
    name: &str,
    value: Option<f64>,
    unit: &str,
    source: &str,
) {
    let Some(value) = value else {
        return;
    };
    let mut sample = metric(
        timestamp, name, value, unit, "sandbox", node_id, sandbox_id, "gvisor",
    );
    sample.attributes = Some(Map::from_iter([
        ("collector.source".to_string(), json!(source)),
        ("collector.plugin".to_string(), json!("gvisor")),
    ]));
    metrics.push(sample);
}

fn sandbox_event(
    timestamp: &str,
    node_id: &str,
    sandbox_id: &str,
    status: &str,
    source: &str,
    platform: Option<&str>,
) -> EventRecord {
    let mut attributes = Map::new();
    attributes.insert("collector.plugin".to_string(), json!("gvisor"));
    attributes.insert("collector.source".to_string(), json!(source));
    attributes.insert("sandbox.status".to_string(), json!(status));
    if let Some(platform) = platform {
        attributes.insert("gvisor.platform".to_string(), json!(platform));
    }

    EventRecord {
        id: format!(
            "gvisor-{}-{}",
            sanitize_id(sandbox_id),
            sanitize_id(timestamp)
        ),
        timestamp: timestamp.to_string(),
        severity: if is_failure_status(status) {
            "warning"
        } else {
            "info"
        }
        .to_string(),
        event_type: "sandbox".to_string(),
        event_name: "gvisor.sandbox.observed".to_string(),
        message: format!("gVisor sandbox {sandbox_id} observed with status {status}."),
        source: format!("runtimepulse-rust-collector/{node_id}/gvisor"),
        attributes,
        sandbox_id: Some(sandbox_id.to_string()),
        image_id: None,
        node_id: Some(node_id.to_string()),
        runtime_type: Some("gvisor".to_string()),
        reason: Some(status.to_string()),
    }
}

fn boot_span(
    timestamp: &str,
    sandbox_id: &str,
    platform: Option<&str>,
    duration_ms: f64,
    source: &str,
) -> TraceSpan {
    let start_time =
        start_time_from_end(timestamp, duration_ms).unwrap_or_else(|| timestamp.to_string());
    let mut attributes = Map::new();
    attributes.insert("collector.plugin".to_string(), json!("gvisor"));
    attributes.insert("collector.source".to_string(), json!(source));
    if let Some(platform) = platform {
        attributes.insert("gvisor.platform".to_string(), json!(platform));
    }

    TraceSpan {
        trace_id: format!("gvisor-{}", sanitize_id(sandbox_id)),
        span_id: format!("gvisor-{}-boot", sanitize_id(sandbox_id)),
        span_name: "gvisor.sandbox.boot".to_string(),
        start_time,
        end_time: timestamp.to_string(),
        duration_ms,
        status: "ok".to_string(),
        attributes,
        sandbox_id: Some(sandbox_id.to_string()),
        image_id: None,
        parent_span_id: None,
    }
}

fn has_plugin_output_payload(output: &PluginOutput) -> bool {
    !output.metadata.clusters.is_empty()
        || !output.metadata.nodes.is_empty()
        || !output.metadata.images.is_empty()
        || !output.metadata.sandboxes.is_empty()
        || !output.metrics.is_empty()
        || !output.events.is_empty()
        || !output.traces.is_empty()
        || !output.profiles.is_empty()
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

fn normalize_status(status: &str) -> String {
    match status.to_ascii_lowercase().as_str() {
        "created" | "creating" => "starting".to_string(),
        "running" | "ready" => "running".to_string(),
        "stopped" | "exited" | "deleted" => "stopped".to_string(),
        "failed" | "error" | "crashed" => "failed".to_string(),
        other => other.to_string(),
    }
}

fn is_failure_status(status: &str) -> bool {
    matches!(
        status.to_ascii_lowercase().as_str(),
        "failed" | "error" | "crashed" | "oom" | "timeout"
    )
}

fn start_time_from_end(end_time: &str, duration_ms: f64) -> Option<String> {
    let end = DateTime::parse_from_rfc3339(end_time)
        .ok()?
        .with_timezone(&Utc);
    Some(timestamp(
        end - chrono::Duration::milliseconds(duration_ms.max(0.0).round() as i64),
    ))
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

fn env_path(name: &str) -> Option<PathBuf> {
    env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn test_config() -> CollectorConfig {
        CollectorConfig {
            ingest_url: "http://localhost:8081/api/ingest/batch".to_string(),
            node_id: "node-a".to_string(),
            cluster_id: "cluster-a".to_string(),
            interval: Duration::from_secs(5),
            local_report_addr: "127.0.0.1:9091".to_string(),
            local_report_url: "http://localhost:9091/api/local/ingest".to_string(),
            collection_scope: "host".to_string(),
            once: true,
            cgroup_root: PathBuf::from("/sys/fs/cgroup"),
            cgroup_max_entries: 100,
            image_cache_report_path: None,
            profile_report_path: None,
            diagnostic_report_path: None,
            diagnostic_report_command: None,
            diagnostic_report_command_timeout: Duration::from_secs(5),
            perf_report_path: None,
            perf_folded_path: None,
            ebpf_report_path: None,
            ebpf_folded_path: None,
            perf_profile_command: None,
            ebpf_profile_command: None,
            profile_command_timeout: Duration::from_secs(5),
            plugins: Vec::new(),
            command_plugins: Vec::new(),
            http_plugins: Vec::new(),
        }
    }

    #[test]
    fn parses_gvisor_report_into_runtimepulse_output() {
        let content = r#"{
            "timestamp":"2026-05-25T00:00:00.000Z",
            "sandboxes":[{
                "id":"gvisor-demo",
                "workloadName":"demo/gvisor",
                "namespace":"default",
                "imageRef":"registry.example/gvisor:v1",
                "status":"running",
                "runtimeVersion":"runsc-20260520",
                "platform":"ptrace",
                "sandboxPid":1234,
                "goferPid":1235,
                "sentryPid":1236,
                "bootTimeMs":222,
                "syscallCount":1200,
                "syscallLatencyMs":3.5,
                "goferIoBytes":4096,
                "faults":2
            }]
        }"#;

        let output = gvisor_output_from_content(content, Utc::now(), &test_config()).unwrap();

        assert_eq!(output.metadata.sandboxes.len(), 1);
        assert_eq!(output.metadata.sandboxes[0]["runtimeType"], "gvisor");
        assert!(output.metrics.iter().any(|metric| metric.name
            == "sandbox.runtime.syscalls_total"
            && metric.value == 1200.0));
        assert!(output
            .events
            .iter()
            .any(|event| event.event_name == "gvisor.sandbox.observed"));
        assert!(output
            .traces
            .iter()
            .any(|span| span.span_name == "gvisor.sandbox.boot"));
    }
}
