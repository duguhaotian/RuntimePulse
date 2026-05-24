//! Native containerd diagnostic exporter.
//!
//! Captures containerd inventory/task/content metadata through the existing
//! containerd gRPC client and prints RuntimePulse diagnostic-report JSONL rows.
//! Raw per-container metadata is stored as local artifact files.

use chrono::{DateTime, SecondsFormat, Utc};
use serde_json::{json, Value};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use crate::collectors::core::config::CollectorConfig;
use crate::collectors::core::error::Result;
use crate::collectors::sources::runtime::containerd::{
    collect_containerd_diagnostic_targets, ContainerdDiagnosticTarget,
};

const DEFAULT_OUTPUT_DIR: &str = "/tmp/runtimepulse/diagnostics/containerd";

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

pub fn emit_containerd_diagnostics(config: &CollectorConfig) -> Result<()> {
    for report in collect_containerd_diagnostics(Utc::now(), config)? {
        println!("{}", serde_json::to_string(&report)?);
    }
    Ok(())
}

pub fn collect_containerd_diagnostics(
    now: DateTime<Utc>,
    config: &CollectorConfig,
) -> Result<Vec<Value>> {
    let targets = filter_containerd_diagnostic_targets(collect_containerd_diagnostic_targets()?);
    let mut reports = Vec::new();
    for target in targets {
        reports.push(collect_containerd_diagnostic_for_target(
            &target, now, config,
        )?);
    }
    Ok(reports)
}

fn collect_containerd_diagnostic_for_target(
    target: &ContainerdDiagnosticTarget,
    now: DateTime<Utc>,
    config: &CollectorConfig,
) -> Result<Value> {
    let started = Instant::now();
    let timestamp = timestamp(now);
    let output_dir = containerd_diagnostic_output_dir()
        .join(&target.sandbox_id)
        .join(sanitize_id(&timestamp));
    fs::create_dir_all(&output_dir)?;

    let metadata = target_metadata(target);
    let metadata_path = output_dir.join("containerd-metadata.json");
    fs::write(&metadata_path, serde_json::to_vec_pretty(&metadata)?)?;
    let artifacts = vec![artifact_file(
        "containerd-metadata.json",
        "metadata",
        metadata_path,
    )?];

    let issues = containerd_target_issues(target);
    Ok(diagnostic_report_from_containerd_target(
        target,
        &timestamp,
        started.elapsed().as_secs_f64() * 1000.0,
        config,
        &output_dir,
        artifacts,
        issues,
    ))
}

fn diagnostic_report_from_containerd_target(
    target: &ContainerdDiagnosticTarget,
    timestamp: &str,
    duration_ms: f64,
    config: &CollectorConfig,
    output_dir: &Path,
    artifacts: Vec<DiagnosticArtifactFile>,
    issues: Vec<DiagnosticIssueRow>,
) -> Value {
    let short_id = short_container_id(&target.container_id);
    let (warnings, errors) = issue_counts(&issues);
    let files = artifacts.len() as u64;
    let size_bytes = artifacts
        .iter()
        .map(|artifact| artifact.size_bytes)
        .sum::<u64>();
    let report_id = format!(
        "containerd-diagnostic-{short_id}-{}",
        sanitize_id(timestamp)
    );

    json!({
        "id": report_id,
        "timestamp": timestamp,
        "sandboxId": target.sandbox_id,
        "nodeId": config.node_id,
        "runtimeType": target.runtime_type,
        "source": "containerd-diagnostics",
        "severity": if errors > 0 { "error" } else if warnings > 0 { "warning" } else { "info" },
        "reason": "containerd_diagnostics",
        "status": if errors > 0 || warnings > 0 { "degraded" } else { "captured" },
        "message": format!("containerd diagnostics captured {} artifact files for {}.", files, target.workload_name),
        "objectUri": file_uri(output_dir),
        "sizeBytes": size_bytes,
        "durationMs": duration_ms,
        "artifactType": "containerd_diagnostics",
        "target": {
            "nodeId": config.node_id,
            "sandboxId": target.sandbox_id,
            "imageId": containerd_image_id(&target.namespace, &target.image_ref),
            "runtimeType": target.runtime_type,
            "workloadName": target.workload_name,
            "namespace": target.namespace,
        },
        "summary": {
            "files": files,
            "logs": 0,
            "warnings": warnings,
            "errors": errors,
            "checks": 3,
            "failedChecks": warnings + errors,
            "contentBytes": target.content_bytes,
            "contentRefs": target.content_count,
            "layers": target.layer_count,
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
            "runtime": target.runtime_type,
            "collector": "containerd-diagnostics",
        },
        "attributes": {
            "containerd.namespace": target.namespace,
            "containerd.id": target.container_id,
            "containerd.shortId": short_id,
            "containerd.taskStatus": target.task_status,
            "containerd.pid": target.task_pid,
            "containerd.exitStatus": target.exit_status,
            "containerd.runtime": target.runtime_name,
            "containerd.snapshotter": target.snapshotter,
            "containerd.snapshotKey": target.snapshot_key,
            "containerd.createdAt": target.created_at,
            "containerd.updatedAt": target.updated_at,
            "containerd.exitedAt": target.exited_at,
            "image.ref": target.image_ref,
            "image.digest": target.image_digest,
            "image.contentBytes": target.content_bytes,
            "image.contentRefs": target.content_count,
            "image.layers": target.layer_count,
            "k8s.namespace": target.labels.get("io.kubernetes.pod.namespace"),
            "k8s.pod": target.labels.get("io.kubernetes.pod.name"),
            "k8s.container": target.labels.get("io.kubernetes.container.name"),
            "k8s.podUid": target.labels.get("io.kubernetes.pod.uid"),
        }
    })
}

fn target_metadata(target: &ContainerdDiagnosticTarget) -> Value {
    json!({
        "containerId": target.container_id,
        "namespace": target.namespace,
        "sandboxId": target.sandbox_id,
        "workloadName": target.workload_name,
        "imageRef": target.image_ref,
        "imageDigest": target.image_digest,
        "runtimeName": target.runtime_name,
        "runtimeType": target.runtime_type,
        "snapshotter": target.snapshotter,
        "snapshotKey": target.snapshot_key,
        "labels": target.labels,
        "task": {
            "pid": target.task_pid,
            "status": target.task_status,
            "exitStatus": target.exit_status,
            "exitedAt": target.exited_at,
        },
        "timestamps": {
            "createdAt": target.created_at,
            "updatedAt": target.updated_at,
        },
        "content": {
            "digest": target.image_digest,
            "bytes": target.content_bytes,
            "refs": target.content_count,
            "layers": target.layer_count,
        }
    })
}

fn containerd_target_issues(target: &ContainerdDiagnosticTarget) -> Vec<DiagnosticIssueRow> {
    let mut issues = Vec::new();
    if target.task_status == "MISSING" {
        issues.push(DiagnosticIssueRow {
            severity: "warning".to_string(),
            category: "task".to_string(),
            message: "containerd container has no matching task in the task service.".to_string(),
            attributes: json!({
                "containerd.namespace": target.namespace,
                "containerd.id": target.container_id,
            }),
        });
    }
    if target.exit_status != 0 {
        issues.push(DiagnosticIssueRow {
            severity: "error".to_string(),
            category: "exit".to_string(),
            message: format!("containerd task exited with status {}.", target.exit_status),
            attributes: json!({
                "containerd.namespace": target.namespace,
                "containerd.id": target.container_id,
                "containerd.exitStatus": target.exit_status,
            }),
        });
    }
    if target.image_digest == "containerd:unknown" || target.content_count == 0 {
        issues.push(DiagnosticIssueRow {
            severity: "warning".to_string(),
            category: "content".to_string(),
            message: "containerd content metadata could not be correlated for the container image."
                .to_string(),
            attributes: json!({
                "containerd.namespace": target.namespace,
                "containerd.id": target.container_id,
                "image.ref": target.image_ref,
            }),
        });
    }
    issues
}

fn filter_containerd_diagnostic_targets(
    targets: Vec<ContainerdDiagnosticTarget>,
) -> Vec<ContainerdDiagnosticTarget> {
    let Some(filter) = containerd_diagnostic_container_filter() else {
        return targets;
    };
    targets
        .into_iter()
        .filter(|target| {
            filter.iter().any(|value| {
                target.container_id == *value
                    || target.sandbox_id == *value
                    || target.workload_name == *value
                    || short_container_id(&target.container_id) == *value
            })
        })
        .collect()
}

fn containerd_diagnostic_container_filter() -> Option<Vec<String>> {
    let value = env::var("RUNTIMEPULSE_CONTAINERD_DIAGNOSTIC_CONTAINERS").ok()?;
    let ids = value
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    if ids.is_empty() {
        None
    } else {
        Some(ids)
    }
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

fn containerd_diagnostic_output_dir() -> PathBuf {
    PathBuf::from(
        env::var("RUNTIMEPULSE_CONTAINERD_DIAGNOSTIC_OUTPUT_DIR")
            .unwrap_or_else(|_| DEFAULT_OUTPUT_DIR.to_string()),
    )
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

fn containerd_image_id(namespace: &str, image_ref: &str) -> String {
    format!(
        "containerd-image-{}-{}",
        sanitize_id(namespace),
        sanitize_id(image_ref)
    )
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
    use std::collections::HashMap;
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
            perf_profile_command: None,
            ebpf_profile_command: None,
            profile_command_timeout: Duration::from_secs(1),
            plugins: Vec::new(),
            command_plugins: Vec::new(),
            http_plugins: Vec::new(),
        }
    }

    #[test]
    fn builds_containerd_diagnostic_report() {
        let mut labels = HashMap::new();
        labels.insert(
            "io.kubernetes.pod.namespace".to_string(),
            "default".to_string(),
        );
        labels.insert("io.kubernetes.pod.name".to_string(), "demo".to_string());
        labels.insert(
            "io.kubernetes.container.name".to_string(),
            "app".to_string(),
        );
        let target = ContainerdDiagnosticTarget {
            container_id: "abcdef1234567890".to_string(),
            namespace: "k8s.io".to_string(),
            sandbox_id: "k8s-default-demo-app".to_string(),
            workload_name: "demo/app".to_string(),
            image_ref: "registry.local/demo:v1".to_string(),
            image_digest: "sha256:demo".to_string(),
            runtime_name: "io.containerd.runsc.v1".to_string(),
            runtime_type: "gvisor".to_string(),
            snapshotter: "overlayfs".to_string(),
            snapshot_key: "snap-key".to_string(),
            labels,
            task_pid: 1234,
            task_status: "STOPPED".to_string(),
            exit_status: 137,
            created_at: Some("2026-05-24T00:00:00.000Z".to_string()),
            updated_at: None,
            exited_at: Some("2026-05-24T00:01:00.000Z".to_string()),
            content_bytes: 4096,
            content_count: 2,
            layer_count: 1,
        };
        let artifact = DiagnosticArtifactFile {
            name: "containerd-metadata.json".to_string(),
            artifact_type: "metadata".to_string(),
            path: PathBuf::from("/tmp/containerd-metadata.json"),
            size_bytes: 128,
        };
        let report = diagnostic_report_from_containerd_target(
            &target,
            "2026-05-24T00:00:00.000Z",
            5.0,
            &test_config(),
            Path::new("/tmp/runtimepulse/containerd"),
            vec![artifact],
            containerd_target_issues(&target),
        );

        assert_eq!(report["sandboxId"], "k8s-default-demo-app");
        assert_eq!(report["runtimeType"], "gvisor");
        assert_eq!(report["artifactType"], "containerd_diagnostics");
        assert_eq!(report["summary"]["errors"], 1);
        assert_eq!(report["issues"][0]["category"], "exit");
        assert_eq!(report["target"]["namespace"], "k8s.io");
    }
}
