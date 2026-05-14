//! Shared RuntimePulse ingest model.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginOutput {
    #[serde(default)]
    pub metadata: Metadata,
    #[serde(default)]
    pub metrics: Vec<MetricSample>,
    #[serde(default)]
    pub events: Vec<EventRecord>,
    #[serde(default)]
    pub traces: Vec<TraceSpan>,
    #[serde(default)]
    pub profiles: Vec<ProfileArtifact>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Metadata {
    #[serde(default)]
    pub clusters: Vec<Value>,
    #[serde(default)]
    pub nodes: Vec<Value>,
    #[serde(default)]
    pub images: Vec<Value>,
    #[serde(default)]
    pub sandboxes: Vec<Value>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MetricSample {
    pub timestamp: String,
    pub name: String,
    pub value: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sandbox_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub node_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runtime_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attributes: Option<Map<String, Value>>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EventRecord {
    pub id: String,
    pub timestamp: String,
    pub severity: String,
    pub event_type: String,
    pub event_name: String,
    pub message: String,
    pub source: String,
    pub attributes: Map<String, Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sandbox_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub node_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runtime_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TraceSpan {
    pub trace_id: String,
    pub span_id: String,
    pub span_name: String,
    pub start_time: String,
    pub end_time: String,
    pub duration_ms: f64,
    pub status: String,
    pub attributes: Map<String, Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sandbox_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_span_id: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileArtifact {
    pub id: String,
    pub timestamp: String,
    pub sandbox_id: String,
    pub profile_type: String,
    pub process_role: String,
    pub duration_ms: f64,
    pub sample_count: u64,
    pub object_uri: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub flamegraph: Option<Value>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IngestBatch {
    pub source: String,
    pub observed_at: String,
    pub metadata: Metadata,
    pub metrics: Vec<MetricSample>,
    pub events: Vec<EventRecord>,
    pub traces: Vec<TraceSpan>,
    pub profiles: Vec<ProfileArtifact>,
}
