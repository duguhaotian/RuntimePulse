//! Node procfs source.
//!
//! Collects host `/proc` CPU, memory, disk, process, PSI, and load signals.

use chrono::{DateTime, SecondsFormat, Utc};
use serde_json::{json, Map};
use std::fs;
use std::time::Instant;

use crate::collectors::core::config::CollectorConfig;
use crate::collectors::core::error::{CollectorError, Result};
use crate::collectors::core::model::{EventRecord, Metadata, PluginOutput};
use crate::collectors::core::plugin::CollectorPlugin;
use crate::collectors::core::report::metric;

pub struct ProcfsPlugin {
    last_cpu: Option<CpuSnapshot>,
    last_disk: Option<DiskSnapshot>,
    last_seen: Option<Instant>,
}

#[derive(Clone, Copy)]
struct CpuSnapshot {
    total: u64,
    idle: u64,
}

#[derive(Clone, Copy)]
struct DiskSnapshot {
    read_sectors: u64,
    write_sectors: u64,
}

struct PsiSnapshot {
    cpu_some: f64,
    io_some: f64,
    io_full: f64,
    memory_some: f64,
    memory_full: f64,
}

#[derive(Debug)]
struct MemorySnapshot {
    total_bytes: u64,
    available_bytes: u64,
    used_bytes: u64,
}

impl ProcfsPlugin {
    pub fn new() -> Self {
        Self {
            last_cpu: None,
            last_disk: None,
            last_seen: None,
        }
    }
}

impl CollectorPlugin for ProcfsPlugin {
    fn name(&self) -> &str {
        "procfs"
    }

    fn collect(&mut self, now: DateTime<Utc>, config: &CollectorConfig) -> Result<PluginOutput> {
        let ts = timestamp(now);
        let cpu = read_cpu_snapshot()?;
        let memory = read_memory_snapshot()?;
        let disk = read_disk_snapshot()?;
        let psi = read_psi_snapshot();
        let process_count = count_processes()?;
        let container_count = count_container_like_processes().unwrap_or(0);
        let load_avg = read_load_average().unwrap_or(0.0);
        let uptime_seconds = read_uptime_seconds().unwrap_or(0.0);
        let sample_interval = self
            .last_seen
            .map(|seen| seen.elapsed().as_secs_f64())
            .unwrap_or(0.0);
        let cpu_usage = self
            .last_cpu
            .map(|last| cpu_usage_ratio(last, cpu))
            .unwrap_or(0.0);
        let read_bytes = self
            .last_disk
            .map(|last| sector_delta_bytes(last.read_sectors, disk.read_sectors, sample_interval))
            .unwrap_or(0.0);
        let write_bytes = self
            .last_disk
            .map(|last| sector_delta_bytes(last.write_sectors, disk.write_sectors, sample_interval))
            .unwrap_or(0.0);
        let sandbox_id = format!("node-observer-{}", sanitize_id(&config.node_id));
        let image_ref = "runtimepulse/node-observer:rust";
        let image_id = "runtimepulse-node-observer";
        let runtime_type = "runc";
        let created_at =
            timestamp(now - chrono::Duration::seconds(uptime_seconds.min(3600.0) as i64));

        self.last_cpu = Some(cpu);
        self.last_disk = Some(disk);
        self.last_seen = Some(Instant::now());

        let metadata = Metadata {
            clusters: vec![json!({
                "id": config.cluster_id,
                "name": config.cluster_id,
                "environment": "collector"
            })],
            nodes: vec![json!({
                "id": config.node_id,
                "clusterId": config.cluster_id,
                "name": config.node_id,
                "kernelVersion": kernel_version().unwrap_or_else(|| "procfs-observed".to_string()),
                "cpuCores": cpu_core_count().unwrap_or(0),
                "memoryBytes": memory.total_bytes,
                "status": if cpu_usage > 0.9 { "degraded" } else { "ready" },
                "labels": {
                    "collector": "runtimepulse-rust-collector",
                    "plugin": "procfs",
                    "scope": config.collection_scope,
                }
            })],
            images: vec![json!({
                "id": image_id,
                "ref": image_ref,
                "digest": "collector:runtimepulse-node-observer",
                "loadingMode": "eager",
                "sizeBytes": 0,
                "layerCount": 0
            })],
            sandboxes: vec![json!({
                "id": sandbox_id,
                "clusterId": config.cluster_id,
                "nodeId": config.node_id,
                "namespace": "node",
                "workloadId": "runtimepulse-rust-collector",
                "workloadName": "node-observer",
                "imageId": image_id,
                "imageRef": image_ref,
                "runtimeType": runtime_type,
                "runtimeVersion": "rust-procfs",
                "status": "running",
                "createdAt": created_at,
                "startedAt": ts,
                "startupDurationMs": 0,
                "cpuAvg": cpu_usage,
                "memoryPeakBytes": memory.used_bytes,
                "labels": {
                    "collector": "runtimepulse-rust-collector",
                    "plugin": "procfs",
                    "scope": config.collection_scope,
                },
                "attributes": {
                    "collector.scope": config.collection_scope,
                    "process.count": process_count,
                    "container.process.count": container_count,
                    "load.avg.1m": load_avg
                }
            })],
        };

        let mut metrics = vec![
            metric(
                &ts,
                "node.cpu.usage_ratio",
                cpu_usage,
                "ratio",
                "cpu",
                &config.node_id,
                &sandbox_id,
                runtime_type,
            ),
            metric(
                &ts,
                "node.memory.used_bytes",
                memory.used_bytes as f64,
                "bytes",
                "memory",
                &config.node_id,
                &sandbox_id,
                runtime_type,
            ),
            metric(
                &ts,
                "node.memory.available_bytes",
                memory.available_bytes as f64,
                "bytes",
                "memory",
                &config.node_id,
                &sandbox_id,
                runtime_type,
            ),
            metric(
                &ts,
                "node.io.read_bytes",
                read_bytes,
                "bytes/s",
                "io",
                &config.node_id,
                &sandbox_id,
                runtime_type,
            ),
            metric(
                &ts,
                "node.io.write_bytes",
                write_bytes,
                "bytes/s",
                "io",
                &config.node_id,
                &sandbox_id,
                runtime_type,
            ),
            metric(
                &ts,
                "node.process.count",
                process_count as f64,
                "count",
                "runtime",
                &config.node_id,
                &sandbox_id,
                runtime_type,
            ),
            metric(
                &ts,
                "node.container.count",
                container_count as f64,
                "count",
                "lifecycle",
                &config.node_id,
                &sandbox_id,
                runtime_type,
            ),
            metric(
                &ts,
                "node.load.1m",
                load_avg,
                "count",
                "runtime",
                &config.node_id,
                &sandbox_id,
                runtime_type,
            ),
            metric(
                &ts,
                "sandbox.cpu.usage_ratio",
                cpu_usage,
                "ratio",
                "cpu",
                &config.node_id,
                &sandbox_id,
                runtime_type,
            ),
            metric(
                &ts,
                "sandbox.memory.working_set_bytes",
                memory.used_bytes as f64,
                "bytes",
                "memory",
                &config.node_id,
                &sandbox_id,
                runtime_type,
            ),
            metric(
                &ts,
                "sandbox.io.read_bytes",
                read_bytes,
                "bytes/s",
                "io",
                &config.node_id,
                &sandbox_id,
                runtime_type,
            ),
            metric(
                &ts,
                "sandbox.startup.duration_ms",
                0.0,
                "ms",
                "startup",
                &config.node_id,
                &sandbox_id,
                runtime_type,
            ),
        ];
        if let Some(psi) = psi {
            metrics.extend([
                metric(
                    &ts,
                    "node.psi.cpu.some",
                    psi.cpu_some,
                    "ratio",
                    "pressure",
                    &config.node_id,
                    &sandbox_id,
                    runtime_type,
                ),
                metric(
                    &ts,
                    "node.psi.io.some",
                    psi.io_some,
                    "ratio",
                    "pressure",
                    &config.node_id,
                    &sandbox_id,
                    runtime_type,
                ),
                metric(
                    &ts,
                    "node.psi.io.full",
                    psi.io_full,
                    "ratio",
                    "pressure",
                    &config.node_id,
                    &sandbox_id,
                    runtime_type,
                ),
                metric(
                    &ts,
                    "node.psi.memory.some",
                    psi.memory_some,
                    "ratio",
                    "pressure",
                    &config.node_id,
                    &sandbox_id,
                    runtime_type,
                ),
                metric(
                    &ts,
                    "node.psi.memory.full",
                    psi.memory_full,
                    "ratio",
                    "pressure",
                    &config.node_id,
                    &sandbox_id,
                    runtime_type,
                ),
            ]);
        }

        let mut attributes = Map::new();
        attributes.insert("plugin".to_string(), json!("procfs"));
        attributes.insert("scope".to_string(), json!(config.collection_scope));
        attributes.insert("processCount".to_string(), json!(process_count));
        attributes.insert("containerProcessCount".to_string(), json!(container_count));

        let events = vec![EventRecord {
            id: format!("{}-procfs-observed-{}", sandbox_id, now.timestamp()),
            timestamp: ts,
            severity: "info".to_string(),
            event_type: "collector".to_string(),
            event_name: "procfs.sample.observed".to_string(),
            message: "Rust procfs collector sampled node metrics".to_string(),
            source: format!("runtimepulse-rust-collector/{}/procfs", config.node_id),
            attributes,
            sandbox_id: Some(sandbox_id),
            node_id: Some(config.node_id.clone()),
            runtime_type: Some(runtime_type.to_string()),
            reason: None,
        }];

        Ok(PluginOutput {
            metadata,
            metrics,
            events,
            traces: Vec::new(),
            profiles: Vec::new(),
        })
    }
}

fn read_cpu_snapshot() -> Result<CpuSnapshot> {
    let stat = fs::read_to_string("/proc/stat")?;
    let cpu_line = stat
        .lines()
        .find(|line| line.starts_with("cpu "))
        .ok_or_else(|| CollectorError::Config("missing /proc/stat cpu line".to_string()))?;
    let values = cpu_line
        .split_whitespace()
        .skip(1)
        .map(|part| part.parse::<u64>().unwrap_or(0))
        .collect::<Vec<_>>();
    let total = values.iter().sum();
    let idle = values.get(3).copied().unwrap_or(0) + values.get(4).copied().unwrap_or(0);

    Ok(CpuSnapshot { total, idle })
}

fn read_memory_snapshot() -> Result<MemorySnapshot> {
    let meminfo = fs::read_to_string("/proc/meminfo")?;
    let total = meminfo_kib(&meminfo, "MemTotal").unwrap_or(0) * 1024;
    let available = meminfo_kib(&meminfo, "MemAvailable").unwrap_or(0) * 1024;

    Ok(MemorySnapshot {
        total_bytes: total,
        available_bytes: available,
        used_bytes: total.saturating_sub(available),
    })
}

fn read_disk_snapshot() -> Result<DiskSnapshot> {
    let diskstats = fs::read_to_string("/proc/diskstats")?;
    let mut read_sectors = 0;
    let mut write_sectors = 0;

    for line in diskstats.lines() {
        let parts = line.split_whitespace().collect::<Vec<_>>();
        if parts.len() < 14 || is_virtual_block_device(parts[2]) {
            continue;
        }
        read_sectors += parts[5].parse::<u64>().unwrap_or(0);
        write_sectors += parts[9].parse::<u64>().unwrap_or(0);
    }

    Ok(DiskSnapshot {
        read_sectors,
        write_sectors,
    })
}

fn count_processes() -> Result<u64> {
    Ok(fs::read_dir("/proc")?
        .filter_map(std::result::Result::ok)
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .chars()
                .all(|char| char.is_ascii_digit())
        })
        .count() as u64)
}

fn count_container_like_processes() -> Result<u64> {
    let mut count = 0;
    for entry in fs::read_dir("/proc")?.filter_map(std::result::Result::ok) {
        let pid = entry.file_name().to_string_lossy().to_string();
        if !pid.chars().all(|char| char.is_ascii_digit()) {
            continue;
        }
        let cgroup = fs::read_to_string(format!("/proc/{pid}/cgroup")).unwrap_or_default();
        if cgroup.contains("docker") || cgroup.contains("containerd") || cgroup.contains("kubepods")
        {
            count += 1;
        }
    }
    Ok(count)
}

fn read_load_average() -> Result<f64> {
    let loadavg = fs::read_to_string("/proc/loadavg")?;
    Ok(loadavg
        .split_whitespace()
        .next()
        .and_then(|value| value.parse::<f64>().ok())
        .unwrap_or(0.0))
}

fn read_uptime_seconds() -> Result<f64> {
    let uptime = fs::read_to_string("/proc/uptime")?;
    Ok(uptime
        .split_whitespace()
        .next()
        .and_then(|value| value.parse::<f64>().ok())
        .unwrap_or(0.0))
}

fn read_psi_snapshot() -> Option<PsiSnapshot> {
    Some(PsiSnapshot {
        cpu_some: read_psi_avg10("/proc/pressure/cpu", "some")?,
        io_some: read_psi_avg10("/proc/pressure/io", "some")?,
        io_full: read_psi_avg10("/proc/pressure/io", "full").unwrap_or(0.0),
        memory_some: read_psi_avg10("/proc/pressure/memory", "some")?,
        memory_full: read_psi_avg10("/proc/pressure/memory", "full").unwrap_or(0.0),
    })
}

fn read_psi_avg10(path: &str, line_name: &str) -> Option<f64> {
    let pressure = fs::read_to_string(path).ok()?;
    let line = pressure
        .lines()
        .find(|line| line.starts_with(&format!("{line_name} ")))?;

    parse_psi_field(line, "avg10").map(|value| (value / 100.0).clamp(0.0, 1.0))
}

fn parse_psi_field(line: &str, field_name: &str) -> Option<f64> {
    line.split_whitespace().find_map(|part| {
        let (key, value) = part.split_once('=')?;
        if key == field_name {
            value.parse::<f64>().ok()
        } else {
            None
        }
    })
}

fn cpu_core_count() -> Result<u64> {
    let cpuinfo = fs::read_to_string("/proc/cpuinfo")?;
    Ok(cpuinfo
        .lines()
        .filter(|line| line.starts_with("processor"))
        .count() as u64)
}

fn kernel_version() -> Option<String> {
    fs::read_to_string("/proc/sys/kernel/osrelease")
        .ok()
        .map(|value| value.trim().to_string())
}

fn cpu_usage_ratio(previous: CpuSnapshot, current: CpuSnapshot) -> f64 {
    let total_delta = current.total.saturating_sub(previous.total);
    let idle_delta = current.idle.saturating_sub(previous.idle);
    if total_delta == 0 {
        return 0.0;
    }
    (total_delta.saturating_sub(idle_delta) as f64 / total_delta as f64).clamp(0.0, 1.0)
}

fn sector_delta_bytes(previous: u64, current: u64, seconds: f64) -> f64 {
    if seconds <= 0.0 {
        return 0.0;
    }
    current.saturating_sub(previous) as f64 * 512.0 / seconds
}

fn meminfo_kib(meminfo: &str, key: &str) -> Option<u64> {
    meminfo.lines().find_map(|line| {
        let mut parts = line.split_whitespace();
        if parts.next()?.trim_end_matches(':') == key {
            parts.next()?.parse::<u64>().ok()
        } else {
            None
        }
    })
}

fn is_virtual_block_device(name: &str) -> bool {
    name.starts_with("loop") || name.starts_with("ram") || name.starts_with("dm-")
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
