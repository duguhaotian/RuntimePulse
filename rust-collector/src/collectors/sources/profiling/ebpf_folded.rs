//! eBPF folded/off-CPU profile source.
//!
//! Many eBPF profilers can export folded stacks for CPU, off-CPU, memory, or
//! syscall profiles. This source reuses the folded-stack flamegraph converter
//! and defaults to off-CPU/runtime-oriented metadata.

use chrono::{DateTime, Utc};
use std::env;
use std::fs;
use std::path::PathBuf;

use crate::collectors::core::config::CollectorConfig;
use crate::collectors::core::error::Result;
use crate::collectors::core::model::PluginOutput;
use crate::collectors::core::plugin::CollectorPlugin;
use crate::collectors::sources::profiling::perf_folded::{
    file_uri, flamegraph_from_folded_stacks, is_missing_folded_path, parse_folded_target,
    perf_folded_duration_ms, read_folded_target, sanitize_id, timestamp, FoldedProfileTarget,
};
use crate::collectors::sources::profiling::report::{
    merge_profile_output, profile_output_from_content_with_plugin,
};
use serde_json::json;

const DEFAULT_OUTPUT_DIR: &str = "/tmp/runtimepulse/profiles/ebpf-folded";

pub struct EbpfFoldedProfilePlugin {
    path: Option<PathBuf>,
}

impl EbpfFoldedProfilePlugin {
    pub fn new(path: Option<PathBuf>) -> Self {
        Self { path }
    }
}

impl CollectorPlugin for EbpfFoldedProfilePlugin {
    fn name(&self) -> &str {
        "ebpf-folded"
    }

    fn collect(&mut self, now: DateTime<Utc>, config: &CollectorConfig) -> Result<PluginOutput> {
        collect_ebpf_folded_profiles_with_path(now, config, self.path.clone())
    }
}

pub fn emit_ebpf_folded_profiles(config: &CollectorConfig) -> Result<()> {
    let output = collect_ebpf_folded_profiles(Utc::now(), config)?;
    println!("{}", serde_json::to_string(&output)?);
    Ok(())
}

pub fn collect_ebpf_folded_profiles(
    now: DateTime<Utc>,
    config: &CollectorConfig,
) -> Result<PluginOutput> {
    collect_ebpf_folded_profiles_with_path(now, config, config.ebpf_folded_path.clone())
}

fn collect_ebpf_folded_profiles_with_path(
    now: DateTime<Utc>,
    config: &CollectorConfig,
    fallback_path: Option<PathBuf>,
) -> Result<PluginOutput> {
    let mut output = PluginOutput::default();
    for target in ebpf_folded_targets(fallback_path) {
        let folded = match read_folded_target(&target) {
            Ok(content) => content,
            Err(error) if is_missing_folded_path(&error) => continue,
            Err(error) => return Err(error),
        };
        if folded.trim().is_empty() {
            continue;
        }
        let report = lightweight_ebpf_report_from_folded(&folded, &target, now, config)?;
        merge_profile_output(
            &mut output,
            profile_output_from_content_with_plugin(&report, now, config, "ebpf-folded")?,
        );
    }
    Ok(output)
}

fn lightweight_ebpf_report_from_folded(
    folded: &str,
    target: &FoldedProfileTarget,
    now: DateTime<Utc>,
    config: &CollectorConfig,
) -> Result<String> {
    let ts = timestamp(now);
    let (sample_count, flamegraph) = flamegraph_from_folded_stacks(folded);
    let duration_ms = ebpf_folded_duration_ms();
    let output_dir = ebpf_folded_output_dir()
        .join(&target.sandbox_id)
        .join(sanitize_id(&ts));
    fs::create_dir_all(&output_dir)?;
    let artifact_path = output_dir.join(
        target
            .folded_path
            .as_ref()
            .and_then(|path| path.file_name())
            .and_then(|name| name.to_str())
            .unwrap_or("ebpf.folded"),
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
        "ebpf-folded-{}-{}-{}",
        sanitize_id(&target.sandbox_id),
        sanitize_id(&target.profile_type),
        sanitize_id(&ts)
    );

    Ok(json!({
        "sandboxId": target.sandbox_id,
        "source": "ebpf-folded",
        "timestamp": ts,
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
                "collector": "ebpf-folded",
                "node": config.node_id,
            },
            "attributes": {
                "profile.sourcePath": target.folded_path,
                "profile.command": target.command,
                "profile.folded": true,
                "profile.backend": "ebpf",
            }
        }]
    })
    .to_string())
}

fn ebpf_folded_targets(fallback_path: Option<PathBuf>) -> Vec<FoldedProfileTarget> {
    if let Ok(value) = env::var("RUNTIMEPULSE_EBPF_FOLDED_TARGETS") {
        let targets = value
            .split(';')
            .filter_map(parse_ebpf_folded_target)
            .collect::<Vec<_>>();
        if !targets.is_empty() {
            return targets;
        }
    }

    let path = fallback_path.or_else(|| {
        env::var("RUNTIMEPULSE_EBPF_FOLDED_PATH")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .map(PathBuf::from)
    });
    let command = env::var("RUNTIMEPULSE_EBPF_FOLDED_CMD")
        .ok()
        .filter(|value| !value.trim().is_empty());
    if path.is_none() && command.is_none() {
        return Vec::new();
    }
    vec![FoldedProfileTarget {
        sandbox_id: env::var("RUNTIMEPULSE_EBPF_FOLDED_SANDBOX_ID")
            .unwrap_or_else(|_| "host-ebpf-folded".to_string()),
        folded_path: path,
        command,
        object_uri: env::var("RUNTIMEPULSE_EBPF_FOLDED_OBJECT_URI").unwrap_or_default(),
        profile_type: env::var("RUNTIMEPULSE_EBPF_FOLDED_PROFILE_TYPE")
            .unwrap_or_else(|_| "off_cpu".to_string()),
        process_role: env::var("RUNTIMEPULSE_EBPF_FOLDED_PROCESS_ROLE")
            .unwrap_or_else(|_| "runtime".to_string()),
    }]
}

fn parse_ebpf_folded_target(value: &str) -> Option<FoldedProfileTarget> {
    let mut target = parse_folded_target(value)?;
    if target.profile_type == "cpu" && !value.contains("profileType=") && !value.contains("type=") {
        target.profile_type = "off_cpu".to_string();
    }
    if target.process_role == "app" && !value.contains("processRole=") && !value.contains("role=") {
        target.process_role = "runtime".to_string();
    }
    Some(target)
}

fn ebpf_folded_output_dir() -> PathBuf {
    PathBuf::from(
        env::var("RUNTIMEPULSE_EBPF_FOLDED_OUTPUT_DIR")
            .unwrap_or_else(|_| DEFAULT_OUTPUT_DIR.to_string()),
    )
}

fn ebpf_folded_duration_ms() -> f64 {
    env::var("RUNTIMEPULSE_EBPF_FOLDED_DURATION_MS")
        .ok()
        .and_then(|value| value.parse::<f64>().ok())
        .unwrap_or_else(perf_folded_duration_ms)
        .max(0.0)
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
    fn converts_ebpf_folded_to_off_cpu_profile() {
        let target = FoldedProfileTarget {
            sandbox_id: "sandbox-a".to_string(),
            folded_path: Some(PathBuf::from("/tmp/ebpf.folded")),
            command: None,
            object_uri: "file:///tmp/ebpf.folded".to_string(),
            profile_type: "off_cpu".to_string(),
            process_role: "runtime".to_string(),
        };
        let report = lightweight_ebpf_report_from_folded(
            "runtime;futex_wait 5\nruntime;io_schedule 7\n",
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
            "ebpf-folded",
        )
        .unwrap();

        assert_eq!(output.profiles.len(), 1);
        assert_eq!(output.profiles[0].profile_type, "off_cpu");
        assert_eq!(output.profiles[0].sample_count, 12);
        assert_eq!(output.events[0].event_name, "profile.off_cpu.observed");
    }
}
