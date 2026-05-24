//! Runtime diagnostic artifact report source.
//!
//! RuntimePulse does not collect large diagnostic bundles directly. This source
//! ingests small JSON/JSONL indexes emitted by host-side tools such as `crictl`,
//! `docker`, runtime-specific debug commands, or support-bundle generators. The
//! raw bundle stays in local/object storage and RuntimePulse records its URI,
//! summary issues, metrics, events, and optional capture spans.

use chrono::{DateTime, SecondsFormat, Utc};
use serde::Deserialize;
use serde_json::{json, Map, Value};
use std::collections::HashSet;
use std::fs;
#[cfg(unix)]
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use crate::collectors::core::config::CollectorConfig;
use crate::collectors::core::error::{CollectorError, Result};
use crate::collectors::core::model::{EventRecord, MetricSample, PluginOutput, TraceSpan};
use crate::collectors::core::plugin::CollectorPlugin;
use crate::collectors::core::report::{metric, node_metric};

#[derive(Default)]
pub struct DiagnosticReportPlugin {
    path: Option<PathBuf>,
    command: Option<String>,
    timeout: Duration,
    seen_event_ids: HashSet<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DiagnosticReport {
    id: Option<String>,
    timestamp: Option<String>,
    sandbox_id: Option<String>,
    node_id: Option<String>,
    runtime_type: Option<String>,
    source: Option<String>,
    severity: Option<String>,
    reason: Option<String>,
    status: Option<String>,
    message: Option<String>,
    object_uri: String,
    size_bytes: Option<u64>,
    duration_ms: Option<f64>,
    artifact_type: Option<String>,
    target: Option<DiagnosticTarget>,
    summary: Option<DiagnosticSummary>,
    #[serde(default)]
    issues: Vec<DiagnosticIssue>,
    #[serde(default)]
    artifacts: Vec<DiagnosticArtifact>,
    labels: Option<Map<String, Value>>,
    attributes: Option<Map<String, Value>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DiagnosticTarget {
    node_id: Option<String>,
    sandbox_id: Option<String>,
    image_id: Option<String>,
    runtime_type: Option<String>,
    workload_name: Option<String>,
    namespace: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DiagnosticSummary {
    files: Option<u64>,
    logs: Option<u64>,
    warnings: Option<u64>,
    errors: Option<u64>,
    checks: Option<u64>,
    failed_checks: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DiagnosticIssue {
    id: Option<String>,
    severity: Option<String>,
    category: Option<String>,
    title: Option<String>,
    message: Option<String>,
    count: Option<u64>,
    attributes: Option<Map<String, Value>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DiagnosticArtifact {
    name: Option<String>,
    artifact_type: Option<String>,
    object_uri: Option<String>,
    size_bytes: Option<u64>,
}

impl DiagnosticReportPlugin {
    pub fn new(path: Option<PathBuf>, command: Option<String>, timeout: Duration) -> Self {
        Self {
            path,
            command,
            timeout,
            seen_event_ids: HashSet::new(),
        }
    }
}

impl CollectorPlugin for DiagnosticReportPlugin {
    fn name(&self) -> &str {
        "diagnostic-report"
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
                merge_output(
                    &mut output,
                    output_from_diagnostic_content(&content, now, config)?,
                );
            }
        }

        if let Some(command) = self.command.clone() {
            merge_output(
                &mut output,
                output_from_diagnostic_command(&command, self.timeout, now, config)?,
            );
        }

        output
            .events
            .retain(|event| self.seen_event_ids.insert(event.id.clone()));
        Ok(output)
    }
}

pub fn output_from_diagnostic_command(
    command: &str,
    timeout: Duration,
    now: DateTime<Utc>,
    config: &CollectorConfig,
) -> Result<PluginOutput> {
    let content = run_diagnostic_command(command, timeout)?;
    if content.trim().is_empty() {
        return Ok(PluginOutput::default());
    }
    output_from_diagnostic_content(&content, now, config)
}

fn merge_output(target: &mut PluginOutput, output: PluginOutput) {
    target.metadata.clusters.extend(output.metadata.clusters);
    target.metadata.nodes.extend(output.metadata.nodes);
    target.metadata.images.extend(output.metadata.images);
    target.metadata.sandboxes.extend(output.metadata.sandboxes);
    target.metrics.extend(output.metrics);
    target.events.extend(output.events);
    target.traces.extend(output.traces);
    target.profiles.extend(output.profiles);
}

pub fn output_from_diagnostic_content(
    content: &str,
    now: DateTime<Utc>,
    config: &CollectorConfig,
) -> Result<PluginOutput> {
    let reports = parse_reports(content)?;
    Ok(output_from_reports(reports, now, config))
}

fn output_from_reports(
    reports: Vec<DiagnosticReport>,
    now: DateTime<Utc>,
    config: &CollectorConfig,
) -> PluginOutput {
    let fallback_timestamp = timestamp(now);
    let mut output = PluginOutput::default();

    for (index, report) in reports.into_iter().enumerate() {
        let timestamp = report
            .timestamp
            .clone()
            .unwrap_or_else(|| fallback_timestamp.clone());
        let source = report
            .source
            .clone()
            .unwrap_or_else(|| "diagnostic-report".to_string());
        let target = report.target.as_ref();
        let sandbox_id = report
            .sandbox_id
            .clone()
            .or_else(|| target.and_then(|target| target.sandbox_id.clone()));
        let node_id = report
            .node_id
            .clone()
            .or_else(|| target.and_then(|target| target.node_id.clone()))
            .unwrap_or_else(|| config.node_id.clone());
        let runtime_type = report
            .runtime_type
            .clone()
            .or_else(|| target.and_then(|target| target.runtime_type.clone()))
            .unwrap_or_else(|| "unknown".to_string());
        let artifact_type = report
            .artifact_type
            .clone()
            .unwrap_or_else(|| "diagnostic_bundle".to_string());
        let report_id = report.id.clone().unwrap_or_else(|| {
            format!(
                "diagnostic-{}-{}-{}",
                sandbox_id
                    .as_deref()
                    .map(sanitize_id)
                    .unwrap_or_else(|| sanitize_id(&node_id)),
                sanitize_id(&artifact_type),
                index
            )
        });
        let severity = normalize_severity(report.severity.as_deref(), report.summary.as_ref());
        let message = report.message.clone().unwrap_or_else(|| {
            format!(
                "{} captured {} for {}.",
                source,
                artifact_type,
                sandbox_id.as_deref().unwrap_or(&node_id)
            )
        });
        let attributes = diagnostic_attributes(&report, &report_id, &artifact_type, &source);

        if let Some(sandbox_id) = &sandbox_id {
            output.metadata.sandboxes.push(json!({
                "id": sandbox_id,
                "nodeId": node_id,
                "runtimeType": runtime_type,
                "runtimeVersion": source,
                "imageRef": target.and_then(|target| target.image_id.as_deref()).unwrap_or("unknown"),
                "workloadName": target.and_then(|target| target.workload_name.as_deref()).unwrap_or(sandbox_id),
                "status": "observed",
                "labels": labels_value(report.labels.as_ref()),
                "attributes": {
                    "collector.plugin": "diagnostic-report",
                    "diagnostic.id": report_id,
                    "diagnostic.artifactType": artifact_type,
                    "diagnostic.objectUri": report.object_uri,
                    "diagnostic.status": report.status.clone().unwrap_or_else(|| "captured".to_string()),
                    "diagnostic.reason": report.reason.clone().unwrap_or_else(|| "diagnostic_report".to_string()),
                    "diagnostic.source": source,
                    "k8s.namespace": target.and_then(|target| target.namespace.clone()),
                }
            }));
        }

        output.metrics.extend(diagnostic_metrics(
            &timestamp,
            &node_id,
            sandbox_id.as_deref(),
            &runtime_type,
            report.size_bytes,
            report.duration_ms,
            report.summary.as_ref(),
            report.issues.len(),
        ));

        let event = EventRecord {
            id: format!("{report_id}-captured"),
            timestamp: timestamp.clone(),
            severity: severity.clone(),
            event_type: "diagnostic".to_string(),
            event_name: format!(
                "diagnostic.{}.captured",
                sanitize_metric_part(&artifact_type)
            ),
            message,
            source: format!("runtimepulse-rust-collector/{source}"),
            attributes: attributes.clone(),
            sandbox_id: sandbox_id.clone(),
            image_id: target.and_then(|target| target.image_id.clone()),
            node_id: Some(node_id.clone()),
            runtime_type: Some(runtime_type.clone()),
            reason: report.reason.clone(),
        };
        output.events.push(event);

        for (issue_index, issue) in report.issues.iter().enumerate() {
            output.events.push(issue_event(
                &report_id,
                issue_index,
                &timestamp,
                &source,
                &node_id,
                sandbox_id.as_deref(),
                &runtime_type,
                issue,
            ));
        }

        if let Some(duration_ms) = report.duration_ms {
            output.traces.push(diagnostic_span(
                &report_id,
                &timestamp,
                duration_ms,
                &source,
                &artifact_type,
                sandbox_id.as_deref(),
                target.and_then(|target| target.image_id.as_deref()),
                &attributes,
            ));
        }
    }

    if has_payload(&output) {
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
                "plugin": "diagnostic-report",
                "scope": config.collection_scope,
            }
        }));
    }

    output
}

fn parse_reports(content: &str) -> Result<Vec<DiagnosticReport>> {
    let trimmed = content.trim();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }
    if trimmed.starts_with('[') {
        return Ok(serde_json::from_str(trimmed)?);
    }
    if trimmed.starts_with('{') {
        return Ok(vec![serde_json::from_str(trimmed)?]);
    }

    let mut reports = Vec::new();
    for line in trimmed
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
    {
        reports.push(serde_json::from_str(line)?);
    }
    Ok(reports)
}

fn diagnostic_attributes(
    report: &DiagnosticReport,
    report_id: &str,
    artifact_type: &str,
    source: &str,
) -> Map<String, Value> {
    let mut attributes = Map::new();
    attributes.insert("diagnostic.id".to_string(), json!(report_id));
    attributes.insert("diagnostic.source".to_string(), json!(source));
    attributes.insert("diagnostic.artifactType".to_string(), json!(artifact_type));
    attributes.insert("diagnostic.objectUri".to_string(), json!(report.object_uri));
    if let Some(size_bytes) = report.size_bytes {
        attributes.insert("diagnostic.sizeBytes".to_string(), json!(size_bytes));
    }
    if let Some(duration_ms) = report.duration_ms {
        attributes.insert("diagnostic.durationMs".to_string(), json!(duration_ms));
    }
    if let Some(status) = &report.status {
        attributes.insert("diagnostic.status".to_string(), json!(status));
    }
    if let Some(reason) = &report.reason {
        attributes.insert("diagnostic.reason".to_string(), json!(reason));
    }
    if let Some(summary) = &report.summary {
        insert_optional_u64(&mut attributes, "diagnostic.files", summary.files);
        insert_optional_u64(&mut attributes, "diagnostic.logs", summary.logs);
        insert_optional_u64(&mut attributes, "diagnostic.warnings", summary.warnings);
        insert_optional_u64(&mut attributes, "diagnostic.errors", summary.errors);
        insert_optional_u64(&mut attributes, "diagnostic.checks", summary.checks);
        insert_optional_u64(
            &mut attributes,
            "diagnostic.failedChecks",
            summary.failed_checks,
        );
    }
    if !report.artifacts.is_empty() {
        attributes.insert(
            "diagnostic.artifacts".to_string(),
            Value::Array(
                report
                    .artifacts
                    .iter()
                    .map(|artifact| {
                        json!({
                            "name": artifact.name,
                            "artifactType": artifact.artifact_type,
                            "objectUri": artifact.object_uri,
                            "sizeBytes": artifact.size_bytes,
                        })
                    })
                    .collect(),
            ),
        );
    }
    if let Some(labels) = &report.labels {
        for (key, value) in labels {
            attributes.insert(format!("label.{key}"), value.clone());
        }
    }
    if let Some(extra) = &report.attributes {
        for (key, value) in extra {
            attributes.insert(key.clone(), value.clone());
        }
    }
    attributes
}

fn diagnostic_metrics(
    timestamp: &str,
    node_id: &str,
    sandbox_id: Option<&str>,
    runtime_type: &str,
    size_bytes: Option<u64>,
    duration_ms: Option<f64>,
    summary: Option<&DiagnosticSummary>,
    issue_count: usize,
) -> Vec<MetricSample> {
    let mut metrics = Vec::new();
    push_diagnostic_metric(
        &mut metrics,
        timestamp,
        node_id,
        sandbox_id,
        runtime_type,
        "diagnostic.artifact_size_bytes",
        size_bytes.map(|value| value as f64),
        "bytes",
        "diagnostic",
    );
    push_diagnostic_metric(
        &mut metrics,
        timestamp,
        node_id,
        sandbox_id,
        runtime_type,
        "diagnostic.capture_duration_ms",
        duration_ms,
        "ms",
        "diagnostic",
    );
    push_diagnostic_metric(
        &mut metrics,
        timestamp,
        node_id,
        sandbox_id,
        runtime_type,
        "diagnostic.issues_total",
        Some(issue_count as f64),
        "issues",
        "diagnostic",
    );
    if let Some(summary) = summary {
        push_diagnostic_metric(
            &mut metrics,
            timestamp,
            node_id,
            sandbox_id,
            runtime_type,
            "diagnostic.files_total",
            summary.files.map(|value| value as f64),
            "files",
            "diagnostic",
        );
        push_diagnostic_metric(
            &mut metrics,
            timestamp,
            node_id,
            sandbox_id,
            runtime_type,
            "diagnostic.logs_total",
            summary.logs.map(|value| value as f64),
            "logs",
            "diagnostic",
        );
        push_diagnostic_metric(
            &mut metrics,
            timestamp,
            node_id,
            sandbox_id,
            runtime_type,
            "diagnostic.warnings_total",
            summary.warnings.map(|value| value as f64),
            "warnings",
            "diagnostic",
        );
        push_diagnostic_metric(
            &mut metrics,
            timestamp,
            node_id,
            sandbox_id,
            runtime_type,
            "diagnostic.errors_total",
            summary.errors.map(|value| value as f64),
            "errors",
            "diagnostic",
        );
        push_diagnostic_metric(
            &mut metrics,
            timestamp,
            node_id,
            sandbox_id,
            runtime_type,
            "diagnostic.failed_checks_total",
            summary.failed_checks.map(|value| value as f64),
            "checks",
            "diagnostic",
        );
    }
    metrics
}

fn push_diagnostic_metric(
    metrics: &mut Vec<MetricSample>,
    timestamp: &str,
    node_id: &str,
    sandbox_id: Option<&str>,
    runtime_type: &str,
    name: &str,
    value: Option<f64>,
    unit: &str,
    group: &str,
) {
    let Some(value) = value else {
        return;
    };
    let mut sample = if let Some(sandbox_id) = sandbox_id {
        metric(
            timestamp,
            name,
            value,
            unit,
            group,
            node_id,
            sandbox_id,
            runtime_type,
        )
    } else {
        node_metric(timestamp, name, value, unit, group, node_id)
    };
    sample.attributes = Some(Map::from_iter([(
        "collector.source".to_string(),
        json!("diagnostic-report"),
    )]));
    metrics.push(sample);
}

fn issue_event(
    report_id: &str,
    index: usize,
    timestamp: &str,
    source: &str,
    node_id: &str,
    sandbox_id: Option<&str>,
    runtime_type: &str,
    issue: &DiagnosticIssue,
) -> EventRecord {
    let severity = issue
        .severity
        .clone()
        .unwrap_or_else(|| "warning".to_string());
    let category = issue
        .category
        .clone()
        .unwrap_or_else(|| "runtime".to_string());
    let issue_id = issue
        .id
        .clone()
        .unwrap_or_else(|| format!("{report_id}-issue-{index}"));
    let mut attributes = issue.attributes.clone().unwrap_or_default();
    attributes.insert("diagnostic.id".to_string(), json!(report_id));
    attributes.insert("diagnostic.issueId".to_string(), json!(issue_id));
    attributes.insert("diagnostic.category".to_string(), json!(category));
    attributes.insert(
        "diagnostic.issueCount".to_string(),
        json!(issue.count.unwrap_or(1)),
    );

    EventRecord {
        id: issue_id,
        timestamp: timestamp.to_string(),
        severity: normalize_issue_severity(&severity),
        event_type: "diagnostic".to_string(),
        event_name: format!("diagnostic.issue.{}", sanitize_metric_part(&category)),
        message: issue
            .message
            .clone()
            .or_else(|| issue.title.clone())
            .unwrap_or_else(|| format!("Diagnostic issue reported in {category}.")),
        source: format!("runtimepulse-rust-collector/{source}"),
        attributes,
        sandbox_id: sandbox_id.map(ToOwned::to_owned),
        image_id: None,
        node_id: Some(node_id.to_string()),
        runtime_type: Some(runtime_type.to_string()),
        reason: Some(category),
    }
}

fn diagnostic_span(
    report_id: &str,
    timestamp: &str,
    duration_ms: f64,
    source: &str,
    artifact_type: &str,
    sandbox_id: Option<&str>,
    image_id: Option<&str>,
    attributes: &Map<String, Value>,
) -> TraceSpan {
    let end = end_time(timestamp, duration_ms).unwrap_or_else(|| timestamp.to_string());
    let span_id = format!("{report_id}-capture");
    let mut span_attributes = attributes.clone();
    span_attributes.insert("plugin".to_string(), json!("diagnostic-report"));
    span_attributes.insert("diagnostic.source".to_string(), json!(source));

    TraceSpan {
        trace_id: format!("diagnostic-{report_id}"),
        span_id,
        span_name: format!("diagnostic.{}.capture", sanitize_metric_part(artifact_type)),
        start_time: timestamp.to_string(),
        end_time: end,
        duration_ms,
        status: "ok".to_string(),
        attributes: span_attributes,
        sandbox_id: sandbox_id.map(ToOwned::to_owned),
        image_id: image_id.map(ToOwned::to_owned),
        parent_span_id: None,
    }
}

fn run_diagnostic_command(command: &str, timeout: Duration) -> Result<String> {
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
                    plugin: "diagnostic-report".to_string(),
                    message: format!(
                        "diagnostic command exited with status {:?}: {}",
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
                plugin: "diagnostic-report".to_string(),
                message: format!(
                    "diagnostic command timed out after {} ms",
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

fn labels_value(labels: Option<&Map<String, Value>>) -> Value {
    Value::Object(labels.cloned().unwrap_or_default())
}

fn normalize_severity(severity: Option<&str>, summary: Option<&DiagnosticSummary>) -> String {
    if let Some(severity) = severity {
        return normalize_issue_severity(severity);
    }
    if summary.and_then(|summary| summary.errors).unwrap_or(0) > 0 {
        "error".to_string()
    } else if summary.and_then(|summary| summary.warnings).unwrap_or(0) > 0 {
        "warning".to_string()
    } else {
        "info".to_string()
    }
}

fn normalize_issue_severity(severity: &str) -> String {
    match severity.to_ascii_lowercase().as_str() {
        "critical" | "error" | "warning" | "info" | "debug" => severity.to_ascii_lowercase(),
        _ => "warning".to_string(),
    }
}

fn insert_optional_u64(attributes: &mut Map<String, Value>, key: &str, value: Option<u64>) {
    if let Some(value) = value {
        attributes.insert(key.to_string(), json!(value));
    }
}

fn has_payload(output: &PluginOutput) -> bool {
    !output.metadata.sandboxes.is_empty()
        || !output.metrics.is_empty()
        || !output.events.is_empty()
        || !output.traces.is_empty()
}

fn end_time(start_time: &str, duration_ms: f64) -> Option<String> {
    let start = DateTime::parse_from_rfc3339(start_time)
        .ok()?
        .with_timezone(&Utc);
    Some(timestamp(
        start + chrono::Duration::milliseconds(duration_ms.max(0.0).round() as i64),
    ))
}

fn sanitize_metric_part(value: &str) -> String {
    let sanitized = sanitize_id(value).replace('-', "_");
    if sanitized.is_empty() {
        "unknown".to_string()
    } else {
        sanitized
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn test_config() -> CollectorConfig {
        CollectorConfig {
            ingest_url: "http://query/api/ingest/batch".to_string(),
            node_id: "node-a".to_string(),
            cluster_id: "cluster-a".to_string(),
            interval: Duration::from_secs(1),
            local_report_addr: "127.0.0.1:9091".to_string(),
            local_report_url: "http://127.0.0.1:9091/api/local/ingest".to_string(),
            collection_scope: "host".to_string(),
            once: true,
            cgroup_root: PathBuf::from("/sys/fs/cgroup"),
            cgroup_max_entries: 1,
            image_cache_report_path: None,
            profile_report_path: None,
            diagnostic_report_path: None,
            diagnostic_report_command: None,
            diagnostic_report_command_timeout: Duration::from_secs(1),
            perf_report_path: None,
            ebpf_report_path: None,
            perf_profile_command: None,
            ebpf_profile_command: None,
            profile_command_timeout: Duration::from_secs(1),
            plugins: Vec::new(),
            command_plugins: Vec::new(),
            http_plugins: Vec::new(),
        }
    }

    #[test]
    fn diagnostic_plugin_collects_command_output() {
        let mut plugin = DiagnosticReportPlugin::new(
            None,
            Some("printf '%s' '{\"id\":\"diag-command\",\"timestamp\":\"2026-05-23T00:00:00Z\",\"sandboxId\":\"docker-runtimepulse-demo\",\"runtimeType\":\"runc\",\"source\":\"docker-debug\",\"objectUri\":\"file:///tmp/diag.tgz\",\"sizeBytes\":512,\"durationMs\":50,\"summary\":{\"warnings\":0,\"errors\":0},\"issues\":[]}'".to_string()),
            Duration::from_secs(1),
        );

        let output = plugin
            .collect(
                DateTime::parse_from_rfc3339("2026-05-23T00:00:00Z")
                    .unwrap()
                    .with_timezone(&Utc),
                &test_config(),
            )
            .unwrap();

        assert_eq!(output.events.len(), 1);
        assert_eq!(output.events[0].id, "diag-command-captured");
        assert_eq!(output.metrics.len(), 5);
        assert_eq!(output.traces.len(), 1);
    }

    #[test]
    fn parses_diagnostic_report_into_events_metrics_and_trace() {
        let output = output_from_diagnostic_content(
            r#"{
              "id":"diag-1",
              "timestamp":"2026-05-23T00:00:00Z",
              "sandboxId":"docker-runtimepulse-demo",
              "runtimeType":"runc",
              "source":"docker-debug",
              "objectUri":"file:///var/lib/runtimepulse/diagnostics/docker-runtimepulse-demo/bundle.tar.zst",
              "sizeBytes":4096,
              "durationMs":250,
              "summary":{"files":12,"logs":3,"warnings":1,"errors":0,"checks":5,"failedChecks":1},
              "issues":[{"severity":"warning","category":"io","message":"High log write latency","count":2}],
              "labels":{"runtime":"runc"}
            }"#,
            DateTime::parse_from_rfc3339("2026-05-23T00:00:00Z")
                .unwrap()
                .with_timezone(&Utc),
            &test_config(),
        )
        .unwrap();

        assert_eq!(output.events.len(), 2);
        assert_eq!(
            output.events[0].event_name,
            "diagnostic.diagnostic_bundle.captured"
        );
        assert_eq!(output.events[0].severity, "warning");
        assert_eq!(output.metrics.len(), 8);
        assert_eq!(output.traces.len(), 1);
        assert_eq!(
            output.metadata.sandboxes[0]["id"],
            "docker-runtimepulse-demo"
        );
        assert_eq!(
            output.metadata.nodes[0]["labels"]["plugin"],
            "diagnostic-report"
        );
    }
}
