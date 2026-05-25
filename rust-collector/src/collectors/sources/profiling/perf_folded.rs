//! perf folded stack exporter.
//!
//! This is a lightweight native perf adapter for environments that already
//! generate folded stacks (`perf script | stackcollapse-perf.pl` style). It
//! converts folded stack files into RuntimePulse profile artifacts with an
//! inline flamegraph tree and profile metrics/events.

use chrono::{DateTime, SecondsFormat, Utc};
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use crate::collectors::core::config::CollectorConfig;
use crate::collectors::core::error::Result;
use crate::collectors::core::model::PluginOutput;
use crate::collectors::core::plugin::CollectorPlugin;
use crate::collectors::sources::profiling::report::{
    merge_profile_output, profile_output_from_content_with_plugin,
};

pub const DEFAULT_OUTPUT_DIR: &str = "/tmp/runtimepulse/profiles/perf-folded";

#[derive(Debug, Default)]
struct FlameNode {
    value: u64,
    children: BTreeMap<String, FlameNode>,
}

#[derive(Debug)]
pub struct FoldedProfileTarget {
    pub sandbox_id: String,
    pub folded_path: PathBuf,
    pub object_uri: String,
    pub profile_type: String,
    pub process_role: String,
}

pub struct PerfFoldedProfilePlugin {
    path: Option<PathBuf>,
}

impl PerfFoldedProfilePlugin {
    pub fn new(path: Option<PathBuf>) -> Self {
        Self { path }
    }
}

impl CollectorPlugin for PerfFoldedProfilePlugin {
    fn name(&self) -> &str {
        "perf-folded"
    }

    fn collect(&mut self, now: DateTime<Utc>, config: &CollectorConfig) -> Result<PluginOutput> {
        collect_perf_folded_profiles_with_path(now, config, self.path.clone())
    }
}

pub fn emit_perf_folded_profiles(config: &CollectorConfig) -> Result<()> {
    let output = collect_perf_folded_profiles(Utc::now(), config)?;
    println!("{}", serde_json::to_string(&output)?);
    Ok(())
}

pub fn collect_perf_folded_profiles(
    now: DateTime<Utc>,
    config: &CollectorConfig,
) -> Result<PluginOutput> {
    collect_perf_folded_profiles_with_path(now, config, config.perf_folded_path.clone())
}

pub fn collect_perf_folded_profiles_with_path(
    now: DateTime<Utc>,
    config: &CollectorConfig,
    fallback_path: Option<PathBuf>,
) -> Result<PluginOutput> {
    let mut output = PluginOutput::default();
    let targets = perf_folded_targets(fallback_path);

    for target in targets {
        let folded = match fs::read_to_string(&target.folded_path) {
            Ok(content) => content,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.into()),
        };
        if folded.trim().is_empty() {
            continue;
        }
        let report = lightweight_report_from_folded(&folded, &target, now, config)?;
        merge_profile_output(
            &mut output,
            profile_output_from_content_with_plugin(&report, now, config, "perf-folded")?,
        );
    }

    Ok(output)
}

pub fn lightweight_report_from_folded(
    folded: &str,
    target: &FoldedProfileTarget,
    now: DateTime<Utc>,
    config: &CollectorConfig,
) -> Result<String> {
    let timestamp = timestamp(now);
    let (sample_count, flamegraph) = flamegraph_from_folded_stacks(folded);
    let duration_ms = perf_folded_duration_ms();
    let output_dir = perf_folded_output_dir()
        .join(&target.sandbox_id)
        .join(sanitize_id(&timestamp));
    fs::create_dir_all(&output_dir)?;
    let artifact_path = output_dir.join(
        target
            .folded_path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("perf.folded"),
    );
    if target.object_uri.is_empty() {
        fs::write(&artifact_path, folded)?;
    }
    let object_uri = if target.object_uri.is_empty() {
        file_uri(&artifact_path)
    } else {
        target.object_uri.clone()
    };
    let id = format!(
        "perf-folded-{}-{}-{}",
        sanitize_id(&target.sandbox_id),
        sanitize_id(&target.profile_type),
        sanitize_id(&timestamp)
    );

    Ok(json!({
        "sandboxId": target.sandbox_id,
        "source": "perf-folded",
        "timestamp": timestamp,
        "profiles": [{
            "id": id,
            "profileType": target.profile_type,
            "processRole": target.process_role,
            "durationMs": duration_ms,
            "sampleCount": sample_count,
            "objectUri": object_uri,
            "flamegraph": flamegraph,
            "target": {
                "runtimeProcess": target.process_role,
            },
            "stats": {
                "sampleRateHz": if duration_ms > 0.0 { (sample_count as f64) / (duration_ms / 1000.0) } else { 0.0 },
                "lostSamples": 0,
            },
            "labels": {
                "collector": "perf-folded",
                "node": config.node_id,
            },
            "attributes": {
                "profile.sourcePath": target.folded_path,
                "profile.folded": true,
            }
        }]
    })
    .to_string())
}

fn perf_folded_targets(fallback_path: Option<PathBuf>) -> Vec<FoldedProfileTarget> {
    if let Ok(value) = env::var("RUNTIMEPULSE_PERF_FOLDED_TARGETS") {
        let targets = value
            .split(';')
            .filter_map(parse_folded_target)
            .collect::<Vec<_>>();
        if !targets.is_empty() {
            return targets;
        }
    }

    let Some(path) = fallback_path.or_else(|| {
        env::var("RUNTIMEPULSE_PERF_FOLDED_PATH")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .map(PathBuf::from)
    }) else {
        return Vec::new();
    };
    vec![FoldedProfileTarget {
        sandbox_id: env::var("RUNTIMEPULSE_PERF_FOLDED_SANDBOX_ID")
            .unwrap_or_else(|_| "host-perf-folded".to_string()),
        folded_path: path,
        object_uri: env::var("RUNTIMEPULSE_PERF_FOLDED_OBJECT_URI").unwrap_or_default(),
        profile_type: env::var("RUNTIMEPULSE_PERF_FOLDED_PROFILE_TYPE")
            .unwrap_or_else(|_| "cpu".to_string()),
        process_role: env::var("RUNTIMEPULSE_PERF_FOLDED_PROCESS_ROLE")
            .unwrap_or_else(|_| "app".to_string()),
    }]
}

pub fn parse_folded_target(value: &str) -> Option<FoldedProfileTarget> {
    let mut sandbox_id = None;
    let mut path = None;
    let mut object_uri = String::new();
    let mut profile_type = "cpu".to_string();
    let mut process_role = "app".to_string();

    for part in value
        .split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
    {
        let Some((key, raw_value)) = part.split_once('=') else {
            continue;
        };
        match key.trim() {
            "sandbox" | "sandboxId" => sandbox_id = Some(raw_value.trim().to_string()),
            "path" | "foldedPath" => path = Some(PathBuf::from(raw_value.trim())),
            "objectUri" | "uri" => object_uri = raw_value.trim().to_string(),
            "profileType" | "type" => profile_type = raw_value.trim().to_string(),
            "processRole" | "role" => process_role = raw_value.trim().to_string(),
            _ => {}
        }
    }

    Some(FoldedProfileTarget {
        sandbox_id: sandbox_id?,
        folded_path: path?,
        object_uri,
        profile_type,
        process_role,
    })
}

pub fn flamegraph_from_folded_stacks(content: &str) -> (u64, Value) {
    let mut root = FlameNode::default();
    for line in content
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
    {
        let Some((stack, count)) = parse_folded_line(line) else {
            continue;
        };
        insert_stack(
            &mut root,
            stack.split(';').filter(|frame| !frame.is_empty()),
            count,
        );
    }
    let total = root.value;
    (total, flame_node_to_json("root", &root))
}

fn parse_folded_line(line: &str) -> Option<(&str, u64)> {
    let (stack, raw_count) = line.rsplit_once(char::is_whitespace)?;
    let count = raw_count.trim().parse::<u64>().ok()?;
    let stack = stack.trim();
    if stack.is_empty() || count == 0 {
        None
    } else {
        Some((stack, count))
    }
}

fn insert_stack<'a>(root: &mut FlameNode, frames: impl Iterator<Item = &'a str>, count: u64) {
    root.value = root.value.saturating_add(count);
    let mut node = root;
    for frame in frames {
        node = node.children.entry(frame.to_string()).or_default();
        node.value = node.value.saturating_add(count);
    }
}

fn flame_node_to_json(name: &str, node: &FlameNode) -> Value {
    let mut object = Map::new();
    object.insert("name".to_string(), json!(name));
    object.insert("value".to_string(), json!(node.value));
    if !node.children.is_empty() {
        object.insert(
            "children".to_string(),
            Value::Array(
                node.children
                    .iter()
                    .map(|(child_name, child)| flame_node_to_json(child_name, child))
                    .collect(),
            ),
        );
    }
    Value::Object(object)
}

fn perf_folded_output_dir() -> PathBuf {
    PathBuf::from(
        env::var("RUNTIMEPULSE_PERF_FOLDED_OUTPUT_DIR")
            .unwrap_or_else(|_| DEFAULT_OUTPUT_DIR.to_string()),
    )
}

pub fn perf_folded_duration_ms() -> f64 {
    env::var("RUNTIMEPULSE_PERF_FOLDED_DURATION_MS")
        .ok()
        .and_then(|value| value.parse::<f64>().ok())
        .unwrap_or(0.0)
        .max(0.0)
}

pub fn file_uri(path: &Path) -> String {
    format!("file://{}", path.display())
}

pub fn sanitize_id(value: &str) -> String {
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

pub fn timestamp(time: DateTime<Utc>) -> String {
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
    fn parses_folded_stacks_into_flamegraph() {
        let (samples, graph) = flamegraph_from_folded_stacks(
            "main;work;syscall 3\nmain;work;compute 7\nmain;idle 2\n",
        );

        assert_eq!(samples, 12);
        assert_eq!(graph["name"], "root");
        assert_eq!(graph["children"][0]["name"], "main");
        assert_eq!(graph["children"][0]["value"], 12);
    }

    #[test]
    fn converts_folded_stacks_to_profile_output() {
        let target = FoldedProfileTarget {
            sandbox_id: "sandbox-a".to_string(),
            folded_path: PathBuf::from("/tmp/runtimepulse-test.folded"),
            object_uri: "file:///tmp/runtimepulse-test.folded".to_string(),
            profile_type: "cpu".to_string(),
            process_role: "runtime".to_string(),
        };
        let report = lightweight_report_from_folded(
            "a;b 4\na;c 6\n",
            &target,
            DateTime::parse_from_rfc3339("2026-05-24T00:00:00Z")
                .unwrap()
                .with_timezone(&Utc),
            &test_config(),
        )
        .unwrap();
        let output = profile_output_from_content_with_plugin(
            &report,
            DateTime::parse_from_rfc3339("2026-05-24T00:00:00Z")
                .unwrap()
                .with_timezone(&Utc),
            &test_config(),
            "perf-folded",
        )
        .unwrap();

        assert_eq!(output.profiles.len(), 1);
        assert_eq!(output.profiles[0].sample_count, 10);
        assert_eq!(output.profiles[0].profile_type, "cpu");
        assert!(output
            .metrics
            .iter()
            .any(|metric| metric.name == "profile.samples_total" && metric.value == 10.0));
        assert_eq!(output.events[0].event_name, "profile.cpu.observed");
    }
}
