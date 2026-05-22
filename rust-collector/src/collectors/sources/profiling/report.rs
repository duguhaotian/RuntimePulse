//! Generic profile artifact report source.
//!
//! Profiling tools differ a lot: perf may write local files, eBPF agents may
//! expose JSON over another process, and third-party profilers may only know the
//! target sandbox. This source reads a small RuntimePulse-compatible JSON/JSONL
//! file and normalizes it into profile artifacts for the shared outlet path.

use chrono::{DateTime, SecondsFormat, Utc};
use serde::Deserialize;
use serde_json::Value;
use std::fs;
use std::path::PathBuf;

use crate::collectors::core::config::CollectorConfig;
use crate::collectors::core::error::Result;
use crate::collectors::core::model::{Metadata, PluginOutput, ProfileArtifact};
use crate::collectors::core::plugin::CollectorPlugin;

pub struct ProfileReportPlugin {
    path: Option<PathBuf>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProfileReport {
    sandbox_id: Option<String>,
    timestamp: Option<String>,
    source: Option<String>,
    profiles: Vec<ProfileArtifactReport>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProfileArtifactReport {
    id: Option<String>,
    timestamp: Option<String>,
    sandbox_id: Option<String>,
    profile_type: String,
    process_role: Option<String>,
    duration_ms: Option<f64>,
    sample_count: Option<u64>,
    object_uri: String,
    flamegraph: Option<Value>,
}

enum ParsedProfileReport {
    RuntimePulse(PluginOutput),
    Lightweight(ProfileReport),
}

impl ProfileReportPlugin {
    pub fn new(path: Option<PathBuf>) -> Self {
        Self { path }
    }
}

impl CollectorPlugin for ProfileReportPlugin {
    fn name(&self) -> &str {
        "profile-report"
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

        profile_output_from_content(&content, now, config)
    }
}

pub fn profile_output_from_content(
    content: &str,
    now: DateTime<Utc>,
    config: &CollectorConfig,
) -> Result<PluginOutput> {
    profile_output_from_content_with_plugin(content, now, config, "profile-report")
}

pub fn profile_output_from_content_with_plugin(
    content: &str,
    now: DateTime<Utc>,
    config: &CollectorConfig,
    plugin_name: &str,
) -> Result<PluginOutput> {
    let fallback_timestamp = timestamp(now);
    let mut output = PluginOutput::default();

    for report in parse_profile_reports(content)? {
        match report {
            ParsedProfileReport::RuntimePulse(runtimepulse_output) => {
                merge_plugin_output(&mut output, runtimepulse_output);
            }
            ParsedProfileReport::Lightweight(report) => {
                merge_plugin_output(&mut output, output_from_lightweight_report(report, &fallback_timestamp));
            }
        }
    }

    if !output.profiles.is_empty() && output.metadata.nodes.is_empty() {
        output.metadata.nodes.push(serde_json::json!({
            "id": config.node_id,
            "clusterId": config.cluster_id,
            "name": config.node_id,
            "status": "ready",
            "labels": {
                "collector": "runtimepulse-rust-collector",
                "plugin": plugin_name,
                "scope": config.collection_scope,
            }
        }));
    }
    if !output.profiles.is_empty() && output.metadata.clusters.is_empty() {
        output.metadata.clusters.push(serde_json::json!({
            "id": config.cluster_id,
            "name": config.cluster_id,
            "environment": "collector"
        }));
    }

    Ok(output)
}

fn parse_profile_reports(content: &str) -> Result<Vec<ParsedProfileReport>> {
    let trimmed = content.trim();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }

    if trimmed.starts_with('[') {
        return Ok(serde_json::from_str::<Vec<ProfileReport>>(trimmed)?
            .into_iter()
            .map(ParsedProfileReport::Lightweight)
            .collect());
    }

    if trimmed.starts_with('{') {
        if let Ok(output) = serde_json::from_str::<PluginOutput>(trimmed) {
            if has_plugin_output_payload(&output) {
                return Ok(vec![ParsedProfileReport::RuntimePulse(output)]);
            }
        }
        if let Ok(report) = serde_json::from_str::<ProfileReport>(trimmed) {
            return Ok(vec![ParsedProfileReport::Lightweight(report)]);
        }
    }

    let mut reports = Vec::new();
    for line in trimmed
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
    {
        if let Ok(output) = serde_json::from_str::<PluginOutput>(line) {
            if has_plugin_output_payload(&output) {
                reports.push(ParsedProfileReport::RuntimePulse(output));
                continue;
            }
        }
        reports.push(ParsedProfileReport::Lightweight(serde_json::from_str(
            line,
        )?));
    }

    Ok(reports)
}

fn output_from_lightweight_report(report: ProfileReport, fallback_timestamp: &str) -> PluginOutput {
    let report_timestamp = report
        .timestamp
        .unwrap_or_else(|| fallback_timestamp.to_string());
    let source = report
        .source
        .unwrap_or_else(|| "profile-report".to_string());
    let profiles = report
        .profiles
        .into_iter()
        .enumerate()
        .filter_map(|(index, profile)| {
            let sandbox_id = profile.sandbox_id.or_else(|| report.sandbox_id.clone())?;
            let profile_timestamp = profile
                .timestamp
                .unwrap_or_else(|| report_timestamp.clone());
            let profile_type = profile.profile_type;
            let id = profile.id.unwrap_or_else(|| {
                format!(
                    "{}-{}-{}-{}-{}",
                    sanitize_id(&source),
                    sanitize_id(&sandbox_id),
                    sanitize_id(&profile_type),
                    sanitize_id(&profile_timestamp),
                    index
                )
            });

            Some(ProfileArtifact {
                id,
                timestamp: profile_timestamp,
                sandbox_id,
                profile_type,
                process_role: profile
                    .process_role
                    .unwrap_or_else(|| "unknown".to_string()),
                duration_ms: profile.duration_ms.unwrap_or(0.0),
                sample_count: profile.sample_count.unwrap_or(0),
                object_uri: profile.object_uri,
                flamegraph: profile.flamegraph,
            })
        })
        .collect();

    PluginOutput {
        source: None,
        metadata: Metadata::default(),
        metrics: Vec::new(),
        events: Vec::new(),
        traces: Vec::new(),
        profiles,
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn test_config() -> CollectorConfig {
        CollectorConfig {
            ingest_url: "http://query/api/ingest/batch".to_string(),
            node_id: "test-node".to_string(),
            cluster_id: "test-cluster".to_string(),
            interval: Duration::from_secs(1),
            local_report_addr: "127.0.0.1:9091".to_string(),
            local_report_url: "http://127.0.0.1:9091/api/local/ingest".to_string(),
            collection_scope: "host".to_string(),
            once: true,
            cgroup_root: PathBuf::from("/sys/fs/cgroup"),
            cgroup_max_entries: 1,
            image_cache_report_path: None,
            profile_report_path: None,
            plugins: Vec::new(),
            command_plugins: Vec::new(),
            http_plugins: Vec::new(),
        }
    }

    #[test]
    fn parses_lightweight_profile_jsonl() {
        let content = r#"
{"sandboxId":"sandbox-a","source":"perf","profiles":[{"profileType":"cpu","processRole":"app","durationMs":1500,"sampleCount":42,"objectUri":"file:///tmp/a.perf"}]}
{"sandboxId":"sandbox-b","profiles":[{"profileType":"block_io","objectUri":"file:///tmp/b.perf"}]}
"#;

        let output = profile_output_from_content(
            content,
            DateTime::parse_from_rfc3339("2026-05-22T00:00:00Z")
                .unwrap()
                .with_timezone(&Utc),
            &test_config(),
        )
        .unwrap();

        assert_eq!(output.profiles.len(), 2);
        assert_eq!(
            output.profiles[0].id,
            "perf-sandbox-a-cpu-2026-05-22t00-00-00-000z-0"
        );
        assert_eq!(output.profiles[0].sample_count, 42);
        assert_eq!(output.profiles[1].sandbox_id, "sandbox-b");
        assert_eq!(output.profiles[1].process_role, "unknown");
        assert_eq!(output.metadata.nodes[0]["id"], "test-node");
    }

    #[test]
    fn accepts_runtimepulse_plugin_output() {
        let content = r#"{
  "profiles": [
    {
      "id": "profile-1",
      "timestamp": "2026-05-22T00:00:00Z",
      "sandboxId": "sandbox-a",
      "profileType": "cpu",
      "processRole": "app",
      "durationMs": 1000,
      "sampleCount": 10,
      "objectUri": "file:///tmp/profile.pprof"
    }
  ]
}"#;

        let output = profile_output_from_content(
            content,
            DateTime::parse_from_rfc3339("2026-05-22T00:00:00Z")
                .unwrap()
                .with_timezone(&Utc),
            &test_config(),
        )
        .unwrap();

        assert_eq!(output.profiles.len(), 1);
        assert_eq!(output.profiles[0].id, "profile-1");
    }
}
