//! Host root cgroupfs collector.
//!
//! This layer reports host/root cgroup metrics only. Sandbox cgroup sampling
//! belongs in `collectors::sandbox` and is started from lifecycle events.
