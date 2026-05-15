//! Node root cgroupfs source.
//!
//! Collects host/root cgroup metrics only. Sandbox cgroup sampling belongs to
//! `sources::sandbox`.

use chrono::{DateTime, SecondsFormat, Utc};
use serde_json::{json, Map};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use crate::collectors::core::config::CollectorConfig;
use crate::collectors::core::error::{CollectorError, Result};
use crate::collectors::core::model::{EventRecord, Metadata, PluginOutput};
use crate::collectors::core::plugin::CollectorPlugin;
use crate::collectors::core::report::node_metric;

pub struct CgroupfsPlugin {
    last_seen: Option<Instant>,
    last_cpu_usage_by_path: HashMap<String, u64>,
    root: PathBuf,
}

struct CgroupSample {
    relative_path: String,
    cpu_usage_usec: Option<u64>,
    memory_current: Option<u64>,
    io_read_bytes: Option<u64>,
    io_write_bytes: Option<u64>,
    process_count: Option<u64>,
}

impl CgroupfsPlugin {
    pub fn new(root: PathBuf, _max_entries: usize) -> Self {
        Self {
            last_seen: None,
            last_cpu_usage_by_path: HashMap::new(),
            root,
        }
    }
}

impl CollectorPlugin for CgroupfsPlugin {
    fn name(&self) -> &str {
        "cgroupfs"
    }

    fn collect(&mut self, now: DateTime<Utc>, config: &CollectorConfig) -> Result<PluginOutput> {
        let ts = timestamp(now);
        let sample_interval = self
            .last_seen
            .map(|seen| seen.elapsed().as_secs_f64())
            .unwrap_or(0.0);
        let sample =
            read_cgroup_sample(&self.root, &self.root).ok_or_else(|| CollectorError::Plugin {
                plugin: "cgroupfs".to_string(),
                message: format!("unable to read host cgroup root at {}", self.root.display()),
            })?;
        let mut metrics = Vec::new();
        let previous_cpu = self.last_cpu_usage_by_path.insert(
            sample.relative_path.clone(),
            sample.cpu_usage_usec.unwrap_or(0),
        );
        let cpu_ratio = match (previous_cpu, sample.cpu_usage_usec) {
            (Some(previous), Some(current)) if sample_interval > 0.0 => {
                (current.saturating_sub(previous) as f64 / 1_000_000.0 / sample_interval).max(0.0)
            }
            _ => 0.0,
        };

        metrics.push(node_metric(
            &ts,
            "node.cgroup.cpu.usage_ratio",
            cpu_ratio,
            "ratio",
            "cpu",
            &config.node_id,
        ));

        if let Some(memory_current) = sample.memory_current {
            metrics.push(node_metric(
                &ts,
                "node.cgroup.memory.current_bytes",
                memory_current as f64,
                "bytes",
                "memory",
                &config.node_id,
            ));
        }

        if let Some(read_bytes) = sample.io_read_bytes {
            metrics.push(node_metric(
                &ts,
                "node.cgroup.io.read_bytes",
                read_bytes as f64,
                "bytes",
                "io",
                &config.node_id,
            ));
        }

        if let Some(write_bytes) = sample.io_write_bytes {
            metrics.push(node_metric(
                &ts,
                "node.cgroup.io.write_bytes",
                write_bytes as f64,
                "bytes",
                "io",
                &config.node_id,
            ));
        }

        if let Some(process_count) = sample.process_count {
            metrics.push(node_metric(
                &ts,
                "node.cgroup.process.count",
                process_count as f64,
                "count",
                "runtime",
                &config.node_id,
            ));
        }

        self.last_seen = Some(Instant::now());

        let mut attributes = Map::new();
        attributes.insert("plugin".to_string(), json!("cgroupfs"));
        attributes.insert("scope".to_string(), json!(config.collection_scope));
        attributes.insert(
            "cgroupRoot".to_string(),
            json!(self.root.display().to_string()),
        );
        attributes.insert("sampleCount".to_string(), json!(1));
        attributes.insert("scopeKind".to_string(), json!("host-root-cgroup"));

        let events = vec![EventRecord {
            id: format!("host-cgroupfs-observed-{}", now.timestamp()),
            timestamp: ts.clone(),
            severity: "info".to_string(),
            event_type: "collector".to_string(),
            event_name: "cgroupfs.sample.observed".to_string(),
            message: "Rust host cgroupfs collector sampled cgroup metrics".to_string(),
            source: format!("runtimepulse-rust-collector/{}/cgroupfs", config.node_id),
            attributes,
            sandbox_id: None,
            node_id: Some(config.node_id.clone()),
            runtime_type: None,
            reason: None,
        }];

        Ok(PluginOutput {
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
                    "kernelVersion": kernel_version().unwrap_or_else(|| "cgroupfs-observed".to_string()),
                    "cpuCores": cpu_core_count().unwrap_or(0),
                    "memoryBytes": read_memory_total_bytes().unwrap_or(0),
                    "status": "ready",
                    "labels": {
                        "collector": "runtimepulse-rust-collector",
                        "plugin": "cgroupfs",
                        "scope": config.collection_scope,
                    }
                })],
                images: Vec::new(),
                sandboxes: Vec::new(),
            },
            metrics,
            events,
            traces: Vec::new(),
            profiles: Vec::new(),
        })
    }
}

fn read_memory_total_bytes() -> Result<u64> {
    let meminfo = fs::read_to_string("/proc/meminfo")?;
    Ok(meminfo_kib(&meminfo, "MemTotal").unwrap_or(0) * 1024)
}

fn read_cgroup_sample(root: &Path, path: &Path) -> Option<CgroupSample> {
    let relative_path = path
        .strip_prefix(root)
        .ok()
        .map(|path| path.to_string_lossy().trim_matches('/').to_string())?;
    let relative_path = if relative_path.is_empty() {
        ".".to_string()
    } else {
        relative_path
    };
    let process_count = read_cgroup_process_count(path);
    let cpu_usage_usec = read_cpu_usage_usec(path);
    let memory_current = read_u64_file(path.join("memory.current"));
    let (io_read_bytes, io_write_bytes) = read_io_stat(path);

    if process_count.unwrap_or(0) == 0
        && memory_current.unwrap_or(0) == 0
        && cpu_usage_usec.unwrap_or(0) == 0
    {
        return None;
    }

    Some(CgroupSample {
        relative_path,
        cpu_usage_usec,
        memory_current,
        io_read_bytes,
        io_write_bytes,
        process_count,
    })
}

fn read_cgroup_process_count(path: &Path) -> Option<u64> {
    fs::read_to_string(path.join("cgroup.procs"))
        .ok()
        .map(|value| value.lines().filter(|line| !line.trim().is_empty()).count() as u64)
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

fn read_u64_file(path: PathBuf) -> Option<u64> {
    fs::read_to_string(path).ok()?.trim().parse::<u64>().ok()
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

fn timestamp(time: DateTime<Utc>) -> String {
    time.to_rfc3339_opts(SecondsFormat::Millis, true)
}
