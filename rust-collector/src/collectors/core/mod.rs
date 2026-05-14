//! Shared collector framework.
//!
//! Core modules own the common RuntimePulse data model, plugin contracts,
//! configuration, report building, and source lifecycle primitives.

pub mod config;
pub mod model;
pub mod plugin;
pub mod report;
