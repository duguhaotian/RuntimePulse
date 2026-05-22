//! Kubernetes metrics adapter.
//!
//! Kubernetes deployments usually already collect container resource metrics via
//! kubelet/cAdvisor into Prometheus. This source imports those metrics instead of
//! re-sampling the same cgroups from RuntimePulse.

use chrono::{DateTime, SecondsFormat, Utc};
use reqwest::blocking::Client;
use serde::Deserialize;
use serde_json::{json, Map};
use std::env;
use std::time::Duration;

use crate::collectors::core::config::CollectorConfig;
use crate::collectors::core::error::{CollectorError, Result};
use crate::collectors::core::model::{Metadata, MetricSample, PluginOutput};
use crate::collectors::core::plugin::CollectorPlugin;

const DEFAULT_TIMEOUT_MS: u64 = 3000;

pub struct KubernetesMetricsPlugin {
    client: Client,
    base_url: String,
    queries: Vec<KubernetesMetricQuery>,
    timeout: Duration,
}

struct KubernetesMetricQuery {
    name: &'static str,
    label: &'static str,
    unit: &'static str,
    group: &'static str,
    scope: KubernetesMetricScope,
    query: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum KubernetesMetricScope {
    Container,
    Pod,
}

#[derive(Debug, Deserialize)]
struct PrometheusResponse {
    status: String,
    data: PrometheusData,
}

#[derive(Debug, Deserialize)]
struct PrometheusData {
    result: Vec<PrometheusVectorSample>,
}

#[derive(Debug, Deserialize)]
struct PrometheusVectorSample {
    metric: Map<String, serde_json::Value>,
    value: (f64, String),
}

impl KubernetesMetricsPlugin {
    pub fn from_env() -> Option<Self> {
        let base_url = env::var("RUNTIMEPULSE_PROMETHEUS_URL")
            .ok()
            .filter(|value| !value.trim().is_empty())?;
        let timeout = Duration::from_millis(
            env_u64("RUNTIMEPULSE_PROMETHEUS_TIMEOUT_MS").unwrap_or(DEFAULT_TIMEOUT_MS),
        );

        Some(Self {
            client: Client::new(),
            base_url: base_url.trim_end_matches('/').to_string(),
            queries: default_queries(),
            timeout,
        })
    }
}

impl CollectorPlugin for KubernetesMetricsPlugin {
    fn name(&self) -> &str {
        "kubernetes-metrics"
    }

    fn collect(&mut self, now: DateTime<Utc>, config: &CollectorConfig) -> Result<PluginOutput> {
        let timestamp = now.to_rfc3339_opts(SecondsFormat::Millis, true);
        let mut output = PluginOutput {
            source: None,
            metadata: Metadata {
                clusters: vec![json!({
                    "id": config.cluster_id,
                    "name": config.cluster_id,
                    "environment": "collector"
                })],
                nodes: Vec::new(),
                images: Vec::new(),
                sandboxes: Vec::new(),
            },
            metrics: Vec::new(),
            events: Vec::new(),
            traces: Vec::new(),
            profiles: Vec::new(),
        };

        for query in &self.queries {
            let response = self.query_prometheus(&query.query)?;
            if response.status != "success" {
                return Err(CollectorError::Plugin {
                    plugin: "kubernetes-metrics".to_string(),
                    message: format!(
                        "Prometheus query `{}` returned {}",
                        query.label, response.status
                    ),
                });
            }

            for sample in response.data.result {
                let Some((namespace, pod, container)) =
                    sample_identity(&sample.metric, query.scope)
                else {
                    continue;
                };
                if container == "POD" || container.is_empty() || pod.is_empty() {
                    continue;
                }

                let value = sample.value.1.parse::<f64>().unwrap_or(0.0);
                let sandbox_id = kubernetes_sandbox_id(&namespace, &pod, &container);
                if query.scope == KubernetesMetricScope::Container {
                    output.metadata.sandboxes.push(json!({
                        "id": sandbox_id,
                        "clusterId": config.cluster_id,
                        "nodeId": label_value(&sample.metric, "node").unwrap_or_else(|| config.node_id.clone()),
                        "namespace": namespace,
                        "workloadId": pod,
                        "workloadName": format!("{pod}/{container}"),
                        "imageId": "kubernetes-image-unknown",
                        "imageRef": label_value(&sample.metric, "image").unwrap_or_else(|| "kubernetes/unknown:latest".to_string()),
                        "runtimeType": "kubernetes",
                        "runtimeVersion": "prometheus-metrics",
                        "status": "running",
                        "startupDurationMs": 0,
                        "cpuAvg": 0,
                        "memoryPeakBytes": 0,
                        "labels": {
                            "collector": "runtimepulse-rust-collector",
                            "plugin": "kubernetes-metrics",
                            "scope": config.collection_scope,
                        },
                        "attributes": {
                            "collector.scope": config.collection_scope,
                            "metrics.source": "prometheus",
                            "metrics.scope": query.scope.as_str(),
                            "k8s.namespace": namespace,
                            "k8s.pod": pod,
                            "k8s.container": container,
                        }
                    }));
                }
                let mut metric = MetricSample {
                    timestamp: timestamp.clone(),
                    name: query.name.to_string(),
                    value,
                    unit: Some(query.unit.to_string()),
                    group: Some(query.group.to_string()),
                    sandbox_id: Some(sandbox_id),
                    node_id: label_value(&sample.metric, "node")
                        .or_else(|| Some(config.node_id.clone())),
                    image_id: None,
                    runtime_type: Some("kubernetes".to_string()),
                    attributes: Some(metric_attributes(&sample.metric, query)),
                };
                if metric.node_id.as_deref() == Some("") {
                    metric.node_id = Some(config.node_id.clone());
                }
                output.metrics.push(metric);
            }
        }

        dedupe_metadata_by_id(&mut output.metadata.sandboxes);
        Ok(output)
    }
}

impl KubernetesMetricsPlugin {
    fn query_prometheus(&self, query: &str) -> Result<PrometheusResponse> {
        Ok(self
            .client
            .get(format!("{}/api/v1/query", self.base_url))
            .query(&[("query", query)])
            .timeout(self.timeout)
            .send()?
            .error_for_status()?
            .json()?)
    }
}

fn default_queries() -> Vec<KubernetesMetricQuery> {
    vec![
        KubernetesMetricQuery {
            name: "sandbox.cpu.usage_ratio",
            label: "container_cpu_usage_seconds_total",
            unit: "ratio",
            group: "cpu",
            scope: KubernetesMetricScope::Container,
            query: env::var("RUNTIMEPULSE_PROMETHEUS_QUERY_CPU").unwrap_or_else(|_| {
                "sum by (namespace,pod,container,node) (rate(container_cpu_usage_seconds_total{container!=\"\",container!=\"POD\"}[1m]))".to_string()
            }),
        },
        KubernetesMetricQuery {
            name: "sandbox.memory.working_set_bytes",
            label: "container_memory_working_set_bytes",
            unit: "bytes",
            group: "memory",
            scope: KubernetesMetricScope::Container,
            query: env::var("RUNTIMEPULSE_PROMETHEUS_QUERY_MEMORY").unwrap_or_else(|_| {
                "sum by (namespace,pod,container,node) (container_memory_working_set_bytes{container!=\"\",container!=\"POD\"})".to_string()
            }),
        },
        KubernetesMetricQuery {
            name: "sandbox.network.rx_bytes",
            label: "container_network_receive_bytes_total",
            unit: "bytes/s",
            group: "network",
            scope: KubernetesMetricScope::Pod,
            query: env::var("RUNTIMEPULSE_PROMETHEUS_QUERY_NETWORK_RX").unwrap_or_else(|_| {
                "sum by (namespace,pod) (rate(container_network_receive_bytes_total[1m]))".to_string()
            }),
        },
        KubernetesMetricQuery {
            name: "sandbox.network.tx_bytes",
            label: "container_network_transmit_bytes_total",
            unit: "bytes/s",
            group: "network",
            scope: KubernetesMetricScope::Pod,
            query: env::var("RUNTIMEPULSE_PROMETHEUS_QUERY_NETWORK_TX").unwrap_or_else(|_| {
                "sum by (namespace,pod) (rate(container_network_transmit_bytes_total[1m]))".to_string()
            }),
        },
        KubernetesMetricQuery {
            name: "sandbox.io.read_bytes",
            label: "container_fs_reads_bytes_total",
            unit: "bytes/s",
            group: "io",
            scope: KubernetesMetricScope::Container,
            query: env::var("RUNTIMEPULSE_PROMETHEUS_QUERY_FS_READ").unwrap_or_else(|_| {
                "sum by (namespace,pod,container,node) (rate(container_fs_reads_bytes_total{container!=\"\",container!=\"POD\"}[1m]))".to_string()
            }),
        },
        KubernetesMetricQuery {
            name: "sandbox.io.write_bytes",
            label: "container_fs_writes_bytes_total",
            unit: "bytes/s",
            group: "io",
            scope: KubernetesMetricScope::Container,
            query: env::var("RUNTIMEPULSE_PROMETHEUS_QUERY_FS_WRITE").unwrap_or_else(|_| {
                "sum by (namespace,pod,container,node) (rate(container_fs_writes_bytes_total{container!=\"\",container!=\"POD\"}[1m]))".to_string()
            }),
        },
    ]
}

impl KubernetesMetricScope {
    fn as_str(self) -> &'static str {
        match self {
            Self::Container => "container",
            Self::Pod => "pod",
        }
    }
}

fn sample_identity(
    metric: &Map<String, serde_json::Value>,
    scope: KubernetesMetricScope,
) -> Option<(String, String, String)> {
    let namespace = label_value(metric, "namespace")?;
    let pod = label_value(metric, "pod")
        .or_else(|| label_value(metric, "pod_name"))
        .unwrap_or_default();
    let container = if scope == KubernetesMetricScope::Pod {
        "pod".to_string()
    } else {
        label_value(metric, "container")
            .or_else(|| label_value(metric, "container_name"))
            .unwrap_or_default()
    };
    Some((namespace, pod, container))
}

fn kubernetes_sandbox_id(namespace: &str, pod: &str, container: &str) -> String {
    format!(
        "k8s-{}-{}-{}",
        sanitize_id(namespace),
        sanitize_id(pod),
        sanitize_id(container)
    )
}

fn metric_attributes(
    metric: &Map<String, serde_json::Value>,
    query: &KubernetesMetricQuery,
) -> Map<String, serde_json::Value> {
    let mut attributes = Map::new();
    attributes.insert("plugin".to_string(), json!("kubernetes-metrics"));
    attributes.insert("metrics.source".to_string(), json!("prometheus"));
    attributes.insert("metrics.scope".to_string(), json!(query.scope.as_str()));
    attributes.insert("prometheus.metric".to_string(), json!(query.label));
    for key in ["namespace", "pod", "container", "node", "image"] {
        if let Some(value) = label_value(metric, key) {
            attributes.insert(format!("k8s.{key}"), json!(value));
        }
    }
    if query.scope == KubernetesMetricScope::Pod {
        attributes.insert("k8s.container".to_string(), json!("pod"));
    }
    attributes
}

fn label_value(metric: &Map<String, serde_json::Value>, key: &str) -> Option<String> {
    metric.get(key)?.as_str().map(ToOwned::to_owned)
}

fn dedupe_metadata_by_id(rows: &mut Vec<serde_json::Value>) {
    let mut seen = std::collections::HashSet::new();
    rows.retain(|row| {
        let Some(id) = row.get("id").and_then(serde_json::Value::as_str) else {
            return false;
        };
        seen.insert(id.to_string())
    });
}

fn sanitize_id(value: &str) -> String {
    let sanitized = value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character.to_ascii_lowercase()
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

fn env_u64(name: &str) -> Option<u64> {
    env::var(name).ok()?.parse::<u64>().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pod_scoped_network_samples_use_pod_identity_without_container_metadata() {
        let query = KubernetesMetricQuery {
            name: "sandbox.network.rx_bytes",
            label: "container_network_receive_bytes_total",
            unit: "bytes/s",
            group: "network",
            scope: KubernetesMetricScope::Pod,
            query: String::new(),
        };
        let metric = Map::from_iter([
            ("namespace".to_string(), json!("default")),
            ("pod".to_string(), json!("runtimepulse-demo")),
        ]);

        let (namespace, pod, container) =
            sample_identity(&metric, query.scope).expect("pod identity");
        assert_eq!(namespace, "default");
        assert_eq!(pod, "runtimepulse-demo");
        assert_eq!(container, "pod");
        assert_eq!(
            kubernetes_sandbox_id(&namespace, &pod, &container),
            "k8s-default-runtimepulse-demo-pod"
        );

        let attributes = metric_attributes(&metric, &query);
        assert_eq!(
            attributes
                .get("metrics.scope")
                .and_then(|value| value.as_str()),
            Some("pod")
        );
        assert_eq!(
            attributes
                .get("k8s.container")
                .and_then(|value| value.as_str()),
            Some("pod")
        );
    }
}
