//! Third-party collector adapters.
//!
//! Third-party collectors plug into outlet, host, or sandbox layers through a
//! stable adapter boundary instead of writing directly to central ingest.

pub mod command;
pub mod http;
