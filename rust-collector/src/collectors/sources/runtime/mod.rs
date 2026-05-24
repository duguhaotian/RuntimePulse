//! Runtime data sources.
//!
//! Runtime sources discover sandbox/container inventory, lifecycle, and runtime
//! metadata. They may need host runtime sockets or APIs.

pub mod containerd;
pub mod containerd_diagnostics;
pub mod cri_diagnostics;
pub mod diagnostics;
pub mod docker;
pub mod kubelet;
