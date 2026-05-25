//! Docker diagnostic exporter.
//!
//! This module is intentionally an exporter, not an ingest source. It creates a
//! small RuntimePulse diagnostic-report JSONL index for Docker containers and
//! writes raw inspect/log artifacts to disk. The JSONL output can be wired into
//! `diagnostic-report` through `RUNTIMEPULSE_DIAGNOSTIC_REPORT_CMD`.

use chrono::{DateTime, SecondsFormat, Utc};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

use crate::collectors::core::config::CollectorConfig;
use crate::collectors::core::error::{CollectorError, Result};

const DEFAULT_OUTPUT_DIR: &str = "/tmp/runtimepulse/diagnostics/docker";
const DEFAULT_LOG_TAIL_LINES: u64 = 200;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct DockerInspectContainer {
    id: String,
    name: String,
    image: String,
    state: Option<DockerState>,
    config: Option<DockerConfig>,
    host_config: Option<DockerHostConfig>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct DockerState {
    status: Option<String>,
    running: Option<bool>,
    #[serde(rename = "OOMKilled")]
    oom_killed: Option<bool>,
    error: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct DockerConfig {
    image: Option<String>,
    #[serde(default)]
    labels: HashMap<String, String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct DockerHostConfig {
    runtime: Option<String>,
}

#[derive(Clone, Debug)]
struct DiagnosticArtifactFile {
    name: String,
    artifact_type: String,
    path: PathBuf,
    size_bytes: u64,
}

#[derive(Clone, Debug)]
struct DiagnosticIssueRow {
    severity: String,
    category: String,
    message: String,
    attributes: Value,
}

pub fn emit_docker_diagnostics(config: &CollectorConfig) -> Result<()> {
    for report in collect_docker_diagnostics(Utc::now(), config)? {
        println!("{}", serde_json::to_string(&report)?);
    }
    Ok(())
}

pub fn collect_docker_diagnostics(
    now: DateTime<Utc>,
    config: &CollectorConfig,
) -> Result<Vec<Value>> {
    let ids = docker_diagnostic_container_ids()?;
    collect_docker_diagnostics_for_ids(&ids, now, config)
}

fn collect_docker_diagnostics_for_ids(
    ids: &[String],
    now: DateTime<Utc>,
    config: &CollectorConfig,
) -> Result<Vec<Value>> {
    let mut reports = Vec::new();
    for id in ids {
        reports.push(collect_docker_diagnostic_for_container(id, now, config)?);
    }
    Ok(reports)
}

fn collect_docker_diagnostic_for_container(
    id: &str,
    now: DateTime<Utc>,
    config: &CollectorConfig,
) -> Result<Value> {
    let started = Instant::now();
    let inspect_output = docker_command(["inspect", id])?;
    let container = parse_inspect_container(&inspect_output)?;
    let sandbox_id = docker_sandbox_id(&container.id);
    let timestamp = timestamp(now);
    let output_dir = docker_diagnostic_output_dir()
        .join(&sandbox_id)
        .join(sanitize_id(&timestamp));
    fs::create_dir_all(&output_dir)?;

    let mut artifacts = Vec::new();
    let inspect_path = output_dir.join("docker-inspect.json");
    fs::write(&inspect_path, &inspect_output)?;
    artifacts.push(artifact_file(
        "docker-inspect.json",
        "metadata",
        inspect_path,
    )?);

    let mut issues = docker_state_issues(&container);
    if docker_diagnostic_include_logs() {
        match collect_docker_logs(id, &output_dir) {
            Ok(Some(log_file)) => artifacts.push(log_file),
            Ok(None) => {}
            Err(error) => issues.push(DiagnosticIssueRow {
                severity: "warning".to_string(),
                category: "logs".to_string(),
                message: format!("Docker logs could not be collected: {error}"),
                attributes: json!({"docker.id": id}),
            }),
        }
    }

    let duration_ms = started.elapsed().as_secs_f64() * 1000.0;
    Ok(diagnostic_report_from_container(
        &container,
        &timestamp,
        duration_ms,
        config,
        &output_dir,
        artifacts,
        issues,
    ))
}

fn docker_diagnostic_container_ids() -> Result<Vec<String>> {
    if let Ok(value) = env::var("RUNTIMEPULSE_DOCKER_DIAGNOSTIC_CONTAINERS") {
        let ids = value
            .split(',')
            .map(str::trim)
            .filter(|item| !item.is_empty())
            .map(ToOwned::to_owned)
            .collect::<Vec<_>>();
        if !ids.is_empty() {
            return Ok(ids);
        }
    }

    let output = docker_command(["ps", "-aq", "--no-trunc"])?;
    Ok(String::from_utf8_lossy(&output)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(ToOwned::to_owned)
        .collect())
}

fn collect_docker_logs(id: &str, output_dir: &Path) -> Result<Option<DiagnosticArtifactFile>> {
    let tail = docker_diagnostic_log_tail_lines().to_string();
    let output = Command::new("docker")
        .args(["logs", "--tail", &tail, "--timestamps", id])
        .output()?;

    if !output.status.success() {
        return Err(CollectorError::Plugin {
            plugin: "docker-diagnostics".to_string(),
            message: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        });
    }

    let mut bytes = Vec::new();
    bytes.extend_from_slice(&output.stdout);
    bytes.extend_from_slice(&output.stderr);
    if bytes.is_empty() {
        return Ok(None);
    }

    let path = output_dir.join("docker-logs.txt");
    fs::write(&path, bytes)?;
    Ok(Some(artifact_file("docker-logs.txt", "log", path)?))
}

fn parse_inspect_container(output: &[u8]) -> Result<DockerInspectContainer> {
    let mut rows = serde_json::from_slice::<Vec<DockerInspectContainer>>(output)?;
    rows.pop().ok_or_else(|| CollectorError::Plugin {
        plugin: "docker-diagnostics".to_string(),
        message: "docker inspect returned no containers".to_string(),
    })
}

fn diagnostic_report_from_container(
    container: &DockerInspectContainer,
    timestamp: &str,
    duration_ms: f64,
    config: &CollectorConfig,
    output_dir: &Path,
    artifacts: Vec<DiagnosticArtifactFile>,
    issues: Vec<DiagnosticIssueRow>,
) -> Value {
    let sandbox_id = docker_sandbox_id(&container.id);
    let short_id = short_container_id(&container.id);
    let runtime = container
        .host_config
        .as_ref()
        .and_then(|host_config| host_config.runtime.as_deref())
        .unwrap_or("runc");
    let runtime_type = runtime_type_from_docker(runtime);
    let image_ref = container
        .config
        .as_ref()
        .and_then(|config| config.image.as_deref())
        .filter(|image| !image.is_empty())
        .unwrap_or(&container.image);
    let workload_name = docker_workload_name(container);
    let (warnings, errors) = issue_counts(&issues);
    let size_bytes = artifacts
        .iter()
        .map(|artifact| artifact.size_bytes)
        .sum::<u64>();
    let files = artifacts.len() as u64;
    let logs = artifacts
        .iter()
        .filter(|artifact| artifact.artifact_type == "log")
        .count() as u64;
    let report_id = format!("docker-diagnostic-{short_id}-{}", sanitize_id(timestamp));

    json!({
        "id": report_id,
        "timestamp": timestamp,
        "sandboxId": sandbox_id,
        "nodeId": config.node_id,
        "runtimeType": runtime_type,
        "source": "docker-diagnostics",
        "severity": if errors > 0 { "error" } else if warnings > 0 { "warning" } else { "info" },
        "reason": "docker_diagnostics",
        "status": if errors > 0 || warnings > 0 { "degraded" } else { "captured" },
        "message": format!("Docker diagnostics captured {} artifact files for {}.", files, workload_name),
        "objectUri": file_uri(output_dir),
        "sizeBytes": size_bytes,
        "durationMs": duration_ms,
        "artifactType": "docker_diagnostics",
        "target": {
            "nodeId": config.node_id,
            "sandboxId": sandbox_id,
            "imageId": image_id_from_ref_or_digest(image_ref, &container.image),
            "runtimeType": runtime_type,
            "workloadName": workload_name,
        },
        "summary": {
            "files": files,
            "logs": logs,
            "warnings": warnings,
            "errors": errors,
            "checks": 2,
            "failedChecks": warnings + errors,
        },
        "issues": issues.iter().enumerate().map(|(index, issue)| {
            json!({
                "id": format!("{report_id}-issue-{index}"),
                "severity": issue.severity,
                "category": issue.category,
                "message": issue.message,
                "count": 1,
                "attributes": issue.attributes,
            })
        }).collect::<Vec<_>>(),
        "artifacts": artifacts.iter().map(|artifact| {
            json!({
                "name": artifact.name,
                "artifactType": artifact.artifact_type,
                "objectUri": file_uri(&artifact.path),
                "sizeBytes": artifact.size_bytes,
            })
        }).collect::<Vec<_>>(),
        "labels": {
            "runtime": runtime_type,
            "collector": "docker-diagnostics",
        },
        "attributes": {
            "docker.id": container.id,
            "docker.shortId": short_id,
            "docker.name": container.name.trim_start_matches('/'),
            "docker.status": container.state.as_ref().and_then(|state| state.status.clone()).unwrap_or_else(|| "unknown".to_string()),
            "docker.running": container.state.as_ref().and_then(|state| state.running).unwrap_or(false),
            "docker.runtime": runtime,
            "image.ref": image_ref,
        }
    })
}

fn docker_state_issues(container: &DockerInspectContainer) -> Vec<DiagnosticIssueRow> {
    let mut issues = Vec::new();
    let Some(state) = &container.state else {
        return issues;
    };

    if state.oom_killed.unwrap_or(false) {
        issues.push(DiagnosticIssueRow {
            severity: "error".to_string(),
            category: "oom".to_string(),
            message: "Docker reports the container was OOM killed.".to_string(),
            attributes: json!({"docker.id": container.id}),
        });
    }

    if let Some(error) = state.error.as_deref().filter(|value| !value.is_empty()) {
        issues.push(DiagnosticIssueRow {
            severity: "error".to_string(),
            category: "runtime".to_string(),
            message: error.to_string(),
            attributes: json!({"docker.id": container.id}),
        });
    }

    issues
}

fn artifact_file(name: &str, artifact_type: &str, path: PathBuf) -> Result<DiagnosticArtifactFile> {
    let size_bytes = fs::metadata(&path)
        .map(|metadata| metadata.len())
        .unwrap_or(0);
    Ok(DiagnosticArtifactFile {
        name: name.to_string(),
        artifact_type: artifact_type.to_string(),
        path,
        size_bytes,
    })
}

fn docker_command<const N: usize>(args: [&str; N]) -> Result<Vec<u8>> {
    let output = Command::new("docker").args(args).output()?;
    if !output.status.success() {
        return Err(CollectorError::Plugin {
            plugin: "docker-diagnostics".to_string(),
            message: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        });
    }
    Ok(output.stdout)
}

fn docker_diagnostic_output_dir() -> PathBuf {
    PathBuf::from(
        env::var("RUNTIMEPULSE_DOCKER_DIAGNOSTIC_OUTPUT_DIR")
            .unwrap_or_else(|_| DEFAULT_OUTPUT_DIR.to_string()),
    )
}

fn docker_diagnostic_include_logs() -> bool {
    !matches!(
        env::var("RUNTIMEPULSE_DOCKER_DIAGNOSTIC_INCLUDE_LOGS")
            .ok()
            .as_deref(),
        Some("0") | Some("false") | Some("FALSE") | Some("no") | Some("NO")
    )
}

fn docker_diagnostic_log_tail_lines() -> u64 {
    env::var("RUNTIMEPULSE_DOCKER_DIAGNOSTIC_TAIL_LINES")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(DEFAULT_LOG_TAIL_LINES)
        .max(1)
}

fn issue_counts(issues: &[DiagnosticIssueRow]) -> (u64, u64) {
    let errors = issues
        .iter()
        .filter(|issue| matches!(issue.severity.as_str(), "error" | "critical"))
        .count() as u64;
    let warnings = issues
        .iter()
        .filter(|issue| issue.severity == "warning")
        .count() as u64;
    (warnings, errors)
}

fn docker_workload_name(container: &DockerInspectContainer) -> String {
    container
        .config
        .as_ref()
        .and_then(|config| config.labels.get("com.docker.compose.service"))
        .cloned()
        .or_else(|| {
            let name = container.name.trim_start_matches('/');
            if name.is_empty() {
                None
            } else {
                Some(name.to_string())
            }
        })
        .unwrap_or_else(|| short_container_id(&container.id))
}

fn runtime_type_from_docker(runtime: &str) -> String {
    let normalized = runtime.to_ascii_lowercase();
    if normalized.contains("runsc") || normalized.contains("gvisor") {
        "gvisor".to_string()
    } else if normalized.contains("kata") {
        "kata".to_string()
    } else if normalized.contains("firecracker") {
        "firecracker".to_string()
    } else {
        "runc".to_string()
    }
}

fn image_id_from_ref_or_digest(image_ref: &str, digest: &str) -> String {
    let source = if image_ref.is_empty() {
        digest
    } else {
        image_ref
    };
    format!("docker-image-{}", sanitize_id(source))
}

fn docker_sandbox_id(id: &str) -> String {
    format!("docker-{}", short_container_id(id))
}

fn short_container_id(id: &str) -> String {
    id.chars().take(12).collect()
}

fn file_uri(path: &Path) -> String {
    format!("file://{}", path.display())
}

fn sanitize_id(value: &str) -> String {
    let sanitized = value
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
        .to_string();
    if sanitized.is_empty() {
        "unknown".to_string()
    } else {
        sanitized
    }
}

fn timestamp(time: DateTime<Utc>) -> String {
    time.to_rfc3339_opts(SecondsFormat::Millis, true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn test_config() -> CollectorConfig {
        CollectorConfig {
            ingest_url: "http://query/api/ingest/batch".to_string(),
            node_id: "node-a".to_string(),
            cluster_id: "cluster-a".to_string(),
            interval: Duration::from_secs(1),
            local_report_addr: "127.0.0.1:9091".to_string(),
            local_report_url: "http://127.0.0.1:9091/api/local/ingest".to_string(),
            collection_scope: "host".to_string(),
            once: true,
            cgroup_root: PathBuf::from("/sys/fs/cgroup"),
            cgroup_max_entries: 1,
            image_cache_report_path: None,
            profile_report_path: None,
            diagnostic_report_path: None,
            diagnostic_report_command: None,
            diagnostic_report_command_timeout: Duration::from_secs(1),
            perf_report_path: None,
            perf_folded_path: None,
            ebpf_report_path: None,
            ebpf_folded_path: None,
            perf_profile_command: None,
            ebpf_profile_command: None,
            profile_command_timeout: Duration::from_secs(1),
            plugins: Vec::new(),
            command_plugins: Vec::new(),
            http_plugins: Vec::new(),
        }
    }

    #[test]
    fn builds_diagnostic_report_from_inspect_data() {
        let inspect = br#"[{
          "Id":"abcdef1234567890",
          "Name":"/demo",
          "Image":"sha256:image",
          "State":{"Status":"exited","Running":false,"OOMKilled":true,"Error":""},
          "Config":{"Image":"registry.local/demo:v1","Labels":{"com.docker.compose.service":"api"}},
          "HostConfig":{"Runtime":"runsc"}
        }]"#;
        let container = parse_inspect_container(inspect).unwrap();
        let artifact = DiagnosticArtifactFile {
            name: "docker-inspect.json".to_string(),
            artifact_type: "metadata".to_string(),
            path: PathBuf::from("/tmp/inspect.json"),
            size_bytes: 128,
        };
        let report = diagnostic_report_from_container(
            &container,
            "2026-05-24T00:00:00.000Z",
            10.0,
            &test_config(),
            Path::new("/tmp/runtimepulse/diag"),
            vec![artifact],
            docker_state_issues(&container),
        );

        assert_eq!(report["sandboxId"], "docker-abcdef123456");
        assert_eq!(report["runtimeType"], "gvisor");
        assert_eq!(report["target"]["workloadName"], "api");
        assert_eq!(report["summary"]["errors"], 1);
        assert_eq!(report["issues"][0]["category"], "oom");
    }
}
