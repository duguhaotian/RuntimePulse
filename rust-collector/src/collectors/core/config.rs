//! Collector configuration.

use std::env;
use std::path::PathBuf;
use std::time::Duration;

use super::error::Result;

#[derive(Clone, Debug)]
pub struct CollectorConfig {
    pub ingest_url: String,
    pub node_id: String,
    pub cluster_id: String,
    pub interval: Duration,
    pub local_report_addr: String,
    pub local_report_url: String,
    pub collection_scope: String,
    pub once: bool,
    pub cgroup_root: PathBuf,
    pub cgroup_max_entries: usize,
    pub image_cache_report_path: Option<PathBuf>,
    pub profile_report_path: Option<PathBuf>,
    pub diagnostic_report_path: Option<PathBuf>,
    pub diagnostic_report_command: Option<String>,
    pub diagnostic_report_command_timeout: Duration,
    pub perf_report_path: Option<PathBuf>,
    pub perf_folded_path: Option<PathBuf>,
    pub ebpf_report_path: Option<PathBuf>,
    pub perf_profile_command: Option<String>,
    pub ebpf_profile_command: Option<String>,
    pub profile_command_timeout: Duration,
    pub plugins: Vec<String>,
    pub command_plugins: Vec<CommandPluginConfig>,
    pub http_plugins: Vec<HttpPluginConfig>,
}

#[derive(Clone, Debug)]
pub struct CommandPluginConfig {
    pub name: String,
    pub command: String,
    pub timeout: Duration,
}

#[derive(Clone, Debug)]
pub struct HttpPluginConfig {
    pub name: String,
    pub url: String,
    pub timeout: Duration,
}

impl CollectorConfig {
    pub fn from_env() -> Result<Self> {
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
                .unwrap_or_else(|_| "runtimepulse-local".to_string()),
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
            image_cache_report_path: env::var("RUNTIMEPULSE_IMAGE_CACHE_REPORT_PATH")
                .ok()
                .filter(|value| !value.trim().is_empty())
                .map(PathBuf::from),
            profile_report_path: env::var("RUNTIMEPULSE_PROFILE_REPORT_PATH")
                .or_else(|_| env::var("RUNTIMEPULSE_PROFILING_REPORT_PATH"))
                .ok()
                .filter(|value| !value.trim().is_empty())
                .map(PathBuf::from),
            diagnostic_report_path: env::var("RUNTIMEPULSE_DIAGNOSTIC_REPORT_PATH")
                .or_else(|_| env::var("RUNTIMEPULSE_DIAGNOSTICS_REPORT_PATH"))
                .ok()
                .filter(|value| !value.trim().is_empty())
                .map(PathBuf::from),
            diagnostic_report_command: env::var("RUNTIMEPULSE_DIAGNOSTIC_REPORT_CMD")
                .or_else(|_| env::var("RUNTIMEPULSE_DIAGNOSTICS_REPORT_CMD"))
                .ok()
                .filter(|value| !value.trim().is_empty()),
            diagnostic_report_command_timeout: env_duration_ms(
                "RUNTIMEPULSE_DIAGNOSTIC_REPORT_TIMEOUT_MS",
            )
            .or_else(|| env_duration_ms("RUNTIMEPULSE_DIAGNOSTICS_REPORT_TIMEOUT_MS"))
            .unwrap_or_else(adapter_timeout),
            perf_report_path: env::var("RUNTIMEPULSE_PERF_REPORT_PATH")
                .ok()
                .filter(|value| !value.trim().is_empty())
                .map(PathBuf::from),
            perf_folded_path: env::var("RUNTIMEPULSE_PERF_FOLDED_PATH")
                .ok()
                .filter(|value| !value.trim().is_empty())
                .map(PathBuf::from),
            ebpf_report_path: env::var("RUNTIMEPULSE_EBPF_REPORT_PATH")
                .ok()
                .filter(|value| !value.trim().is_empty())
                .map(PathBuf::from),
            perf_profile_command: env::var("RUNTIMEPULSE_PERF_PROFILE_CMD")
                .ok()
                .filter(|value| !value.trim().is_empty()),
            ebpf_profile_command: env::var("RUNTIMEPULSE_EBPF_PROFILE_CMD")
                .ok()
                .filter(|value| !value.trim().is_empty()),
            profile_command_timeout: env_duration_ms("RUNTIMEPULSE_PROFILE_COMMAND_TIMEOUT_MS")
                .unwrap_or_else(adapter_timeout),
            plugins,
            command_plugins: command_plugin_configs(),
            http_plugins: http_plugin_configs(),
        })
    }
}

fn command_plugin_configs() -> Vec<CommandPluginConfig> {
    let default_timeout = adapter_timeout();
    let mut configs = indexed_command_plugin_configs();

    if let Ok(command) = env::var("RUNTIMEPULSE_COMMAND_PLUGIN_CMD") {
        configs.push(CommandPluginConfig {
            name: env::var("RUNTIMEPULSE_COMMAND_PLUGIN_NAME")
                .unwrap_or_else(|_| "command".to_string()),
            command,
            timeout: env_duration_ms("RUNTIMEPULSE_COMMAND_PLUGIN_TIMEOUT_MS")
                .unwrap_or(default_timeout),
        });
    }

    configs
}

fn indexed_command_plugin_configs() -> Vec<CommandPluginConfig> {
    let default_timeout = adapter_timeout();
    let mut entries = env::vars()
        .filter_map(|(key, command)| {
            let index = indexed_plugin_key(&key, "RUNTIMEPULSE_COMMAND_PLUGIN_", "_CMD")?;
            let name = env::var(format!("RUNTIMEPULSE_COMMAND_PLUGIN_{index}_NAME"))
                .unwrap_or_else(|_| format!("command-{index}"));
            let timeout =
                env_duration_ms(&format!("RUNTIMEPULSE_COMMAND_PLUGIN_{index}_TIMEOUT_MS"))
                    .unwrap_or(default_timeout);
            Some((
                index,
                CommandPluginConfig {
                    name,
                    command,
                    timeout,
                },
            ))
        })
        .collect::<Vec<_>>();
    entries.sort_by(|left, right| left.0.cmp(&right.0));
    entries.into_iter().map(|(_, config)| config).collect()
}

fn http_plugin_configs() -> Vec<HttpPluginConfig> {
    let default_timeout = adapter_timeout();
    let mut configs = indexed_http_plugin_configs();

    if let Ok(url) = env::var("RUNTIMEPULSE_HTTP_PLUGIN_URL") {
        configs.push(HttpPluginConfig {
            name: env::var("RUNTIMEPULSE_HTTP_PLUGIN_NAME").unwrap_or_else(|_| "http".to_string()),
            url,
            timeout: env_duration_ms("RUNTIMEPULSE_HTTP_PLUGIN_TIMEOUT_MS")
                .unwrap_or(default_timeout),
        });
    }

    configs
}

fn indexed_http_plugin_configs() -> Vec<HttpPluginConfig> {
    let default_timeout = adapter_timeout();
    let mut entries = env::vars()
        .filter_map(|(key, url)| {
            let index = indexed_plugin_key(&key, "RUNTIMEPULSE_HTTP_PLUGIN_", "_URL")?;
            let name = env::var(format!("RUNTIMEPULSE_HTTP_PLUGIN_{index}_NAME"))
                .unwrap_or_else(|_| format!("http-{index}"));
            let timeout = env_duration_ms(&format!("RUNTIMEPULSE_HTTP_PLUGIN_{index}_TIMEOUT_MS"))
                .unwrap_or(default_timeout);
            Some((index, HttpPluginConfig { name, url, timeout }))
        })
        .collect::<Vec<_>>();
    entries.sort_by(|left, right| left.0.cmp(&right.0));
    entries.into_iter().map(|(_, config)| config).collect()
}

fn indexed_plugin_key(key: &str, prefix: &str, suffix: &str) -> Option<u64> {
    key.strip_prefix(prefix)?
        .strip_suffix(suffix)?
        .parse::<u64>()
        .ok()
}

fn env_u64(name: &str) -> Option<u64> {
    env::var(name).ok()?.parse::<u64>().ok()
}

fn env_duration_ms(name: &str) -> Option<Duration> {
    Some(Duration::from_millis(env_u64(name)?.max(100)))
}

fn adapter_timeout() -> Duration {
    env_duration_ms("RUNTIMEPULSE_ADAPTER_TIMEOUT_MS")
        .unwrap_or_else(|| Duration::from_millis(3000))
}

fn env_bool(name: &str) -> bool {
    matches!(
        env::var(name).ok().as_deref(),
        Some("1") | Some("true") | Some("TRUE") | Some("yes") | Some("YES")
    )
}
