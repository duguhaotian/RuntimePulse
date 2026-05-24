//! CRI/crictl diagnostic exporter.
//!
//! This exporter captures `crictl inspect` metadata plus optional container logs
//! into local artifact files and prints a RuntimePulse diagnostic-report JSONL
//! index to stdout. It is designed to be used through the generic
//! `diagnostic-report` command hook on Kubernetes/containerd nodes.

use chrono::{DateTime, SecondsFormat, Utc};
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

use crate::collectors::core::config::CollectorConfig;
use crate::collectors::core::error::{CollectorError, Result};

const DEFAULT_OUTPUT_DIR: &str = "/tmp/runtimepulse/diagnostics/cri";
const DEFAULT_LOG_TAIL_LINES: u64 = 200;

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

#[derive(Clone, Debug)]
struct CriInspectContainer {
    id: String,
    pod_sandbox_id: Option<String>,
    runtime: String,
    runtime_type: String,
    state: String,
    reason: Option<String>,
    message: Option<String>,
    exit_code: Option<i64>,
    labels: HashMap<String, String>,
    annotations: HashMap<String, String>,
    image_ref: Option<String>,
    image_id: Option<String>,
}

pub fn emit_crictl_diagnostics(config: &CollectorConfig) -> Result<()> {
    for report in collect_crictl_diagnostics(Utc::now(), config)? {
        println!("{}", serde_json::to_string(&report)?);
    }
    Ok(())
}

pub fn collect_crictl_diagnostics(
    now: DateTime<Utc>,
    config: &CollectorConfig,
) -> Result<Vec<Value>> {
    let ids = crictl_diagnostic_container_ids()?;
    collect_crictl_diagnostics_for_ids(&ids, now, config)
}

fn collect_crictl_diagnostics_for_ids(
    ids: &[String],
    now: DateTime<Utc>,
    config: &CollectorConfig,
) -> Result<Vec<Value>> {
    let mut reports = Vec::new();
    for id in ids {
        reports.push(collect_crictl_diagnostic_for_container(id, now, config)?);
    }
    Ok(reports)
}

fn collect_crictl_diagnostic_for_container(
    id: &str,
    now: DateTime<Utc>,
    config: &CollectorConfig,
) -> Result<Value> {
    let started = Instant::now();
    let inspect_output = crictl_command(["inspect", id])?;
    let container = parse_crictl_inspect_container(&inspect_output)?;
    let sandbox_id = cri_sandbox_id(&container);
    let timestamp = timestamp(now);
    let output_dir = crictl_diagnostic_output_dir()
        .join(&sandbox_id)
        .join(sanitize_id(&timestamp));
    fs::create_dir_all(&output_dir)?;

    let mut artifacts = Vec::new();
    let inspect_path = output_dir.join("crictl-inspect.json");
    fs::write(&inspect_path, &inspect_output)?;
    artifacts.push(artifact_file(
        "crictl-inspect.json",
        "metadata",
        inspect_path,
    )?);

    let mut issues = cri_state_issues(&container);
    if crictl_diagnostic_include_logs() {
        match collect_crictl_logs(id, &output_dir) {
            Ok(Some(log_file)) => artifacts.push(log_file),
            Ok(None) => {}
            Err(error) => issues.push(DiagnosticIssueRow {
                severity: "warning".to_string(),
                category: "logs".to_string(),
                message: format!("crictl logs could not be collected: {error}"),
                attributes: json!({"cri.containerId": id}),
            }),
        }
    }

    let duration_ms = started.elapsed().as_secs_f64() * 1000.0;
    Ok(diagnostic_report_from_cri_container(
        &container,
        &timestamp,
        duration_ms,
        config,
        &output_dir,
        artifacts,
        issues,
    ))
}

fn crictl_diagnostic_container_ids() -> Result<Vec<String>> {
    if let Ok(value) = env::var("RUNTIMEPULSE_CRI_DIAGNOSTIC_CONTAINERS") {
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

    let output = crictl_command(["ps", "-a", "-q"])?;
    Ok(String::from_utf8_lossy(&output)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(ToOwned::to_owned)
        .collect())
}

fn collect_crictl_logs(id: &str, output_dir: &Path) -> Result<Option<DiagnosticArtifactFile>> {
    let tail = crictl_diagnostic_log_tail_lines().to_string();
    let output = Command::new(crictl_bin())
        .args(["logs", "--tail", &tail, id])
        .output()?;

    if !output.status.success() {
        return Err(CollectorError::Plugin {
            plugin: "crictl-diagnostics".to_string(),
            message: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        });
    }

    let mut bytes = Vec::new();
    bytes.extend_from_slice(&output.stdout);
    bytes.extend_from_slice(&output.stderr);
    if bytes.is_empty() {
        return Ok(None);
    }

    let path = output_dir.join("crictl-logs.txt");
    fs::write(&path, bytes)?;
    Ok(Some(artifact_file("crictl-logs.txt", "log", path)?))
}

fn parse_crictl_inspect_container(output: &[u8]) -> Result<CriInspectContainer> {
    let raw = serde_json::from_slice::<Value>(output)?;
    Ok(cri_container_from_value(raw))
}

fn cri_container_from_value(raw: Value) -> CriInspectContainer {
    let labels = string_map_from_value(
        first_value(
            &raw,
            &[&["status", "labels"], &["info", "config", "labels"]],
        )
        .unwrap_or(&Value::Null),
    );
    let annotations = string_map_from_value(
        first_value(
            &raw,
            &[
                &["status", "annotations"],
                &["info", "config", "annotations"],
                &["info", "runtimeSpec", "annotations"],
            ],
        )
        .unwrap_or(&Value::Null),
    );
    let id = first_string(
        &raw,
        &[
            &["status", "id"],
            &["id"],
            &["info", "config", "metadata", "uid"],
            &["info", "config", "id"],
        ],
    )
    .unwrap_or_else(|| "unknown".to_string());
    let pod_sandbox_id = first_string(
        &raw,
        &[
            &["status", "podSandboxId"],
            &["status", "podSandboxID"],
            &["info", "sandboxID"],
            &["info", "sandboxId"],
            &["info", "sandboxID"],
            &["info", "config", "sandboxID"],
        ],
    );
    let runtime = first_string(
        &raw,
        &[
            &["info", "runtimeType"],
            &["info", "runtimeName"],
            &["status", "runtimeType"],
            &["status", "runtime"],
            &["info", "config", "runtimeHandler"],
        ],
    )
    .or_else(|| labels.get("io.kubernetes.cri.runtime-handler").cloned())
    .or_else(|| {
        annotations
            .get("io.kubernetes.cri.runtime-handler")
            .cloned()
    })
    .or_else(|| {
        annotations
            .get("io.kubernetes.cri-o.RuntimeHandler")
            .cloned()
    })
    .unwrap_or_else(|| "runc".to_string());

    CriInspectContainer {
        id,
        pod_sandbox_id,
        runtime_type: runtime_type_from_cri(&runtime, &labels, &annotations),
        runtime,
        state: first_string(
            &raw,
            &[
                &["status", "state"],
                &["status", "status"],
                &["info", "state"],
            ],
        )
        .unwrap_or_else(|| "unknown".to_string()),
        reason: first_string(&raw, &[&["status", "reason"]]),
        message: first_string(&raw, &[&["status", "message"], &["status", "error"]]),
        exit_code: first_i64(&raw, &[&["status", "exitCode"], &["status", "exit_code"]]),
        image_ref: first_string(
            &raw,
            &[
                &["status", "image", "image"],
                &["status", "imageRef"],
                &["status", "image", "ref"],
                &["info", "config", "image", "image"],
            ],
        ),
        image_id: first_string(
            &raw,
            &[
                &["status", "imageRef"],
                &["status", "image", "imageRef"],
                &["info", "imageRef"],
            ],
        ),
        labels,
        annotations,
    }
}

fn diagnostic_report_from_cri_container(
    container: &CriInspectContainer,
    timestamp: &str,
    duration_ms: f64,
    config: &CollectorConfig,
    output_dir: &Path,
    artifacts: Vec<DiagnosticArtifactFile>,
    issues: Vec<DiagnosticIssueRow>,
) -> Value {
    let sandbox_id = cri_sandbox_id(container);
    let short_id = short_container_id(&container.id);
    let workload_name = cri_workload_name(container);
    let namespace = k8s_label(container, "io.kubernetes.pod.namespace");
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
    let report_id = format!("crictl-diagnostic-{short_id}-{}", sanitize_id(timestamp));
    let image_ref = container.image_ref.clone().unwrap_or_default();
    let image_id = image_id_from_cri(container);

    let mut target = Map::new();
    target.insert("nodeId".to_string(), json!(config.node_id));
    target.insert("sandboxId".to_string(), json!(sandbox_id));
    target.insert("imageId".to_string(), json!(image_id));
    target.insert("runtimeType".to_string(), json!(container.runtime_type));
    target.insert("workloadName".to_string(), json!(workload_name));
    if let Some(namespace) = namespace.as_deref() {
        target.insert("namespace".to_string(), json!(namespace));
    }

    json!({
        "id": report_id,
        "timestamp": timestamp,
        "sandboxId": sandbox_id,
        "nodeId": config.node_id,
        "runtimeType": container.runtime_type,
        "source": "crictl-diagnostics",
        "severity": if errors > 0 { "error" } else if warnings > 0 { "warning" } else { "info" },
        "reason": "crictl_diagnostics",
        "status": if errors > 0 || warnings > 0 { "degraded" } else { "captured" },
        "message": format!("CRI diagnostics captured {} artifact files for {}.", files, workload_name),
        "objectUri": file_uri(output_dir),
        "sizeBytes": size_bytes,
        "durationMs": duration_ms,
        "artifactType": "crictl_diagnostics",
        "target": Value::Object(target),
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
            "runtime": container.runtime_type,
            "collector": "crictl-diagnostics",
        },
        "attributes": {
            "cri.containerId": container.id,
            "cri.shortId": short_id,
            "cri.podSandboxId": container.pod_sandbox_id,
            "cri.state": container.state,
            "cri.runtime": container.runtime,
            "image.ref": image_ref,
            "k8s.namespace": namespace,
            "k8s.pod": k8s_label(container, "io.kubernetes.pod.name"),
            "k8s.container": k8s_label(container, "io.kubernetes.container.name"),
            "k8s.podUid": k8s_label(container, "io.kubernetes.pod.uid"),
        }
    })
}

fn cri_state_issues(container: &CriInspectContainer) -> Vec<DiagnosticIssueRow> {
    let mut issues = Vec::new();
    let state = container.state.to_ascii_lowercase();
    let reason = container.reason.clone().unwrap_or_default();
    let message = container.message.clone().unwrap_or_default();
    let reason_lower = reason.to_ascii_lowercase();
    let message_lower = message.to_ascii_lowercase();

    if reason_lower.contains("oom") || message_lower.contains("oom") {
        issues.push(DiagnosticIssueRow {
            severity: "error".to_string(),
            category: "oom".to_string(),
            message: non_empty_or(&message, "CRI reports the container was OOM killed."),
            attributes: json!({
                "cri.containerId": container.id,
                "cri.reason": reason,
                "cri.exitCode": container.exit_code,
            }),
        });
    }

    if let Some(exit_code) = container.exit_code.filter(|code| *code != 0) {
        issues.push(DiagnosticIssueRow {
            severity: "error".to_string(),
            category: "exit".to_string(),
            message: if message.is_empty() {
                format!("CRI reports non-zero container exit code {exit_code}.")
            } else {
                message.clone()
            },
            attributes: json!({
                "cri.containerId": container.id,
                "cri.reason": reason,
                "cri.exitCode": exit_code,
            }),
        });
    } else if !reason.is_empty() && !matches!(state.as_str(), "container_running" | "running") {
        issues.push(DiagnosticIssueRow {
            severity: "warning".to_string(),
            category: "state".to_string(),
            message: non_empty_or(&message, &reason),
            attributes: json!({
                "cri.containerId": container.id,
                "cri.reason": reason,
                "cri.state": container.state,
            }),
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

fn crictl_command<const N: usize>(args: [&str; N]) -> Result<Vec<u8>> {
    let output = Command::new(crictl_bin()).args(args).output()?;
    if !output.status.success() {
        return Err(CollectorError::Plugin {
            plugin: "crictl-diagnostics".to_string(),
            message: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        });
    }
    Ok(output.stdout)
}

fn crictl_bin() -> String {
    env::var("RUNTIMEPULSE_CRICTL_BIN").unwrap_or_else(|_| "crictl".to_string())
}

fn crictl_diagnostic_output_dir() -> PathBuf {
    PathBuf::from(
        env::var("RUNTIMEPULSE_CRI_DIAGNOSTIC_OUTPUT_DIR")
            .unwrap_or_else(|_| DEFAULT_OUTPUT_DIR.to_string()),
    )
}

fn crictl_diagnostic_include_logs() -> bool {
    !matches!(
        env::var("RUNTIMEPULSE_CRI_DIAGNOSTIC_INCLUDE_LOGS")
            .ok()
            .as_deref(),
        Some("0") | Some("false") | Some("FALSE") | Some("no") | Some("NO")
    )
}

fn crictl_diagnostic_log_tail_lines() -> u64 {
    env::var("RUNTIMEPULSE_CRI_DIAGNOSTIC_TAIL_LINES")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(DEFAULT_LOG_TAIL_LINES)
        .max(1)
}

fn string_map_from_value(value: &Value) -> HashMap<String, String> {
    value
        .as_object()
        .map(|object| {
            object
                .iter()
                .filter_map(|(key, value)| Some((key.clone(), value.as_str()?.to_string())))
                .collect()
        })
        .unwrap_or_default()
}

fn first_value<'a>(value: &'a Value, paths: &[&[&str]]) -> Option<&'a Value> {
    paths.iter().find_map(|path| value_at(value, path))
}

fn first_string(value: &Value, paths: &[&[&str]]) -> Option<String> {
    first_value(value, paths).and_then(string_from_value)
}

fn first_i64(value: &Value, paths: &[&[&str]]) -> Option<i64> {
    first_value(value, paths).and_then(|value| {
        value
            .as_i64()
            .or_else(|| value.as_u64().and_then(|number| i64::try_from(number).ok()))
            .or_else(|| value.as_str()?.parse::<i64>().ok())
    })
}

fn value_at<'a>(value: &'a Value, path: &[&str]) -> Option<&'a Value> {
    let mut current = value;
    for segment in path {
        current = current.get(*segment)?;
    }
    Some(current)
}

fn string_from_value(value: &Value) -> Option<String> {
    match value {
        Value::String(text) if !text.trim().is_empty() => Some(text.trim().to_string()),
        Value::Number(number) => Some(number.to_string()),
        Value::Bool(value) => Some(value.to_string()),
        _ => None,
    }
}

fn k8s_label(container: &CriInspectContainer, key: &str) -> Option<String> {
    container
        .labels
        .get(key)
        .or_else(|| container.annotations.get(key))
        .filter(|value| !value.trim().is_empty())
        .cloned()
}

fn cri_sandbox_id(container: &CriInspectContainer) -> String {
    match (
        k8s_label(container, "io.kubernetes.pod.namespace"),
        k8s_label(container, "io.kubernetes.pod.name"),
        k8s_label(container, "io.kubernetes.container.name"),
    ) {
        (Some(namespace), Some(pod), Some(container_name)) => format!(
            "k8s-{}-{}-{}",
            sanitize_id(&namespace),
            sanitize_id(&pod),
            sanitize_id(&container_name)
        ),
        _ => container
            .pod_sandbox_id
            .as_deref()
            .filter(|value| !value.is_empty())
            .map(|id| format!("cri-pod-{}", short_container_id(id)))
            .unwrap_or_else(|| format!("cri-{}", short_container_id(&container.id))),
    }
}

fn cri_workload_name(container: &CriInspectContainer) -> String {
    match (
        k8s_label(container, "io.kubernetes.pod.name"),
        k8s_label(container, "io.kubernetes.container.name"),
    ) {
        (Some(pod), Some(container_name)) => format!("{pod}/{container_name}"),
        (Some(pod), None) => pod,
        (None, Some(container_name)) => container_name,
        _ => short_container_id(&container.id),
    }
}

fn runtime_type_from_cri(
    runtime: &str,
    labels: &HashMap<String, String>,
    annotations: &HashMap<String, String>,
) -> String {
    let haystack = std::iter::once(runtime)
        .chain(labels.values().map(String::as_str))
        .chain(annotations.values().map(String::as_str))
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase();
    if haystack.contains("runsc") || haystack.contains("gvisor") {
        "gvisor".to_string()
    } else if haystack.contains("kata") {
        "kata".to_string()
    } else if haystack.contains("firecracker") {
        "firecracker".to_string()
    } else {
        "runc".to_string()
    }
}

fn image_id_from_cri(container: &CriInspectContainer) -> String {
    if let Some(image_id) = container
        .image_id
        .as_deref()
        .filter(|value| !value.is_empty())
    {
        return format!("cri-image-{}", sanitize_id(image_id));
    }
    if let Some(image_ref) = container
        .image_ref
        .as_deref()
        .filter(|value| !value.is_empty())
    {
        return format!("cri-image-{}", sanitize_id(image_ref));
    }
    format!("cri-image-{}", short_container_id(&container.id))
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

fn non_empty_or(value: &str, fallback: &str) -> String {
    if value.trim().is_empty() {
        fallback.to_string()
    } else {
        value.to_string()
    }
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
            ebpf_report_path: None,
            perf_profile_command: None,
            ebpf_profile_command: None,
            profile_command_timeout: Duration::from_secs(1),
            plugins: Vec::new(),
            command_plugins: Vec::new(),
            http_plugins: Vec::new(),
        }
    }

    #[test]
    fn builds_diagnostic_report_from_crictl_inspect_data() {
        let raw = json!({
            "status": {
                "id": "abcdef1234567890",
                "podSandboxId": "podsandbox1234567890",
                "state": "CONTAINER_EXITED",
                "reason": "OOMKilled",
                "message": "container was OOMKilled",
                "exitCode": 137,
                "image": {"image": "registry.local/demo:v1"},
                "imageRef": "sha256:demo",
                "labels": {
                    "io.kubernetes.pod.namespace": "default",
                    "io.kubernetes.pod.name": "demo",
                    "io.kubernetes.container.name": "app"
                },
                "annotations": {"io.kubernetes.cri.runtime-handler": "runsc"}
            },
            "info": {"runtimeType": "runsc"}
        });
        let container = cri_container_from_value(raw);
        let artifact = DiagnosticArtifactFile {
            name: "crictl-inspect.json".to_string(),
            artifact_type: "metadata".to_string(),
            path: PathBuf::from("/tmp/inspect.json"),
            size_bytes: 128,
        };
        let report = diagnostic_report_from_cri_container(
            &container,
            "2026-05-24T00:00:00.000Z",
            10.0,
            &test_config(),
            Path::new("/tmp/runtimepulse/diag"),
            vec![artifact],
            cri_state_issues(&container),
        );

        assert_eq!(report["sandboxId"], "k8s-default-demo-app");
        assert_eq!(report["runtimeType"], "gvisor");
        assert_eq!(report["target"]["workloadName"], "demo/app");
        assert_eq!(report["target"]["namespace"], "default");
        assert_eq!(report["artifactType"], "crictl_diagnostics");
        assert_eq!(report["summary"]["errors"], 2);
        assert_eq!(report["issues"][0]["category"], "oom");
    }

    #[test]
    fn falls_back_to_cri_pod_sandbox_id_without_kubernetes_labels() {
        let raw = json!({
            "status": {
                "id": "fedcba9876543210",
                "podSandboxId": "podsandboxabcdef1234",
                "state": "CONTAINER_RUNNING",
                "image": {"image": "busybox:latest"}
            }
        });
        let container = cri_container_from_value(raw);
        assert_eq!(cri_sandbox_id(&container), "cri-pod-podsandboxab");
        assert_eq!(cri_workload_name(&container), "fedcba987654");
    }
}
