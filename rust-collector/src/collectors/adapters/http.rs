//! HTTP adapter for external collectors.

use chrono::{DateTime, Utc};
use reqwest::blocking::Client;

use crate::collectors::core::config::CollectorConfig;
use crate::collectors::core::error::Result;
use crate::collectors::core::model::PluginOutput;
use crate::collectors::core::plugin::CollectorPlugin;

pub struct HttpPlugin {
    pub name: String,
    pub url: String,
    pub client: Client,
}

impl CollectorPlugin for HttpPlugin {
    fn name(&self) -> &str {
        &self.name
    }

    fn collect(&mut self, _now: DateTime<Utc>, _config: &CollectorConfig) -> Result<PluginOutput> {
        Ok(self
            .client
            .get(&self.url)
            .send()?
            .error_for_status()?
            .json()?)
    }
}
