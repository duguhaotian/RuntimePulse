//! eBPF profile artifact source.
//!
//! This source ingests profile artifact indexes written by eBPF profilers. The
//! eBPF agent owns kernel attachment and profile capture; RuntimePulse can read
//! its exported JSON/JSONL report file or execute a configured exporter command
//! and normalize stdout through the shared outlet path.

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

pub struct EbpfProfilePlugin {
    path: Option<PathBuf>,
    command: Option<String>,
    timeout: Duration,
}

impl EbpfProfilePlugin {
    pub fn new(path: Option<PathBuf>, command: Option<String>, timeout: Duration) -> Self {
        Self {
            path,
            command,
            timeout,
        }
    }
}

impl CollectorPlugin for EbpfProfilePlugin {
    fn name(&self) -> &str {
        "ebpf"
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
                    profile_output_from_content_with_plugin(&content, now, config, "ebpf")?,
                );
            }
        }

        if let Some(command) = self.command.clone() {
            merge_profile_output(
                &mut output,
                profile_output_from_command(&command, self.timeout, now, config, "ebpf")?,
            );
        }

        Ok(output)
    }
}
