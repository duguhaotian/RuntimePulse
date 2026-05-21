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

Host-side collectors run as one `host-agent` process on the node. Individual
host collectors and runtime watchers must not synchronously POST to the outlet
from their sampling/event threads. They enqueue partial RuntimePulse reports
into a bounded in-process queue, and a dedicated batch sender flushes those
reports to the outlet.

```text
host-agent
  -> node plugins
  -> runtime event watchers
  -> sandbox sampler manager backed by runtime active sets
  -> bounded report queue
  -> batch sender
  -> collector-outlet POST /api/local/ingest
```

The queue is the host-agent backpressure boundary:

- high-value lifecycle/image failure events should be preserved whenever possible
- periodic metrics and inventory snapshots may be coalesced or dropped first when the queue is full
- sender failures should retry without blocking sampler/event threads
- personal-use deployments persist failed sender batches as JSON spool files and replay them on later flushes

The host-agent reports its own health as node-level metrics:

- `host_agent.up`
- `host_agent.queue.depth`
- `host_agent.reports.enqueued_total`
- `host_agent.reports.dropped_total`
- `host_agent.collect.errors_total`
- `host_agent.sender.batches_sent_total`
- `host_agent.sender.batches_failed_total`
- `host_agent.sender.reports_sent_total`
- `host_agent.spool.files`
- `host_agent.spool.batches_spooled_total`
- `host_agent.spool.batches_replayed_total`
- `host_agent.source.collect.duration_ms`
- `host_agent.source.collect.success`
- `host_agent.source.collect.errors_total`

The first host-agent implementation is configured with
`RUNTIMEPULSE_HOST_AGENT_SOURCES`. The default source set is:

```text
procfs,psi,cgroupfs,docker-inventory,docker-events,docker-sandbox-cgroupfs,image-cache
```

`procfs` reports node CPU, memory, IO, process, and load metrics. `psi` reports
node pressure stall metrics from `/proc/pressure/*` as a separate host source,
so pressure collection health can be tracked independently.

`docker-sandbox-cgroupfs` uses Docker startup inventory to initialize an active
container set, then Docker lifecycle events keep that set current. Periodic
sampling reads only cgroups resolved from that active set.

`containerd-inventory` and `containerd-events` are optional host-agent sources.
They use containerd's Rust client over the host Unix socket and are intended for
root/systemd deployment. Kubernetes collection should use the containerd
`k8s.io` namespace as its primary runtime path; CRI/Kubelet event ingestion is
an enrichment stream for lifecycle context, not the main inventory source. The
event source owns one containerd event subscription, then dispatches
container/task lifecycle updates into RuntimePulse sandbox metadata and event
records, and derives `container.startup` trace spans from create/task-start
pairs. It also turns content and snapshot events into first-pass image timeline
observations, without pretending that generic containerd events contain full
registry pull sub-stage timings.

Docker sources are retained for single-node and local validation scenarios.
They are not the preferred Kubernetes path.

Regular Kubernetes resource metrics should be imported from the cluster's
existing metrics platform, typically Prometheus scraping kubelet/cAdvisor. The
`kubernetes-metrics` host-agent source maps those Prometheus series into
RuntimePulse sandbox metrics. RuntimePulse host-side collectors should focus on
runtime/image/startup/profile gaps that the standard metrics platform does not
cover.

`command` and `http` adapters can also run inside host-agent for host-visible
third-party tools. They must emit RuntimePulse partial output and are enqueued
through the same bounded queue, batching sender, and spool path as native host
sources.

`image-cache` reads RuntimePulse JSON/JSONL emitted by real snapshotter/cache
tools. It is the P0 path for precise image stage durations and lazy-loading
block cache hit curves. RuntimePulse still does not initiate pulls to create
measurements.

Host-agent also reports self-observability metrics for outlet backpressure,
spool usage, per-source collection health, and runtime event stream health. This
lets the UI distinguish central ingest/outlet problems from runtime watcher
problems such as a stopped Docker or containerd event stream.

Failed sender batches are written to `RUNTIMEPULSE_HOST_AGENT_SPOOL_DIR`
(``/tmp/runtimepulse/host-agent-spool`` by default) up to
`RUNTIMEPULSE_HOST_AGENT_SPOOL_MAX_FILES` files. The sender replays the oldest
spooled files before sending new in-memory batches.

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
- lazy-loading cache and block hit data from snapshotter/cache reports
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
| `host-agent` | host process or host-visible deployment, normally systemd-managed | `sources/node/`, `sources/runtime/`, `sources/image/`, host profiling, bounded report queue, batch sender |
| `sandbox-agent` | lifecycle-triggered worker managed by host-agent at first | `sources/sandbox/`, sandbox profiling |
| `third-party-adapters` | outlet, host-agent, or sandbox-agent | `adapters/` used by any source domain |

First implementation can keep `collector-outlet`, `host-agent`, and sandbox samplers as subcommands in one binary. Later they can split into separate binaries without changing the directory model.

## Current Mapping

| Current command/plugin | Target source ownership |
| --- | --- |
| `host-agent` | systemd-friendly host process that runs configurable host collectors, containerd/CRI dispatch for Kubernetes, Docker dispatch for single-node use, image-cache report ingestion, active-set sandbox sampling, and queue-backed sender |
| `host-procfs` | `sources/node/procfs.rs` plus PSI support |
| `host-cgroupfs` | `sources/node/cgroupfs.rs` |
| `host-docker` | `sources/runtime/docker/inventory.rs` |
| `host-containerd` | `sources/runtime/containerd.rs` inventory through containerd gRPC over Unix socket plus content-store image enrichment |
| `host-containerd-events` | `sources/runtime/containerd.rs` containerd event subscription plus lifecycle and image content/snapshot conversion |
| Docker image metadata | `sources/image/layer.rs` via Docker image inspect/history enrichment |
| Docker event stream | `sources/runtime/docker/events.rs` parses Docker container/image events once |
| Docker container lifecycle | `sources/runtime/docker/lifecycle.rs` converts Docker container events to sandbox lifecycle output |
| Docker image events | `sources/image/download.rs` converts Docker image events to image pull/tag/delete output |
| Snapshotter/image cache reports | `sources/image/cache.rs` converts real exporter JSON/JSONL into image stage spans and lazy block-cache curves |
| `host-docker-events` | unified Docker event collection plus semantic dispatch |
| `host-docker-cgroupfs` | `sources/sandbox/cgroupfs.rs` using Docker PID cgroup resolution |
| `host-docker-sandbox-agent` | Debug command for `sources/sandbox/manager.rs`; the same active-set helpers are now reused by `host-agent` |
| `command` | `adapters/command.rs`; outlet plugin and optional host-agent source |
| `http` | `adapters/http.rs`; outlet plugin and optional host-agent source |
| `POST /api/local/ingest` | `outlet/http_ingress.rs` and `adapters/local_push.rs` |

The individual `host-*` commands remain useful for debugging and focused
validation. Production-style node collection should run `host-agent` instead of
starting each source manually.

## Next Steps

1. Complete the Kubernetes path on containerd: treat `containerd-inventory` and `containerd-events` in the `k8s.io` namespace as the primary source, and use CRI/Kubelet JSONL events only as lifecycle enrichment until a native CRI client is needed.
2. Expand the Kubernetes metrics adapter beyond the first Prometheus vector queries when real cluster label shapes are known; keep direct cgroup sampling as a fallback only.
3. Add native parsers for specific snapshotters once their local report formats are known; the generic `image-cache` report ingestion path is in place.
4. Add Kata and Firecracker sandbox sources after the containerd/Kubernetes path is stable.
5. Add eBPF/perf profiling sources and profile artifact ingestion.
6. Add gVisor-specific sources last, after the common containerd/Kubernetes, Kata, Firecracker, and profiling paths are usable.
7. Split active sandbox sampling into separate processes only if the single host-agent process becomes too coarse.
