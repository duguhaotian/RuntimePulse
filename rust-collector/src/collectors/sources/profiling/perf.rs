//! perf profile artifact source.
//!
//! This source ingests profile artifact indexes written by host-side perf
//! tooling. RuntimePulse can either read a profiler-exported JSON/JSONL report
//! file or execute a configured perf exporter command that writes compatible
//! JSON to stdout.

use chrono::{DateTime, Utc};
use std::fs;
use std::path::PathBuf;
use std::time::Duration;

use crate::collectors::core::config::CollectorConfig;
use crate::collectors::core::error::Result;
use crate::collectors::core::model::PluginOutput;
use crate::collectors::core::plugin::CollectorPlugin;
use crate::collectors::sources::profiling::report::{
    merge_profile_output, profile_output_from_command, profile_output_from_content_with_plugin,
};

pub struct PerfProfilePlugin {
    path: Option<PathBuf>,
    command: Option<String>,
    timeout: Duration,
}

impl PerfProfilePlugin {
    pub fn new(path: Option<PathBuf>, command: Option<String>, timeout: Duration) -> Self {
        Self {
            path,
            command,
            timeout,
        }
    }
}

impl CollectorPlugin for PerfProfilePlugin {
    fn name(&self) -> &str {
        "perf"
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
                merge_profile_output(
                    &mut output,
                    profile_output_from_content_with_plugin(&content, now, config, "perf")?,
                );
            }
        }

        if let Some(command) = self.command.clone() {
            merge_profile_output(
                &mut output,
                profile_output_from_command(&command, self.timeout, now, config, "perf")?,
            );
        }

        Ok(output)
    }
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
    fn perf_plugin_uses_perf_node_label() {
        let output = profile_output_from_content_with_plugin(
            r#"{"sandboxId":"sandbox-a","source":"perf","profiles":[{"profileType":"cpu","objectUri":"file:///tmp/a.perf"}]}"#,
            Utc::now(),
            &test_config(),
            "perf",
        )
        .unwrap();

        assert_eq!(output.profiles.len(), 1);
        assert_eq!(output.metadata.nodes[0]["labels"]["plugin"], "perf");
    }

    #[test]
    fn perf_plugin_collects_command_output() {
        let mut plugin = PerfProfilePlugin::new(
            None,
            Some("printf '%s' '{\"sandboxId\":\"sandbox-a\",\"source\":\"perf\",\"profiles\":[{\"profileType\":\"cpu\",\"sampleCount\":7,\"objectUri\":\"file:///tmp/a.perf\"}]}'".to_string()),
            Duration::from_secs(1),
        );

        let output = plugin.collect(Utc::now(), &test_config()).unwrap();

        assert_eq!(output.profiles.len(), 1);
        assert_eq!(output.profiles[0].sample_count, 7);
        assert_eq!(output.metadata.nodes[0]["labels"]["plugin"], "perf");
    }
}
