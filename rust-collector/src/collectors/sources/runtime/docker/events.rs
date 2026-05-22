//! Docker raw event stream.
//!
//! Runtime sources own the Docker event stream. Domain modules consume parsed
//! events and convert them into RuntimePulse container, image, or sampler output.

use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};

use crate::collectors::core::config::CollectorConfig;
use crate::collectors::core::error::{CollectorError, Result};
use crate::collectors::core::model::{Metadata, PluginOutput};

#[derive(Clone, Debug, Deserialize)]
pub struct DockerEvent {
    #[serde(default, rename = "Type")]
    pub event_type: String,
    #[serde(default, rename = "Action")]
    pub action: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub id: String,
    #[serde(default, rename = "from")]
    pub image: String,
    #[serde(default, rename = "Actor")]
    pub actor: DockerEventActor,
    #[serde(default)]
    pub scope: String,
    #[serde(default)]
    pub time: i64,
    #[serde(default, rename = "timeNano")]
    pub time_nano: i64,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct DockerEventActor {
    #[serde(default, rename = "ID")]
    pub id: String,
    #[serde(default, rename = "Attributes")]
    pub attributes: HashMap<String, String>,
}

pub fn collect_recent_docker_events<F>(
    now: DateTime<Utc>,
    config: &CollectorConfig,
    mut on_event: F,
) -> Result<PluginOutput>
where
    F: FnMut(DockerEvent, &CollectorConfig) -> Result<Option<PluginOutput>>,
{
    let since = now.timestamp() - config.interval.as_secs() as i64;
    let until = now.timestamp() + 1;
    let output = docker_events_command()
        .args(["--since", &since.to_string(), "--until", &until.to_string()])
        .output()?;

    if !output.status.success() {
        return Err(CollectorError::Plugin {
            plugin: "docker-events".to_string(),
            message: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        });
    }

    let mut combined = empty_output(config);
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let Some(event) = event_from_line(line)? else {
            continue;
        };
        if let Some(output) = on_event(event, config)? {
            merge_output(&mut combined, output);
        }
    }

    Ok(combined)
}

pub fn stream_docker_events<F>(_config: &CollectorConfig, mut on_event: F) -> Result<()>
where
    F: FnMut(DockerEvent) -> Result<()>,
{
    let mut child = docker_events_command().stdout(Stdio::piped()).spawn()?;
    let stdout = child.stdout.take().ok_or_else(|| CollectorError::Plugin {
        plugin: "docker-events".to_string(),
        message: "docker events did not expose stdout".to_string(),
    })?;

    for line in BufReader::new(stdout).lines() {
        if let Some(event) = event_from_line(&line?)? {
            on_event(event)?;
        }
    }

    let status = child.wait()?;
    if !status.success() {
        return Err(CollectorError::Plugin {
            plugin: "docker-events".to_string(),
            message: format!("docker events exited with status {status}"),
        });
    }

    Ok(())
}

pub fn empty_output(_config: &CollectorConfig) -> PluginOutput {
    PluginOutput {
        source: None,
        metadata: Metadata::default(),
        metrics: Vec::new(),
        events: Vec::new(),
        traces: Vec::new(),
        profiles: Vec::new(),
    }
}

pub fn merge_output(target: &mut PluginOutput, output: PluginOutput) {
    merge_source(&mut target.source, output.source);
    extend_unique_by_id(&mut target.metadata.clusters, output.metadata.clusters);
    extend_unique_by_id(&mut target.metadata.nodes, output.metadata.nodes);
    extend_unique_by_id(&mut target.metadata.images, output.metadata.images);
    extend_unique_by_id(&mut target.metadata.sandboxes, output.metadata.sandboxes);
    target.metrics.extend(output.metrics);
    target.events.extend(output.events);
    target.traces.extend(output.traces);
    target.profiles.extend(output.profiles);
}

fn merge_source(target: &mut Option<String>, source: Option<String>) {
    let Some(source) = source else {
        return;
    };
    match target {
        None => *target = Some(source),
        Some(existing) if existing == &source => {}
        Some(_) => *target = None,
    }
}

fn docker_events_command() -> Command {
    let mut command = Command::new("docker");
    command.args(["events", "--format", "{{json .}}"]);
    command
}

fn event_from_line(line: &str) -> Result<Option<DockerEvent>> {
    let line = line.trim();
    if line.is_empty() {
        return Ok(None);
    }
    Ok(Some(serde_json::from_str::<DockerEvent>(line)?))
}

fn extend_unique_by_id(target: &mut Vec<serde_json::Value>, rows: Vec<serde_json::Value>) {
    for row in rows {
        let Some(id) = row.get("id").and_then(serde_json::Value::as_str) else {
            target.push(row);
            continue;
        };
        if let Some(existing) = target
            .iter_mut()
            .find(|item| item.get("id").and_then(serde_json::Value::as_str) == Some(id))
        {
            *existing = merge_metadata_row(existing, row);
        } else {
            target.push(row);
        }
    }
}

fn merge_metadata_row(existing: &Value, incoming: Value) -> Value {
    let (Some(existing_object), Some(incoming_object)) =
        (existing.as_object(), incoming.as_object())
    else {
        return incoming;
    };
    let mut merged = existing_object.clone();
    for (key, value) in incoming_object {
        if meaningful_json_value(value) || !merged.contains_key(key) {
            merged.insert(key.clone(), value.clone());
        }
    }
    Value::Object(merged)
}

fn meaningful_json_value(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Number(number) => number.as_f64().is_some_and(|value| value != 0.0),
        Value::String(value) => meaningful_string(value),
        Value::Array(values) => !values.is_empty(),
        Value::Object(values) => !values.is_empty(),
        Value::Bool(_) => true,
    }
}

fn meaningful_string(value: &str) -> bool {
    !value.is_empty()
        && !value.ends_with("-observed")
        && value != "collector-observed"
        && !value.starts_with("collector:")
        && value != "containerd:unknown"
        && value != "collector/unknown:latest"
        && value != "containerd/unknown:latest"
        && value != "docker/unknown:latest"
}
