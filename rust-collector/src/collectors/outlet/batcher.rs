//! Outlet batch construction and delivery orchestration.

use std::sync::mpsc::Receiver;

use chrono::{DateTime, SecondsFormat, Utc};
use reqwest::blocking::Client;

use crate::collectors::core::config::CollectorConfig;
use crate::collectors::core::error::{CollectorError, Result};
use crate::collectors::core::model::{IngestBatch, Metadata, PluginOutput};
use crate::collectors::core::plugin::CollectorPlugin;
use crate::collectors::core::report::{has_batch_payload, merge_output};
use crate::collectors::outlet::sender::send_batch;

pub struct BatchSummary {
    pub source: String,
    pub submitted: bool,
    pub local_reports: usize,
    pub metrics: usize,
    pub events: usize,
    pub traces: usize,
    pub profiles: usize,
}

pub fn collect_once(
    client: &Client,
    config: &CollectorConfig,
    plugins: &mut [Box<dyn CollectorPlugin>],
    local_reports: &Receiver<PluginOutput>,
    now: DateTime<Utc>,
) -> Result<BatchSummary> {
    let mut batch = IngestBatch {
        source: format!("runtimepulse-rust-collector/{}", config.node_id),
        observed_at: timestamp(now),
        metadata: Metadata::default(),
        metrics: Vec::new(),
        events: Vec::new(),
        traces: Vec::new(),
        profiles: Vec::new(),
    };

    for plugin in plugins {
        let plugin_name = plugin.name().to_string();
        let output = plugin
            .collect(now, config)
            .map_err(|error| CollectorError::Plugin {
                plugin: plugin_name,
                message: error.to_string(),
            })?;
        merge_output(&mut batch, output, now, config);
    }

    let mut local_report_count = 0;
    while let Ok(output) = local_reports.try_recv() {
        local_report_count += 1;
        merge_output(&mut batch, output, now, config);
    }

    if !has_batch_payload(&batch) {
        return Ok(BatchSummary {
            source: batch.source,
            submitted: false,
            local_reports: local_report_count,
            metrics: 0,
            events: 0,
            traces: 0,
            profiles: 0,
        });
    }

    send_batch(client, config, &batch)?;

    Ok(BatchSummary {
        source: batch.source,
        submitted: true,
        local_reports: local_report_count,
        metrics: batch.metrics.len(),
        events: batch.events.len(),
        traces: batch.traces.len(),
        profiles: batch.profiles.len(),
    })
}

fn timestamp(time: DateTime<Utc>) -> String {
    time.to_rfc3339_opts(SecondsFormat::Millis, true)
}
