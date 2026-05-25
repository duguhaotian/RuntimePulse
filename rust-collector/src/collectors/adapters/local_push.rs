//! Local push adapter.
//!
//! Sidecars and host tools push RuntimePulse partial ingest JSON to the outlet's
//! local HTTP endpoint. This module owns the local-push contract so outlet HTTP
//! ingress can stay small and future non-HTTP transports can reuse the same
//! validation/acknowledgement boundary.

use serde_json::json;
use std::sync::mpsc::Sender;

use crate::collectors::core::error::{CollectorError, Result};
use crate::collectors::core::model::PluginOutput;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalPushAck {
    pub status: u16,
    pub reason: &'static str,
    pub body: String,
}

impl LocalPushAck {
    fn accepted() -> Self {
        Self {
            status: 202,
            reason: "Accepted",
            body: json!({ "status": "accepted" }).to_string(),
        }
    }

    fn rejected(status: u16, reason: &'static str, error: impl ToString) -> Self {
        Self {
            status,
            reason,
            body: json!({ "error": error.to_string() }).to_string(),
        }
    }
}

pub fn accept_local_push_json(body: &[u8], sender: &Sender<PluginOutput>) -> LocalPushAck {
    match parse_local_push_json(body).and_then(|output| enqueue_local_push(output, sender)) {
        Ok(()) => LocalPushAck::accepted(),
        Err(CollectorError::Json(error)) => LocalPushAck::rejected(400, "Bad Request", error),
        Err(error) => LocalPushAck::rejected(503, "Service Unavailable", error),
    }
}

pub fn parse_local_push_json(body: &[u8]) -> Result<PluginOutput> {
    Ok(serde_json::from_slice::<PluginOutput>(body)?)
}

pub fn enqueue_local_push(output: PluginOutput, sender: &Sender<PluginOutput>) -> Result<()> {
    sender.send(output).map_err(|error| CollectorError::Plugin {
        plugin: "local-push".to_string(),
        message: error.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    #[test]
    fn accepts_valid_partial_output() {
        let (tx, rx) = mpsc::channel();
        let ack = accept_local_push_json(br#"{"source":"sidecar","metrics":[]}"#, &tx);

        assert_eq!(ack.status, 202);
        assert_eq!(rx.try_recv().unwrap().source.as_deref(), Some("sidecar"));
    }

    #[test]
    fn rejects_invalid_json() {
        let (tx, _rx) = mpsc::channel();
        let ack = accept_local_push_json(b"not-json", &tx);

        assert_eq!(ack.status, 400);
        assert!(ack.body.contains("error"));
    }
}
