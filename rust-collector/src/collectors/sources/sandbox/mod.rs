//! Sandbox data sources.
//!
//! Sandbox sources are lifecycle-triggered. They attach after a runtime
//! `started` event and stop after the matching `stopped` event.

pub mod cgroupfs;
pub mod firecracker;
pub mod gvisor;
pub mod kata;
pub mod manager;
pub mod reconcile;
