//! Snapshotter image cache source.
//!
//! This source does not infer cache behavior from Docker metadata. It reads
//! RuntimePulse-shaped reports emitted by real snapshotter/cache tools, such as
//! nydus, stargz, overlaybd, or a local cache exporter.

use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{json, Map, Value};
use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;

use crate::collectors::core::config::CollectorConfig;
use crate::collectors::core::error::Result;
use crate::collectors::core::model::{EventRecord, MetricSample, PluginOutput, TraceSpan};
use crate::collectors::core::plugin::CollectorPlugin;
use crate::collectors::core::report::image_metric;
use crate::collectors::sources::image::layer::docker_image_id_from_ref_or_digest;

#[derive(Default)]
pub struct ImageCachePlugin {
    path: Option<PathBuf>,
    seen_event_ids: HashSet<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SnapshotterReport {
    image_id: Option<String>,
    image_ref: Option<String>,
    image_digest: Option<String>,
    loading_mode: Option<String>,
    size_bytes: Option<u64>,
    layer_count: Option<u64>,
    timestamp: Option<String>,
    snapshotter: Option<String>,
    cache: Option<CacheReport>,
    #[serde(default)]
    layers: Vec<LayerCacheReport>,
    #[serde(default)]
    prefetches: Vec<PrefetchReport>,
    download_timeline: Option<Vec<DownloadStepReport>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CacheReport {
    requested_blocks: Option<u64>,
    hit_blocks: Option<u64>,
    local_read_bytes: Option<u64>,
    remote_read_bytes: Option<u64>,
    block_size_bytes: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LayerCacheReport {
    id: Option<String>,
    digest: Option<String>,
    media_type: Option<String>,
    size_bytes: Option<u64>,
    requested_blocks: Option<u64>,
    hit_blocks: Option<u64>,
    local_read_bytes: Option<u64>,
    remote_read_bytes: Option<u64>,
    block_size_bytes: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PrefetchReport {
    id: Option<String>,
    name: Option<String>,
    phase: Option<String>,
    started_at: Option<String>,
    duration_ms: Option<f64>,
    bytes: Option<u64>,
    hit_blocks: Option<u64>,
    requested_blocks: Option<u64>,
    detail: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DownloadStepReport {
    id: Option<String>,
    name: String,
    phase: String,
    duration_ms: f64,
    bytes: Option<u64>,
    timestamp: Option<String>,
    detail: Option<String>,
}

impl ImageCachePlugin {
    pub fn new(path: Option<PathBuf>) -> Self {
        Self {
            path,
            seen_event_ids: HashSet::new(),
        }
    }
}

impl CollectorPlugin for ImageCachePlugin {
    fn name(&self) -> &str {
        "image-cache"
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

        let reports = parse_reports(&content)?;
        let mut output = PluginOutput::default();
        let fallback_timestamp = timestamp(now);

        for report in reports {
            let timestamp = report.timestamp.as_deref().unwrap_or(&fallback_timestamp);
            let image_ref = report
                .image_ref
                .clone()
                .unwrap_or_else(|| "snapshotter/unknown:latest".to_string());
            let image_digest = report
                .image_digest
                .clone()
                .unwrap_or_else(|| format!("collector:{}", sanitize_id(&image_ref)));
            let image_id = report
                .image_id
                .clone()
                .unwrap_or_else(|| docker_image_id_from_ref_or_digest(&image_ref, &image_digest));
            let snapshotter = report.snapshotter.as_deref().unwrap_or("snapshotter");
            let timeline_steps = normalized_timeline_steps(&report, timestamp);
            let timeline = timeline_rows(&image_id, &timeline_steps);
            let layers = layer_rows(&report.layers);
            let layer_summary = layer_cache_summary(&report.layers);
            let prefetch_summary = prefetch_summary(&report.prefetches);
            let layer_count = report.layer_count.unwrap_or(layers.len() as u64);
            let size_bytes = report.size_bytes.unwrap_or_else(|| {
                report
                    .layers
                    .iter()
                    .filter_map(|layer| layer.size_bytes)
                    .sum()
            });

            output.metadata.images.push(json!({
                "id": image_id,
                "ref": image_ref,
                "digest": image_digest,
                "loadingMode": report.loading_mode.unwrap_or_else(|| "lazy".to_string()),
                "sizeBytes": size_bytes,
                "layerCount": layer_count,
                "layers": layers,
                "downloadTimeline": timeline,
                "attributes": {
                    "collector.plugin": "image-cache",
                    "snapshotter": snapshotter,
                    "snapshotter.reportPath": path.display().to_string(),
                    "snapshotter.prefetches": report.prefetches.len(),
                    "snapshotter.layers": report.layers.len(),
                    "snapshotter.layerRequestedBlocks": layer_summary.requested_blocks,
                    "snapshotter.layerHitBlocks": layer_summary.hit_blocks,
                    "snapshotter.layerRemoteReadBytes": layer_summary.remote_read_bytes,
                    "snapshotter.layerLocalReadBytes": layer_summary.local_read_bytes,
                    "snapshotter.prefetchBytes": prefetch_summary.bytes,
                    "snapshotter.prefetchRequestedBlocks": prefetch_summary.requested_blocks,
                    "snapshotter.prefetchHitBlocks": prefetch_summary.hit_blocks,
                }
            }));

            if let Some(cache) = &report.cache {
                output.metrics.extend(cache_metrics(
                    timestamp,
                    &config.node_id,
                    &image_id,
                    snapshotter,
                    cache,
                ));
            }
            output.metrics.extend(layer_cache_metrics(
                timestamp,
                &config.node_id,
                &image_id,
                snapshotter,
                &report.layers,
            ));
            output.metrics.extend(prefetch_metrics(
                timestamp,
                &config.node_id,
                &image_id,
                snapshotter,
                &report.prefetches,
            ));

            for span in timeline_spans(
                timestamp,
                &image_id,
                &image_ref,
                snapshotter,
                &timeline_steps,
            ) {
                let event_id = format!("{}-event", span.span_id);
                if self.seen_event_ids.insert(event_id.clone()) {
                    output.events.push(timeline_event(
                        &event_id,
                        timestamp,
                        &config.node_id,
                        &image_id,
                        &image_ref,
                        snapshotter,
                        &span,
                    ));
                }
                output.traces.push(span);
            }
        }

        if !output.metadata.images.is_empty() {
            output.metadata.clusters.push(json!({
                "id": config.cluster_id,
                "name": config.cluster_id,
                "environment": "collector"
            }));
            output.metadata.nodes.push(json!({
                "id": config.node_id,
                "clusterId": config.cluster_id,
                "name": config.node_id,
                "status": "ready",
                "labels": {
                    "collector": "runtimepulse-rust-collector",
                    "plugin": "image-cache",
                    "scope": config.collection_scope,
                }
            }));
        }

        Ok(output)
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct CacheSummary {
    requested_blocks: u64,
    hit_blocks: u64,
    local_read_bytes: u64,
    remote_read_bytes: u64,
}

#[derive(Clone, Copy, Debug, Default)]
struct PrefetchSummary {
    bytes: u64,
    requested_blocks: u64,
    hit_blocks: u64,
}

fn layer_cache_summary(layers: &[LayerCacheReport]) -> CacheSummary {
    CacheSummary {
        requested_blocks: layers
            .iter()
            .filter_map(|layer| layer.requested_blocks)
            .sum(),
        hit_blocks: layers.iter().filter_map(|layer| layer.hit_blocks).sum(),
        local_read_bytes: layers
            .iter()
            .filter_map(|layer| layer.local_read_bytes)
            .sum(),
        remote_read_bytes: layers
            .iter()
            .filter_map(|layer| layer.remote_read_bytes)
            .sum(),
    }
}

fn prefetch_summary(prefetches: &[PrefetchReport]) -> PrefetchSummary {
    PrefetchSummary {
        bytes: prefetches
            .iter()
            .filter_map(|prefetch| prefetch.bytes)
            .sum(),
        requested_blocks: prefetches
            .iter()
            .filter_map(|prefetch| prefetch.requested_blocks)
            .sum(),
        hit_blocks: prefetches
            .iter()
            .filter_map(|prefetch| prefetch.hit_blocks)
            .sum(),
    }
}

fn normalized_timeline_steps(
    report: &SnapshotterReport,
    fallback_timestamp: &str,
) -> Vec<DownloadStepReport> {
    let mut steps = report.download_timeline.clone().unwrap_or_default();
    steps.extend(
        report
            .prefetches
            .iter()
            .enumerate()
            .map(|(index, prefetch)| {
                let name = prefetch
                    .name
                    .clone()
                    .unwrap_or_else(|| format!("Prefetch {}", index + 1));
                DownloadStepReport {
                    id: prefetch
                        .id
                        .clone()
                        .or_else(|| Some(format!("prefetch-{index}"))),
                    name,
                    phase: prefetch
                        .phase
                        .clone()
                        .unwrap_or_else(|| "prefetch".to_string()),
                    duration_ms: prefetch.duration_ms.unwrap_or(0.0),
                    bytes: prefetch.bytes,
                    timestamp: prefetch
                        .started_at
                        .clone()
                        .or_else(|| Some(fallback_timestamp.to_string())),
                    detail: prefetch.detail.clone(),
                }
            }),
    );
    steps
}

fn layer_rows(layers: &[LayerCacheReport]) -> Vec<Value> {
    layers
        .iter()
        .enumerate()
        .map(|(index, layer)| {
            let digest = layer
                .digest
                .clone()
                .unwrap_or_else(|| layer.id.clone().unwrap_or_else(|| format!("layer-{index}")));
            json!({
                "digest": digest,
                "sizeBytes": layer.size_bytes.unwrap_or(0),
                "command": layer.media_type.clone().unwrap_or_else(|| "snapshotter layer".to_string()),
                "attributes": {
                    "snapshotter.layerId": layer.id.clone().unwrap_or_else(|| format!("layer-{index}")),
                    "snapshotter.mediaType": layer.media_type,
                    "snapshotter.requestedBlocks": layer.requested_blocks.unwrap_or(0),
                    "snapshotter.hitBlocks": layer.hit_blocks.unwrap_or(0),
                    "snapshotter.localReadBytes": layer.local_read_bytes.unwrap_or(0),
                    "snapshotter.remoteReadBytes": layer.remote_read_bytes.unwrap_or(0),
                    "snapshotter.blockSizeBytes": layer.block_size_bytes.unwrap_or(0),
                }
            })
        })
        .collect()
}

fn layer_cache_metrics(
    timestamp: &str,
    node_id: &str,
    image_id: &str,
    snapshotter: &str,
    layers: &[LayerCacheReport],
) -> Vec<MetricSample> {
    let summary = layer_cache_summary(layers);
    let requested = summary.requested_blocks;
    let hit = summary.hit_blocks;
    let local_read = summary.local_read_bytes;
    let remote_read = summary.remote_read_bytes;
    if requested == 0 && hit == 0 && local_read == 0 && remote_read == 0 {
        return Vec::new();
    }
    let hit_ratio = if requested == 0 {
        0.0
    } else {
        hit as f64 / requested as f64
    };
    [
        ("image.lazy.layer_cache_hit_ratio", hit_ratio, "ratio"),
        (
            "image.lazy.layer_remote_read_bytes",
            remote_read as f64,
            "bytes",
        ),
        (
            "image.lazy.layer_local_read_bytes",
            local_read as f64,
            "bytes",
        ),
        (
            "image.lazy.layer_requested_blocks",
            requested as f64,
            "blocks",
        ),
        ("image.lazy.layer_hit_blocks", hit as f64, "blocks"),
    ]
    .into_iter()
    .map(|(name, value, unit)| {
        let mut metric = image_metric(timestamp, name, value, unit, "io", node_id, image_id);
        metric.attributes = Some(Map::from_iter([
            ("collector.source".to_string(), json!("image-cache")),
            ("snapshotter".to_string(), json!(snapshotter)),
            ("snapshotter.metricScope".to_string(), json!("layers")),
        ]));
        metric
    })
    .collect()
}

fn prefetch_metrics(
    timestamp: &str,
    node_id: &str,
    image_id: &str,
    snapshotter: &str,
    prefetches: &[PrefetchReport],
) -> Vec<MetricSample> {
    let summary = prefetch_summary(prefetches);
    let bytes = summary.bytes;
    let requested = summary.requested_blocks;
    let hit = summary.hit_blocks;
    if bytes == 0 && requested == 0 && hit == 0 {
        return Vec::new();
    }
    let hit_ratio = if requested == 0 {
        0.0
    } else {
        hit as f64 / requested as f64
    };
    [
        ("image.lazy.prefetch_bytes", bytes as f64, "bytes"),
        (
            "image.lazy.prefetch_requested_blocks",
            requested as f64,
            "blocks",
        ),
        ("image.lazy.prefetch_hit_blocks", hit as f64, "blocks"),
        ("image.lazy.prefetch_hit_ratio", hit_ratio, "ratio"),
    ]
    .into_iter()
    .map(|(name, value, unit)| {
        let mut metric = image_metric(timestamp, name, value, unit, "io", node_id, image_id);
        metric.attributes = Some(Map::from_iter([
            ("collector.source".to_string(), json!("image-cache")),
            ("snapshotter".to_string(), json!(snapshotter)),
            ("snapshotter.metricScope".to_string(), json!("prefetch")),
        ]));
        metric
    })
    .collect()
}

fn parse_reports(content: &str) -> Result<Vec<SnapshotterReport>> {
    let trimmed = content.trim();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }
    if trimmed.starts_with('[') {
        return Ok(serde_json::from_str(trimmed)?);
    }
    if trimmed.starts_with('{') {
        return Ok(vec![serde_json::from_str(trimmed)?]);
    }

    let mut reports = Vec::new();
    for line in trimmed
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
    {
        reports.push(serde_json::from_str(line)?);
    }
    Ok(reports)
}

fn cache_metrics(
    timestamp: &str,
    node_id: &str,
    image_id: &str,
    snapshotter: &str,
    cache: &CacheReport,
) -> Vec<MetricSample> {
    let requested = cache.requested_blocks.unwrap_or(0);
    let hit = cache.hit_blocks.unwrap_or(0);
    let hit_ratio = if requested == 0 {
        0.0
    } else {
        hit as f64 / requested as f64
    };

    [
        ("image.lazy.cache_hit_ratio", hit_ratio, "ratio"),
        (
            "image.lazy.remote_read_bytes",
            cache.remote_read_bytes.unwrap_or(0) as f64,
            "bytes",
        ),
        (
            "image.lazy.local_read_bytes",
            cache.local_read_bytes.unwrap_or(0) as f64,
            "bytes",
        ),
        ("image.lazy.requested_blocks", requested as f64, "blocks"),
        ("image.lazy.hit_blocks", hit as f64, "blocks"),
        (
            "image.lazy.block_size_bytes",
            cache.block_size_bytes.unwrap_or(0) as f64,
            "bytes",
        ),
    ]
    .into_iter()
    .map(|(name, value, unit)| {
        let mut metric = image_metric(timestamp, name, value, unit, "io", node_id, image_id);
        metric.attributes = Some(Map::from_iter([
            ("collector.source".to_string(), json!("image-cache")),
            ("snapshotter".to_string(), json!(snapshotter)),
        ]));
        metric
    })
    .collect()
}

fn timeline_rows(image_id: &str, steps: &[DownloadStepReport]) -> Vec<Value> {
    steps
        .iter()
        .enumerate()
        .map(|(index, step)| {
            json!({
                "id": step.id.clone().unwrap_or_else(|| format!("{}-{}-{}", image_id, sanitize_id(&step.phase), index)),
                "name": step.name,
                "phase": step.phase,
                "durationMs": step.duration_ms,
                "bytes": step.bytes,
                "timestamp": step.timestamp,
                "detail": step.detail.clone().unwrap_or_else(|| format!("{} reported by snapshotter exporter", step.name)),
            })
        })
        .collect()
}

fn timeline_spans(
    fallback_timestamp: &str,
    image_id: &str,
    image_ref: &str,
    snapshotter: &str,
    steps: &[DownloadStepReport],
) -> Vec<TraceSpan> {
    steps
        .iter()
        .enumerate()
        .map(|(index, step)| {
            let start_time = step.timestamp.as_deref().unwrap_or(fallback_timestamp);
            let end_time =
                end_time(start_time, step.duration_ms).unwrap_or_else(|| start_time.to_string());
            let phase = sanitize_id(&step.phase);
            let span_id = step
                .id
                .clone()
                .unwrap_or_else(|| format!("image-cache-{image_id}-{phase}-{index}"));
            let mut attributes = Map::new();
            attributes.insert("plugin".to_string(), json!("image-cache"));
            attributes.insert("image.id".to_string(), json!(image_id));
            attributes.insert("image.ref".to_string(), json!(image_ref));
            attributes.insert("image.phase".to_string(), json!(step.phase));
            attributes.insert("snapshotter".to_string(), json!(snapshotter));
            if let Some(bytes) = step.bytes {
                attributes.insert("image.bytes".to_string(), json!(bytes));
            }

            TraceSpan {
                trace_id: format!("image-cache-{image_id}"),
                span_id,
                span_name: format!("image.{}", step.phase),
                start_time: start_time.to_string(),
                end_time,
                duration_ms: step.duration_ms,
                status: "ok".to_string(),
                attributes,
                sandbox_id: None,
                image_id: Some(image_id.to_string()),
                parent_span_id: None,
            }
        })
        .collect()
}

fn timeline_event(
    id: &str,
    timestamp: &str,
    node_id: &str,
    image_id: &str,
    image_ref: &str,
    snapshotter: &str,
    span: &TraceSpan,
) -> EventRecord {
    let mut attributes = span.attributes.clone();
    attributes.insert("snapshotter".to_string(), json!(snapshotter));

    EventRecord {
        id: id.to_string(),
        timestamp: timestamp.to_string(),
        severity: "info".to_string(),
        event_type: "image".to_string(),
        event_name: span.span_name.clone(),
        message: format!(
            "{} reported {} for {}.",
            snapshotter, span.span_name, image_ref
        ),
        source: format!("runtimepulse-rust-collector/{node_id}/image-cache"),
        attributes,
        sandbox_id: None,
        image_id: Some(image_id.to_string()),
        node_id: Some(node_id.to_string()),
        runtime_type: None,
        reason: None,
    }
}

fn end_time(start_time: &str, duration_ms: f64) -> Option<String> {
    let start = DateTime::parse_from_rfc3339(start_time)
        .ok()?
        .with_timezone(&Utc);
    Some(timestamp(
        start + chrono::Duration::milliseconds(duration_ms.max(0.0).round() as i64),
    ))
}

fn sanitize_id(value: &str) -> String {
    value
        .chars()
        .map(|char| {
            if char.is_ascii_alphanumeric() {
                char.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .to_string()
}

fn timestamp(time: DateTime<Utc>) -> String {
    time.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}
