//! Report construction and normalization helpers.

use chrono::{DateTime, SecondsFormat, Utc};

use super::config::CollectorConfig;
use super::model::{IngestBatch, MetricSample, PluginOutput};

pub fn merge_output(
    batch: &mut IngestBatch,
    output: PluginOutput,
    now: DateTime<Utc>,
    config: &CollectorConfig,
) {
    batch.metadata.clusters.extend(output.metadata.clusters);
    batch.metadata.nodes.extend(output.metadata.nodes);
    batch.metadata.images.extend(output.metadata.images);
    batch.metadata.sandboxes.extend(output.metadata.sandboxes);

    let ts = timestamp(now);

    batch
        .metrics
        .extend(output.metrics.into_iter().map(|mut metric| {
            if metric.timestamp.is_empty() {
                metric.timestamp = ts.clone();
            }
            metric
        }));

    batch
        .events
        .extend(output.events.into_iter().map(|mut event| {
            if event.timestamp.is_empty() {
                event.timestamp = ts.clone();
            }
            if event.source.is_empty() {
                event.source = format!("runtimepulse-rust-collector/{}", config.node_id);
            }
            event
        }));

    batch.traces.extend(output.traces);
    batch.profiles.extend(output.profiles);
}

pub fn has_batch_payload(batch: &IngestBatch) -> bool {
    !batch.metadata.clusters.is_empty()
        || !batch.metadata.nodes.is_empty()
        || !batch.metadata.images.is_empty()
        || !batch.metadata.sandboxes.is_empty()
        || !batch.metrics.is_empty()
        || !batch.events.is_empty()
        || !batch.traces.is_empty()
        || !batch.profiles.is_empty()
}

pub fn metric(
    timestamp: &str,
    name: &str,
    value: f64,
    unit: &str,
    group: &str,
    node_id: &str,
    sandbox_id: &str,
    runtime_type: &str,
) -> MetricSample {
    MetricSample {
        timestamp: timestamp.to_string(),
        name: name.to_string(),
        value,
        unit: Some(unit.to_string()),
        group: Some(group.to_string()),
        sandbox_id: Some(sandbox_id.to_string()),
        node_id: Some(node_id.to_string()),
        image_id: None,
        runtime_type: Some(runtime_type.to_string()),
        attributes: None,
    }
}

pub fn node_metric(
    timestamp: &str,
    name: &str,
    value: f64,
    unit: &str,
    group: &str,
    node_id: &str,
) -> MetricSample {
    MetricSample {
        timestamp: timestamp.to_string(),
        name: name.to_string(),
        value,
        unit: Some(unit.to_string()),
        group: Some(group.to_string()),
        sandbox_id: None,
        node_id: Some(node_id.to_string()),
        image_id: None,
        runtime_type: None,
        attributes: None,
    }
}

fn timestamp(time: DateTime<Utc>) -> String {
    time.to_rfc3339_opts(SecondsFormat::Millis, true)
}
