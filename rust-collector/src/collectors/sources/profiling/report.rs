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
    #[serde(alias = "sandbox_id")]
    sandbox_id: Option<String>,
    timestamp: Option<String>,
    source: Option<String>,
    profiles: Vec<ProfileArtifactReport>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProfileArtifactReport {
    id: Option<String>,
    timestamp: Option<String>,
    #[serde(alias = "sandbox_id")]
    sandbox_id: Option<String>,
    #[serde(
        alias = "profile_type",
        alias = "type",
        alias = "artifactType",
        alias = "artifact_type"
    )]
    profile_type: String,
    #[serde(alias = "process_role", alias = "role")]
    process_role: Option<String>,
    #[serde(alias = "duration_ms", alias = "duration")]
    duration_ms: Option<f64>,
    #[serde(alias = "sample_count", alias = "samples")]
    sample_count: Option<u64>,
    #[serde(alias = "object_uri", alias = "uri", alias = "path", alias = "file")]
    object_uri: String,
    flamegraph: Option<Value>,
    target: Option<ProfileTargetReport>,
    stats: Option<ProfileStatsReport>,
    labels: Option<Map<String, Value>>,
    attributes: Option<Map<String, Value>>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProfileTargetReport {
    pid: Option<u64>,
    command: Option<String>,
    #[serde(alias = "thread_id", alias = "tid", alias = "thread")]
    thread_id: Option<u64>,
    #[serde(alias = "runtime_process", alias = "process")]
    runtime_process: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProfileStatsReport {
    #[serde(alias = "lost_samples")]
    lost_samples: Option<u64>,
    #[serde(alias = "sample_rate_hz")]
    sample_rate_hz: Option<f64>,
    #[serde(alias = "cpu_time_ms")]
    cpu_time_ms: Option<f64>,
    #[serde(alias = "wall_time_ms")]
    wall_time_ms: Option<f64>,
    #[serde(alias = "kernel_samples")]
    kernel_samples: Option<u64>,
    #[serde(alias = "user_samples")]
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
        if let Ok(reports) = serde_json::from_str::<Vec<ProfileReport>>(trimmed) {
            return Ok(reports
                .into_iter()
                .map(ParsedProfileReport::Lightweight)
                .collect());
        }
        return Ok(parse_generic_profile_value(serde_json::from_str(trimmed)?)
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
        if let Ok(value) = serde_json::from_str::<Value>(trimmed) {
            return Ok(parse_generic_profile_value(value)
                .into_iter()
                .map(ParsedProfileReport::Lightweight)
                .collect());
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
        if let Ok(report) = serde_json::from_str::<ProfileReport>(line) {
            reports.push(ParsedProfileReport::Lightweight(report));
            continue;
        }
        reports.extend(
            parse_generic_profile_value(serde_json::from_str(line)?)
                .into_iter()
                .map(ParsedProfileReport::Lightweight),
        );
    }

    Ok(reports)
}

fn parse_generic_profile_value(value: Value) -> Vec<ProfileReport> {
    match value {
        Value::Array(items) => items
            .into_iter()
            .filter_map(generic_profile_artifact_from_value)
            .map(|profile| ProfileReport {
                sandbox_id: profile.sandbox_id.clone(),
                timestamp: profile.timestamp.clone(),
                source: Some("profile-index".to_string()),
                profiles: vec![profile],
            })
            .collect(),
        Value::Object(mut object) => {
            let defaults = GenericProfileDefaults::from_object(&object);
            for key in ["profiles", "artifacts", "files", "items", "records"] {
                if let Some(Value::Array(items)) = object.remove(key) {
                    let profiles = items
                        .into_iter()
                        .filter_map(|value| {
                            generic_profile_artifact_from_value_with_defaults(value, &defaults)
                        })
                        .collect::<Vec<_>>();
                    if profiles.is_empty() {
                        return Vec::new();
                    }
                    return vec![ProfileReport {
                        sandbox_id: defaults.sandbox_id,
                        timestamp: defaults.timestamp,
                        source: defaults
                            .source
                            .or_else(|| Some("profile-index".to_string())),
                        profiles,
                    }];
                }
            }
            generic_profile_artifact_from_object_with_defaults(object, &defaults)
                .map(|profile| ProfileReport {
                    sandbox_id: profile.sandbox_id.clone().or(defaults.sandbox_id),
                    timestamp: profile.timestamp.clone().or(defaults.timestamp),
                    source: defaults
                        .source
                        .or_else(|| Some("profile-index".to_string())),
                    profiles: vec![profile],
                })
                .into_iter()
                .collect()
        }
        _ => Vec::new(),
    }
}

#[derive(Clone, Debug, Default)]
struct GenericProfileDefaults {
    sandbox_id: Option<String>,
    timestamp: Option<String>,
    source: Option<String>,
    target: Option<ProfileTargetReport>,
    labels: Option<Map<String, Value>>,
}

impl GenericProfileDefaults {
    fn from_object(object: &Map<String, Value>) -> Self {
        let target = object
            .get("target")
            .and_then(|value| value.as_object())
            .map(profile_target_from_map);
        Self {
            sandbox_id: value_string_from_keys(
                object,
                &[
                    "sandboxId",
                    "sandbox_id",
                    "sandbox",
                    "containerId",
                    "container_id",
                ],
            )
            .or_else(|| {
                target
                    .as_ref()
                    .and_then(|target| target.runtime_process.clone())
            }),
            timestamp: value_string_from_keys(object, &["timestamp", "observedAt", "observed_at"]),
            source: value_string_from_keys(object, &["source", "collector", "profiler"]),
            target,
            labels: object
                .get("labels")
                .and_then(|value| value.as_object())
                .cloned(),
        }
    }
}

fn generic_profile_artifact_from_value(value: Value) -> Option<ProfileArtifactReport> {
    generic_profile_artifact_from_value_with_defaults(value, &GenericProfileDefaults::default())
}

fn generic_profile_artifact_from_value_with_defaults(
    value: Value,
    defaults: &GenericProfileDefaults,
) -> Option<ProfileArtifactReport> {
    match value {
        Value::Object(object) => {
            generic_profile_artifact_from_object_with_defaults(object, defaults)
        }
        _ => None,
    }
}

fn generic_profile_artifact_from_object_with_defaults(
    mut object: Map<String, Value>,
    defaults: &GenericProfileDefaults,
) -> Option<ProfileArtifactReport> {
    let nested_target = take_object(&mut object, &["target"]);
    let nested_stats = take_object(&mut object, &["stats", "statistics"]);
    let labels = take_object(&mut object, &["labels", "tags"]).or_else(|| defaults.labels.clone());
    let mut attributes = take_object(&mut object, &["attributes", "metadata"]);

    let object_uri = take_string(
        &mut object,
        &[
            "objectUri",
            "object_uri",
            "uri",
            "path",
            "file",
            "profilePath",
            "profile_path",
            "pprof",
            "flamegraphUri",
            "flamegraph_uri",
        ],
    )?;
    let profile_type = take_string(
        &mut object,
        &[
            "profileType",
            "profile_type",
            "type",
            "artifactType",
            "artifact_type",
            "kind",
        ],
    )
    .unwrap_or_else(|| infer_profile_type(&object_uri));
    let sandbox_id = take_string(
        &mut object,
        &[
            "sandboxId",
            "sandbox_id",
            "sandbox",
            "containerId",
            "container_id",
        ],
    )
    .or_else(|| defaults.sandbox_id.clone())?;
    let target = nested_target
        .as_ref()
        .map(profile_target_from_map)
        .or_else(|| profile_target_from_flat_object(&object))
        .or_else(|| defaults.target.clone());
    let stats = nested_stats
        .as_ref()
        .map(profile_stats_from_map)
        .or_else(|| Some(profile_stats_from_map(&object)))
        .filter(profile_stats_has_payload);

    if attributes.is_none() && !object.is_empty() {
        let mut metadata = Map::new();
        for (key, value) in object.iter() {
            if !matches!(value, Value::Null) {
                metadata.insert(format!("profile.raw.{key}"), value.clone());
            }
        }
        if !metadata.is_empty() {
            attributes = Some(metadata);
        }
    }

    Some(ProfileArtifactReport {
        id: take_string(&mut object, &["id", "name"]),
        timestamp: take_string(&mut object, &["timestamp", "observedAt", "observed_at"])
            .or_else(|| defaults.timestamp.clone()),
        sandbox_id: Some(sandbox_id),
        profile_type,
        process_role: take_string(&mut object, &["processRole", "process_role", "role"]).or_else(
            || {
                target
                    .as_ref()
                    .and_then(|target| target.runtime_process.clone())
            },
        ),
        duration_ms: take_f64(&mut object, &["durationMs", "duration_ms", "duration"]),
        sample_count: take_u64(
            &mut object,
            &["sampleCount", "sample_count", "samples", "count"],
        ),
        object_uri,
        flamegraph: take_value(&mut object, &["flamegraph", "profile", "tree"]),
        target,
        stats,
        labels,
        attributes,
    })
}

fn profile_target_from_map(object: &Map<String, Value>) -> ProfileTargetReport {
    ProfileTargetReport {
        pid: value_u64_from_keys(object, &["pid", "processId", "process_id"]),
        command: value_string_from_keys(
            object,
            &["command", "cmd", "comm", "processName", "process_name"],
        ),
        thread_id: value_u64_from_keys(object, &["threadId", "thread_id", "tid", "thread"]),
        runtime_process: value_string_from_keys(
            object,
            &["runtimeProcess", "runtime_process", "process", "role"],
        ),
    }
}

fn profile_target_from_flat_object(object: &Map<String, Value>) -> Option<ProfileTargetReport> {
    let target = profile_target_from_map(object);
    if target.pid.is_some()
        || target.command.is_some()
        || target.thread_id.is_some()
        || target.runtime_process.is_some()
    {
        Some(target)
    } else {
        None
    }
}

fn profile_stats_from_map(object: &Map<String, Value>) -> ProfileStatsReport {
    ProfileStatsReport {
        lost_samples: value_u64_from_keys(object, &["lostSamples", "lost_samples"]),
        sample_rate_hz: value_f64_from_keys(
            object,
            &["sampleRateHz", "sample_rate_hz", "rateHz", "rate_hz"],
        ),
        cpu_time_ms: value_f64_from_keys(object, &["cpuTimeMs", "cpu_time_ms"]),
        wall_time_ms: value_f64_from_keys(object, &["wallTimeMs", "wall_time_ms"]),
        kernel_samples: value_u64_from_keys(object, &["kernelSamples", "kernel_samples"]),
        user_samples: value_u64_from_keys(object, &["userSamples", "user_samples"]),
    }
}

fn profile_stats_has_payload(stats: &ProfileStatsReport) -> bool {
    stats.lost_samples.is_some()
        || stats.sample_rate_hz.is_some()
        || stats.cpu_time_ms.is_some()
        || stats.wall_time_ms.is_some()
        || stats.kernel_samples.is_some()
        || stats.user_samples.is_some()
}

fn infer_profile_type(object_uri: &str) -> String {
    let lower = object_uri.to_ascii_lowercase();
    if lower.contains("offcpu") || lower.contains("off-cpu") {
        "off_cpu".to_string()
    } else if lower.contains("mem") || lower.contains("alloc") {
        "memory".to_string()
    } else if lower.contains("block") || lower.contains("io") {
        "block_io".to_string()
    } else {
        "cpu".to_string()
    }
}

fn take_value(object: &mut Map<String, Value>, keys: &[&str]) -> Option<Value> {
    keys.iter().find_map(|key| object.remove(*key))
}

fn take_object(object: &mut Map<String, Value>, keys: &[&str]) -> Option<Map<String, Value>> {
    keys.iter().find_map(|key| match object.remove(*key) {
        Some(Value::Object(value)) => Some(value),
        _ => None,
    })
}

fn take_string(object: &mut Map<String, Value>, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| object.remove(*key).and_then(value_to_string))
}

fn take_u64(object: &mut Map<String, Value>, keys: &[&str]) -> Option<u64> {
    keys.iter()
        .find_map(|key| object.remove(*key).and_then(|value| value_to_u64(&value)))
}

fn take_f64(object: &mut Map<String, Value>, keys: &[&str]) -> Option<f64> {
    keys.iter()
        .find_map(|key| object.remove(*key).and_then(|value| value_to_f64(&value)))
}

fn value_string_from_keys(object: &Map<String, Value>, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| object.get(*key).cloned().and_then(value_to_string))
}

fn value_u64_from_keys(object: &Map<String, Value>, keys: &[&str]) -> Option<u64> {
    keys.iter()
        .find_map(|key| object.get(*key).and_then(value_to_u64))
}

fn value_f64_from_keys(object: &Map<String, Value>, keys: &[&str]) -> Option<f64> {
    keys.iter()
        .find_map(|key| object.get(*key).and_then(value_to_f64))
}

fn value_to_string(value: Value) -> Option<String> {
    match value {
        Value::String(value) if !value.is_empty() => Some(value),
        Value::Number(value) => Some(value.to_string()),
        _ => None,
    }
}

fn value_to_u64(value: &Value) -> Option<u64> {
    match value {
        Value::Number(number) => number
            .as_u64()
            .or_else(|| number.as_f64().map(|value| value.max(0.0).round() as u64)),
        Value::String(value) => value.parse::<u64>().ok(),
        _ => None,
    }
}

fn value_to_f64(value: &Value) -> Option<f64> {
    match value {
        Value::Number(number) => number.as_f64(),
        Value::String(value) => value.parse::<f64>().ok(),
        _ => None,
    }
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
    #[test]
    fn parses_generic_profile_index_json() {
        let content = r#"{
          "sandbox_id": "sandbox-index",
          "source": "perf-index",
          "timestamp": "2026-05-25T00:00:00Z",
          "target": {"pid": 4321, "command": "demo", "runtime_process": "app"},
          "artifacts": [
            {
              "path": "file:///tmp/sandbox-index/cpu.pprof",
              "type": "cpu",
              "samples": 88,
              "duration_ms": 1200,
              "stats": {"sample_rate_hz": 73.3, "lost_samples": 1},
              "labels": {"runtime": "runc"}
            }
          ]
        }"#;

        let output = profile_output_from_content(
            content,
            DateTime::parse_from_rfc3339("2026-05-25T00:00:00Z")
                .unwrap()
                .with_timezone(&Utc),
            &test_config(),
        )
        .unwrap();

        assert_eq!(output.profiles.len(), 1);
        assert_eq!(output.profiles[0].sandbox_id, "sandbox-index");
        assert_eq!(output.profiles[0].profile_type, "cpu");
        assert_eq!(output.profiles[0].sample_count, 88);
        let metadata = output.profiles[0]
            .flamegraph
            .as_ref()
            .and_then(|value| value.get("metadata"))
            .unwrap();
        assert_eq!(metadata["target.pid"], 4321);
        assert_eq!(metadata["stats.lostSamples"], 1);
        assert!(output
            .metrics
            .iter()
            .any(|metric| metric.name == "profile.sample_rate_hz" && metric.value == 73.3));
    }
}
