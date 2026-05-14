//! Profiling data sources.
//!
//! Profiling sources may attach at host or sandbox scope depending on the probe
//! and runtime permissions.

pub mod ebpf;
pub mod perf;
