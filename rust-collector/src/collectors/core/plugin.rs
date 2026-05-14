//! Collector plugin trait and lifecycle boundary.

use chrono::{DateTime, Utc};

use super::config::CollectorConfig;
use super::error::Result;
use super::model::PluginOutput;

pub trait CollectorPlugin {
    fn name(&self) -> &str;
    fn collect(&mut self, now: DateTime<Utc>, config: &CollectorConfig) -> Result<PluginOutput>;
}
