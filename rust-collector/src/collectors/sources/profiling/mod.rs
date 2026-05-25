//! Profiling data sources.
//!
//! Profiling sources may attach at host or sandbox scope depending on the probe
//! and runtime permissions.

pub mod ebpf;
pub mod ebpf_folded;
pub mod perf;
pub mod perf_folded;
pub mod perf_script;
pub mod report;
