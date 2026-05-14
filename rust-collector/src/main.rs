use chrono::{DateTime, SecondsFormat, Utc};
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::env;
use std::fs;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};
use thiserror::Error;

type Result<T> = std::result::Result<T, CollectorError>;

#[derive(Debug, Error)]
enum CollectorError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("collector config error: {0}")]
    Config(String),
    #[error("plugin {plugin} failed: {message}")]
    Plugin { plugin: String, message: String },
    #[error("ingest failed with status {status}: {body}")]
    Ingest { status: u16, body: String },
}

#[derive(Clone, Debug)]
struct CollectorConfig {
    ingest_url: String,
    node_id: String,
    cluster_id: String,
    interval: Duration,
    local_report_addr: String,
    local_report_url: String,
    collection_scope: String,
    once: bool,
    cgroup_root: PathBuf,
    cgroup_max_entries: usize,
    plugins: Vec<String>,
    command_plugin: Option<CommandPluginConfig>,
    http_plugin: Option<HttpPluginConfig>,
}

#[derive(Clone, Debug)]
struct CommandPluginConfig {
    name: String,
    command: String,
}

#[derive(Clone, Debug)]
struct HttpPluginConfig {
    name: String,
    url: String,
}

trait CollectorPlugin {
    fn name(&self) -> &str;
    fn collect(&mut self, now: DateTime<Utc>, config: &CollectorConfig) -> Result<PluginOutput>;
}

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct PluginOutput {
    #[serde(default)]
    metadata: Metadata,
    #[serde(default)]
    metrics: Vec<MetricSample>,
    #[serde(default)]
    events: Vec<EventRecord>,
    #[serde(default)]
    traces: Vec<TraceSpan>,
    #[serde(default)]
    profiles: Vec<ProfileArtifact>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct Metadata {
    #[serde(default)]
    clusters: Vec<Value>,
    #[serde(default)]
    nodes: Vec<Value>,
    #[serde(default)]
    images: Vec<Value>,
    #[serde(default)]
    sandboxes: Vec<Value>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct MetricSample {
    timestamp: String,
    name: String,
    value: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    unit: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    group: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    sandbox_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    node_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    image_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    runtime_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    attributes: Option<Map<String, Value>>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct EventRecord {
    id: String,
    timestamp: String,
    severity: String,
    event_type: String,
    event_name: String,
    message: String,
    source: String,
    attributes: Map<String, Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    sandbox_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    node_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    runtime_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct TraceSpan {
    trace_id: String,
    span_id: String,
    span_name: String,
    start_time: String,
    end_time: String,
    duration_ms: f64,
    status: String,
    attributes: Map<String, Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    sandbox_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    parent_span_id: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProfileArtifact {
    id: String,
    timestamp: String,
    sandbox_id: String,
    profile_type: String,
    process_role: String,
    duration_ms: f64,
    sample_count: u64,
    object_uri: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    flamegraph: Option<Value>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct IngestBatch {
    source: String,
    observed_at: String,
    metadata: Metadata,
    metrics: Vec<MetricSample>,
    events: Vec<EventRecord>,
    traces: Vec<TraceSpan>,
    profiles: Vec<ProfileArtifact>,
}

struct ProcfsPlugin {
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

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct DockerInspectContainer {
    id: String,
    name: String,
    created: String,
    image: String,
    state: DockerState,
    config: DockerConfig,
    host_config: DockerHostConfig,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct DockerState {
    status: String,
    running: bool,
    #[serde(rename = "OOMKilled")]
    oom_killed: bool,
    error: String,
    started_at: String,
    finished_at: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct DockerConfig {
    image: String,
    #[serde(default)]
    labels: std::collections::HashMap<String, String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct DockerHostConfig {
    runtime: String,
}

struct CommandPlugin {
    name: String,
    command: String,
}

struct HttpPlugin {
    name: String,
    url: String,
    client: Client,
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

struct BatchSummary {
    source: String,
    submitted: bool,
    local_reports: usize,
    metrics: usize,
    events: usize,
    traces: usize,
    profiles: usize,
}

fn collect_once(
    client: &Client,
    config: &CollectorConfig,
    plugins: &mut [Box<dyn CollectorPlugin>],
    local_reports: &Receiver<PluginOutput>,
    now: DateTime<Utc>,
) -> Result<BatchSummary> {
    let mut batch = IngestBatch {
        source: format!("runtimepulse-rust-collector/{}", config.node_id),
        observed_at: timestamp(now),
        metadata: Metadata::default(),
        metrics: Vec::new(),
        events: Vec::new(),
        traces: Vec::new(),
        profiles: Vec::new(),
    };

    for plugin in plugins {
        let plugin_name = plugin.name().to_string();
        let output = plugin
            .collect(now, config)
            .map_err(|error| CollectorError::Plugin {
                plugin: plugin_name,
                message: error.to_string(),
            })?;
        merge_output(&mut batch, output, now, config);
    }

    let mut local_report_count = 0;
    while let Ok(output) = local_reports.try_recv() {
        local_report_count += 1;
        merge_output(&mut batch, output, now, config);
    }

    if !has_batch_payload(&batch) {
        return Ok(BatchSummary {
            source: batch.source,
            submitted: false,
            local_reports: local_report_count,
            metrics: 0,
            events: 0,
            traces: 0,
            profiles: 0,
        });
    }

    send_batch(client, config, &batch)?;

    Ok(BatchSummary {
        source: batch.source,
        submitted: true,
        local_reports: local_report_count,
        metrics: batch.metrics.len(),
        events: batch.events.len(),
        traces: batch.traces.len(),
        profiles: batch.profiles.len(),
    })
}

impl CollectorConfig {
    fn from_env() -> Result<Self> {
        let interval_ms = env_u64("RUNTIMEPULSE_COLLECTOR_INTERVAL_MS")
            .or_else(|| env_u64("COLLECTOR_INTERVAL_MS"))
            .unwrap_or(5000);
        let plugins = env::var("RUNTIMEPULSE_COLLECTOR_PLUGINS")
            .unwrap_or_default()
            .split(',')
            .map(str::trim)
            .filter(|item| !item.is_empty())
            .filter(|item| *item != "none")
            .map(ToOwned::to_owned)
            .collect::<Vec<_>>();

        Ok(Self {
            ingest_url: env::var("INGEST_URL").unwrap_or_else(|_| {
                "http://runtimepulse-query-api:8081/api/ingest/batch".to_string()
            }),
            node_id: env::var("RUNTIMEPULSE_COLLECTOR_NODE_ID")
                .or_else(|_| env::var("COLLECTOR_NODE_ID"))
                .unwrap_or_else(|_| "rust-node-a".to_string()),
            cluster_id: env::var("RUNTIMEPULSE_COLLECTOR_CLUSTER_ID")
                .unwrap_or_else(|_| "cluster-prod".to_string()),
            interval: Duration::from_millis(interval_ms.max(1000)),
            local_report_addr: env::var("RUNTIMEPULSE_LOCAL_REPORT_ADDR")
                .unwrap_or_else(|_| "0.0.0.0:9091".to_string()),
            local_report_url: env::var("RUNTIMEPULSE_LOCAL_REPORT_URL")
                .unwrap_or_else(|_| "http://localhost:9091/api/local/ingest".to_string()),
            collection_scope: env::var("RUNTIMEPULSE_COLLECTOR_SCOPE")
                .unwrap_or_else(|_| "outlet".to_string()),
            once: env_bool("RUNTIMEPULSE_COLLECTOR_ONCE"),
            cgroup_root: PathBuf::from(
                env::var("RUNTIMEPULSE_CGROUP_ROOT")
                    .unwrap_or_else(|_| "/sys/fs/cgroup".to_string()),
            ),
            cgroup_max_entries: env_u64("RUNTIMEPULSE_CGROUP_MAX_ENTRIES")
                .unwrap_or(200)
                .max(1) as usize,
            plugins,
            command_plugin: command_plugin_config(),
            http_plugin: http_plugin_config(),
        })
    }
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

impl ProcfsPlugin {
    fn new() -> Self {
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
                    "memoryBytes": read_memory_snapshot().map(|memory| memory.total_bytes).unwrap_or(0),
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

impl CollectorPlugin for CommandPlugin {
    fn name(&self) -> &str {
        &self.name
    }

    fn collect(&mut self, _now: DateTime<Utc>, _config: &CollectorConfig) -> Result<PluginOutput> {
        let output = Command::new("sh").arg("-lc").arg(&self.command).output()?;

        if !output.status.success() {
            return Err(CollectorError::Plugin {
                plugin: self.name.clone(),
                message: String::from_utf8_lossy(&output.stderr).trim().to_string(),
            });
        }

        Ok(serde_json::from_slice(&output.stdout)?)
    }
}

impl CollectorPlugin for HttpPlugin {
    fn name(&self) -> &str {
        &self.name
    }

    fn collect(&mut self, _now: DateTime<Utc>, _config: &CollectorConfig) -> Result<PluginOutput> {
        Ok(self
            .client
            .get(&self.url)
            .send()?
            .error_for_status()?
            .json()?)
    }
}

fn docker_container_ids() -> Result<Vec<String>> {
    let output = Command::new("docker")
        .args(["ps", "-aq", "--no-trunc"])
        .output()?;

    if !output.status.success() {
        return Err(CollectorError::Plugin {
            plugin: "docker".to_string(),
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
            plugin: "docker".to_string(),
            message: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        });
    }

    Ok(serde_json::from_slice(&output.stdout)?)
}

fn collect_docker_inventory(now: DateTime<Utc>, config: &CollectorConfig) -> Result<PluginOutput> {
    let ids = docker_container_ids()?;
    let containers = docker_inspect_containers(&ids)?;
    let ts = timestamp(now);
    let mut images = std::collections::BTreeMap::new();
    let mut sandboxes = Vec::new();
    let mut events = Vec::new();

    for container in containers {
        let short_id = short_container_id(&container.id);
        let sandbox_id = docker_sandbox_id(&container.id);
        let image_ref = if container.config.image.is_empty() {
            container.image.clone()
        } else {
            container.config.image.clone()
        };
        let image_id = image_id_from_ref_or_digest(&image_ref, &container.image);
        let runtime_type = runtime_type_from_docker(&container.host_config.runtime);
        let status = sandbox_status_from_docker(&container.state);
        let workload_name = docker_workload_name(&container);
        let namespace = docker_namespace(&container);
        let created_at =
            normalize_docker_timestamp(&container.created).unwrap_or_else(|| ts.clone());
        let started_at = normalize_docker_timestamp(&container.state.started_at);
        let stopped_at = normalize_docker_timestamp(&container.state.finished_at);
        let startup_duration_ms = started_at
            .as_deref()
            .and_then(|started| duration_ms_between(&created_at, started))
            .unwrap_or(0.0);

        let image_row_id = image_id.clone();
        let image_row_ref = image_ref.clone();
        let image_row_digest = container.image.clone();
        images.entry(image_id.clone()).or_insert_with(|| {
            json!({
                "id": image_row_id,
                "ref": image_row_ref,
                "digest": image_row_digest,
                "loadingMode": "eager",
                "sizeBytes": 0,
                "layerCount": 0,
            })
        });

        sandboxes.push(json!({
            "id": sandbox_id,
            "clusterId": config.cluster_id,
            "nodeId": config.node_id,
            "namespace": namespace,
            "workloadId": workload_name,
            "workloadName": workload_name,
            "imageId": image_id,
            "imageRef": image_ref,
            "runtimeType": runtime_type,
            "runtimeVersion": container.host_config.runtime,
            "status": status,
            "createdAt": created_at,
            "startedAt": started_at,
            "stoppedAt": stopped_at,
            "startupDurationMs": startup_duration_ms,
            "cpuAvg": 0,
            "memoryPeakBytes": 0,
            "labels": {
                "collector": "runtimepulse-rust-collector",
                "plugin": "docker",
                "scope": config.collection_scope,
            },
            "attributes": {
                "collector.scope": config.collection_scope,
                "docker.id": container.id,
                "docker.short_id": short_id,
                "docker.name": container.name.trim_start_matches('/'),
                "docker.status": container.state.status,
                "docker.oom_killed": container.state.oom_killed,
                "docker.error": container.state.error,
            }
        }));

        let mut attributes = Map::new();
        attributes.insert("plugin".to_string(), json!("docker"));
        attributes.insert("scope".to_string(), json!(config.collection_scope));
        attributes.insert("dockerId".to_string(), json!(container.id));
        attributes.insert("dockerStatus".to_string(), json!(container.state.status));

        events.push(EventRecord {
            id: format!("docker-{}-observed-{}", short_id, now.timestamp()),
            timestamp: ts.clone(),
            severity: if status == "failed" { "error" } else { "info" }.to_string(),
            event_type: "container".to_string(),
            event_name: "docker.container.observed".to_string(),
            message: format!("Docker container {workload_name} is {status}."),
            source: format!("runtimepulse-rust-collector/{}/docker", config.node_id),
            attributes,
            sandbox_id: Some(docker_sandbox_id(&container.id)),
            node_id: Some(config.node_id.clone()),
            runtime_type: Some(runtime_type.to_string()),
            reason: if container.state.oom_killed {
                Some("oom_killed".to_string())
            } else if !container.state.error.is_empty() {
                Some(container.state.error)
            } else {
                None
            },
        });
    }

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
                "kernelVersion": kernel_version().unwrap_or_else(|| "docker-observed".to_string()),
                "cpuCores": cpu_core_count().unwrap_or(0),
                "memoryBytes": read_memory_snapshot().map(|memory| memory.total_bytes).unwrap_or(0),
                "status": "ready",
                "labels": {
                    "collector": "runtimepulse-rust-collector",
                    "plugin": "docker",
                    "scope": config.collection_scope,
                }
            })],
            images: images.into_values().collect(),
            sandboxes,
        },
        metrics: Vec::new(),
        events,
        traces: Vec::new(),
        profiles: Vec::new(),
    })
}

fn merge_output(
    batch: &mut IngestBatch,
    output: PluginOutput,
    now: DateTime<Utc>,
    config: &CollectorConfig,
) {
    batch.metadata.clusters.extend(output.metadata.clusters);
    batch.metadata.nodes.extend(output.metadata.nodes);
    batch.metadata.images.extend(output.metadata.images);
    batch.metadata.sandboxes.extend(output.metadata.sandboxes);

    let ts = timestamp(now);

    batch
        .metrics
        .extend(output.metrics.into_iter().map(|mut metric| {
            if metric.timestamp.is_empty() {
                metric.timestamp = ts.clone();
            }
            metric
        }));

    batch
        .events
        .extend(output.events.into_iter().map(|mut event| {
            if event.timestamp.is_empty() {
                event.timestamp = ts.clone();
            }
            if event.source.is_empty() {
                event.source = format!("runtimepulse-rust-collector/{}", config.node_id);
            }
            event
        }));

    batch.traces.extend(output.traces);
    batch.profiles.extend(output.profiles);
}

fn send_batch(client: &Client, config: &CollectorConfig, batch: &IngestBatch) -> Result<()> {
    let response = client.post(&config.ingest_url).json(batch).send()?;
    let status = response.status();
    let body = response.text().unwrap_or_default();

    if !status.is_success() {
        return Err(CollectorError::Ingest {
            status: status.as_u16(),
            body,
        });
    }

    Ok(())
}

fn send_local_report(client: &Client, url: &str, output: &PluginOutput) -> Result<()> {
    let response = client.post(url).json(output).send()?;
    let status = response.status();
    let body = response.text().unwrap_or_default();

    if !status.is_success() {
        return Err(CollectorError::Ingest {
            status: status.as_u16(),
            body,
        });
    }

    Ok(())
}

fn has_batch_payload(batch: &IngestBatch) -> bool {
    !batch.metadata.clusters.is_empty()
        || !batch.metadata.nodes.is_empty()
        || !batch.metadata.images.is_empty()
        || !batch.metadata.sandboxes.is_empty()
        || !batch.metrics.is_empty()
        || !batch.events.is_empty()
        || !batch.traces.is_empty()
        || !batch.profiles.is_empty()
}

struct LocalHttpRequest {
    method: String,
    path: String,
    body: Vec<u8>,
}

fn start_local_report_server(addr: &str) -> Result<Receiver<PluginOutput>> {
    let listener = TcpListener::bind(addr)?;
    let (sender, receiver) = mpsc::channel();
    let addr = addr.to_string();

    thread::spawn(move || {
        println!(
            "{}",
            json!({
                "level": "info",
                "message": "local_report_server_started",
                "addr": addr,
                "endpoint": "/api/local/ingest",
            })
        );

        for stream in listener.incoming() {
            match stream {
                Ok(stream) => {
                    let sender = sender.clone();
                    thread::spawn(move || handle_local_report_connection(stream, sender));
                }
                Err(error) => eprintln!(
                    "{}",
                    json!({
                        "level": "error",
                        "message": "local_report_accept_failed",
                        "error": error.to_string(),
                    })
                ),
            }
        }
    });

    Ok(receiver)
}

fn handle_local_report_connection(mut stream: TcpStream, sender: Sender<PluginOutput>) {
    match read_local_http_request(&mut stream) {
        Ok(request) => {
            if request.method == "GET" && request.path == "/health" {
                let _ = write_http_response(&mut stream, 200, "OK", r#"{"status":"ok"}"#);
                return;
            }

            if request.path != "/api/local/ingest" {
                let _ =
                    write_http_response(&mut stream, 404, "Not Found", r#"{"error":"not_found"}"#);
                return;
            }

            if request.method != "POST" {
                let _ = write_http_response(
                    &mut stream,
                    405,
                    "Method Not Allowed",
                    r#"{"error":"method_not_allowed"}"#,
                );
                return;
            }

            match serde_json::from_slice::<PluginOutput>(&request.body) {
                Ok(output) => match sender.send(output) {
                    Ok(()) => {
                        let _ = write_http_response(
                            &mut stream,
                            202,
                            "Accepted",
                            r#"{"status":"accepted"}"#,
                        );
                    }
                    Err(error) => {
                        let _ = write_http_response(
                            &mut stream,
                            503,
                            "Service Unavailable",
                            &json!({ "error": error.to_string() }).to_string(),
                        );
                    }
                },
                Err(error) => {
                    let _ = write_http_response(
                        &mut stream,
                        400,
                        "Bad Request",
                        &json!({ "error": error.to_string() }).to_string(),
                    );
                }
            }
        }
        Err(error) => {
            let _ = write_http_response(
                &mut stream,
                400,
                "Bad Request",
                &json!({ "error": error.to_string() }).to_string(),
            );
        }
    }
}

fn read_local_http_request(stream: &mut TcpStream) -> Result<LocalHttpRequest> {
    let mut buffer = Vec::new();
    let mut chunk = [0_u8; 4096];
    let header_end = loop {
        let size = stream.read(&mut chunk)?;
        if size == 0 {
            return Err(CollectorError::Config(
                "connection closed before HTTP headers".to_string(),
            ));
        }
        buffer.extend_from_slice(&chunk[..size]);

        if buffer.len() > 1024 * 1024 {
            return Err(CollectorError::Config(
                "local report request is too large".to_string(),
            ));
        }

        if let Some(index) = find_header_end(&buffer) {
            break index;
        }
    };

    let header_bytes = &buffer[..header_end];
    let headers = String::from_utf8_lossy(header_bytes);
    let mut lines = headers.lines();
    let request_line = lines
        .next()
        .ok_or_else(|| CollectorError::Config("missing HTTP request line".to_string()))?;
    let mut parts = request_line.split_whitespace();
    let method = parts
        .next()
        .ok_or_else(|| CollectorError::Config("missing HTTP method".to_string()))?
        .to_string();
    let path = parts
        .next()
        .ok_or_else(|| CollectorError::Config("missing HTTP path".to_string()))?
        .to_string();
    let content_length = lines
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            if name.eq_ignore_ascii_case("content-length") {
                value.trim().parse::<usize>().ok()
            } else {
                None
            }
        })
        .unwrap_or(0);

    if content_length > 1024 * 1024 {
        return Err(CollectorError::Config(
            "local report body is too large".to_string(),
        ));
    }

    let body_start = header_end + 4;
    while buffer.len() < body_start + content_length {
        let size = stream.read(&mut chunk)?;
        if size == 0 {
            return Err(CollectorError::Config(
                "connection closed before HTTP body".to_string(),
            ));
        }
        buffer.extend_from_slice(&chunk[..size]);
    }

    Ok(LocalHttpRequest {
        method,
        path,
        body: buffer[body_start..body_start + content_length].to_vec(),
    })
}

fn find_header_end(buffer: &[u8]) -> Option<usize> {
    buffer.windows(4).position(|window| window == b"\r\n\r\n")
}

fn write_http_response(
    stream: &mut TcpStream,
    status: u16,
    reason: &str,
    body: &str,
) -> std::io::Result<()> {
    let response = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    );
    stream.write_all(response.as_bytes())
}

#[derive(Debug)]
struct MemorySnapshot {
    total_bytes: u64,
    available_bytes: u64,
    used_bytes: u64,
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

fn sandbox_status_from_docker(state: &DockerState) -> String {
    let status = state.status.to_ascii_lowercase();
    if state.oom_killed || !state.error.is_empty() {
        "failed".to_string()
    } else if state.running || matches!(status.as_str(), "created" | "paused" | "restarting") {
        "running".to_string()
    } else {
        "stopped".to_string()
    }
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

fn normalize_docker_timestamp(value: &str) -> Option<String> {
    if value.is_empty() || value.starts_with("0001-01-01T00:00:00") {
        return None;
    }

    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|time| timestamp(time.with_timezone(&Utc)))
}

fn duration_ms_between(start: &str, end: &str) -> Option<f64> {
    let start = DateTime::parse_from_rfc3339(start).ok()?;
    let end = DateTime::parse_from_rfc3339(end).ok()?;
    let duration = end.signed_duration_since(start);
    Some(duration.num_milliseconds().max(0) as f64)
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

fn metric(
    timestamp: &str,
    name: &str,
    value: f64,
    unit: &str,
    group: &str,
    node_id: &str,
    sandbox_id: &str,
    runtime_type: &str,
) -> MetricSample {
    MetricSample {
        timestamp: timestamp.to_string(),
        name: name.to_string(),
        value,
        unit: Some(unit.to_string()),
        group: Some(group.to_string()),
        sandbox_id: Some(sandbox_id.to_string()),
        node_id: Some(node_id.to_string()),
        image_id: None,
        runtime_type: Some(runtime_type.to_string()),
        attributes: None,
    }
}

fn node_metric(
    timestamp: &str,
    name: &str,
    value: f64,
    unit: &str,
    group: &str,
    node_id: &str,
) -> MetricSample {
    MetricSample {
        timestamp: timestamp.to_string(),
        name: name.to_string(),
        value,
        unit: Some(unit.to_string()),
        group: Some(group.to_string()),
        sandbox_id: None,
        node_id: Some(node_id.to_string()),
        image_id: None,
        runtime_type: None,
        attributes: None,
    }
}

fn command_plugin_config() -> Option<CommandPluginConfig> {
    let command = env::var("RUNTIMEPULSE_COMMAND_PLUGIN_CMD").ok()?;
    Some(CommandPluginConfig {
        name: env::var("RUNTIMEPULSE_COMMAND_PLUGIN_NAME")
            .unwrap_or_else(|_| "command".to_string()),
        command,
    })
}

fn http_plugin_config() -> Option<HttpPluginConfig> {
    let url = env::var("RUNTIMEPULSE_HTTP_PLUGIN_URL").ok()?;
    Some(HttpPluginConfig {
        name: env::var("RUNTIMEPULSE_HTTP_PLUGIN_NAME").unwrap_or_else(|_| "http".to_string()),
        url,
    })
}

fn env_u64(name: &str) -> Option<u64> {
    env::var(name).ok()?.parse::<u64>().ok()
}

fn env_bool(name: &str) -> bool {
    matches!(
        env::var(name).ok().as_deref(),
        Some("1") | Some("true") | Some("TRUE") | Some("yes") | Some("YES")
    )
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
