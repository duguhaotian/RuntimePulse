//! Host collector layer.
//!
//! Host collectors run with host visibility and report node-level data,
//! runtime inventory, lifecycle events, image state, and kernel/profile data.

pub mod cgroupfs;
pub mod docker;
pub mod ebpf;
pub mod procfs;
