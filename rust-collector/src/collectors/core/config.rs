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
    pub plugins: Vec<String>,
    pub command_plugin: Option<CommandPluginConfig>,
    pub http_plugin: Option<HttpPluginConfig>,
}

#[derive(Clone, Debug)]
pub struct CommandPluginConfig {
    pub name: String,
    pub command: String,
}

#[derive(Clone, Debug)]
pub struct HttpPluginConfig {
    pub name: String,
    pub url: String,
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
