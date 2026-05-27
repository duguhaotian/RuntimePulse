//! Snapshotter image cache source.
//!
//! This source does not infer cache behavior from Docker metadata. It reads
//! RuntimePulse-shaped reports emitted by real snapshotter/cache tools, such as
//! nydus, stargz, overlaybd, or a local cache exporter.

use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, HashSet};
use std::fs;
#[cfg(unix)]
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use crate::collectors::core::config::CollectorConfig;
use crate::collectors::core::error::{CollectorError, Result};
use crate::collectors::core::model::{EventRecord, MetricSample, PluginOutput, TraceSpan};
use crate::collectors::core::plugin::CollectorPlugin;
use crate::collectors::core::report::image_metric;
use crate::collectors::sources::image::layer::docker_image_id_from_ref_or_digest;

#[derive(Default)]
pub struct ImageCachePlugin {
    path: Option<PathBuf>,
    command: Option<String>,
    command_timeout: Duration,
    seen_event_ids: HashSet<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SnapshotterReport {
    #[serde(alias = "image_id")]
    image_id: Option<String>,
    #[serde(alias = "image_ref", alias = "reference", alias = "ref")]
    image_ref: Option<String>,
    #[serde(alias = "image_digest", alias = "digest")]
    image_digest: Option<String>,
    #[serde(alias = "loading_mode")]
    loading_mode: Option<String>,
    #[serde(alias = "size_bytes", alias = "size")]
    size_bytes: Option<u64>,
    #[serde(alias = "layer_count")]
    layer_count: Option<u64>,
    timestamp: Option<String>,
    snapshotter: Option<String>,
    cache: Option<CacheReport>,
    #[serde(default)]
    layers: Vec<LayerCacheReport>,
    #[serde(default)]
    prefetches: Vec<PrefetchReport>,
    #[serde(alias = "download_timeline", alias = "timeline", alias = "stages")]
    download_timeline: Option<Vec<DownloadStepReport>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CacheReport {
    #[serde(alias = "requested_blocks", alias = "requests")]
    requested_blocks: Option<u64>,
    #[serde(alias = "hit_blocks", alias = "hits")]
    hit_blocks: Option<u64>,
    #[serde(alias = "local_read_bytes")]
    local_read_bytes: Option<u64>,
    #[serde(alias = "remote_read_bytes")]
    remote_read_bytes: Option<u64>,
    #[serde(alias = "block_size_bytes", alias = "block_size")]
    block_size_bytes: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LayerCacheReport {
    id: Option<String>,
    digest: Option<String>,
    #[serde(alias = "media_type")]
    media_type: Option<String>,
    #[serde(alias = "size_bytes", alias = "size")]
    size_bytes: Option<u64>,
    #[serde(alias = "requested_blocks", alias = "requests")]
    requested_blocks: Option<u64>,
    #[serde(alias = "hit_blocks", alias = "hits")]
    hit_blocks: Option<u64>,
    #[serde(alias = "local_read_bytes")]
    local_read_bytes: Option<u64>,
    #[serde(alias = "remote_read_bytes")]
    remote_read_bytes: Option<u64>,
    #[serde(alias = "block_size_bytes", alias = "block_size")]
    block_size_bytes: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PrefetchReport {
    id: Option<String>,
    name: Option<String>,
    phase: Option<String>,
    #[serde(alias = "started_at", alias = "start_time", alias = "startTime")]
    started_at: Option<String>,
    #[serde(alias = "duration_ms", alias = "duration")]
    duration_ms: Option<f64>,
    bytes: Option<u64>,
    #[serde(alias = "hit_blocks", alias = "hits")]
    hit_blocks: Option<u64>,
    #[serde(alias = "requested_blocks", alias = "requests")]
    requested_blocks: Option<u64>,
    detail: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DownloadStepReport {
    id: Option<String>,
    #[serde(default)]
    name: String,
    #[serde(default)]
    phase: String,
    #[serde(alias = "duration_ms", alias = "duration", default)]
    duration_ms: f64,
    bytes: Option<u64>,
    timestamp: Option<String>,
    detail: Option<String>,
}

impl ImageCachePlugin {
    pub fn new(path: Option<PathBuf>, command: Option<String>, command_timeout: Duration) -> Self {
        Self {
            path,
            command,
            command_timeout,
            seen_event_ids: HashSet::new(),
        }
    }
}

impl CollectorPlugin for ImageCachePlugin {
    fn name(&self) -> &str {
        "image-cache"
    }

    fn collect(&mut self, now: DateTime<Utc>, config: &CollectorConfig) -> Result<PluginOutput> {
        let mut reports = Vec::new();
        let source_detail = if let Some(command) = self.command.clone() {
            let content = run_image_cache_command(&command, self.command_timeout)?;
            if !content.trim().is_empty() {
                reports.extend(parse_reports(&content)?);
            }
            Some("command".to_string())
        } else if let Some(path) = self.path.clone() {
            let content = match fs::read_to_string(&path) {
                Ok(content) => content,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    return Ok(PluginOutput::default());
                }
                Err(error) => return Err(error.into()),
            };
            reports.extend(parse_reports(&content)?);
            Some(path.display().to_string())
        } else {
            return Ok(PluginOutput::default());
        };

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
                    "snapshotter.reportSource": source_detail.as_deref().unwrap_or("unknown"),
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

fn run_image_cache_command(command: &str, timeout: Duration) -> Result<String> {
    let mut child_command = Command::new("sh");
    child_command
        .arg("-lc")
        .arg(command)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    child_command.process_group(0);
    let mut child = child_command.spawn()?;

    let started = Instant::now();
    loop {
        if child.try_wait()?.is_some() {
            let output = child.wait_with_output()?;
            if !output.status.success() {
                return Err(CollectorError::Plugin {
                    plugin: "image-cache".to_string(),
                    message: format!(
                        "image cache command exited with status {:?}: {}",
                        output.status.code(),
                        String::from_utf8_lossy(&output.stderr).trim()
                    ),
                });
            }
            return Ok(String::from_utf8_lossy(&output.stdout).to_string());
        }

        if started.elapsed() >= timeout {
            kill_child_tree(&mut child);
            let _ = child.wait();
            return Err(CollectorError::Plugin {
                plugin: "image-cache".to_string(),
                message: format!(
                    "image cache command timed out after {} ms",
                    timeout.as_millis()
                ),
            });
        }

        thread::sleep(Duration::from_millis(20));
    }
}

fn kill_child_tree(child: &mut std::process::Child) {
    #[cfg(unix)]
    {
        let group = format!("-{}", child.id());
        let _ = Command::new("kill").args(["-TERM", &group]).status();
        thread::sleep(Duration::from_millis(50));
        let _ = Command::new("kill").args(["-KILL", &group]).status();
        return;
    }

    #[allow(unreachable_code)]
    {
        let _ = child.kill();
    }
}

fn parse_reports(content: &str) -> Result<Vec<SnapshotterReport>> {
    let trimmed = content.trim();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }
    if trimmed.starts_with('[') {
        if let Ok(reports) = serde_json::from_str::<Vec<SnapshotterReport>>(trimmed) {
            if reports.iter().any(snapshotter_report_has_payload) {
                return Ok(reports);
            }
        }
        return Ok(parse_snapshotter_state_value(serde_json::from_str(
            trimmed,
        )?));
    }
    if trimmed.starts_with('{') {
        if let Ok(report) = serde_json::from_str::<SnapshotterReport>(trimmed) {
            if snapshotter_report_has_payload(&report) {
                return Ok(vec![report]);
            }
        }
        if let Ok(value) = serde_json::from_str::<Value>(trimmed) {
            return Ok(parse_snapshotter_state_value(value));
        }
    }
    if looks_like_prometheus_text(trimmed) {
        return Ok(parse_prometheus_reports(trimmed));
    }

    let mut reports = Vec::new();
    for line in trimmed
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
    {
        if let Ok(value) = serde_json::from_str::<Value>(line) {
            let parsed = parse_snapshotter_state_value(value);
            if !parsed.is_empty() {
                reports.extend(parsed);
                continue;
            }
        }
        if let Ok(report) = serde_json::from_str::<SnapshotterReport>(line) {
            if snapshotter_report_has_payload(&report) {
                reports.push(report);
                continue;
            }
        }
        reports.extend(parse_snapshotter_state_value(serde_json::from_str(line)?));
    }
    Ok(reports)
}

fn snapshotter_report_has_payload(report: &SnapshotterReport) -> bool {
    report.image_id.is_some()
        || report.image_ref.is_some()
        || report.image_digest.is_some()
        || report.cache.is_some()
        || !report.layers.is_empty()
        || !report.prefetches.is_empty()
        || report
            .download_timeline
            .as_ref()
            .is_some_and(|timeline| !timeline.is_empty())
}

fn parse_snapshotter_state_value(value: Value) -> Vec<SnapshotterReport> {
    match value {
        Value::Array(items) => items
            .into_iter()
            .filter_map(snapshotter_state_report_from_value)
            .collect(),
        Value::Object(mut object) => {
            let root_defaults = SnapshotterStateDefaults::from_object(&object);
            for key in ["images", "snapshots", "entries", "records", "items"] {
                if let Some(Value::Array(items)) = object.remove(key) {
                    return items
                        .into_iter()
                        .filter_map(|value| {
                            snapshotter_state_report_from_value_with_defaults(value, &root_defaults)
                        })
                        .collect();
                }
            }
            snapshotter_state_report_from_object(object)
                .into_iter()
                .collect()
        }
        _ => Vec::new(),
    }
}

#[derive(Clone, Debug, Default)]
struct SnapshotterStateDefaults {
    snapshotter: Option<String>,
    timestamp: Option<String>,
}

impl SnapshotterStateDefaults {
    fn from_object(object: &Map<String, Value>) -> Self {
        Self {
            snapshotter: value_string_from_keys(
                object,
                &[
                    "snapshotter",
                    "remoteSnapshotter",
                    "remote_snapshotter",
                    "driver",
                ],
            ),
            timestamp: value_string_from_keys(object, &["timestamp", "observedAt", "observed_at"]),
        }
    }
}

fn snapshotter_state_report_from_value(value: Value) -> Option<SnapshotterReport> {
    snapshotter_state_report_from_value_with_defaults(value, &SnapshotterStateDefaults::default())
}

fn snapshotter_state_report_from_value_with_defaults(
    value: Value,
    defaults: &SnapshotterStateDefaults,
) -> Option<SnapshotterReport> {
    match value {
        Value::Object(object) => {
            let mut report = snapshotter_state_report_from_object(object)?;
            if report.snapshotter.is_none() {
                report.snapshotter = defaults.snapshotter.clone();
            }
            if report.timestamp.is_none() {
                report.timestamp = defaults.timestamp.clone();
            }
            Some(report)
        }
        _ => None,
    }
}

fn snapshotter_state_report_from_object(
    mut object: Map<String, Value>,
) -> Option<SnapshotterReport> {
    let image_ref = take_string(
        &mut object,
        &[
            "imageRef",
            "image_ref",
            "reference",
            "ref",
            "name",
            "targetRef",
        ],
    );
    let image_id = take_string(&mut object, &["imageId", "image_id", "id", "target"]);
    let image_digest = take_string(
        &mut object,
        &["imageDigest", "image_digest", "digest", "targetDigest"],
    );
    let snapshotter = take_string(
        &mut object,
        &[
            "snapshotter",
            "remoteSnapshotter",
            "remote_snapshotter",
            "driver",
        ],
    )
    .or_else(|| infer_snapshotter_from_object(&object));
    let loading_mode = take_string(&mut object, &["loadingMode", "loading_mode", "mode"]);
    let size_bytes = take_u64(&mut object, &["sizeBytes", "size_bytes", "size"]);
    let layer_count = take_u64(&mut object, &["layerCount", "layer_count"]);
    let timestamp = take_string(&mut object, &["timestamp", "observedAt", "observed_at"]);

    let cache = cache_from_state_object(&mut object);
    let layers = take_array(&mut object, &["layers", "blobs", "chunks", "files"])
        .unwrap_or_default()
        .into_iter()
        .filter_map(layer_from_state_value)
        .collect::<Vec<_>>();
    let prefetches = take_array(&mut object, &["prefetches", "prefetch", "warmups"])
        .unwrap_or_default()
        .into_iter()
        .filter_map(prefetch_from_state_value)
        .collect::<Vec<_>>();
    let timeline = take_array(
        &mut object,
        &[
            "downloadTimeline",
            "download_timeline",
            "timeline",
            "stages",
            "events",
        ],
    )
    .unwrap_or_default()
    .into_iter()
    .filter_map(timeline_from_state_value)
    .collect::<Vec<_>>();

    if image_ref.is_none()
        && image_id.is_none()
        && image_digest.is_none()
        && cache.is_none()
        && layers.is_empty()
        && prefetches.is_empty()
        && timeline.is_empty()
    {
        return None;
    }

    Some(SnapshotterReport {
        image_id,
        image_ref,
        image_digest,
        loading_mode,
        size_bytes,
        layer_count,
        timestamp,
        snapshotter,
        cache,
        layers,
        prefetches,
        download_timeline: Some(timeline),
    })
}

fn cache_from_state_object(object: &mut Map<String, Value>) -> Option<CacheReport> {
    let nested = take_object(
        object,
        &["cache", "blockCache", "block_cache", "stats", "metrics"],
    );
    let mut values = nested.unwrap_or_else(Map::new);
    for (key, value) in object.iter() {
        if key.contains("cache")
            || key.contains("blocks")
            || key.contains("Bytes")
            || key.contains("bytes")
            || key.contains("block")
        {
            values.entry(key.clone()).or_insert_with(|| value.clone());
        }
    }
    let cache = CacheReport {
        requested_blocks: value_u64_from_keys(
            &values,
            &[
                "requestedBlocks",
                "requested_blocks",
                "requests",
                "blockRequests",
                "block_requests",
            ],
        ),
        hit_blocks: value_u64_from_keys(
            &values,
            &["hitBlocks", "hit_blocks", "hits", "cacheHits", "cache_hits"],
        ),
        local_read_bytes: value_u64_from_keys(
            &values,
            &[
                "localReadBytes",
                "local_read_bytes",
                "localBytes",
                "local_bytes",
            ],
        ),
        remote_read_bytes: value_u64_from_keys(
            &values,
            &[
                "remoteReadBytes",
                "remote_read_bytes",
                "remoteBytes",
                "remote_bytes",
                "fetchedBytes",
                "fetched_bytes",
            ],
        ),
        block_size_bytes: value_u64_from_keys(
            &values,
            &[
                "blockSizeBytes",
                "block_size_bytes",
                "blockSize",
                "block_size",
                "chunkSize",
                "chunk_size",
            ],
        ),
    };
    if cache.requested_blocks.is_none()
        && cache.hit_blocks.is_none()
        && cache.local_read_bytes.is_none()
        && cache.remote_read_bytes.is_none()
        && cache.block_size_bytes.is_none()
    {
        None
    } else {
        Some(cache)
    }
}

fn layer_from_state_value(value: Value) -> Option<LayerCacheReport> {
    let Value::Object(mut object) = value else {
        return None;
    };
    let cache = cache_from_state_object(&mut object);
    Some(LayerCacheReport {
        id: take_string(&mut object, &["id", "layerId", "layer_id", "name"]),
        digest: take_string(
            &mut object,
            &["digest", "layerDigest", "layer_digest", "blob"],
        ),
        media_type: take_string(&mut object, &["mediaType", "media_type", "type"]),
        size_bytes: take_u64(&mut object, &["sizeBytes", "size_bytes", "size", "bytes"]),
        requested_blocks: take_u64(
            &mut object,
            &["requestedBlocks", "requested_blocks", "requests"],
        )
        .or_else(|| cache.as_ref().and_then(|cache| cache.requested_blocks)),
        hit_blocks: take_u64(&mut object, &["hitBlocks", "hit_blocks", "hits"])
            .or_else(|| cache.as_ref().and_then(|cache| cache.hit_blocks)),
        local_read_bytes: take_u64(&mut object, &["localReadBytes", "local_read_bytes"])
            .or_else(|| cache.as_ref().and_then(|cache| cache.local_read_bytes)),
        remote_read_bytes: take_u64(
            &mut object,
            &[
                "remoteReadBytes",
                "remote_read_bytes",
                "fetchedBytes",
                "fetched_bytes",
            ],
        )
        .or_else(|| cache.as_ref().and_then(|cache| cache.remote_read_bytes)),
        block_size_bytes: take_u64(
            &mut object,
            &[
                "blockSizeBytes",
                "block_size_bytes",
                "blockSize",
                "block_size",
                "chunkSize",
                "chunk_size",
            ],
        )
        .or_else(|| cache.as_ref().and_then(|cache| cache.block_size_bytes)),
    })
}

fn prefetch_from_state_value(value: Value) -> Option<PrefetchReport> {
    let Value::Object(mut object) = value else {
        return None;
    };
    Some(PrefetchReport {
        id: take_string(&mut object, &["id", "name"]),
        name: take_string(&mut object, &["name", "path", "pattern"]),
        phase: take_string(&mut object, &["phase", "stage"])
            .or_else(|| Some("prefetch".to_string())),
        started_at: take_string(
            &mut object,
            &[
                "startedAt",
                "started_at",
                "startTime",
                "start_time",
                "timestamp",
            ],
        ),
        duration_ms: take_f64(&mut object, &["durationMs", "duration_ms", "duration"]),
        bytes: take_u64(&mut object, &["bytes", "sizeBytes", "size_bytes", "size"]),
        hit_blocks: take_u64(&mut object, &["hitBlocks", "hit_blocks", "hits"]),
        requested_blocks: take_u64(
            &mut object,
            &["requestedBlocks", "requested_blocks", "requests"],
        ),
        detail: take_string(&mut object, &["detail", "message"]),
    })
}

fn timeline_from_state_value(value: Value) -> Option<DownloadStepReport> {
    let Value::Object(mut object) = value else {
        return None;
    };
    let phase = take_string(&mut object, &["phase", "stage", "operation", "op"])
        .unwrap_or_else(|| "snapshotter".to_string());
    Some(DownloadStepReport {
        id: take_string(&mut object, &["id"]),
        name: take_string(&mut object, &["name", "title"]).unwrap_or_else(|| title_case(&phase)),
        phase,
        duration_ms: take_f64(&mut object, &["durationMs", "duration_ms", "duration"])
            .unwrap_or(0.0),
        bytes: take_u64(&mut object, &["bytes", "sizeBytes", "size_bytes"]),
        timestamp: take_string(
            &mut object,
            &[
                "timestamp",
                "startedAt",
                "started_at",
                "startTime",
                "start_time",
            ],
        ),
        detail: take_string(&mut object, &["detail", "message"]),
    })
}

fn take_array(object: &mut Map<String, Value>, keys: &[&str]) -> Option<Vec<Value>> {
    keys.iter().find_map(|key| match object.remove(*key) {
        Some(Value::Array(values)) => Some(values),
        _ => None,
    })
}

fn take_object(object: &mut Map<String, Value>, keys: &[&str]) -> Option<Map<String, Value>> {
    keys.iter().find_map(|key| match object.remove(*key) {
        Some(Value::Object(value)) => Some(value),
        _ => None,
    })
}

fn take_string(object: &mut Map<String, Value>, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| object.remove(*key).and_then(value_to_string))
}

fn take_u64(object: &mut Map<String, Value>, keys: &[&str]) -> Option<u64> {
    keys.iter()
        .find_map(|key| object.remove(*key).and_then(|value| value_to_u64(&value)))
}

fn take_f64(object: &mut Map<String, Value>, keys: &[&str]) -> Option<f64> {
    keys.iter()
        .find_map(|key| object.remove(*key).and_then(|value| value_to_f64(&value)))
}

fn value_string_from_keys(object: &Map<String, Value>, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| object.get(*key).cloned().and_then(value_to_string))
}

fn value_u64_from_keys(object: &Map<String, Value>, keys: &[&str]) -> Option<u64> {
    keys.iter()
        .find_map(|key| object.get(*key).and_then(value_to_u64))
}

fn value_to_string(value: Value) -> Option<String> {
    match value {
        Value::String(value) if !value.is_empty() => Some(value),
        Value::Number(value) => Some(value.to_string()),
        _ => None,
    }
}

fn value_to_u64(value: &Value) -> Option<u64> {
    match value {
        Value::Number(number) => number
            .as_u64()
            .or_else(|| number.as_f64().map(|value| value.max(0.0).round() as u64)),
        Value::String(value) => value.parse::<u64>().ok(),
        _ => None,
    }
}

fn value_to_f64(value: &Value) -> Option<f64> {
    match value {
        Value::Number(number) => number.as_f64(),
        Value::String(value) => value.parse::<f64>().ok(),
        _ => None,
    }
}

fn infer_snapshotter_from_object(object: &Map<String, Value>) -> Option<String> {
    for value in object.values() {
        let text = match value {
            Value::String(value) => value.to_ascii_lowercase(),
            _ => continue,
        };
        for snapshotter in ["nydus", "stargz", "overlaybd"] {
            if text.contains(snapshotter) {
                return Some(snapshotter.to_string());
            }
        }
    }
    None
}

#[derive(Clone, Debug, Default)]
struct PrometheusImageRow {
    image_id: Option<String>,
    image_ref: Option<String>,
    image_digest: Option<String>,
    snapshotter: Option<String>,
    loading_mode: Option<String>,
    size_bytes: Option<u64>,
    layer_count: Option<u64>,
    cache: CacheReportBuilder,
    layers: BTreeMap<String, LayerCacheReportBuilder>,
    prefetches: BTreeMap<String, PrefetchReportBuilder>,
    timeline: BTreeMap<String, DownloadStepReportBuilder>,
}

#[derive(Clone, Debug, Default)]
struct CacheReportBuilder {
    requested_blocks: Option<u64>,
    hit_blocks: Option<u64>,
    local_read_bytes: Option<u64>,
    remote_read_bytes: Option<u64>,
    block_size_bytes: Option<u64>,
}

#[derive(Clone, Debug, Default)]
struct LayerCacheReportBuilder {
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

#[derive(Clone, Debug, Default)]
struct PrefetchReportBuilder {
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

#[derive(Clone, Debug, Default)]
struct DownloadStepReportBuilder {
    id: Option<String>,
    name: Option<String>,
    phase: Option<String>,
    duration_ms: Option<f64>,
    bytes: Option<u64>,
    timestamp: Option<String>,
    detail: Option<String>,
}

fn looks_like_prometheus_text(content: &str) -> bool {
    content.lines().any(|line| {
        let line = line.trim();
        line.starts_with("# HELP")
            || line.starts_with("# TYPE")
            || line.starts_with("containerd_")
            || line.starts_with("nydus_")
            || line.starts_with("stargz_")
            || line.starts_with("overlaybd_")
            || line.starts_with("image_cache_")
            || line.starts_with("snapshotter_")
    })
}

fn parse_prometheus_reports(content: &str) -> Vec<SnapshotterReport> {
    let mut rows = BTreeMap::<String, PrometheusImageRow>::new();

    for line in content.lines().map(str::trim) {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some(sample) = parse_prometheus_sample(line) else {
            continue;
        };
        if !is_image_cache_metric(&sample.name) {
            continue;
        }
        let image_key = prometheus_image_key(&sample.labels);
        let row = rows.entry(image_key).or_default();
        apply_prometheus_sample(row, &sample);
    }

    rows.into_values()
        .filter_map(prometheus_row_to_report)
        .collect()
}

#[derive(Clone, Debug)]
struct PrometheusSample {
    name: String,
    labels: BTreeMap<String, String>,
    value: f64,
}

fn parse_prometheus_sample(line: &str) -> Option<PrometheusSample> {
    let (metric, value_text) = line.rsplit_once(char::is_whitespace)?;
    let value = value_text.trim().parse::<f64>().ok()?;
    let (name, labels) = if let Some(start) = metric.find('{') {
        let end = metric.rfind('}')?;
        (
            &metric[..start],
            parse_prometheus_labels(&metric[start + 1..end]),
        )
    } else {
        (metric, BTreeMap::new())
    };
    Some(PrometheusSample {
        name: name.to_string(),
        labels,
        value,
    })
}

fn parse_prometheus_labels(input: &str) -> BTreeMap<String, String> {
    let mut labels = BTreeMap::new();
    let mut key = String::new();
    let mut value = String::new();
    let mut in_key = true;
    let mut in_quote = false;
    let mut escape = false;

    for char in input.chars() {
        if in_key {
            if char == '=' {
                in_key = false;
            } else if char != ' ' {
                key.push(char);
            }
            continue;
        }

        if escape {
            value.push(match char {
                'n' => '\n',
                'r' => '\r',
                't' => '\t',
                other => other,
            });
            escape = false;
            continue;
        }
        match char {
            '\\' if in_quote => escape = true,
            '"' => in_quote = !in_quote,
            ',' if !in_quote => {
                if !key.trim().is_empty() {
                    labels.insert(key.trim().to_string(), value.clone());
                }
                key.clear();
                value.clear();
                in_key = true;
            }
            other => value.push(other),
        }
    }
    if !key.trim().is_empty() {
        labels.insert(key.trim().to_string(), value);
    }
    labels
}

fn is_image_cache_metric(name: &str) -> bool {
    normalized_metric_name(name).contains("image_cache")
        || normalized_metric_name(name).contains("snapshotter")
        || normalized_metric_name(name).contains("nydus")
        || normalized_metric_name(name).contains("stargz")
        || normalized_metric_name(name).contains("overlaybd")
}

fn normalized_metric_name(name: &str) -> String {
    name.trim_end_matches("_total")
        .trim_end_matches("_sum")
        .trim_end_matches("_count")
        .trim_end_matches("_bucket")
        .to_ascii_lowercase()
}

fn prometheus_image_key(labels: &BTreeMap<String, String>) -> String {
    label_value(
        labels,
        &["image_id", "imageId", "image", "image_ref", "ref", "digest"],
    )
    .unwrap_or_else(|| "snapshotter/unknown:latest".to_string())
}

fn apply_prometheus_sample(row: &mut PrometheusImageRow, sample: &PrometheusSample) {
    fill_prometheus_identity(row, &sample.labels);
    let metric = normalized_metric_name(&sample.name);
    let value_u64 = sample.value.max(0.0).round() as u64;

    if metric.contains("layer_count") || metric.contains("layers") {
        row.layer_count = Some(value_u64);
        return;
    }
    if metric.contains("layer") {
        apply_prometheus_layer_sample(row, sample, &metric, value_u64);
        return;
    }
    if metric.contains("prefetch") || metric.contains("warmup") {
        apply_prometheus_prefetch_sample(row, sample, &metric, value_u64);
        return;
    }
    if metric.contains("duration")
        || metric.contains("latency")
        || metric.contains("stage")
        || metric.contains("timeline")
    {
        apply_prometheus_timeline_sample(row, sample, &metric);
        return;
    }

    assign_cache_metric(&mut row.cache, &metric, value_u64);
    if metric.contains("size") && !metric.contains("block") {
        row.size_bytes = Some(value_u64);
    }
}

fn fill_prometheus_identity(row: &mut PrometheusImageRow, labels: &BTreeMap<String, String>) {
    row.image_id = row
        .image_id
        .clone()
        .or_else(|| label_value(labels, &["image_id", "imageId"]));
    row.image_ref = row
        .image_ref
        .clone()
        .or_else(|| label_value(labels, &["image_ref", "image", "ref", "reference"]));
    row.image_digest = row
        .image_digest
        .clone()
        .or_else(|| label_value(labels, &["image_digest", "digest"]));
    row.snapshotter = row.snapshotter.clone().or_else(|| {
        label_value(labels, &["snapshotter", "remote_snapshotter", "driver"])
            .or_else(|| infer_snapshotter_from_labels(labels))
    });
    row.loading_mode = row
        .loading_mode
        .clone()
        .or_else(|| label_value(labels, &["loading_mode", "mode"]));
}

fn apply_prometheus_layer_sample(
    row: &mut PrometheusImageRow,
    sample: &PrometheusSample,
    metric: &str,
    value: u64,
) {
    let layer_key = label_value(&sample.labels, &["layer", "layer_id", "layerId", "digest"])
        .unwrap_or_else(|| "layer-0".to_string());
    let layer = row.layers.entry(layer_key.clone()).or_default();
    layer.id = layer.id.clone().or_else(|| Some(layer_key));
    layer.digest = layer
        .digest
        .clone()
        .or_else(|| label_value(&sample.labels, &["layer_digest", "digest"]));
    layer.media_type = layer
        .media_type
        .clone()
        .or_else(|| label_value(&sample.labels, &["media_type", "mediaType"]));

    if metric.contains("size") && !metric.contains("block") {
        layer.size_bytes = Some(value);
    } else if metric.contains("requested") || metric.contains("request") {
        layer.requested_blocks = Some(value);
    } else if metric.contains("hit") || metric.contains("cached") {
        layer.hit_blocks = Some(value);
    } else if metric.contains("local") {
        layer.local_read_bytes = Some(value);
    } else if metric.contains("remote") || metric.contains("read") || metric.contains("fetch") {
        layer.remote_read_bytes = Some(value);
    } else if metric.contains("block_size") {
        layer.block_size_bytes = Some(value);
    }
}

fn apply_prometheus_prefetch_sample(
    row: &mut PrometheusImageRow,
    sample: &PrometheusSample,
    metric: &str,
    value: u64,
) {
    let key = label_value(&sample.labels, &["prefetch", "phase", "stage", "name"])
        .unwrap_or_else(|| "prefetch".to_string());
    let prefetch = row.prefetches.entry(key.clone()).or_default();
    prefetch.id = prefetch.id.clone().or_else(|| Some(sanitize_id(&key)));
    prefetch.name = prefetch.name.clone().or_else(|| Some(key.clone()));
    prefetch.phase = prefetch
        .phase
        .clone()
        .or_else(|| Some("prefetch".to_string()));
    prefetch.started_at = prefetch
        .started_at
        .clone()
        .or_else(|| label_value(&sample.labels, &["timestamp", "started_at", "start_time"]));

    if metric.contains("duration") || metric.contains("latency") {
        prefetch.duration_ms = Some(prometheus_duration_ms(&sample.name, sample.value));
    } else if metric.contains("byte") || metric.contains("size") {
        prefetch.bytes = Some(value);
    } else if metric.contains("requested") || metric.contains("request") {
        prefetch.requested_blocks = Some(value);
    } else if metric.contains("hit") || metric.contains("cached") {
        prefetch.hit_blocks = Some(value);
    }
}

fn apply_prometheus_timeline_sample(
    row: &mut PrometheusImageRow,
    sample: &PrometheusSample,
    metric: &str,
) {
    let phase = label_value(&sample.labels, &["phase", "stage", "operation", "op"])
        .unwrap_or_else(|| infer_timeline_phase(metric));
    let step = row.timeline.entry(phase.clone()).or_default();
    step.id = step.id.clone().or_else(|| Some(sanitize_id(&phase)));
    step.name = step.name.clone().or_else(|| Some(title_case(&phase)));
    step.phase = step.phase.clone().or_else(|| Some(phase));
    step.duration_ms = Some(prometheus_duration_ms(&sample.name, sample.value));
    step.bytes = step.bytes.or_else(|| {
        label_value(&sample.labels, &["bytes"]).and_then(|value| value.parse::<u64>().ok())
    });
    step.timestamp = step
        .timestamp
        .clone()
        .or_else(|| label_value(&sample.labels, &["timestamp", "started_at", "start_time"]));
    step.detail = step.detail.clone().or_else(|| {
        Some(format!(
            "{} from Prometheus metric {}",
            step.name.clone().unwrap_or_else(|| "stage".to_string()),
            sample.name
        ))
    });
}

fn assign_cache_metric(cache: &mut CacheReportBuilder, metric: &str, value: u64) {
    if metric.contains("requested") || metric.contains("request") {
        cache.requested_blocks = Some(value);
    } else if metric.contains("hit") || metric.contains("cached") {
        cache.hit_blocks = Some(value);
    } else if metric.contains("local") {
        cache.local_read_bytes = Some(value);
    } else if metric.contains("remote") || metric.contains("fetch") || metric.contains("read") {
        cache.remote_read_bytes = Some(value);
    } else if metric.contains("block_size") {
        cache.block_size_bytes = Some(value);
    }
}

fn prometheus_row_to_report(row: PrometheusImageRow) -> Option<SnapshotterReport> {
    let image_ref = row
        .image_ref
        .clone()
        .or_else(|| row.image_id.clone())
        .or_else(|| row.image_digest.clone());
    if image_ref.is_none()
        && row.cache.requested_blocks.is_none()
        && row.cache.hit_blocks.is_none()
        && row.layers.is_empty()
        && row.prefetches.is_empty()
        && row.timeline.is_empty()
    {
        return None;
    }

    Some(SnapshotterReport {
        image_id: row.image_id,
        image_ref,
        image_digest: row.image_digest,
        loading_mode: row.loading_mode.or_else(|| Some("lazy".to_string())),
        size_bytes: row.size_bytes,
        layer_count: row.layer_count,
        timestamp: None,
        snapshotter: row.snapshotter.or_else(|| Some("snapshotter".to_string())),
        cache: cache_builder_to_report(row.cache),
        layers: row
            .layers
            .into_values()
            .map(layer_builder_to_report)
            .collect(),
        prefetches: row
            .prefetches
            .into_values()
            .map(prefetch_builder_to_report)
            .collect(),
        download_timeline: Some(
            row.timeline
                .into_values()
                .map(timeline_builder_to_report)
                .collect(),
        ),
    })
}

fn cache_builder_to_report(cache: CacheReportBuilder) -> Option<CacheReport> {
    if cache.requested_blocks.is_none()
        && cache.hit_blocks.is_none()
        && cache.local_read_bytes.is_none()
        && cache.remote_read_bytes.is_none()
        && cache.block_size_bytes.is_none()
    {
        return None;
    }
    Some(CacheReport {
        requested_blocks: cache.requested_blocks,
        hit_blocks: cache.hit_blocks,
        local_read_bytes: cache.local_read_bytes,
        remote_read_bytes: cache.remote_read_bytes,
        block_size_bytes: cache.block_size_bytes,
    })
}

fn layer_builder_to_report(layer: LayerCacheReportBuilder) -> LayerCacheReport {
    LayerCacheReport {
        id: layer.id,
        digest: layer.digest,
        media_type: layer.media_type,
        size_bytes: layer.size_bytes,
        requested_blocks: layer.requested_blocks,
        hit_blocks: layer.hit_blocks,
        local_read_bytes: layer.local_read_bytes,
        remote_read_bytes: layer.remote_read_bytes,
        block_size_bytes: layer.block_size_bytes,
    }
}

fn prefetch_builder_to_report(prefetch: PrefetchReportBuilder) -> PrefetchReport {
    PrefetchReport {
        id: prefetch.id,
        name: prefetch.name,
        phase: prefetch.phase,
        started_at: prefetch.started_at,
        duration_ms: prefetch.duration_ms,
        bytes: prefetch.bytes,
        hit_blocks: prefetch.hit_blocks,
        requested_blocks: prefetch.requested_blocks,
        detail: prefetch.detail,
    }
}

fn timeline_builder_to_report(step: DownloadStepReportBuilder) -> DownloadStepReport {
    DownloadStepReport {
        id: step.id,
        name: step.name.unwrap_or_else(|| "Snapshotter stage".to_string()),
        phase: step.phase.unwrap_or_else(|| "snapshotter".to_string()),
        duration_ms: step.duration_ms.unwrap_or(0.0),
        bytes: step.bytes,
        timestamp: step.timestamp,
        detail: step.detail,
    }
}

fn label_value(labels: &BTreeMap<String, String>, names: &[&str]) -> Option<String> {
    names
        .iter()
        .find_map(|name| labels.get(*name).filter(|value| !value.is_empty()).cloned())
}

fn infer_snapshotter_from_labels(labels: &BTreeMap<String, String>) -> Option<String> {
    for value in labels.values() {
        let lower = value.to_ascii_lowercase();
        for snapshotter in ["nydus", "stargz", "overlaybd"] {
            if lower.contains(snapshotter) {
                return Some(snapshotter.to_string());
            }
        }
    }
    None
}

fn infer_timeline_phase(metric: &str) -> String {
    for phase in [
        "resolve", "fetch", "download", "mount", "prefetch", "unpack",
    ] {
        if metric.contains(phase) {
            return phase.to_string();
        }
    }
    "snapshotter".to_string()
}

fn prometheus_duration_ms(name: &str, value: f64) -> f64 {
    if name.ends_with("_seconds") || name.contains("seconds") {
        value * 1000.0
    } else {
        value
    }
}

fn title_case(value: &str) -> String {
    value
        .split(|char: char| !char.is_ascii_alphanumeric())
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => format!("{}{}", first.to_ascii_uppercase(), chars.as_str()),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
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
                runtime_type: None,
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn test_config() -> CollectorConfig {
        CollectorConfig {
            ingest_url: "http://localhost:8081/api/ingest/batch".to_string(),
            node_id: "node-a".to_string(),
            cluster_id: "cluster-a".to_string(),
            interval: Duration::from_secs(5),
            local_report_addr: "127.0.0.1:9091".to_string(),
            local_report_url: "http://localhost:9091/api/local/ingest".to_string(),
            collection_scope: "host".to_string(),
            once: true,
            cgroup_root: PathBuf::from("/sys/fs/cgroup"),
            cgroup_max_entries: 100,
            image_cache_report_path: None,
            image_cache_report_command: None,
            image_cache_report_command_timeout: Duration::from_secs(5),
            profile_report_path: None,
            diagnostic_report_path: None,
            diagnostic_report_command: None,
            diagnostic_report_command_timeout: Duration::from_secs(5),
            perf_report_path: None,
            perf_script_path: None,
            perf_script_command: None,
            perf_script_command_timeout: Duration::from_secs(1),
            perf_folded_path: None,
            ebpf_report_path: None,
            ebpf_folded_path: None,
            perf_profile_command: None,
            ebpf_profile_command: None,
            profile_command_timeout: Duration::from_secs(5),
            plugins: Vec::new(),
            command_plugins: Vec::new(),
            http_plugins: Vec::new(),
        }
    }

    #[test]
    fn command_output_is_normalized_as_image_cache_report() {
        let command = r#"cat <<'JSON'
{"timestamp":"2026-05-25T00:00:00.000Z","imageRef":"registry.example/app:v1","snapshotter":"nydus","cache":{"requestedBlocks":10,"hitBlocks":7,"remoteReadBytes":4096,"localReadBytes":8192,"blockSizeBytes":131072},"downloadTimeline":[{"name":"prefetch","phase":"prefetch","durationMs":25,"bytes":1024}]}
JSON"#;
        let mut plugin =
            ImageCachePlugin::new(None, Some(command.to_string()), Duration::from_secs(5));
        let output = plugin.collect(Utc::now(), &test_config()).unwrap();

        assert_eq!(output.metadata.images.len(), 1);
        assert_eq!(
            output.metadata.images[0]["attributes"]["snapshotter.reportSource"],
            "command"
        );
        assert!(output
            .metrics
            .iter()
            .any(|metric| metric.name == "image.lazy.cache_hit_ratio" && metric.value == 0.7));
        assert!(output
            .traces
            .iter()
            .any(|span| span.span_name == "image.prefetch"));
    }

    #[test]
    fn accepts_snake_case_snapshotter_fields() {
        let content = r#"{
            "image_id":"img-native",
            "image_ref":"registry.example/native:v1",
            "image_digest":"sha256:native",
            "loading_mode":"lazy",
            "size_bytes":2048,
            "layer_count":1,
            "timestamp":"2026-05-25T00:00:00.000Z",
            "snapshotter":"stargz",
            "cache":{
                "requested_blocks":20,
                "hit_blocks":15,
                "local_read_bytes":1024,
                "remote_read_bytes":512,
                "block_size":131072
            },
            "layers":[{
                "id":"layer-a",
                "media_type":"application/vnd.oci.image.layer.v1.tar+gzip",
                "size":2048,
                "requested_blocks":20,
                "hit_blocks":15
            }],
            "prefetches":[{
                "name":"Warm hot files",
                "phase":"prefetch",
                "started_at":"2026-05-25T00:00:00.010Z",
                "duration_ms":33,
                "bytes":512,
                "requested_blocks":5,
                "hit_blocks":4
            }],
            "download_timeline":[{
                "name":"Resolve",
                "phase":"resolve",
                "duration_ms":10
            }]
        }"#;

        let reports = parse_reports(content).unwrap();
        assert_eq!(reports.len(), 1);
        assert_eq!(reports[0].image_id.as_deref(), Some("img-native"));
        assert_eq!(reports[0].cache.as_ref().unwrap().hit_blocks, Some(15));
        assert_eq!(reports[0].layers[0].size_bytes, Some(2048));
        assert_eq!(reports[0].prefetches[0].duration_ms, Some(33.0));
        assert_eq!(
            reports[0].download_timeline.as_ref().unwrap()[0].duration_ms,
            10.0
        );
    }

    #[test]
    fn parses_snapshotter_state_jsonl() {
        let content = r#"{"snapshotter":"nydus","name":"registry.example/jsonl-a:v1","digest":"sha256:jsonl-a","blockCache":{"requests":10,"hits":8},"stages":[{"stage":"mount","durationMs":3}]}
{"snapshotter":"stargz","name":"registry.example/jsonl-b:v1","digest":"sha256:jsonl-b","cache":{"requestedBlocks":20,"hitBlocks":15},"prefetch":[{"name":"hot","requests":5,"hits":4}]}"#;

        let reports = parse_reports(content).unwrap();

        assert_eq!(reports.len(), 2);
        assert_eq!(
            reports[0].image_ref.as_deref(),
            Some("registry.example/jsonl-a:v1")
        );
        assert_eq!(reports[0].cache.as_ref().unwrap().hit_blocks, Some(8));
        assert_eq!(reports[1].snapshotter.as_deref(), Some("stargz"));
        assert_eq!(reports[1].prefetches[0].requested_blocks, Some(5));
    }

    #[test]
    fn parses_prometheus_snapshotter_metrics() {
        let content = r#"
# HELP image_cache_requested_blocks requested lazy blocks
# TYPE image_cache_requested_blocks counter
image_cache_requested_blocks{image_ref="registry.example/prom:v1",snapshotter="nydus"} 100
image_cache_hit_blocks{image_ref="registry.example/prom:v1",snapshotter="nydus"} 75
image_cache_remote_read_bytes{image_ref="registry.example/prom:v1",snapshotter="nydus"} 4096
image_cache_local_read_bytes{image_ref="registry.example/prom:v1",snapshotter="nydus"} 8192
image_cache_block_size_bytes{image_ref="registry.example/prom:v1",snapshotter="nydus"} 131072
image_cache_layer_requested_blocks{image_ref="registry.example/prom:v1",snapshotter="nydus",layer="sha256:layer-a"} 60
image_cache_layer_hit_blocks{image_ref="registry.example/prom:v1",snapshotter="nydus",layer="sha256:layer-a"} 50
image_cache_prefetch_duration_seconds{image_ref="registry.example/prom:v1",snapshotter="nydus",phase="prefetch"} 0.125
image_cache_prefetch_bytes{image_ref="registry.example/prom:v1",snapshotter="nydus",phase="prefetch"} 2048
image_cache_stage_duration_seconds{image_ref="registry.example/prom:v1",snapshotter="nydus",phase="mount"} 0.04
"#;

        let reports = parse_reports(content).unwrap();
        assert_eq!(reports.len(), 1);
        let report = &reports[0];
        assert_eq!(
            report.image_ref.as_deref(),
            Some("registry.example/prom:v1")
        );
        assert_eq!(report.snapshotter.as_deref(), Some("nydus"));
        assert_eq!(report.cache.as_ref().unwrap().requested_blocks, Some(100));
        assert_eq!(report.cache.as_ref().unwrap().hit_blocks, Some(75));
        assert_eq!(report.layers.len(), 1);
        assert_eq!(report.layers[0].requested_blocks, Some(60));
        assert_eq!(report.prefetches.len(), 1);
        assert_eq!(report.prefetches[0].duration_ms, Some(125.0));
        assert!(report
            .download_timeline
            .as_ref()
            .unwrap()
            .iter()
            .any(|step| step.phase == "mount" && step.duration_ms == 40.0));
    }
    #[test]
    fn parses_snapshotter_state_json() {
        let content = r#"{
            "snapshotter":"overlaybd",
            "images":[{
                "name":"registry.example/state:v1",
                "digest":"sha256:state",
                "size":4096,
                "blockCache":{
                    "requests":32,
                    "hits":24,
                    "remoteBytes":1024,
                    "localBytes":2048,
                    "blockSize":131072
                },
                "layers":[{
                    "digest":"sha256:layer-state",
                    "size":4096,
                    "cache":{"requests":16,"hits":12,"remoteBytes":512}
                }],
                "prefetch":[{"name":"hot files","durationMs":12,"bytes":256,"requests":4,"hits":3}],
                "stages":[{"stage":"mount","durationMs":7,"bytes":128}]
            }]
        }"#;

        let reports = parse_reports(content).unwrap();
        assert_eq!(reports.len(), 1);
        let report = &reports[0];
        assert_eq!(
            report.image_ref.as_deref(),
            Some("registry.example/state:v1")
        );
        assert_eq!(report.snapshotter.as_deref(), Some("overlaybd"));
        assert_eq!(report.cache.as_ref().unwrap().requested_blocks, Some(32));
        assert_eq!(report.cache.as_ref().unwrap().hit_blocks, Some(24));
        assert_eq!(report.layers[0].remote_read_bytes, Some(512));
        assert_eq!(report.prefetches[0].requested_blocks, Some(4));
        assert!(report
            .download_timeline
            .as_ref()
            .unwrap()
            .iter()
            .any(|step| step.phase == "mount" && step.duration_ms == 7.0));
    }
}
