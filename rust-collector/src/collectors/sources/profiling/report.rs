//! Generic profile artifact report source.
//!
//! Profiling tools differ a lot: perf may write local files, eBPF agents may
//! expose JSON over another process, and third-party profilers may only know the
//! target sandbox. This source reads a small RuntimePulse-compatible JSON/JSONL
//! file and normalizes it into profile artifacts for the shared outlet path.

use chrono::{DateTime, SecondsFormat, Utc};
use serde::Deserialize;
use serde_json::{json, Map, Value};
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
    EventRecord, Metadata, MetricSample, PluginOutput, ProfileArtifact,
};
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
    target: Option<ProfileTargetReport>,
    stats: Option<ProfileStatsReport>,
    labels: Option<Map<String, Value>>,
    attributes: Option<Map<String, Value>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProfileTargetReport {
    pid: Option<u64>,
    command: Option<String>,
    thread_id: Option<u64>,
    runtime_process: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProfileStatsReport {
    lost_samples: Option<u64>,
    sample_rate_hz: Option<f64>,
    cpu_time_ms: Option<f64>,
    wall_time_ms: Option<f64>,
    kernel_samples: Option<u64>,
    user_samples: Option<u64>,
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

pub fn profile_output_from_command(
    command: &str,
    timeout: Duration,
    now: DateTime<Utc>,
    config: &CollectorConfig,
    plugin_name: &str,
) -> Result<PluginOutput> {
    let content = run_profile_command(command, timeout, plugin_name)?;
    if content.trim().is_empty() {
        return Ok(PluginOutput::default());
    }
    profile_output_from_content_with_plugin(&content, now, config, plugin_name)
}

pub fn merge_profile_output(target: &mut PluginOutput, output: PluginOutput) {
    merge_plugin_output(target, output);
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
                merge_plugin_output(
                    &mut output,
                    output_from_lightweight_report(report, &fallback_timestamp),
                );
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

            let mut flamegraph = profile.flamegraph;
            let mut metadata = Map::new();
            if let Some(target) = profile.target {
                if let Some(pid) = target.pid {
                    metadata.insert("target.pid".to_string(), json!(pid));
                }
                if let Some(command) = target.command {
                    metadata.insert("target.command".to_string(), json!(command));
                }
                if let Some(thread_id) = target.thread_id {
                    metadata.insert("target.threadId".to_string(), json!(thread_id));
                }
                if let Some(runtime_process) = target.runtime_process {
                    metadata.insert("target.runtimeProcess".to_string(), json!(runtime_process));
                }
            }
            if let Some(stats) = profile.stats {
                if let Some(lost_samples) = stats.lost_samples {
                    metadata.insert("stats.lostSamples".to_string(), json!(lost_samples));
                }
                if let Some(sample_rate_hz) = stats.sample_rate_hz {
                    metadata.insert("stats.sampleRateHz".to_string(), json!(sample_rate_hz));
                }
                if let Some(cpu_time_ms) = stats.cpu_time_ms {
                    metadata.insert("stats.cpuTimeMs".to_string(), json!(cpu_time_ms));
                }
                if let Some(wall_time_ms) = stats.wall_time_ms {
                    metadata.insert("stats.wallTimeMs".to_string(), json!(wall_time_ms));
                }
                if let Some(kernel_samples) = stats.kernel_samples {
                    metadata.insert("stats.kernelSamples".to_string(), json!(kernel_samples));
                }
                if let Some(user_samples) = stats.user_samples {
                    metadata.insert("stats.userSamples".to_string(), json!(user_samples));
                }
            }
            if let Some(labels) = profile.labels {
                for (key, value) in labels {
                    metadata.insert(format!("label.{key}"), value);
                }
            }
            if let Some(attributes) = profile.attributes {
                for (key, value) in attributes {
                    metadata.insert(key, value);
                }
            }
            if !metadata.is_empty() {
                flamegraph = Some(merge_flamegraph_metadata(flamegraph, metadata));
            }

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
                flamegraph,
            })
        })
        .collect::<Vec<_>>();

    let metrics = profile_metrics(&profiles, &source);
    let events = profile_events(&profiles, &source);

    PluginOutput {
        source: None,
        metadata: Metadata::default(),
        metrics,
        events,
        traces: Vec::new(),
        profiles,
    }
}

fn merge_flamegraph_metadata(flamegraph: Option<Value>, metadata: Map<String, Value>) -> Value {
    match flamegraph {
        Some(Value::Object(mut object)) => {
            object.insert("metadata".to_string(), Value::Object(metadata));
            Value::Object(object)
        }
        Some(value) => json!({
            "profile": value,
            "metadata": metadata,
        }),
        None => json!({
            "metadata": metadata,
        }),
    }
}

fn profile_metrics(profiles: &[ProfileArtifact], source: &str) -> Vec<MetricSample> {
    let mut metrics = Vec::new();
    for profile in profiles {
        let attributes = profile_metric_attributes(profile, source);
        metrics.push(profile_metric(
            &profile.timestamp,
            "profile.samples_total",
            profile.sample_count as f64,
            "samples",
            &profile.sandbox_id,
            &attributes,
        ));
        metrics.push(profile_metric(
            &profile.timestamp,
            "profile.duration_ms",
            profile.duration_ms,
            "ms",
            &profile.sandbox_id,
            &attributes,
        ));
        if let Some(value) = profile_metadata_number(profile, "stats.lostSamples") {
            metrics.push(profile_metric(
                &profile.timestamp,
                "profile.lost_samples_total",
                value,
                "samples",
                &profile.sandbox_id,
                &attributes,
            ));
        }
        if let Some(value) = profile_metadata_number(profile, "stats.sampleRateHz") {
            metrics.push(profile_metric(
                &profile.timestamp,
                "profile.sample_rate_hz",
                value,
                "hz",
                &profile.sandbox_id,
                &attributes,
            ));
        }
        if let Some(value) = profile_metadata_number(profile, "stats.cpuTimeMs") {
            metrics.push(profile_metric(
                &profile.timestamp,
                "profile.cpu_time_ms",
                value,
                "ms",
                &profile.sandbox_id,
                &attributes,
            ));
        }
        if let Some(value) = profile_metadata_number(profile, "stats.kernelSamples") {
            let mut kernel_attributes = attributes.clone();
            kernel_attributes.insert("profile.sampleMode".to_string(), json!("kernel"));
            metrics.push(profile_metric(
                &profile.timestamp,
                "profile.kernel_samples_total",
                value,
                "samples",
                &profile.sandbox_id,
                &kernel_attributes,
            ));
        }
        if let Some(value) = profile_metadata_number(profile, "stats.userSamples") {
            let mut user_attributes = attributes.clone();
            user_attributes.insert("profile.sampleMode".to_string(), json!("user"));
            metrics.push(profile_metric(
                &profile.timestamp,
                "profile.user_samples_total",
                value,
                "samples",
                &profile.sandbox_id,
                &user_attributes,
            ));
        }
    }
    metrics
}

fn profile_metric(
    timestamp: &str,
    name: &str,
    value: f64,
    unit: &str,
    sandbox_id: &str,
    attributes: &Map<String, Value>,
) -> MetricSample {
    MetricSample {
        timestamp: timestamp.to_string(),
        name: name.to_string(),
        value,
        unit: Some(unit.to_string()),
        group: Some("profile".to_string()),
        sandbox_id: Some(sandbox_id.to_string()),
        node_id: None,
        image_id: None,
        runtime_type: None,
        attributes: Some(attributes.clone()),
    }
}

fn profile_metric_attributes(profile: &ProfileArtifact, source: &str) -> Map<String, Value> {
    Map::from_iter([
        ("collector.source".to_string(), json!(source)),
        ("profile.id".to_string(), json!(profile.id)),
        ("profile.type".to_string(), json!(profile.profile_type)),
        (
            "profile.processRole".to_string(),
            json!(profile.process_role),
        ),
        ("profile.objectUri".to_string(), json!(profile.object_uri)),
    ])
}

fn profile_metadata_number(profile: &ProfileArtifact, key: &str) -> Option<f64> {
    profile
        .flamegraph
        .as_ref()?
        .get("metadata")?
        .get(key)?
        .as_f64()
}

fn profile_events(profiles: &[ProfileArtifact], source: &str) -> Vec<EventRecord> {
    profiles
        .iter()
        .map(|profile| {
            let mut attributes = profile_metric_attributes(profile, source);
            if let Some(metadata) = profile
                .flamegraph
                .as_ref()
                .and_then(|value| value.get("metadata"))
            {
                attributes.insert("profile.metadata".to_string(), metadata.clone());
            }
            EventRecord {
                id: format!("{}-observed", profile.id),
                timestamp: profile.timestamp.clone(),
                severity: if profile.sample_count == 0 {
                    "warning"
                } else {
                    "info"
                }
                .to_string(),
                event_type: "profile".to_string(),
                event_name: format!("profile.{}.observed", profile.profile_type),
                message: format!(
                    "{} profile {} captured {} samples for {}.",
                    source, profile.profile_type, profile.sample_count, profile.sandbox_id
                ),
                source: format!("runtimepulse-rust-collector/{source}"),
                attributes,
                sandbox_id: Some(profile.sandbox_id.clone()),
                image_id: None,
                node_id: None,
                runtime_type: None,
                reason: None,
            }
        })
        .collect()
}

fn run_profile_command(command: &str, timeout: Duration, plugin_name: &str) -> Result<String> {
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
                    plugin: plugin_name.to_string(),
                    message: format!(
                        "profile command exited with status {:?}: {}",
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
                plugin: plugin_name.to_string(),
                message: format!("profile command timed out after {} ms", timeout.as_millis()),
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
            diagnostic_report_path: None,
            diagnostic_report_command: None,
            diagnostic_report_command_timeout: Duration::from_secs(1),
            perf_report_path: None,
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
        assert_eq!(output.metrics.len(), 4);
        assert_eq!(output.events.len(), 2);
    }

    #[test]
    fn parses_profile_target_stats_and_labels() {
        let content = r#"{
  "sandboxId": "sandbox-a",
  "source": "ebpf",
  "timestamp": "2026-05-22T00:00:00Z",
  "profiles": [{
    "profileType": "off_cpu",
    "processRole": "runtime",
    "durationMs": 2000,
    "sampleCount": 100,
    "objectUri": "file:///tmp/offcpu.pprof",
    "target": {"pid": 1234, "command": "runsc-sandbox", "runtimeProcess": "sentry"},
    "stats": {"lostSamples": 3, "sampleRateHz": 99.5, "cpuTimeMs": 250, "kernelSamples": 70, "userSamples": 30},
    "labels": {"runtime": "gvisor"},
    "attributes": {"profile.trigger": "manual"}
  }]
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
        let metadata = output.profiles[0]
            .flamegraph
            .as_ref()
            .and_then(|value| value.get("metadata"))
            .unwrap();
        assert_eq!(metadata["target.pid"], 1234);
        assert_eq!(metadata["stats.lostSamples"], 3);
        assert_eq!(metadata["label.runtime"], "gvisor");
        assert!(output
            .metrics
            .iter()
            .any(|metric| metric.name == "profile.lost_samples_total" && metric.value == 3.0));
        assert!(output
            .metrics
            .iter()
            .any(|metric| metric.name == "profile.kernel_samples_total" && metric.value == 70.0));
        assert_eq!(output.events[0].event_type, "profile");
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
