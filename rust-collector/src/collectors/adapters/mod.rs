//! External collector adapters.
//!
//! Adapters are integration mechanisms, not data-source layers. They can run in
//! the outlet, host agent, or sandbox agent depending on the data source.

pub mod command;
pub mod http;
pub mod local_push;
