//! eBPF profile artifact source.
//!
//! This source ingests profile artifact indexes written by eBPF profilers. The
//! eBPF agent owns kernel attachment and profile capture; RuntimePulse forwards
//! the normalized artifact metadata through the shared outlet path.

use chrono::{DateTime, Utc};
use std::fs;
use std::path::PathBuf;

use crate::collectors::core::config::CollectorConfig;
use crate::collectors::core::error::Result;
use crate::collectors::core::model::PluginOutput;
use crate::collectors::core::plugin::CollectorPlugin;
use crate::collectors::sources::profiling::report::profile_output_from_content_with_plugin;

pub struct EbpfProfilePlugin {
    path: Option<PathBuf>,
}

impl EbpfProfilePlugin {
    pub fn new(path: Option<PathBuf>) -> Self {
        Self { path }
    }
}

impl CollectorPlugin for EbpfProfilePlugin {
    fn name(&self) -> &str {
        "ebpf"
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

        profile_output_from_content_with_plugin(&content, now, config, "ebpf")
    }
}
