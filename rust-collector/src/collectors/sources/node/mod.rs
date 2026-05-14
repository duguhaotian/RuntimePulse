//! Node data sources.
//!
//! Node sources report host/node-level state and must not create sandbox or
//! image inventory records.

pub mod cgroupfs;
pub mod procfs;
pub mod psi;
