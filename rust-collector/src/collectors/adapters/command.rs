//! Command adapter for external collectors.

use std::process::Command;

use chrono::{DateTime, Utc};

use crate::collectors::core::config::CollectorConfig;
use crate::collectors::core::error::{CollectorError, Result};
use crate::collectors::core::model::PluginOutput;
use crate::collectors::core::plugin::CollectorPlugin;

pub struct CommandPlugin {
    pub name: String,
    pub command: String,
}

impl CollectorPlugin for CommandPlugin {
    fn name(&self) -> &str {
        &self.name
    }

    fn collect(&mut self, _now: DateTime<Utc>, _config: &CollectorConfig) -> Result<PluginOutput> {
        let output = Command::new("sh").arg("-lc").arg(&self.command).output()?;

        if !output.status.success() {
            return Err(CollectorError::Plugin {
                plugin: self.name.clone(),
                message: String::from_utf8_lossy(&output.stderr).trim().to_string(),
            });
        }

        Ok(serde_json::from_slice(&output.stdout)?)
    }
}
