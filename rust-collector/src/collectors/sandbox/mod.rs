//! Sandbox collector layer.
//!
//! Sandbox collectors attach after lifecycle `started` events and stop after
//! matching `stopped` events. Each runtime shape owns its path resolution and
//! sampling details.

pub mod cgroupfs;
pub mod firecracker;
pub mod gvisor;
pub mod kata;
pub mod lifecycle;
