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

`containerd-sandbox-cgroupfs` reuses the lifecycle-owned containerd event stream
to maintain an active task set keyed by namespace/container id. It samples only
those task PIDs via `/proc/<pid>/cgroup`, so it avoids broad cgroup scanning and
keeps Kubernetes/containerd sandbox metrics aligned with the same
`k8s-{namespace}-{pod}-{container}` identity used by inventory, events, and
Prometheus metrics.

`cri-startup-trace` is the lightweight CRI+containerd startup correlator for
CRI/containerd deployments. Enabling it also enables `kubelet-events` and
`containerd-events`. It does not proxy the CRI socket. Instead it correlates
`SANDBOX_CREATED`/`SANDBOX_READY` CRI events with containerd
container/task create/start events and emits a `sandbox.startup.e2e` trace plus
child spans such as `cri.sandbox.create_to_ready`,
`containerd.container.create_to_task_start`, `containerd.task.create`, and
`containerd.task.start`. This first pass is intentionally event based: its
start point is the first observable CRI/containerd sandbox event, not the
internal `RunPodSandbox` function entry.

The high-fidelity startup path now includes a minimal containerd CRI
`RunPodSandbox`, CNI setup, and OCI/Kata runtime shim Go-uProbe profiles in the
bundled startup probe, and should next add request-object decoding,
combined with exec/exit capture for CNI and runtime helper binaries. The goal is
to attribute CNI plugin cost, `iptables`/`nft`/`ip`/`tc` helper calls, and
`runc`/`kata-runtime`/hypervisor invocations to the active RunPodSandbox
context. Plain process exec tracing is not sufficient by itself because
containerd can run concurrent sandbox creations; RunPodSandbox context and
stable IDs such as `CNI_CONTAINERID`, containerd sandbox id, OCI bundle path,
and CRI sandbox id must drive correlation. CRI socket proxying is not part of
the preferred design.

The first concrete exporter bridge is available as
`tools/runtimepulse-startup-probe/runtimepulse-startup-probe`. It uses real
`bpftrace` `execve`/process-exit tracepoints to emit raw call-chain events for
CNI plugin binaries, OCI/Kata runtime binaries, containerd shims, and optional
helper binaries. It now supports an optional `--enable-go-uprobes` mode that discovers stripped
containerd/runtime-shim Go pclntab symbols and emits CRI `RunPodSandbox`,
optional `cni.setup`, and optional OCI/Kata shim uprobe observations joined to
the sandbox event window. Future native depth should decode RunPodSandbox
request/response context while preserving the same startup-callchain JSON
contract.

`startup-callchain` is the report-ingestion bridge for that later high-fidelity
path. External uprobe/eBPF exporters can write JSON/JSONL or be invoked by
`RUNTIMEPULSE_STARTUP_CALLCHAIN_REPORT_CMD`; the host-agent normalizes the
report into trace spans, startup metrics, and a `startup.callchain.observed`
event. The expected report can either carry one sandbox startup trace plus spans
such as `cri.run_pod_sandbox`, `cni.plugin.bridge`, `process.exec.iptables`,
`oci.runc.create`, `kata.vm.boot`, or `kata.agent.connect`, or carry raw
uprobe/eBPF enter/exit events under `events`/`uprobeEvents`/`rawEvents`.
Raw events are paired by `requestId`/`correlationId`/`callId` first, then by
function + pid + sandbox id, and are converted into the same stage spans before
metric derivation. Pairing quality is emitted as `sandbox.startup.uprobe_*`
metrics plus event attributes, so missing enter/exit records from an exporter are
visible instead of silently skewing the call-chain. The CNI plugin stage is identified from the CNI plugin binary
that the CRI/containerd path invokes, not from helper commands executed inside
that plugin. Helper binaries such as `iptables`, `nft`, `ip`, and `tc` remain
secondary drill-down spans/metrics.
Attributes should include stable correlation fields such as `CNI_CONTAINERID`,
OCI bundle path, runtime handler, PID/PPID, argv/env, and helper-binary role. If
the exporter does not provide an explicit `traceId`, RuntimePulse derives the
same stable sandbox id as CRI/containerd events (`k8s-{namespace}-{pod}-{container}`
when pod fields are present, otherwise the CRI/containerd sandbox id), uses
`cri-containerd-startup-{stableSandboxId}`, and parents the call-chain root under
the existing `sandbox.startup.e2e` root span. Detailed RunPod/CNI/OCI/Kata spans
therefore join the lightweight CRI+containerd startup trace by default. If one
exporter capture window sees multiple sandbox ids, it should emit a top-level
`reports` array with one lightweight report per sandbox id plus a per-report
`startTime`/`endTime`/`durationMs` window; the bundled
`runtimepulse-startup-probe` does this by default. See
`rust-collector/examples/startup-uprobe-events.json` for the raw event shape.
When summary values are absent, RuntimePulse derives first-pass aggregate
metrics from the spans, including CNI duration/plugin count, OCI duration/call
count, binary/helper execution count and duration, and Kata stage duration. The CNI plugin binary is identified from the plugin-stage spans themselves rather than from helper binaries inside the plugin, and per-plugin metrics such as `sandbox.startup.cni.plugin.bridge_duration_ms` provide plugin-level attribution when multiple CNI binaries run in one RunPodSandbox call.

Docker sources are retained for single-node and local validation scenarios.
They are not the preferred Kubernetes path.

Regular Kubernetes resource metrics should be imported from the cluster's
existing metrics platform, typically Prometheus scraping kubelet/cAdvisor. The
`kubernetes-metrics` host-agent source maps those Prometheus series into
RuntimePulse sandbox metrics. RuntimePulse host-side collectors should focus on
runtime/image/startup/profile gaps that the standard metrics platform does not
cover.
Prometheus network counters are normally pod-scoped rather than container-scoped;
RuntimePulse keeps them as pod-scoped metric series and attaches them to matching
container sandbox detail queries by `k8s.namespace` and `k8s.pod`.

For Kubernetes identity, containerd inventory/events and Prometheus metrics
should converge on `k8s-{namespace}-{pod}-{container}` sandbox ids. The raw
containerd container id remains available in attributes for runtime debugging.
Containerd startup trace spans follow the same identity rule, so `container.startup`
timing appears on the same sandbox detail page as K8s resource metrics and
containerd lifecycle events.
CRI/Kubelet enrichment events follow this identity rule when pod/container
labels are present; only label-poor events fall back to runtime object ids.

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
`profile-report` is the first generic ingestion path: real profiling tools write
RuntimePulse JSON/JSONL profile artifact indexes, and host-agent forwards them
through the same bounded queue, batch sender, and outlet path as other host
sources. Native perf/eBPF collectors can later reuse the same normalized
`ProfileArtifact` output instead of creating a separate profile channel.

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
| Profile artifact reports | `sources/profiling/report.rs` converts perf/eBPF/third-party JSON/JSONL profile indexes into `ProfileArtifact` rows |
| Runtime diagnostic reports | `sources/runtime/diagnostics.rs` converts support-bundle/log/inspect artifact indexes into diagnostic events, metrics, and capture spans |
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

1. Keep the non-Kubernetes P1 path first. Kata, Firecracker, and gVisor now have baseline host-agent/outlet report sources, and `sandbox-reconcile` can remove stale live sandboxes for report-only runtimes via `snapshot.scope`/`snapshot.sandboxIds`.
2. Deepen snapshotter/image-cache fidelity next. The generic `image-cache` report path accepts file reports, exporter commands, common snapshotter state/index JSON and JSONL, Prometheus text exposition, layer-level cache counters, prefetch records, and timeline spans; add native parsers for nydus, stargz, overlaybd, or other snapshotters once their real local formats are selected.
3. Deepen runtime diagnostic and profile artifact workflows without moving eBPF-specific work forward yet. Generic diagnostic-report ingestion supports files, exporter commands, and artifact-index aliases, Docker diagnostics can be exported with `runtimepulse-collector docker-diagnostics`, and CRI/containerd diagnostics are available through `crictl-diagnostics` and `containerd-diagnostics`. Generic profile reports and artifact-index aliases plus native perf-script target/duration inference and perf folded-stack conversion with command input is available; eBPF backend deepening remains deferred.
4. Complete the CRI+containerd runc/Kata path before broader Kubernetes integration: deepen `cri-startup-trace`, feed detailed uprobe/exporter data through `startup-callchain`, add RunPodSandbox/OCI-runtime uprobe profiles, then attribute CNI and helper-binary costs without introducing a CRI proxy.
5. Complete the Kubernetes path on containerd after the CRI+containerd runtime path: treat `containerd-inventory` and `containerd-events` in the `k8s.io` namespace as the primary source, and use CRI/Kubelet JSONL events only as lifecycle enrichment until a native CRI client is needed.
6. Expand the Kubernetes metrics adapter beyond the first Prometheus vector queries when real cluster label shapes are known; keep direct cgroup sampling as a fallback only.
7. Split active sandbox sampling into separate processes only if the single host-agent process becomes too coarse.
8. Keep production security, tenancy/RBAC, and full hardening last until collector data quality and workflows stabilize.
