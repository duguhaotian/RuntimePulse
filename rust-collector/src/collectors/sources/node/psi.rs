//! Node PSI source.
//!
//! Collects host pressure stall information from `/proc/pressure/*`.

use chrono::{DateTime, SecondsFormat, Utc};
use serde_json::json;
use std::fs;

use crate::collectors::core::config::CollectorConfig;
use crate::collectors::core::error::Result;
use crate::collectors::core::model::{Metadata, PluginOutput};
use crate::collectors::core::plugin::CollectorPlugin;
use crate::collectors::core::report::node_metric;

pub struct PsiPlugin;

impl PsiPlugin {
    pub fn new() -> Self {
        Self
    }
}

impl CollectorPlugin for PsiPlugin {
    fn name(&self) -> &str {
        "psi"
    }

    fn collect(&mut self, now: DateTime<Utc>, config: &CollectorConfig) -> Result<PluginOutput> {
        let ts = timestamp(now);
        let readings = read_psi_readings();
        let mut metrics = Vec::new();

        for reading in &readings {
            metrics.push(node_metric(
                &ts,
                &format!("node.psi.{}.{}", reading.resource, reading.stall),
                reading.avg10,
                "ratio",
                "pressure",
                &config.node_id,
            ));
        }

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
                    "kernelVersion": kernel_version().unwrap_or_else(|| "psi-observed".to_string()),
                    "cpuCores": cpu_core_count().unwrap_or(0),
                    "memoryBytes": read_memory_total_bytes().unwrap_or(0),
                    "status": "ready",
                    "labels": {
                        "collector": "runtimepulse-rust-collector",
                        "plugin": "psi",
                        "scope": config.collection_scope,
                    }
                })],
                images: Vec::new(),
                sandboxes: Vec::new(),
            },
            metrics,
            events: Vec::new(),
            traces: Vec::new(),
            profiles: Vec::new(),
        })
    }
}

struct PsiReading {
    resource: &'static str,
    stall: &'static str,
    avg10: f64,
}

fn read_psi_readings() -> Vec<PsiReading> {
    let mut readings = Vec::new();
    for (resource, path) in [
        ("cpu", "/proc/pressure/cpu"),
        ("io", "/proc/pressure/io"),
        ("memory", "/proc/pressure/memory"),
    ] {
        for stall in ["some", "full"] {
            if let Some(avg10) = read_psi_avg10(path, stall) {
                readings.push(PsiReading {
                    resource,
                    stall,
                    avg10,
                });
            }
        }
    }
    readings
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

fn read_memory_total_bytes() -> Result<u64> {
    let meminfo = fs::read_to_string("/proc/meminfo")?;
    Ok(meminfo_kib(&meminfo, "MemTotal").unwrap_or(0) * 1024)
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

fn timestamp(time: DateTime<Utc>) -> String {
    time.to_rfc3339_opts(SecondsFormat::Millis, true)
}
