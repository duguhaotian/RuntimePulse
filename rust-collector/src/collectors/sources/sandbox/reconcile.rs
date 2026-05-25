//! Generic sandbox snapshot reconciliation source.
//!
//! Reads JSON/JSONL snapshot reports from real runtime-side exporters and emits
//! node snapshot metadata. The Query API uses `snapshot.scope` plus
//! `snapshot.sandboxIds` to remove stale live sandboxes for that runtime/node.
//! This source does not fabricate runtime state; without a configured report it
//! emits an empty output.

use chrono::{DateTime, SecondsFormat, Utc};
use serde::Deserialize;
use serde_json::json;
use std::env;
use std::fs;
use std::path::PathBuf;

use crate::collectors::core::config::CollectorConfig;
use crate::collectors::core::error::Result;
use crate::collectors::core::model::{EventRecord, Metadata, PluginOutput};
use crate::collectors::core::plugin::CollectorPlugin;

pub struct SandboxReconcilePlugin {
    path: Option<PathBuf>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SandboxSnapshotReport {
    timestamp: Option<String>,
    source: Option<String>,
    runtime_type: Option<String>,
    scope: Option<String>,
    node_id: Option<String>,
    reason: Option<String>,
    #[serde(default)]
    sandbox_ids: Vec<String>,
    #[serde(default)]
    sandboxes: Vec<SandboxSnapshotRow>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SandboxSnapshotRow {
    id: Option<String>,
    sandbox_id: Option<String>,
    runtime_type: Option<String>,
    node_id: Option<String>,
}

enum ParsedSandboxSnapshot {
    RuntimePulse(PluginOutput),
    Lightweight(SandboxSnapshotReport),
}

impl SandboxReconcilePlugin {
    pub fn new(path: Option<PathBuf>) -> Self {
        Self { path }
    }

    pub fn from_env() -> Self {
        Self::new(env_path("RUNTIMEPULSE_SANDBOX_RECONCILE_REPORT_PATH"))
    }
}

impl CollectorPlugin for SandboxReconcilePlugin {
    fn name(&self) -> &str {
        "sandbox-reconcile"
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

        sandbox_reconcile_output_from_content(&content, now, config)
    }
}

pub fn sandbox_reconcile_output_from_content(
    content: &str,
    now: DateTime<Utc>,
    config: &CollectorConfig,
) -> Result<PluginOutput> {
    let fallback_timestamp = timestamp(now);
    let mut output = PluginOutput::default();

    for report in parse_reports(content)? {
        match report {
            ParsedSandboxSnapshot::RuntimePulse(runtimepulse_output) => {
                merge_plugin_output(&mut output, runtimepulse_output);
            }
            ParsedSandboxSnapshot::Lightweight(report) => {
                merge_plugin_output(
                    &mut output,
                    output_from_lightweight_report(report, &fallback_timestamp, config),
                );
            }
        }
    }

    Ok(output)
}

fn parse_reports(content: &str) -> Result<Vec<ParsedSandboxSnapshot>> {
    let trimmed = content.trim();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }

    if trimmed.starts_with('[') {
        return Ok(serde_json::from_str::<Vec<SandboxSnapshotReport>>(trimmed)?
            .into_iter()
            .map(ParsedSandboxSnapshot::Lightweight)
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

fn parse_single(value: &str) -> Result<ParsedSandboxSnapshot> {
    if let Ok(output) = serde_json::from_str::<PluginOutput>(value) {
        if has_plugin_output_payload(&output) {
            return Ok(ParsedSandboxSnapshot::RuntimePulse(output));
        }
    }
    Ok(ParsedSandboxSnapshot::Lightweight(serde_json::from_str(
        value,
    )?))
}

fn output_from_lightweight_report(
    report: SandboxSnapshotReport,
    fallback_timestamp: &str,
    config: &CollectorConfig,
) -> PluginOutput {
    let report_timestamp = report
        .timestamp
        .unwrap_or_else(|| fallback_timestamp.to_string());
    let node_id = report.node_id.unwrap_or_else(|| config.node_id.clone());
    let runtime_type = report
        .runtime_type
        .or_else(|| {
            report
                .sandboxes
                .iter()
                .find_map(|row| row.runtime_type.clone())
        })
        .unwrap_or_else(|| "sandbox".to_string());
    let scope = report
        .scope
        .unwrap_or_else(|| format!("{}-running", sanitize_id(&runtime_type)));
    let source = report
        .source
        .unwrap_or_else(|| "sandbox-reconcile".to_string());
    let mut sandbox_ids = report.sandbox_ids;
    sandbox_ids.extend(report.sandboxes.into_iter().filter_map(|row| {
        let row_node_id = row.node_id.as_deref().unwrap_or(&node_id);
        if row_node_id != node_id {
            return None;
        }
        row.sandbox_id.or(row.id)
    }));
    sandbox_ids.sort();
    sandbox_ids.dedup();

    let event = snapshot_event(
        &report_timestamp,
        &node_id,
        &scope,
        &runtime_type,
        &sandbox_ids,
        report.reason.as_deref(),
    );

    PluginOutput {
        source: Some(source),
        metadata: Metadata {
            clusters: vec![json!({
                "id": config.cluster_id,
                "name": config.cluster_id,
                "environment": "collector"
            })],
            nodes: vec![json!({
                "id": node_id,
                "clusterId": config.cluster_id,
                "name": node_id,
                "status": "ready",
                "labels": {
                    "collector": "runtimepulse-rust-collector",
                    "plugin": "sandbox-reconcile",
                    "scope": config.collection_scope,
                    "runtime": runtime_type,
                },
                "attributes": {
                    "plugin": "sandbox-reconcile",
                    "scope": config.collection_scope,
                    "runtime.type": runtime_type,
                    "snapshot.scope": scope,
                    "snapshot.nodeId": node_id,
                    "snapshot.sandboxIds": sandbox_ids,
                    "snapshot.sandboxCount": sandbox_ids.len(),
                }
            })],
            images: Vec::new(),
            sandboxes: Vec::new(),
        },
        metrics: Vec::new(),
        events: vec![event],
        traces: Vec::new(),
        profiles: Vec::new(),
    }
}

fn snapshot_event(
    timestamp: &str,
    node_id: &str,
    scope: &str,
    runtime_type: &str,
    sandbox_ids: &[String],
    reason: Option<&str>,
) -> EventRecord {
    EventRecord {
        id: format!(
            "sandbox-reconcile-{}-{}-{}",
            sanitize_id(node_id),
            sanitize_id(scope),
            sanitize_id(timestamp)
        ),
        timestamp: timestamp.to_string(),
        severity: "info".to_string(),
        event_type: "collector".to_string(),
        event_name: "sandbox.snapshot.reconciled".to_string(),
        message: format!(
            "Reconciled {} live sandbox ids for snapshot scope {}.",
            sandbox_ids.len(),
            scope
        ),
        source: format!("runtimepulse-rust-collector/{node_id}/sandbox-reconcile"),
        attributes: serde_json::Map::from_iter([
            ("collector.plugin".to_string(), json!("sandbox-reconcile")),
            ("runtime.type".to_string(), json!(runtime_type)),
            ("snapshot.scope".to_string(), json!(scope)),
            ("snapshot.nodeId".to_string(), json!(node_id)),
            ("snapshot.sandboxIds".to_string(), json!(sandbox_ids)),
            (
                "snapshot.sandboxCount".to_string(),
                json!(sandbox_ids.len()),
            ),
        ]),
        sandbox_id: None,
        image_id: None,
        node_id: Some(node_id.to_string()),
        runtime_type: Some(runtime_type.to_string()),
        reason: reason.map(ToOwned::to_owned),
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
            image_cache_report_command: None,
            image_cache_report_command_timeout: Duration::from_secs(5),
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
    fn emits_snapshot_metadata_from_lightweight_report() {
        let content = r#"{
            "timestamp":"2026-05-25T00:00:00.000Z",
            "runtimeType":"kata",
            "scope":"kata-running",
            "sandboxIds":["kata-live"],
            "sandboxes":[{"id":"kata-extra"}]
        }"#;

        let output =
            sandbox_reconcile_output_from_content(content, Utc::now(), &test_config()).unwrap();

        assert_eq!(output.metadata.nodes.len(), 1);
        let attrs = output.metadata.nodes[0]["attributes"].as_object().unwrap();
        assert_eq!(attrs["snapshot.scope"], "kata-running");
        assert_eq!(attrs["snapshot.sandboxCount"], 2);
        assert!(output
            .events
            .iter()
            .any(|event| event.event_name == "sandbox.snapshot.reconciled"));
    }

    #[test]
    fn accepts_runtimepulse_plugin_output() {
        let content = r#"{
            "metadata": {
                "nodes": [{
                    "id": "node-a",
                    "attributes": {
                        "snapshot.scope": "firecracker-running",
                        "snapshot.nodeId": "node-a",
                        "snapshot.sandboxIds": []
                    }
                }]
            }
        }"#;

        let output =
            sandbox_reconcile_output_from_content(content, Utc::now(), &test_config()).unwrap();
        assert_eq!(output.metadata.nodes.len(), 1);
    }
}
