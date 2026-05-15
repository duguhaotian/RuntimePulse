# Collector Architecture

RuntimePulse collectors are organized by responsibility first, then by where they run.

The main source tree is:

```text
rust-collector/src/collectors/
  core/
  outlet/
  adapters/
  sources/
    node/
    runtime/
    image/
    sandbox/
    profiling/
```

## Design Principles

- Data semantics decide source ownership.
- Runtime location is a deployment concern, not the only directory boundary.
- Third-party collectors are adapters, not a separate data layer.
- Sandbox metrics are driven by runtime inventory reconciliation plus lifecycle events, not by broad host scans.
- The node-local outlet is the only component that posts to central ingest.

## Core

`core/` owns shared collector framework pieces:

- RuntimePulse ingest model
- plugin/source traits
- configuration
- report normalization
- common source lifecycle contracts

The goal is to keep source implementations small and to avoid each collector inventing its own payload rules.

## Outlet

`outlet/` runs in the node-local collector container.

Responsibilities:

- accept local reports over HTTP
- normalize partial reports
- batch local and in-process outputs
- forward batches to Query API ingest
- later own retry and buffering

The outlet can run generic adapters such as command and HTTP when those adapters are container-safe.

## Adapters

`adapters/` contains integration mechanisms for external collectors:

- `command`: execute a binary that emits RuntimePulse partial ingest JSON
- `http`: poll an API that returns RuntimePulse partial ingest JSON
- `local_push`: accept reports from host tools, sidecars, or sandbox workers

Adapters do not define the data domain. The same adapter style can be used for node, runtime, image, sandbox, or profiling sources.

## Sources

`sources/` is grouped by data semantics.

### Node Sources

`sources/node/` reports host/node state only.

Examples:

- host `/proc`
- host/root cgroupfs
- PSI pressure

Node sources must not create sandbox or image inventory records.

### Runtime Sources

`sources/runtime/` discovers runtime inventory and owns runtime event streams.

Examples:

- Docker inventory
- Docker event stream
- containerd lifecycle
- kubelet pod/sandbox lifecycle

Runtime sources create or update sandbox and image metadata. They can also emit lifecycle events that drive sandbox samplers.

Runtime event streams are collected once per runtime and then dispatched by semantic handlers:

```text
runtime/docker/events.rs
  -> runtime/docker/lifecycle.rs handles container lifecycle events
  -> sources/image/download.rs handles image pull/tag/delete events
  -> sources/sandbox/manager.rs consumes container start/stop events for active-set sampling
```

This keeps event collection unified while preserving data ownership. Runtime modules parse raw runtime events; image, sandbox, and profiling sources derive domain-specific reports from those events. Multiple collectors should not independently run their own `docker events` or containerd event streams for the same node.

Runtime sources must support two discovery paths:

- startup inventory: list already-running sandboxes from the runtime API when the collector starts
- lifecycle watch: observe `started`, `stopped`, and failure events after startup

The startup inventory path prevents collector restarts from missing sandboxes that were already running before the collector came up.

### Image Sources

`sources/image/` reports image behavior.

Examples:

- eager download timeline
- layer timing
- lazy-loading cache and block hit data
- snapshotter cache state

Image sources should attach image-level metrics to image ids created by runtime or image inventory sources.

Image collection has two modes:

- existing image state: inspect already-present images and derive metadata/layer/cache state without changing the host
- pull/process tracking: observe image pull events emitted by Docker/containerd/snapshotters and derive timeline data only when a real pull happens

Collector sources must not initiate production pulls just to create measurements.

### Sandbox Sources

`sources/sandbox/` reports per-sandbox data after lifecycle discovery.

Expected startup flow:

```text
collector/sandbox sampler startup
  -> runtime inventory API
  -> existing sandbox list
  -> sandbox reconcile
  -> sandbox sampler manager
  -> runtime-specific path resolution
  -> sandbox cgroup/trace/profile sampler
  -> local outlet report
```

Expected event flow:

```text
runtime lifecycle watch
  -> started/stopped event
  -> sandbox sampler manager
  -> runtime-specific path resolution
  -> sandbox cgroup/trace/profile sampler
  -> local outlet report
```

Sandbox cgroupfs must not be implemented as a broad host cgroup scan. It should start from either startup inventory reconciliation or a sandbox/container `started` event, and stop or expire after the matching `stopped` event.

Reconciliation should be idempotent:

- already-sampled sandboxes keep their existing sampler
- newly discovered running sandboxes start a sampler
- missing sandboxes are marked stopped or expired after a grace window

### Profiling Sources

`sources/profiling/` reports profile artifacts.

Examples:

- eBPF
- perf
- runtime-specific profiles

Profiling sources can be host-scoped or sandbox-scoped depending on the probe and permissions.

## Runtime Groups

The repo can keep one Rust crate and one binary at first, while still separating runtime groups by responsibility.

| Runtime group | Runs with | Owns |
| --- | --- | --- |
| `collector-outlet` | collector container | `outlet/`, container-safe `adapters/` |
| `host-agent` | host process or host-visible deployment | `sources/node/`, `sources/runtime/`, `sources/image/`, host profiling |
| `sandbox-agent` | lifecycle-triggered worker managed by host-agent at first | `sources/sandbox/`, sandbox profiling |
| `third-party-adapters` | outlet, host-agent, or sandbox-agent | `adapters/` used by any source domain |

First implementation can keep `collector-outlet`, `host-agent`, and sandbox samplers as subcommands in one binary. Later they can split into separate binaries without changing the directory model.

## Current Mapping

| Current command/plugin | Target source ownership |
| --- | --- |
| `host-procfs` | `sources/node/procfs.rs` plus PSI support |
| `host-cgroupfs` | `sources/node/cgroupfs.rs` |
| `host-docker` | `sources/runtime/docker/inventory.rs` |
| Docker image metadata | `sources/image/layer.rs` via Docker image inspect/history enrichment |
| Docker event stream | `sources/runtime/docker/events.rs` parses Docker container/image events once |
| Docker container lifecycle | `sources/runtime/docker/lifecycle.rs` converts Docker container events to sandbox lifecycle output |
| Docker image events | `sources/image/download.rs` converts Docker image events to image pull/tag/delete output |
| `host-docker-events` | unified Docker event collection plus semantic dispatch |
| `host-docker-cgroupfs` | `sources/sandbox/cgroupfs.rs` using Docker PID cgroup resolution |
| `host-docker-sandbox-agent` | `sources/sandbox/manager.rs` combining Docker lifecycle events with active-set cgroupfs sampling |
| `command` | `adapters/command.rs` |
| `http` | `adapters/http.rs` |
| `POST /api/local/ingest` | `outlet/http_ingress.rs` and `adapters/local_push.rs` |

## Next Steps

1. Move shared structs and plugin traits from `main.rs` into `core/`.
2. Move outlet HTTP ingress, batching, and sender logic into `outlet/`.
3. Move command and HTTP plugin implementations into `adapters/`.
4. Move `host-procfs`, `host-cgroupfs`, and `host-docker` implementations into their target `sources/` modules.
5. Add unified runtime event watchers and semantic dispatchers.
6. Add Docker inventory-driven sandbox cgroupfs sampling.
7. Add runtime startup inventory reconciliation and the sandbox sampler manager. Docker has an initial combined manager through `host-docker-sandbox-agent`; next iterations can split active sandbox sampling into dedicated workers if the single-process sampler becomes too coarse.
