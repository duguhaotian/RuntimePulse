//! Docker raw event stream.
//!
//! Runtime sources own the Docker event stream. Domain modules consume parsed
//! events and convert them into RuntimePulse container, image, or sampler output.

use chrono::{DateTime, Utc};
use serde::Deserialize;
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
        metadata: Metadata::default(),
        metrics: Vec::new(),
        events: Vec::new(),
        traces: Vec::new(),
        profiles: Vec::new(),
    }
}

pub fn merge_output(target: &mut PluginOutput, output: PluginOutput) {
    extend_unique_by_id(&mut target.metadata.clusters, output.metadata.clusters);
    extend_unique_by_id(&mut target.metadata.nodes, output.metadata.nodes);
    extend_unique_by_id(&mut target.metadata.images, output.metadata.images);
    extend_unique_by_id(&mut target.metadata.sandboxes, output.metadata.sandboxes);
    target.metrics.extend(output.metrics);
    target.events.extend(output.events);
    target.traces.extend(output.traces);
    target.profiles.extend(output.profiles);
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
            *existing = row;
        } else {
            target.push(row);
        }
    }
}
