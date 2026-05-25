//! perf script profile source.
//!
//! Converts plain `perf script` text into RuntimePulse profile artifacts. This
//! gives host-agent a native non-eBPF profiling path for hosts that already run
//! `perf record && perf script`, without requiring stackcollapse-perf.pl.

use chrono::{DateTime, SecondsFormat, Utc};
use serde_json::json;
use std::collections::{BTreeMap, HashMap};
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
use crate::collectors::core::model::PluginOutput;
use crate::collectors::core::plugin::CollectorPlugin;
use crate::collectors::sources::profiling::perf_folded::{
    file_uri, flamegraph_from_folded_stacks, sanitize_id,
};
use crate::collectors::sources::profiling::report::{
    merge_profile_output, profile_output_from_content_with_plugin,
};

const DEFAULT_OUTPUT_DIR: &str = "/tmp/runtimepulse/profiles/perf-script";

pub struct PerfScriptProfilePlugin {
    path: Option<PathBuf>,
    command: Option<String>,
    timeout: Duration,
}

#[derive(Clone, Debug)]
pub struct PerfScriptTarget {
    pub sandbox_id: String,
    pub script_path: Option<PathBuf>,
    pub object_uri: String,
    pub profile_type: String,
    pub process_role: String,
    pub command: Option<String>,
    pub pid: Option<u64>,
}

impl PerfScriptProfilePlugin {
    pub fn new(path: Option<PathBuf>, command: Option<String>, timeout: Duration) -> Self {
        Self {
            path,
            command,
            timeout,
        }
    }
}

impl CollectorPlugin for PerfScriptProfilePlugin {
    fn name(&self) -> &str {
        "perf-script"
    }

    fn collect(&mut self, now: DateTime<Utc>, config: &CollectorConfig) -> Result<PluginOutput> {
        collect_perf_script_profiles(
            now,
            config,
            self.path.clone(),
            self.command.clone(),
            self.timeout,
        )
    }
}

pub fn emit_perf_script_profiles(config: &CollectorConfig) -> Result<()> {
    let output = collect_perf_script_profiles(
        Utc::now(),
        config,
        config.perf_script_path.clone(),
        config.perf_script_command.clone(),
        config.perf_script_command_timeout,
    )?;
    println!("{}", serde_json::to_string(&output)?);
    Ok(())
}

pub fn collect_perf_script_profiles(
    now: DateTime<Utc>,
    config: &CollectorConfig,
    fallback_path: Option<PathBuf>,
    fallback_command: Option<String>,
    timeout: Duration,
) -> Result<PluginOutput> {
    let mut output = PluginOutput::default();
    for target in perf_script_targets(fallback_path, fallback_command) {
        let script = match read_perf_script(&target, timeout) {
            Ok(content) => content,
            Err(error) => return Err(error),
        };
        if script.trim().is_empty() {
            continue;
        }
        let report = lightweight_report_from_perf_script(&script, &target, now, config)?;
        merge_profile_output(
            &mut output,
            profile_output_from_content_with_plugin(&report, now, config, "perf-script")?,
        );
    }

    Ok(output)
}

fn read_perf_script(target: &PerfScriptTarget, timeout: Duration) -> Result<String> {
    if let Some(command) = &target.command {
        return run_perf_script_command(command, timeout);
    }

    let Some(path) = &target.script_path else {
        return Ok(String::new());
    };
    match fs::read_to_string(path) {
        Ok(content) => Ok(content),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(error) => Err(error.into()),
    }
}

pub fn lightweight_report_from_perf_script(
    script: &str,
    target: &PerfScriptTarget,
    now: DateTime<Utc>,
    config: &CollectorConfig,
) -> Result<String> {
    let timestamp = timestamp(now);
    let summary = summarize_perf_script(script);
    let (sample_count, flamegraph) = flamegraph_from_folded_stacks(&summary.folded);
    let configured_duration_ms = perf_script_duration_ms();
    let duration_ms = if configured_duration_ms > 0.0 {
        configured_duration_ms
    } else {
        summary.duration_ms.unwrap_or(0.0)
    };
    let object_uri = persist_perf_script(script, target, &timestamp)?;
    let id = format!(
        "perf-script-{}-{}-{}",
        sanitize_id(&target.sandbox_id),
        sanitize_id(&target.profile_type),
        sanitize_id(&timestamp)
    );
    let mut target_json = json!({
        "runtimeProcess": target.process_role,
    });
    if let Some(pid) = target.pid.or(summary.pid) {
        target_json["pid"] = json!(pid);
    }
    if let Some(command) = &summary.command {
        target_json["command"] = json!(command);
    }
    Ok(json!({
        "sandboxId": target.sandbox_id,
        "source": "perf-script",
        "timestamp": timestamp,
        "profiles": [{
            "id": id,
            "profileType": target.profile_type,
            "processRole": target.process_role,
            "durationMs": duration_ms,
            "sampleCount": sample_count,
            "objectUri": object_uri,
            "flamegraph": flamegraph,
            "target": target_json,
            "stats": {
                "sampleRateHz": if duration_ms > 0.0 { (sample_count as f64) / (duration_ms / 1000.0) } else { 0.0 },
                "lostSamples": 0,
            },
            "labels": {
                "collector": "perf-script",
                "node": config.node_id,
            },
            "attributes": {
                "profile.perfScript": true,
                "profile.sourcePath": target.script_path,
                "profile.foldedStacks": summary.folded.lines().count(),
                "profile.perfScriptSamples": summary.sample_headers,
                "profile.perfScriptStartSeconds": summary.start_seconds,
                "profile.perfScriptEndSeconds": summary.end_seconds,
            }
        }]
    })
    .to_string())
}

#[derive(Clone, Debug, Default)]
pub struct PerfScriptSummary {
    pub folded: String,
    pub command: Option<String>,
    pub pid: Option<u64>,
    pub start_seconds: Option<f64>,
    pub end_seconds: Option<f64>,
    pub duration_ms: Option<f64>,
    pub sample_headers: u64,
}

#[derive(Clone, Debug)]
struct PerfSampleHeader {
    command: String,
    pid: Option<u64>,
    timestamp_seconds: Option<f64>,
}

#[allow(dead_code)]
pub fn folded_stacks_from_perf_script(script: &str) -> String {
    summarize_perf_script(script).folded
}

pub fn summarize_perf_script(script: &str) -> PerfScriptSummary {
    let mut samples: BTreeMap<String, u64> = BTreeMap::new();
    let mut current = Vec::new();
    let mut saw_header = false;
    let mut command = None;
    let mut pid = None;
    let mut start_seconds: Option<f64> = None;
    let mut end_seconds: Option<f64> = None;
    let mut sample_headers = 0_u64;

    for line in script.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            flush_perf_sample(&mut samples, &mut current);
            saw_header = false;
            continue;
        }
        if trimmed.starts_with('#') {
            continue;
        }
        if let Some(header) = parse_perf_sample_header(trimmed) {
            flush_perf_sample(&mut samples, &mut current);
            if command.is_none() && !header.command.is_empty() {
                command = Some(header.command);
            }
            if pid.is_none() {
                pid = header.pid;
            }
            if let Some(seconds) = header.timestamp_seconds {
                start_seconds = Some(start_seconds.map_or(seconds, |value| value.min(seconds)));
                end_seconds = Some(end_seconds.map_or(seconds, |value| value.max(seconds)));
            }
            sample_headers = sample_headers.saturating_add(1);
            saw_header = true;
            continue;
        }
        if saw_header || !current.is_empty() {
            if let Some(frame) = perf_frame_name(trimmed) {
                current.push(frame);
            }
        }
    }

    flush_perf_sample(&mut samples, &mut current);

    let folded = samples
        .into_iter()
        .map(|(stack, count)| format!("{stack} {count}"))
        .collect::<Vec<_>>()
        .join("\n");
    let duration_ms = start_seconds
        .zip(end_seconds)
        .map(|(start, end)| ((end - start) * 1000.0).max(0.0));

    PerfScriptSummary {
        folded,
        command,
        pid,
        start_seconds,
        end_seconds,
        duration_ms,
        sample_headers,
    }
}

fn flush_perf_sample(samples: &mut BTreeMap<String, u64>, frames: &mut Vec<String>) {
    if frames.is_empty() {
        return;
    }
    let stack = frames
        .iter()
        .rev()
        .map(|frame| sanitize_frame(frame))
        .filter(|frame| !frame.is_empty())
        .collect::<Vec<_>>()
        .join(";");
    frames.clear();
    if stack.is_empty() {
        return;
    }
    *samples.entry(stack).or_insert(0) += 1;
}

fn parse_perf_sample_header(line: &str) -> Option<PerfSampleHeader> {
    let mut parts = line.split_whitespace();
    let command = parts.next()?.to_string();
    let pid_token = parts.next()?;
    if !pid_token
        .chars()
        .all(|char| char.is_ascii_digit() || char == '/')
    {
        return None;
    }
    let pid = pid_token
        .split('/')
        .next()
        .and_then(|value| value.parse::<u64>().ok());
    let timestamp_seconds = parts.find_map(|part| {
        part.strip_suffix(':')
            .and_then(|value| value.parse::<f64>().ok())
    })?;
    Some(PerfSampleHeader {
        command,
        pid,
        timestamp_seconds: Some(timestamp_seconds),
    })
}

fn perf_frame_name(line: &str) -> Option<String> {
    let raw = line.trim();
    let without_dso = raw
        .rsplit_once('(')
        .filter(|(_, suffix)| suffix.trim_end().ends_with(')'))
        .map(|(name, _)| name.trim())
        .unwrap_or(raw);
    let mut parts = without_dso.split_whitespace().collect::<Vec<_>>();
    if parts.first().is_some_and(|part| looks_like_address(part)) {
        parts.remove(0);
    }
    let candidate = parts.join(" ");
    let candidate = candidate
        .split_once('+')
        .map(|(name, _)| name.trim())
        .unwrap_or(candidate.trim())
        .trim_start_matches('+')
        .trim();
    if candidate.is_empty() || looks_like_address(candidate) {
        None
    } else {
        Some(candidate.to_string())
    }
}

fn looks_like_address(value: &str) -> bool {
    let value = value.trim_start_matches("0x");
    value.len() >= 4 && value.chars().all(|char| char.is_ascii_hexdigit())
}

fn sanitize_frame(frame: &str) -> String {
    frame
        .chars()
        .map(|char| match char {
            ';' | '\n' | '\r' => '_',
            _ => char,
        })
        .collect::<String>()
        .trim()
        .to_string()
}

fn perf_script_targets(
    fallback_path: Option<PathBuf>,
    fallback_command: Option<String>,
) -> Vec<PerfScriptTarget> {
    if let Ok(value) = env::var("RUNTIMEPULSE_PERF_SCRIPT_TARGETS") {
        let targets = value
            .split(';')
            .filter_map(parse_perf_script_target)
            .collect::<Vec<_>>();
        if !targets.is_empty() {
            return targets;
        }
    }

    let path = fallback_path.or_else(|| env_path("RUNTIMEPULSE_PERF_SCRIPT_PATH"));
    let command = fallback_command.or_else(|| env_string("RUNTIMEPULSE_PERF_SCRIPT_CMD"));
    if path.is_none() && command.is_none() {
        return Vec::new();
    }

    vec![PerfScriptTarget {
        sandbox_id: env::var("RUNTIMEPULSE_PERF_SCRIPT_SANDBOX_ID")
            .unwrap_or_else(|_| "host-perf-script".to_string()),
        script_path: path,
        object_uri: env::var("RUNTIMEPULSE_PERF_SCRIPT_OBJECT_URI").unwrap_or_default(),
        profile_type: env::var("RUNTIMEPULSE_PERF_SCRIPT_PROFILE_TYPE")
            .unwrap_or_else(|_| "cpu".to_string()),
        process_role: env::var("RUNTIMEPULSE_PERF_SCRIPT_PROCESS_ROLE")
            .unwrap_or_else(|_| "app".to_string()),
        command,
        pid: env::var("RUNTIMEPULSE_PERF_SCRIPT_PID")
            .ok()
            .and_then(|value| value.parse::<u64>().ok()),
    }]
}

fn parse_perf_script_target(value: &str) -> Option<PerfScriptTarget> {
    let mut fields = HashMap::new();
    for part in value
        .split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
    {
        let Some((key, value)) = part.split_once('=') else {
            continue;
        };
        fields.insert(key.trim().to_string(), value.trim().to_string());
    }

    let sandbox_id = fields
        .remove("sandbox")
        .or_else(|| fields.remove("sandboxId"))?;
    let script_path = fields
        .remove("path")
        .or_else(|| fields.remove("scriptPath"))
        .map(PathBuf::from);
    let command = fields.remove("command").or_else(|| fields.remove("cmd"));
    if script_path.is_none() && command.is_none() {
        return None;
    }

    Some(PerfScriptTarget {
        sandbox_id,
        script_path,
        object_uri: fields
            .remove("objectUri")
            .or_else(|| fields.remove("uri"))
            .unwrap_or_default(),
        profile_type: fields
            .remove("profileType")
            .or_else(|| fields.remove("type"))
            .unwrap_or_else(|| "cpu".to_string()),
        process_role: fields
            .remove("processRole")
            .or_else(|| fields.remove("role"))
            .unwrap_or_else(|| "app".to_string()),
        command,
        pid: fields
            .remove("pid")
            .and_then(|value| value.parse::<u64>().ok()),
    })
}

fn persist_perf_script(script: &str, target: &PerfScriptTarget, timestamp: &str) -> Result<String> {
    if !target.object_uri.is_empty() {
        return Ok(target.object_uri.clone());
    }
    let output_dir = perf_script_output_dir()
        .join(&target.sandbox_id)
        .join(sanitize_id(timestamp));
    fs::create_dir_all(&output_dir)?;
    let artifact_path = output_dir.join(
        target
            .script_path
            .as_ref()
            .and_then(|path| path.file_name())
            .and_then(|name| name.to_str())
            .unwrap_or("perf.script"),
    );
    fs::write(&artifact_path, script)?;
    Ok(file_uri(&artifact_path))
}

fn run_perf_script_command(command: &str, timeout: Duration) -> Result<String> {
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
                    plugin: "perf-script".to_string(),
                    message: format!(
                        "perf script command exited with status {:?}: {}",
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
                plugin: "perf-script".to_string(),
                message: format!(
                    "perf script command timed out after {} ms",
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

fn env_path(name: &str) -> Option<PathBuf> {
    env::var(name)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .map(PathBuf::from)
}

fn env_string(name: &str) -> Option<String> {
    env::var(name).ok().filter(|value| !value.trim().is_empty())
}

fn perf_script_output_dir() -> PathBuf {
    PathBuf::from(
        env::var("RUNTIMEPULSE_PERF_SCRIPT_OUTPUT_DIR")
            .unwrap_or_else(|_| DEFAULT_OUTPUT_DIR.to_string()),
    )
}

pub fn perf_script_duration_ms() -> f64 {
    env::var("RUNTIMEPULSE_PERF_SCRIPT_DURATION_MS")
        .ok()
        .and_then(|value| value.parse::<f64>().ok())
        .unwrap_or(0.0)
        .max(0.0)
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
    fn folds_perf_script_samples() {
        let folded = folded_stacks_from_perf_script(
            "demo 123 100.000: cycles:\n        ffffffff8101 start_kernel ([kernel.kallsyms])\n        000000000040 work (/usr/bin/demo)\n\ndemo 123 101.000: cycles:\n        ffffffff8101 start_kernel ([kernel.kallsyms])\n        000000000040 work (/usr/bin/demo)\n",
        );

        assert_eq!(folded, "work;start_kernel 2");
    }

    #[test]
    fn summarizes_perf_script_target_metadata_and_duration() {
        let summary = summarize_perf_script(
            "demo 123/123 100.000: cycles:
        ffffffff8101 start_kernel ([kernel.kallsyms])
        000000000040 work (/usr/bin/demo)

demo 123/123 101.250: cycles:
        ffffffff8102 finish_task_switch ([kernel.kallsyms])
        000000000041 wait (/usr/bin/demo)
",
        );

        assert_eq!(summary.command.as_deref(), Some("demo"));
        assert_eq!(summary.pid, Some(123));
        assert_eq!(summary.sample_headers, 2);
        assert_eq!(summary.duration_ms, Some(1250.0));
        assert_eq!(summary.folded.lines().count(), 2);
    }

    #[test]
    fn converts_perf_script_to_profile_output() {
        let target = PerfScriptTarget {
            sandbox_id: "sandbox-a".to_string(),
            script_path: Some(PathBuf::from("/tmp/runtimepulse-test.perf-script")),
            object_uri: "file:///tmp/runtimepulse-test.perf-script".to_string(),
            profile_type: "cpu".to_string(),
            process_role: "app".to_string(),
            command: None,
            pid: Some(123),
        };
        let report = lightweight_report_from_perf_script(
            "demo 123 100.000: cycles:\n        ffffffff8101 start_kernel ([kernel.kallsyms])\n        000000000040 work (/usr/bin/demo)\n",
            &target,
            DateTime::parse_from_rfc3339("2026-05-25T00:00:00Z")
                .unwrap()
                .with_timezone(&Utc),
            &test_config(),
        )
        .unwrap();
        let output = profile_output_from_content_with_plugin(
            &report,
            DateTime::parse_from_rfc3339("2026-05-25T00:00:00Z")
                .unwrap()
                .with_timezone(&Utc),
            &test_config(),
            "perf-script",
        )
        .unwrap();

        assert_eq!(output.profiles.len(), 1);
        assert_eq!(output.profiles[0].sample_count, 1);
        assert_eq!(output.profiles[0].process_role, "app");
        assert_eq!(
            output.events[0].attributes["profile.metadata"]["target.command"],
            "demo"
        );
        assert_eq!(
            output.events[0].attributes["profile.metadata"]["target.pid"],
            123
        );
        assert!(output
            .metrics
            .iter()
            .any(|metric| metric.name == "profile.samples_total" && metric.value == 1.0));
    }
}
