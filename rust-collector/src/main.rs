mod collectors;

use chrono::{DateTime, SecondsFormat, Utc};
use collectors::adapters::command::CommandPlugin;
use collectors::adapters::http::HttpPlugin;
use collectors::core::config::CollectorConfig;
use collectors::core::error::{CollectorError, Result};
use collectors::core::model::{EventRecord, Metadata, PluginOutput};
use collectors::core::plugin::CollectorPlugin;
use collectors::core::report::node_metric;
use collectors::outlet::batcher::collect_once;
use collectors::outlet::http_ingress::start_local_report_server;
use collectors::outlet::sender::send_local_report;
use collectors::sources::node::procfs::ProcfsPlugin;
use collectors::sources::runtime::docker::inventory::collect_docker_inventory;
use reqwest::blocking::Client;
use serde_json::{json, Map};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Instant;

struct CgroupfsPlugin {
    last_seen: Option<Instant>,
    last_cpu_usage_by_path: std::collections::HashMap<String, u64>,
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

fn main() {
    if env::args().any(|arg| arg == "--version") {
        println!("runtimepulse-collector {}", env!("CARGO_PKG_VERSION"));
        return;
    }

    let result = if env::args().any(|arg| arg == "host-procfs") {
        run_host_procfs()
    } else if env::args().any(|arg| arg == "host-cgroupfs") {
        run_host_cgroupfs()
    } else if env::args().any(|arg| arg == "host-docker") {
        run_host_docker()
    } else {
        run_outlet()
    };

    if let Err(error) = result {
        eprintln!(
            "{}",
            json!({
                "level": "error",
                "message": "collector_failed",
                "error": error.to_string(),
            })
        );
        std::process::exit(1);
    }
}

fn run_outlet() -> Result<()> {
    let config = CollectorConfig::from_env()?;
    let mut plugins = build_plugins(&config)?;
    let client = Client::new();
    let local_reports = start_local_report_server(&config.local_report_addr)?;

    loop {
        let started = Instant::now();
        let now = Utc::now();
        match collect_once(&client, &config, &mut plugins, &local_reports, now) {
            Ok(summary) => {
                if summary.submitted {
                    println!(
                        "{}",
                        json!({
                            "level": "info",
                            "message": "ingest_batch_accepted",
                            "source": summary.source,
                            "localReports": summary.local_reports,
                            "metrics": summary.metrics,
                            "events": summary.events,
                            "traces": summary.traces,
                            "profiles": summary.profiles,
                        })
                    );
                }
            }
            Err(error) => eprintln!(
                "{}",
                json!({
                    "level": "error",
                    "message": "collector_tick_failed",
                    "error": error.to_string(),
                })
            ),
        }

        let elapsed = started.elapsed();
        if config.interval > elapsed {
            thread::sleep(config.interval - elapsed);
        }

        if config.once {
            break;
        }
    }

    Ok(())
}

fn run_host_procfs() -> Result<()> {
    let mut config = CollectorConfig::from_env()?;
    config.collection_scope = "host".to_string();
    config.plugins = vec!["procfs".to_string()];

    let client = Client::new();
    let mut plugin = ProcfsPlugin::new();

    loop {
        let started = Instant::now();
        let now = Utc::now();
        match plugin.collect(now, &config) {
            Ok(output) => match send_local_report(&client, &config.local_report_url, &output) {
                Ok(()) => println!(
                    "{}",
                    json!({
                        "level": "info",
                        "message": "host_procfs_report_accepted",
                        "url": config.local_report_url,
                        "metrics": output.metrics.len(),
                        "events": output.events.len(),
                    })
                ),
                Err(error) => eprintln!(
                    "{}",
                    json!({
                        "level": "error",
                        "message": "host_procfs_report_failed",
                        "error": error.to_string(),
                    })
                ),
            },
            Err(error) => eprintln!(
                "{}",
                json!({
                    "level": "error",
                    "message": "host_procfs_collect_failed",
                    "error": error.to_string(),
                })
            ),
        }

        let elapsed = started.elapsed();
        if config.interval > elapsed {
            thread::sleep(config.interval - elapsed);
        }

        if config.once {
            break;
        }
    }

    Ok(())
}

fn run_host_cgroupfs() -> Result<()> {
    let mut config = CollectorConfig::from_env()?;
    config.collection_scope = "host".to_string();
    config.plugins = vec!["cgroupfs".to_string()];

    let client = Client::new();
    let mut plugin = CgroupfsPlugin::new(config.cgroup_root.clone(), config.cgroup_max_entries);

    loop {
        let started = Instant::now();
        let now = Utc::now();
        match plugin.collect(now, &config) {
            Ok(output) => match send_local_report(&client, &config.local_report_url, &output) {
                Ok(()) => println!(
                    "{}",
                    json!({
                        "level": "info",
                        "message": "host_cgroupfs_report_accepted",
                        "url": config.local_report_url,
                        "sandboxes": output.metadata.sandboxes.len(),
                        "metrics": output.metrics.len(),
                        "events": output.events.len(),
                    })
                ),
                Err(error) => eprintln!(
                    "{}",
                    json!({
                        "level": "error",
                        "message": "host_cgroupfs_report_failed",
                        "error": error.to_string(),
                    })
                ),
            },
            Err(error) => eprintln!(
                "{}",
                json!({
                    "level": "error",
                    "message": "host_cgroupfs_collect_failed",
                    "error": error.to_string(),
                })
            ),
        }

        if config.once {
            break;
        }

        let elapsed = started.elapsed();
        if config.interval > elapsed {
            thread::sleep(config.interval - elapsed);
        }
    }

    Ok(())
}

fn run_host_docker() -> Result<()> {
    let mut config = CollectorConfig::from_env()?;
    config.collection_scope = "host".to_string();

    let client = Client::new();

    loop {
        let started = Instant::now();
        let now = Utc::now();
        match collect_docker_inventory(now, &config) {
            Ok(output) => match send_local_report(&client, &config.local_report_url, &output) {
                Ok(()) => println!(
                    "{}",
                    json!({
                        "level": "info",
                        "message": "host_docker_report_accepted",
                        "url": config.local_report_url,
                        "sandboxes": output.metadata.sandboxes.len(),
                        "images": output.metadata.images.len(),
                        "events": output.events.len(),
                    })
                ),
                Err(error) => eprintln!(
                    "{}",
                    json!({
                        "level": "error",
                        "message": "host_docker_report_failed",
                        "error": error.to_string(),
                    })
                ),
            },
            Err(error) => eprintln!(
                "{}",
                json!({
                    "level": "error",
                    "message": "host_docker_collect_failed",
                    "error": error.to_string(),
                })
            ),
        }

        if config.once {
            break;
        }

        let elapsed = started.elapsed();
        if config.interval > elapsed {
            thread::sleep(config.interval - elapsed);
        }
    }

    Ok(())
}

fn build_plugins(config: &CollectorConfig) -> Result<Vec<Box<dyn CollectorPlugin>>> {
    let mut plugins: Vec<Box<dyn CollectorPlugin>> = Vec::new();

    for name in &config.plugins {
        match name.as_str() {
            "procfs" => plugins.push(Box::new(ProcfsPlugin::new())),
            "command" => {
                let command = config.command_plugin.clone().ok_or_else(|| {
                    CollectorError::Config(
                        "command plugin requires RUNTIMEPULSE_COMMAND_PLUGIN_CMD".to_string(),
                    )
                })?;
                plugins.push(Box::new(CommandPlugin {
                    name: command.name,
                    command: command.command,
                }));
            }
            "http" => {
                let http = config.http_plugin.clone().ok_or_else(|| {
                    CollectorError::Config(
                        "http plugin requires RUNTIMEPULSE_HTTP_PLUGIN_URL".to_string(),
                    )
                })?;
                plugins.push(Box::new(HttpPlugin {
                    name: http.name,
                    url: http.url,
                    client: Client::new(),
                }));
            }
            other => {
                return Err(CollectorError::Config(format!(
                    "unknown collector plugin: {other}"
                )));
            }
        }
    }

    Ok(plugins)
}

impl CgroupfsPlugin {
    fn new(root: PathBuf, _max_entries: usize) -> Self {
        Self {
            last_seen: None,
            last_cpu_usage_by_path: std::collections::HashMap::new(),
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
