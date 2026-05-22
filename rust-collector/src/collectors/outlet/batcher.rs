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
    let collector_source = format!("runtimepulse-rust-collector/{}", config.node_id);
    let observed_at = timestamp(now);
    let mut batch = IngestBatch {
        source: collector_source.clone(),
        observed_at: observed_at.clone(),
        metadata: Metadata::default(),
        metrics: Vec::new(),
        events: Vec::new(),
        traces: Vec::new(),
        profiles: Vec::new(),
    };
    let mut summary = BatchSummary {
        source: collector_source.clone(),
        submitted: false,
        local_reports: 0,
        metrics: 0,
        events: 0,
        traces: 0,
        profiles: 0,
    };
    let mut submitted_sources = Vec::new();

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

    if has_batch_payload(&batch) {
        send_batch(client, config, &batch)?;
        summary.submitted = true;
        summary.metrics += batch.metrics.len();
        summary.events += batch.events.len();
        summary.traces += batch.traces.len();
        summary.profiles += batch.profiles.len();
        submitted_sources.push(batch.source.clone());
    }

    while let Ok(mut output) = local_reports.try_recv() {
        summary.local_reports += 1;
        let source = output
            .source
            .take()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| collector_source.clone());
        let mut local_batch = IngestBatch {
            source,
            observed_at: observed_at.clone(),
            metadata: Metadata::default(),
            metrics: Vec::new(),
            events: Vec::new(),
            traces: Vec::new(),
            profiles: Vec::new(),
        };
        merge_output(&mut local_batch, output, now, config);
        if !has_batch_payload(&local_batch) {
            continue;
        }

        send_batch(client, config, &local_batch)?;
        summary.submitted = true;
        summary.metrics += local_batch.metrics.len();
        summary.events += local_batch.events.len();
        summary.traces += local_batch.traces.len();
        summary.profiles += local_batch.profiles.len();
        submitted_sources.push(local_batch.source);
    }

    summary.source = match submitted_sources.as_slice() {
        [] => collector_source,
        [source] => source.clone(),
        _ => "multiple".to_string(),
    };

    Ok(summary)
}

fn timestamp(time: DateTime<Utc>) -> String {
    time.to_rfc3339_opts(SecondsFormat::Millis, true)
}
