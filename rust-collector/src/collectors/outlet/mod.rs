//! Node-local collector outlet.
//!
//! The outlet owns local ingress, batching, retry, and central ingest delivery.

pub mod batcher;
pub mod http_ingress;
pub mod sender;
