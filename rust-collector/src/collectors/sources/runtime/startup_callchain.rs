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

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StartupCallchainReport {
    id: Option<String>,
    #[serde(alias = "trace_id")]
    trace_id: Option<String>,
    #[serde(alias = "sandbox_id")]
    sandbox_id: String,
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
    #[serde(default, alias = "stages")]
    spans: Vec<StartupStageReport>,
}

#[derive(Debug, Deserialize)]
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

    let mut reports = Vec::new();
    for line in trimmed
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
    {
        reports.extend(parse_report_value(serde_json::from_str::<Value>(line)?)?);
    }
    Ok(reports)
}

fn parse_report_value(value: Value) -> Result<Vec<ParsedCallchainReport>> {
    if value.get("traces").is_some()
        || value.get("metrics").is_some()
        || value.get("events").is_some()
        || value.get("metadata").is_some()
    {
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

    if let Some(items) = value.as_array() {
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

fn output_from_lightweight_report(
    report: StartupCallchainReport,
    fallback_timestamp: &str,
    config: &CollectorConfig,
) -> PluginOutput {
    let trace_id = report
        .trace_id
        .clone()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| format!("startup-callchain-{}", sanitize_id(&report.sandbox_id)));
    let root_span_id = format!("{trace_id}-root");
    let status = report.status.as_deref().unwrap_or("ok").to_string();
    let runtime_type = report
        .runtime_type
        .clone()
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
    if let Some(handler) = &report.runtime_handler {
        base_attributes.insert("runtime.handler".to_string(), json!(handler));
    }
    if let Some(summary) = &report.summary {
        base_attributes.insert("startup.summary".to_string(), json!(summary));
    }

    let mut traces = Vec::new();
    if let Some(root_span) = root_span_from_report(
        &report,
        &trace_id,
        &root_span_id,
        &status,
        base_attributes.clone(),
    ) {
        traces.push(root_span);
    }

    let default_parent = if traces.is_empty() {
        None
    } else {
        Some(root_span_id.clone())
    };
    for stage in &report.spans {
        if let Some(span) = span_from_stage(
            stage,
            &trace_id,
            default_parent.as_deref(),
            &report.sandbox_id,
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
        &report.sandbox_id,
        &runtime_type,
        &base_attributes,
        config,
    );
    if let Some(duration_ms) = report.duration_ms {
        metrics.push(metric(
            &observed_at,
            "sandbox.startup.callchain_duration_ms",
            duration_ms,
            "ms",
            &report.sandbox_id,
            &runtime_type,
            &base_attributes,
            config,
        ));
    }

    PluginOutput {
        source: None,
        metadata: Metadata::default(),
        metrics,
        events: vec![EventRecord {
            id: format!(
                "startup-callchain-{}-{}",
                sanitize_id(report.id.as_deref().unwrap_or(&trace_id)),
                sanitize_id(&observed_at)
            ),
            timestamp: observed_at,
            severity: if status == "ok" { "info" } else { "warning" }.to_string(),
            event_type: "startup".to_string(),
            event_name: "startup.callchain.observed".to_string(),
            message: format!(
                "Startup call-chain report observed {} spans for {}.",
                traces.len(),
                report.sandbox_id
            ),
            source: format!(
                "runtimepulse-rust-collector/{}/startup-callchain",
                config.node_id
            ),
            attributes: base_attributes,
            sandbox_id: Some(report.sandbox_id),
            image_id: None,
            node_id: Some(config.node_id.clone()),
            runtime_type: Some(runtime_type),
            reason: None,
        }],
        traces,
        profiles: Vec::new(),
    }
}

fn root_span_from_report(
    report: &StartupCallchainReport,
    trace_id: &str,
    root_span_id: &str,
    status: &str,
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
        sandbox_id: Some(report.sandbox_id.clone()),
        image_id: None,
        parent_span_id: None,
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
    if let Some(plugin) = &stage.cni_plugin {
        attributes.insert("cni.plugin".to_string(), json!(plugin));
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
          "summary": {"cniDurationMs": 250, "binaryExecCount": 4},
          "spans": [
            {
              "spanName": "cri.run_pod_sandbox",
              "startTime": "2026-05-26T01:00:00.000Z",
              "durationMs": 1000
            },
            {
              "spanName": "cni.plugin.bridge",
              "startTime": "2026-05-26T01:00:00.100Z",
              "durationMs": 100,
              "binary": "/opt/cni/bin/bridge",
              "cniPlugin": "bridge",
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
            }
          ]
        }
        "#;

        let output = startup_callchain_output_from_content(content, Utc::now(), &config).unwrap();
        assert_eq!(output.traces.len(), 4);
        assert_eq!(output.traces[0].span_name, "sandbox.startup.callchain");
        assert!(output
            .traces
            .iter()
            .any(|span| span.span_name == "cni.plugin.bridge"
                && span.attributes["cni.plugin"] == json!("bridge")));
        assert!(output.metrics.iter().any(|metric| {
            metric.name == "sandbox.startup.cni_duration_ms" && metric.value == 250.0
        }));
        assert_eq!(output.events[0].event_name, "startup.callchain.observed");
        assert_eq!(output.metadata.nodes.len(), 1);
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
