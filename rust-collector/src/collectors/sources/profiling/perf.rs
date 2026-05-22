//! perf profile artifact source.
//!
//! This source ingests profile artifact indexes written by host-side perf
//! tooling. It does not start `perf record`; the profiler owns collection and
//! file creation, while RuntimePulse normalizes the exported artifact metadata.

use chrono::{DateTime, Utc};
use std::fs;
use std::path::PathBuf;

use crate::collectors::core::config::CollectorConfig;
use crate::collectors::core::error::Result;
use crate::collectors::core::model::PluginOutput;
use crate::collectors::core::plugin::CollectorPlugin;
use crate::collectors::sources::profiling::report::profile_output_from_content_with_plugin;

pub struct PerfProfilePlugin {
    path: Option<PathBuf>,
}

impl PerfProfilePlugin {
    pub fn new(path: Option<PathBuf>) -> Self {
        Self { path }
    }
}

impl CollectorPlugin for PerfProfilePlugin {
    fn name(&self) -> &str {
        "perf"
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

        profile_output_from_content_with_plugin(&content, now, config, "perf")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn perf_plugin_uses_perf_node_label() {
        let config = CollectorConfig {
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
            plugins: Vec::new(),
            command_plugins: Vec::new(),
            http_plugins: Vec::new(),
        };

        let output = profile_output_from_content_with_plugin(
            r#"{"sandboxId":"sandbox-a","source":"perf","profiles":[{"profileType":"cpu","objectUri":"file:///tmp/a.perf"}]}"#,
            Utc::now(),
            &config,
            "perf",
        )
        .unwrap();

        assert_eq!(output.profiles.len(), 1);
        assert_eq!(output.metadata.nodes[0]["labels"]["plugin"], "perf");
    }
}
