//! Image layer helpers.
//!
//! Docker exposes rootfs layer digests through `docker image inspect` and
//! history rows through `docker history`. This module converts those values
//! into RuntimePulse image metadata without inventing pull/download timings.

use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};
use std::process::Command;

use crate::collectors::core::error::{CollectorError, Result};

const BLOCK_SIZE_BYTES: u64 = 128 * 1024;

#[derive(Clone, Debug)]
pub struct DockerImageCandidate {
    pub id: String,
    pub reference: String,
    pub digest: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct DockerInspectImage {
    id: String,
    repo_tags: Option<Vec<String>>,
    repo_digests: Option<Vec<String>>,
    #[serde(default)]
    size: u64,
    #[serde(default, rename = "RootFS")]
    root_fs: DockerRootFs,
    #[serde(default)]
    config: DockerImageConfig,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct DockerRootFs {
    #[serde(default)]
    layers: Vec<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct DockerImageConfig {
    labels: Option<HashMap<String, String>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct DockerHistoryRow {
    #[serde(rename = "ID")]
    id: String,
    created_by: String,
    size: String,
}

pub fn docker_image_metadata_rows(
    candidates: Vec<DockerImageCandidate>,
) -> Result<BTreeMap<String, Value>> {
    let mut rows = BTreeMap::new();

    for candidate in candidates {
        if rows.contains_key(&candidate.id) {
            continue;
        }
        let row = docker_image_metadata_row(&candidate)?;
        rows.insert(candidate.id.clone(), row);
    }

    Ok(rows)
}

fn docker_image_metadata_row(candidate: &DockerImageCandidate) -> Result<Value> {
    let inspected = inspect_docker_image(&candidate.reference)
        .or_else(|_| inspect_docker_image(&candidate.digest))
        .ok();
    let history = docker_image_history(&candidate.reference)
        .or_else(|_| docker_image_history(&candidate.digest))
        .unwrap_or_default();

    let size_bytes = inspected.as_ref().map(|image| image.size).unwrap_or(0);
    let layer_digests = inspected
        .as_ref()
        .map(|image| image.root_fs.layers.clone())
        .unwrap_or_default();
    let layer_count = if layer_digests.is_empty() {
        non_empty_history_layer_count(&history)
    } else {
        layer_digests.len()
    };
    let loading_mode = inspected
        .as_ref()
        .map(|image| loading_mode(candidate, image))
        .unwrap_or("eager");
    let digest = inspected
        .as_ref()
        .and_then(best_repo_digest)
        .unwrap_or_else(|| candidate.digest.clone());
    let reference = inspected
        .as_ref()
        .and_then(best_repo_tag)
        .unwrap_or_else(|| candidate.reference.clone());
    let layers = image_layers(candidate, &layer_digests, &history);

    Ok(json!({
        "id": candidate.id,
        "ref": reference,
        "digest": digest,
        "loadingMode": loading_mode,
        "sizeBytes": size_bytes,
        "layerCount": layer_count,
        "layers": layers,
    }))
}

fn inspect_docker_image(reference: &str) -> Result<DockerInspectImage> {
    if reference.is_empty() {
        return Err(CollectorError::Plugin {
            plugin: "docker-image".to_string(),
            message: "image reference is empty".to_string(),
        });
    }

    let output = Command::new("docker")
        .args(["image", "inspect", reference])
        .output()?;

    if !output.status.success() {
        return Err(CollectorError::Plugin {
            plugin: "docker-image".to_string(),
            message: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        });
    }

    let images = serde_json::from_slice::<Vec<DockerInspectImage>>(&output.stdout)?;
    images
        .into_iter()
        .next()
        .ok_or_else(|| CollectorError::Plugin {
            plugin: "docker-image".to_string(),
            message: format!("docker image inspect returned no rows for {reference}"),
        })
}

fn docker_image_history(reference: &str) -> Result<Vec<DockerHistoryRow>> {
    if reference.is_empty() {
        return Ok(Vec::new());
    }

    let output = Command::new("docker")
        .args(["history", reference, "--no-trunc", "--format", "{{json .}}"])
        .output()?;

    if !output.status.success() {
        return Err(CollectorError::Plugin {
            plugin: "docker-image".to_string(),
            message: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        });
    }

    let mut rows = Vec::new();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        if line.trim().is_empty() {
            continue;
        }
        rows.push(serde_json::from_str::<DockerHistoryRow>(line)?);
    }
    rows.reverse();
    Ok(rows)
}

fn image_layers(
    candidate: &DockerImageCandidate,
    layer_digests: &[String],
    history: &[DockerHistoryRow],
) -> Vec<Value> {
    let non_empty_history = history
        .iter()
        .filter(|row| parse_size_bytes(&row.size) > 0)
        .collect::<Vec<_>>();

    if !non_empty_history.is_empty() {
        return non_empty_history
            .iter()
            .enumerate()
            .map(|(index, row)| {
                let digest = layer_digests
                    .get(index)
                    .cloned()
                    .unwrap_or_else(|| row.id.clone());
                layer_row(
                    candidate,
                    index,
                    &digest,
                    &row.created_by,
                    parse_size_bytes(&row.size),
                )
            })
            .collect();
    }

    layer_digests
        .iter()
        .enumerate()
        .map(|(index, digest)| layer_row(candidate, index, digest, "rootfs layer", 0))
        .collect()
}

fn layer_row(
    candidate: &DockerImageCandidate,
    index: usize,
    digest: &str,
    command: &str,
    size_bytes: u64,
) -> Value {
    let block_count = block_count(size_bytes);

    json!({
        "id": format!("{}-layer-{}", candidate.id, index + 1),
        "command": shorten(command, 140),
        "sizeBytes": size_bytes,
        "blockSizeBytes": BLOCK_SIZE_BYTES,
        "blockCount": block_count,
        "requestedBlockCount": 0,
        "cacheHitBlockCount": 0,
        "localReadBytes": 0,
        "remoteReadBytes": 0,
        "pullDurationMs": 0,
        "unpackDurationMs": 0,
        "digest": digest,
    })
}

fn loading_mode<'a>(candidate: &DockerImageCandidate, image: &'a DockerInspectImage) -> &'a str {
    if image
        .config
        .labels
        .as_ref()
        .and_then(|labels| labels.get("runtimepulse.image.loading_mode"))
        .map(|value| value.eq_ignore_ascii_case("lazy"))
        .unwrap_or(false)
        || image
            .config
            .labels
            .as_ref()
            .and_then(|labels| labels.get("containerd.io/snapshot/remote"))
            .map(|value| value == "true")
            .unwrap_or(false)
        || [
            candidate.reference.as_str(),
            candidate.digest.as_str(),
            image.id.as_str(),
        ]
        .iter()
        .any(|value| {
            let lower = value.to_ascii_lowercase();
            lower.contains("nydus")
                || lower.contains("stargz")
                || lower.contains("estargz")
                || lower.contains("overlaybd")
        })
    {
        "lazy"
    } else {
        "eager"
    }
}

fn best_repo_digest(image: &DockerInspectImage) -> Option<String> {
    image
        .repo_digests
        .as_ref()?
        .iter()
        .find(|value| !value.is_empty())
        .cloned()
}

fn best_repo_tag(image: &DockerInspectImage) -> Option<String> {
    image
        .repo_tags
        .as_ref()?
        .iter()
        .find(|value| !value.is_empty() && *value != "<none>:<none>")
        .cloned()
}

fn non_empty_history_layer_count(history: &[DockerHistoryRow]) -> usize {
    history
        .iter()
        .filter(|row| parse_size_bytes(&row.size) > 0)
        .count()
}

fn block_count(size_bytes: u64) -> u64 {
    if size_bytes == 0 {
        0
    } else {
        size_bytes.div_ceil(BLOCK_SIZE_BYTES)
    }
}

fn parse_size_bytes(value: &str) -> u64 {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return 0;
    }

    let split = trimmed
        .char_indices()
        .find(|(_, char)| !char.is_ascii_digit() && *char != '.')
        .map(|(index, _)| index)
        .unwrap_or(trimmed.len());
    let (number, unit) = trimmed.split_at(split);
    let Ok(number) = number.parse::<f64>() else {
        return 0;
    };
    let multiplier = match unit.trim().to_ascii_lowercase().as_str() {
        "b" | "" => 1.0,
        "kb" => 1_000.0,
        "mb" => 1_000_000.0,
        "gb" => 1_000_000_000.0,
        "tb" => 1_000_000_000_000.0,
        "kib" => 1024.0,
        "mib" => 1024.0 * 1024.0,
        "gib" => 1024.0 * 1024.0 * 1024.0,
        "tib" => 1024.0 * 1024.0 * 1024.0 * 1024.0,
        _ => 1.0,
    };
    (number * multiplier).round().max(0.0) as u64
}

fn shorten(value: &str, max_chars: usize) -> String {
    let clean = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if clean.chars().count() <= max_chars {
        return clean;
    }
    clean.chars().take(max_chars).collect()
}
