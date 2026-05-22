//! Sandbox cgroupfs source.
//!
//! Samples exact cgroup paths resolved from runtime inventory. This source must
//! not scan the host cgroup tree broadly.

use chrono::{DateTime, SecondsFormat, Utc};
use serde::Deserialize;
use serde_json::{json, Map};
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

use crate::collectors::core::config::CollectorConfig;
use crate::collectors::core::error::{CollectorError, Result};
use crate::collectors::core::model::{EventRecord, Metadata, PluginOutput};
use crate::collectors::core::plugin::CollectorPlugin;
use crate::collectors::core::report::metric;
use crate::collectors::sources::image::layer::{docker_image_metadata_rows, DockerImageCandidate};

pub struct DockerSandboxCgroupfsPlugin {
    root: PathBuf,
    last_seen: Option<Instant>,
    last_cpu_usage_by_sandbox: HashMap<String, u64>,
    last_io_read_by_sandbox: HashMap<String, u64>,
    last_io_write_by_sandbox: HashMap<String, u64>,
    last_network_rx_by_sandbox: HashMap<String, u64>,
    last_network_tx_by_sandbox: HashMap<String, u64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct DockerInspectContainer {
    id: String,
    name: String,
    image: String,
    state: DockerState,
    config: DockerConfig,
    host_config: DockerHostConfig,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct DockerState {
    running: bool,
    #[serde(default)]
    pid: u64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct DockerConfig {
    image: String,
    #[serde(default)]
    labels: HashMap<String, String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct DockerHostConfig {
    runtime: String,
}

struct SandboxCgroupTarget {
    sandbox_id: String,
    runtime_source: String,
    runtime_id_key: String,
    runtime_id: String,
    runtime_name_key: String,
    runtime_name: String,
    workload_name: String,
    namespace: String,
    image_id: String,
    image_ref: String,
    runtime_type: String,
    runtime_version: String,
    pid: u64,
    cgroup_path: PathBuf,
    cgroup_relative_path: String,
}

#[derive(Clone, Debug)]
pub struct ContainerdSandboxCgroupTarget {
    pub sandbox_id: String,
    pub containerd_id: String,
    pub containerd_namespace: String,
    pub workload_name: String,
    pub namespace: String,
    pub image_id: String,
    pub image_ref: String,
    pub runtime_type: String,
    pub runtime_version: String,
    pub pid: u64,
}

struct SandboxCgroupSample {
    cpu_usage_usec: Option<u64>,
    memory_current: Option<u64>,
    io_read_bytes: Option<u64>,
    io_write_bytes: Option<u64>,
    network_rx_bytes: Option<u64>,
    network_tx_bytes: Option<u64>,
    process_count: Option<u64>,
}

impl DockerSandboxCgroupfsPlugin {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            last_seen: None,
            last_cpu_usage_by_sandbox: HashMap::new(),
            last_io_read_by_sandbox: HashMap::new(),
            last_io_write_by_sandbox: HashMap::new(),
            last_network_rx_by_sandbox: HashMap::new(),
            last_network_tx_by_sandbox: HashMap::new(),
        }
    }

    pub fn collect_for_docker_ids(
        &mut self,
        now: DateTime<Utc>,
        config: &CollectorConfig,
        docker_ids: &[String],
    ) -> Result<PluginOutput> {
        collect_cgroup_targets(
            self,
            now,
            config,
            docker_cgroup_targets_for_ids(&self.root, docker_ids)?,
        )
    }

    pub fn collect_for_containerd_targets(
        &mut self,
        now: DateTime<Utc>,
        config: &CollectorConfig,
        targets: &[ContainerdSandboxCgroupTarget],
    ) -> Result<PluginOutput> {
        collect_cgroup_targets_with_source(
            self,
            now,
            config,
            containerd_cgroup_targets_for_targets(&self.root, targets),
            Some("containerd"),
        )
    }
}

impl CollectorPlugin for DockerSandboxCgroupfsPlugin {
    fn name(&self) -> &str {
        "docker-sandbox-cgroupfs"
    }

    fn collect(&mut self, now: DateTime<Utc>, config: &CollectorConfig) -> Result<PluginOutput> {
        let targets = docker_cgroup_targets(&self.root)?;
        collect_cgroup_targets(self, now, config, targets)
    }
}

fn collect_cgroup_targets(
    plugin: &mut DockerSandboxCgroupfsPlugin,
    now: DateTime<Utc>,
    config: &CollectorConfig,
    targets: Vec<SandboxCgroupTarget>,
) -> Result<PluginOutput> {
    collect_cgroup_targets_with_source(plugin, now, config, targets, None)
}

fn collect_cgroup_targets_with_source(
    plugin: &mut DockerSandboxCgroupfsPlugin,
    now: DateTime<Utc>,
    config: &CollectorConfig,
    targets: Vec<SandboxCgroupTarget>,
    runtime_source_hint: Option<&str>,
) -> Result<PluginOutput> {
    let ts = timestamp(now);
    let sample_interval = plugin
        .last_seen
        .map(|seen| seen.elapsed().as_secs_f64())
        .unwrap_or(0.0);
    let mut image_candidates = BTreeMap::new();
    let mut sandboxes = Vec::new();
    let mut sampled_sandbox_ids = Vec::new();
    let mut metrics = Vec::new();
    let target_count = targets.len();
    let sample_plugin = sample_plugin_name(&targets, runtime_source_hint);

    for target in targets {
        let Some(sample) = read_sandbox_cgroup_sample(&target.cgroup_path, target.pid) else {
            continue;
        };
        sampled_sandbox_ids.push(target.sandbox_id.clone());

        image_candidates
            .entry(target.image_id.clone())
            .or_insert_with(|| DockerImageCandidate {
                id: target.image_id.clone(),
                reference: target.image_ref.clone(),
                digest: format!("collector:{}", target.image_id),
            });

        let mut attributes = Map::new();
        attributes.insert(
            "collector.scope".to_string(),
            json!(config.collection_scope),
        );
        attributes.insert("runtime.source".to_string(), json!(target.runtime_source));
        attributes.insert(
            "snapshot.scope".to_string(),
            json!(format!("{}-running", target.runtime_source)),
        );
        attributes.insert("lifecycle.current".to_string(), json!(true));
        attributes.insert(target.runtime_id_key.clone(), json!(target.runtime_id));
        attributes.insert(target.runtime_name_key.clone(), json!(target.runtime_name));
        attributes.insert(
            "cgroup.path".to_string(),
            json!(target.cgroup_relative_path),
        );

        sandboxes.push(json!({
            "id": target.sandbox_id,
            "clusterId": config.cluster_id,
            "nodeId": config.node_id,
            "namespace": target.namespace,
            "workloadId": target.workload_name,
            "workloadName": target.workload_name,
            "imageId": target.image_id,
            "imageRef": target.image_ref,
            "runtimeType": target.runtime_type,
            "runtimeVersion": target.runtime_version,
            "status": "running",
            "startupDurationMs": 0,
            "cpuAvg": 0,
            "memoryPeakBytes": sample.memory_current.unwrap_or(0),
            "labels": {
                "collector": "runtimepulse-rust-collector",
                "plugin": format!("{}-sandbox-cgroupfs", target.runtime_source),
                "scope": config.collection_scope,
            },
            "attributes": attributes
        }));

        let previous_cpu = plugin.last_cpu_usage_by_sandbox.insert(
            target.sandbox_id.clone(),
            sample.cpu_usage_usec.unwrap_or(0),
        );
        let cpu_ratio = match (previous_cpu, sample.cpu_usage_usec) {
            (Some(previous), Some(current)) if sample_interval > 0.0 => {
                (current.saturating_sub(previous) as f64 / 1_000_000.0 / sample_interval).max(0.0)
            }
            _ => 0.0,
        };

        metrics.push(metric(
            &ts,
            "sandbox.cpu.usage_ratio",
            cpu_ratio,
            "ratio",
            "cpu",
            &config.node_id,
            &target.sandbox_id,
            &target.runtime_type,
        ));

        if let Some(memory_current) = sample.memory_current {
            metrics.push(metric(
                &ts,
                "sandbox.memory.working_set_bytes",
                memory_current as f64,
                "bytes",
                "memory",
                &config.node_id,
                &target.sandbox_id,
                &target.runtime_type,
            ));
        }

        if let Some(read_bytes) = sample.io_read_bytes {
            let previous = plugin
                .last_io_read_by_sandbox
                .insert(target.sandbox_id.clone(), read_bytes);
            metrics.push(metric(
                &ts,
                "sandbox.io.read_bytes",
                rate(previous, read_bytes, sample_interval),
                "bytes/s",
                "io",
                &config.node_id,
                &target.sandbox_id,
                &target.runtime_type,
            ));
        }

        if let Some(write_bytes) = sample.io_write_bytes {
            let previous = plugin
                .last_io_write_by_sandbox
                .insert(target.sandbox_id.clone(), write_bytes);
            metrics.push(metric(
                &ts,
                "sandbox.io.write_bytes",
                rate(previous, write_bytes, sample_interval),
                "bytes/s",
                "io",
                &config.node_id,
                &target.sandbox_id,
                &target.runtime_type,
            ));
        }

        if let Some(rx_bytes) = sample.network_rx_bytes {
            let previous = plugin
                .last_network_rx_by_sandbox
                .insert(target.sandbox_id.clone(), rx_bytes);
            metrics.push(metric(
                &ts,
                "sandbox.network.rx_bytes",
                rate(previous, rx_bytes, sample_interval),
                "bytes/s",
                "network",
                &config.node_id,
                &target.sandbox_id,
                &target.runtime_type,
            ));
            metrics.push(metric(
                &ts,
                "sandbox.network.rx_total_bytes",
                rx_bytes as f64,
                "bytes",
                "network",
                &config.node_id,
                &target.sandbox_id,
                &target.runtime_type,
            ));
        }

        if let Some(tx_bytes) = sample.network_tx_bytes {
            let previous = plugin
                .last_network_tx_by_sandbox
                .insert(target.sandbox_id.clone(), tx_bytes);
            metrics.push(metric(
                &ts,
                "sandbox.network.tx_bytes",
                rate(previous, tx_bytes, sample_interval),
                "bytes/s",
                "network",
                &config.node_id,
                &target.sandbox_id,
                &target.runtime_type,
            ));
            metrics.push(metric(
                &ts,
                "sandbox.network.tx_total_bytes",
                tx_bytes as f64,
                "bytes",
                "network",
                &config.node_id,
                &target.sandbox_id,
                &target.runtime_type,
            ));
        }

        if let Some(process_count) = sample.process_count {
            metrics.push(metric(
                &ts,
                "sandbox.process.count",
                process_count as f64,
                "count",
                "runtime",
                &config.node_id,
                &target.sandbox_id,
                &target.runtime_type,
            ));
        }
    }

    plugin.last_seen = Some(Instant::now());
    retain_seen(&mut plugin.last_cpu_usage_by_sandbox, &sandboxes);
    retain_seen(&mut plugin.last_io_read_by_sandbox, &sandboxes);
    retain_seen(&mut plugin.last_io_write_by_sandbox, &sandboxes);
    retain_seen(&mut plugin.last_network_rx_by_sandbox, &sandboxes);
    retain_seen(&mut plugin.last_network_tx_by_sandbox, &sandboxes);

    let mut attributes = Map::new();
    attributes.insert("plugin".to_string(), json!(sample_plugin));
    attributes.insert("scope".to_string(), json!(config.collection_scope));
    attributes.insert("targetCount".to_string(), json!(target_count));
    attributes.insert("sampleCount".to_string(), json!(sandboxes.len()));
    attributes.insert("resolver".to_string(), json!("runtime-pid-cgroup"));
    attributes.insert("snapshot.nodeId".to_string(), json!(config.node_id));
    attributes.insert(
        "snapshot.sandboxIds".to_string(),
        json!(sampled_sandbox_ids),
    );

    let events = vec![EventRecord {
        id: format!("docker-sandbox-cgroupfs-observed-{}", now.timestamp()),
        timestamp: ts,
        severity: "info".to_string(),
        event_type: "collector".to_string(),
        event_name: "sandbox.cgroupfs.sample.observed".to_string(),
        message: "Sandbox cgroupfs collector sampled runtime-resolved cgroups".to_string(),
        source: format!(
            "runtimepulse-rust-collector/{}/{}",
            config.node_id, sample_plugin
        ),
        attributes,
        sandbox_id: None,
        image_id: None,
        node_id: Some(config.node_id.clone()),
        runtime_type: None,
        reason: None,
    }];

    Ok(PluginOutput {
        source: None,
        metadata: Metadata {
            clusters: vec![json!({
                "id": config.cluster_id,
                "name": config.cluster_id,
                "environment": "collector"
            })],
            nodes: vec![json!({
                "id": config.node_id,
                "clusterId": config.cluster_id,
                "name": config.node_id,
                "status": "ready",
                "labels": {
                    "collector": "runtimepulse-rust-collector",
                    "plugin": "docker-sandbox-cgroupfs",
                    "scope": config.collection_scope,
                }
            })],
            images: image_metadata_rows(image_candidates.into_values().collect()),
            sandboxes,
        },
        metrics,
        events,
        traces: Vec::new(),
        profiles: Vec::new(),
    })
}

fn sample_plugin_name(
    targets: &[SandboxCgroupTarget],
    runtime_source_hint: Option<&str>,
) -> &'static str {
    if runtime_source_hint == Some("containerd")
        || targets
            .iter()
            .any(|target| target.runtime_source == "containerd")
    {
        "containerd-sandbox-cgroupfs"
    } else {
        "docker-sandbox-cgroupfs"
    }
}

fn image_metadata_rows(candidates: Vec<DockerImageCandidate>) -> Vec<serde_json::Value> {
    let fallback_rows = candidates
        .iter()
        .map(|candidate| {
            json!({
                "id": candidate.id,
                "ref": candidate.reference,
                "digest": candidate.digest,
                "loadingMode": "eager",
                "sizeBytes": 0,
                "layerCount": 0,
                "attributes": {
                    "collector.source": "sandbox-cgroupfs",
                }
            })
        })
        .collect::<Vec<_>>();

    match docker_image_metadata_rows(candidates) {
        Ok(rows) => rows.into_values().collect(),
        Err(_) => fallback_rows,
    }
}

fn docker_cgroup_targets(root: &Path) -> Result<Vec<SandboxCgroupTarget>> {
    let ids = docker_running_container_ids()?;
    docker_cgroup_targets_for_ids(root, &ids)
}

fn docker_cgroup_targets_for_ids(root: &Path, ids: &[String]) -> Result<Vec<SandboxCgroupTarget>> {
    let containers = docker_inspect_containers(&ids)?;
    let mut targets = Vec::new();

    for container in containers {
        if !container.state.running || container.state.pid == 0 {
            continue;
        }

        let Some((cgroup_path, cgroup_relative_path)) =
            resolve_pid_cgroup_path(root, container.state.pid)
        else {
            continue;
        };
        let image_ref = if container.config.image.is_empty() {
            container.image.clone()
        } else {
            container.config.image.clone()
        };
        let runtime_type = runtime_type_from_docker(&container.host_config.runtime);

        targets.push(SandboxCgroupTarget {
            sandbox_id: docker_sandbox_id(&container.id),
            runtime_source: "docker".to_string(),
            runtime_id_key: "docker.id".to_string(),
            runtime_id: container.id.clone(),
            runtime_name_key: "docker.name".to_string(),
            runtime_name: container.name.trim_start_matches('/').to_string(),
            workload_name: docker_workload_name(&container),
            namespace: docker_namespace(&container),
            image_id: image_id_from_ref_or_digest(&image_ref, &container.image),
            image_ref,
            runtime_type,
            runtime_version: container.host_config.runtime,
            pid: container.state.pid,
            cgroup_path,
            cgroup_relative_path,
        });
    }

    Ok(targets)
}

fn containerd_cgroup_targets_for_targets(
    root: &Path,
    targets: &[ContainerdSandboxCgroupTarget],
) -> Vec<SandboxCgroupTarget> {
    targets
        .iter()
        .filter_map(|target| {
            let (cgroup_path, cgroup_relative_path) = resolve_pid_cgroup_path(root, target.pid)?;
            Some(SandboxCgroupTarget {
                sandbox_id: target.sandbox_id.clone(),
                runtime_source: "containerd".to_string(),
                runtime_id_key: "containerd.id".to_string(),
                runtime_id: target.containerd_id.clone(),
                runtime_name_key: "containerd.namespace".to_string(),
                runtime_name: target.containerd_namespace.clone(),
                workload_name: target.workload_name.clone(),
                namespace: target.namespace.clone(),
                image_id: target.image_id.clone(),
                image_ref: target.image_ref.clone(),
                runtime_type: target.runtime_type.clone(),
                runtime_version: target.runtime_version.clone(),
                pid: target.pid,
                cgroup_path,
                cgroup_relative_path,
            })
        })
        .collect()
}

pub fn docker_running_container_ids() -> Result<Vec<String>> {
    let output = Command::new("docker")
        .args(["ps", "-q", "--no-trunc"])
        .output()?;

    if !output.status.success() {
        return Err(CollectorError::Plugin {
            plugin: "docker-sandbox-cgroupfs".to_string(),
            message: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        });
    }

    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(ToOwned::to_owned)
        .collect())
}

fn docker_inspect_containers(ids: &[String]) -> Result<Vec<DockerInspectContainer>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }

    let output = Command::new("docker").arg("inspect").args(ids).output()?;

    if !output.status.success() {
        return Err(CollectorError::Plugin {
            plugin: "docker-sandbox-cgroupfs".to_string(),
            message: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        });
    }

    Ok(serde_json::from_slice(&output.stdout)?)
}

fn resolve_pid_cgroup_path(root: &Path, pid: u64) -> Option<(PathBuf, String)> {
    let cgroup = fs::read_to_string(format!("/proc/{pid}/cgroup")).ok()?;
    let relative = cgroup.lines().find_map(|line| {
        let mut parts = line.splitn(3, ':');
        let _hierarchy = parts.next()?;
        let controllers = parts.next()?;
        let path = parts.next()?;
        if controllers.is_empty() || controllers.split(',').any(|item| item == "cpu") {
            Some(path.trim_start_matches('/').to_string())
        } else {
            None
        }
    })?;
    let path = if relative.is_empty() {
        root.to_path_buf()
    } else {
        root.join(&relative)
    };

    if path.exists() {
        Some((
            path,
            if relative.is_empty() {
                ".".to_string()
            } else {
                relative
            },
        ))
    } else {
        None
    }
}

fn read_sandbox_cgroup_sample(path: &Path, pid: u64) -> Option<SandboxCgroupSample> {
    let (io_read_bytes, io_write_bytes) = read_io_stat(path);
    let (network_rx_bytes, network_tx_bytes) = read_pid_network_stat(pid);
    Some(SandboxCgroupSample {
        cpu_usage_usec: read_cpu_usage_usec(path),
        memory_current: read_u64_file(path.join("memory.current")),
        io_read_bytes,
        io_write_bytes,
        network_rx_bytes,
        network_tx_bytes,
        process_count: read_cgroup_process_count(path),
    })
}

fn read_cpu_usage_usec(path: &Path) -> Option<u64> {
    let stat = fs::read_to_string(path.join("cpu.stat")).ok()?;
    stat.lines().find_map(|line| {
        let mut parts = line.split_whitespace();
        if parts.next()? == "usage_usec" {
            parts.next()?.parse::<u64>().ok()
        } else {
            None
        }
    })
}

fn read_io_stat(path: &Path) -> (Option<u64>, Option<u64>) {
    let stat = match fs::read_to_string(path.join("io.stat")) {
        Ok(stat) => stat,
        Err(_) => return (None, None),
    };
    let mut read_bytes = 0_u64;
    let mut write_bytes = 0_u64;
    let mut seen = false;

    for line in stat.lines() {
        for part in line.split_whitespace().skip(1) {
            if let Some(value) = part.strip_prefix("rbytes=") {
                read_bytes = read_bytes.saturating_add(value.parse::<u64>().unwrap_or(0));
                seen = true;
            }
            if let Some(value) = part.strip_prefix("wbytes=") {
                write_bytes = write_bytes.saturating_add(value.parse::<u64>().unwrap_or(0));
                seen = true;
            }
        }
    }

    if seen {
        (Some(read_bytes), Some(write_bytes))
    } else {
        (None, None)
    }
}

fn read_pid_network_stat(pid: u64) -> (Option<u64>, Option<u64>) {
    let stat = match fs::read_to_string(format!("/proc/{pid}/net/dev")) {
        Ok(stat) => stat,
        Err(_) => return (None, None),
    };
    let mut rx_bytes = 0_u64;
    let mut tx_bytes = 0_u64;
    let mut seen = false;

    for line in stat.lines().skip(2) {
        let Some((interface, counters)) = line.split_once(':') else {
            continue;
        };
        let interface = interface.trim();
        if interface.is_empty() || interface == "lo" {
            continue;
        }

        let values: Vec<&str> = counters.split_whitespace().collect();
        if values.len() < 16 {
            continue;
        }

        rx_bytes = rx_bytes.saturating_add(values[0].parse::<u64>().unwrap_or(0));
        tx_bytes = tx_bytes.saturating_add(values[8].parse::<u64>().unwrap_or(0));
        seen = true;
    }

    if seen {
        (Some(rx_bytes), Some(tx_bytes))
    } else {
        (None, None)
    }
}

fn read_cgroup_process_count(path: &Path) -> Option<u64> {
    fs::read_to_string(path.join("cgroup.procs"))
        .ok()
        .map(|value| value.lines().filter(|line| !line.trim().is_empty()).count() as u64)
}

fn read_u64_file(path: PathBuf) -> Option<u64> {
    fs::read_to_string(path).ok()?.trim().parse::<u64>().ok()
}

fn rate(previous: Option<u64>, current: u64, seconds: f64) -> f64 {
    if seconds <= 0.0 {
        return 0.0;
    }
    previous
        .map(|value| current.saturating_sub(value) as f64 / seconds)
        .unwrap_or(0.0)
}

fn retain_seen(values: &mut HashMap<String, u64>, sandboxes: &[serde_json::Value]) {
    values.retain(|sandbox_id, _| {
        sandboxes.iter().any(|sandbox| {
            sandbox.get("id").and_then(serde_json::Value::as_str) == Some(sandbox_id)
        })
    });
}

fn docker_workload_name(container: &DockerInspectContainer) -> String {
    container
        .config
        .labels
        .get("com.docker.compose.service")
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

fn docker_namespace(container: &DockerInspectContainer) -> String {
    container
        .config
        .labels
        .get("com.docker.compose.project")
        .cloned()
        .unwrap_or_else(|| "docker".to_string())
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

fn short_container_id(id: &str) -> String {
    id.chars().take(12).collect()
}

fn docker_sandbox_id(id: &str) -> String {
    format!("docker-{}", short_container_id(id))
}

fn image_id_from_ref_or_digest(image_ref: &str, digest: &str) -> String {
    let source = if image_ref.is_empty() {
        digest
    } else {
        image_ref
    };
    format!("docker-image-{}", sanitize_id(source))
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
    time.to_rfc3339_opts(SecondsFormat::Millis, true)
}
