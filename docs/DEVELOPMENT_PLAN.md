# RuntimePulse Development Plan

## Current Principle

Development is now collector-first and container-first.

- Default UI data must come from accepted collector batches.
- Do not bind pages directly to backend storage.
- Do not run host-level Node development as the default workflow.
- Build and verify the app through Docker Compose.
- Keep mock data available only as an optional validation profile.

## Phase 1: Frontend Prototype

Status: in progress.

Goals:

- Validate expert analysis workflows.
- Validate information architecture.
- Validate visualization choices for metrics, events, traces, and profiles.
- Keep API adapter replaceable.

Implemented:

- React + TypeScript + Vite frontend.
- Docker/Nginx container deployment.
- `RuntimePulseApi` frontend contract.
- `MockRuntimePulseApi` adapter for optional local UI validation.
- W&B-inspired workspace shell.
- Runs table for sandbox instances.
- Run detail page for individual sandbox analysis.
- Runtime comparison report.
- Metric chart, event timeline, trace waterfall, profile artifact table.

Next UI improvements:

- Metric hover tooltip. Implemented with per-point hover cards and chart crosshair.
- Multiple-run comparison overlays. Implemented for selected runs in the Runs table.
- Resizable metric panels. Implemented with compact, standard, and expanded chart layouts.
- Metric group pinning. Implemented for metrics tab chart ordering and pinned state.
- Run selection and bulk compare. Implemented for startup metrics and trace waterfall comparison.
- Trace span detail panel. Implemented for startup waterfall spans.
- Profile artifact preview panel. Implemented with flamegraph preview and selected profile details.
- Saved views for common expert workflows. Implemented for explorer filters and selection state.

## Phase 2: Query API and Storage Design

Goals:

- Replace mock adapter with an HTTP adapter.
- Add a backend query API without implementing collectors yet.
- Define storage schema and query boundaries.

Implemented:

- `HttpRuntimePulseApi` frontend adapter that matches the existing `RuntimePulseApi` contract.
- `HttpRuntimePulseApi` frontend adapter is now the default frontend data path.
- Minimal `runtimepulse-query-api` service that serves live collector telemetry through the planned HTTP endpoints.
- Docker Compose wiring for frontend-to-query-api validation through `/api`.
- Storage model and Query API boundary design: [`docs/STORAGE_DESIGN.md`](STORAGE_DESIGN.md)

Query API contract:

- Machine-readable OpenAPI spec: [`docs/openapi.yaml`](openapi.yaml)
- Implemented Query API endpoints:

```text
GET /api/clusters
GET /api/nodes
GET /api/images
GET /api/sandboxes
GET /api/sandboxes/{id}
GET /api/sandboxes/{id}/metrics
GET /api/sandboxes/{id}/events
GET /api/sandboxes/{id}/trace
GET /api/sandboxes/{id}/profiles
GET /api/sandboxes/{id}/analysis
GET /api/runtimes/compare
GET /api/images/{id}
GET /api/nodes/{id}
```

Time-range capable endpoints accept optional ISO timestamp query parameters:

```text
?from=2026-05-09T03:00:00.000Z&to=2026-05-09T04:00:00.000Z
```

- Metrics filter individual points by timestamp.
- Events and profiles filter records by timestamp.
- Trace spans are included when their `[startTime, endTime]` interval overlaps the requested range.

Storage design:

- PostgreSQL for metadata and object relationships.
- ClickHouse for metrics, events, trace spans, and lifecycle stage samples.
- Object storage for profiles, flamegraphs, and diagnostic bundles.
- Query API composes storage data and keeps the frontend isolated from database-shaped records.

## Phase 3: Collector Integration

Goals:

- Add real data ingestion after the UI and query model are stable.
- Keep collectors independent from the frontend.
- Support pluggable runtime collectors.

Implemented:

- Phase 3 ingest boundary skeleton: `POST /api/ingest/batch` validates collector batch payloads and returns accepted counts without persisting data yet.
- OpenAPI contract now includes the ingest batch request, accepted response, and validation error shape.
- Mock node collector container that periodically posts node, sandbox, metric, event, and trace samples to the ingest boundary.
- In-memory ingest status endpoint: `GET /api/ingest/status` exposes accepted/rejected batch counters, totals, sources, and last batch summaries.
- Frontend Collectors page that visualizes ingest acceptance counters, per-source totals, and latest batch status.
- Collectors page auto-refresh and short activity trend preview for accepted batches and records.
- Mock collector interval aligned with the frontend refresh interval so status counters visibly update during local validation.
- Recent ingest sample endpoint: `GET /api/ingest/recent` keeps bounded in-memory previews of recent batches, metrics, events, trace spans, and profiles for debugging.
- Collectors page recent sample preview for latest ingested metrics, events, trace spans, and profiles.
- Collectors page tabbed recent sample browser for metrics, events, trace spans, and profiles.
- Node metric query endpoint: `GET /api/nodes/{id}/metrics` exposes node-level collector metrics such as host-agent health and node PSI/CPU/IO curves.
- Node event query endpoint: `GET /api/nodes/{id}/events` exposes host collector, sandbox lifecycle, and image snapshot events at node scope.
- Image metric query endpoint: `GET /api/images/{id}/metrics` exposes image-level collector metrics such as observed image size, layer count, and future lazy-cache/download timeline samples.
- Docker Compose runs two mock node collectors to validate multi-source ingest aggregation.
- Collectors page source filter for recent samples, so multi-node ingest can be inspected per source.
- Mock node collector emits profile artifacts so the ingest boundary exercises all frontend domain output types.
- Query API keeps accepted collector payloads in an in-memory live query store and merges them into cluster, node, sandbox, metric, event, trace, and profile GET responses.
- Cluster, node, and sandbox detail pages auto-refresh query data so live collector updates are visible outside the Collectors page.
- Runtime comparison aggregates now refresh from live Query API sandbox data instead of staying fixed to static mock rows.
- Collectors page shows live query store occupancy for in-memory entities, metric series, points, events, traces, and profiles.
- Live query store status breaks down in-memory occupancy by collector source for multi-node validation.
- Rust collector skeleton with a containerized outlet plus pluginized data sources: host-side `procfs`, host-side `cgroupfs`, external command output conversion, and HTTP API output conversion.
- Rust collector tree now centralizes collector ownership under `rust-collector/src/collectors/` with `core`, `outlet`, `adapters`, and semantic `sources` groups for node, runtime, image, sandbox, and profiling data.
- Docker Compose default path runs the Rust collector as the node-local outlet; node-wide `/proc`, host/root cgroupfs, and PSI data is collected by host-side tools and pushed to the outlet.
- Collector deployment guidance now separates container-friendly sources from host-only sources such as PSI, containerd lifecycle, image cache, and eBPF/profiling.
- Node-level unified outlet design: host tools and container tools report to the collector container over `POST /api/local/ingest`, and the collector outlet is the only component that posts to central ingest.
- Frontend default data path now uses the HTTP Query API directly instead of the embedded mock adapter.
- Query API defaults to live ingest data only; seeded mock telemetry is available only with `RUNTIMEPULSE_SEED_MOCK_DATA=true`.
- Docker Compose default path runs the Rust collector and excludes mock collectors unless the `mock` profile is enabled.
- Rust host-side procfs tool reports real Linux PSI samples from `/proc/pressure/*` as node pressure metrics.
- Rust host-side cgroupfs tool reports host/root cgroup v2 CPU, memory, IO, and process count samples as node-level metrics without creating sandbox or image metadata.
- Rust host-side Docker tool reports real Docker container and image inventory.
- Docker image inventory now enriches image rows with real `docker image inspect` size/rootfs layers and `docker history` layer command/size breakdown.
- Docker image inventory now emits image-level metric series for observed image size, layer count, and lazy-cache summary fields when those fields are available from image metadata.
- Query API exposes image-level metrics and events through `GET /api/images/{id}/metrics` and `GET /api/images/{id}/events`; Node and Sandbox detail views consume these endpoints for image detail panels.
- Docker event handling is moving toward one runtime-owned event stream with semantic dispatch for container lifecycle, sandbox sampler state, and image pull/tag/delete observations.
- Rust host-side Docker lifecycle tool reports Docker container create/start/stop/die/kill/oom/destroy events as sandbox lifecycle events through the local outlet.
- Query API live store applies Docker lifecycle mutations: stop/die update sandbox status and destroy removes the sandbox from the current live container set while preserving event records.
- Query API derives sandbox startup summaries from runtime startup trace spans and gives Docker/containerd event-derived startup traces priority over periodic inventory snapshots.
- Rust host-side Docker sandbox cgroupfs tool resolves cgroup paths from Docker running-container inventory and samples per-sandbox CPU, memory, IO, network, and process metrics without broad host cgroup scanning.
- Rust host-side Docker sandbox agent combines startup/running-container reconciliation with Docker lifecycle event streaming, maintains the active Docker container set, and samples only those active cgroups.
- Host collection is converging on a single `host-agent` binary for systemd: configurable host sources and event watchers enqueue reports into a bounded in-process queue, while a dedicated sender batches HTTP reports to the collector outlet.
- Rust host-side containerd inventory source is available as optional `containerd-inventory`/`host-containerd`, using containerd's gRPC API over the host Unix socket.
- containerd inventory enriches existing image rows from the content store when available, deriving layer-like content entries, byte totals, and layer counts without reading blob payloads.
- Rust host-side containerd event streaming is available as optional `containerd-events`/`host-containerd-events`, using one runtime-owned subscription and converting container/task lifecycle events into RuntimePulse sandbox updates, `container.startup` trace spans, plus first-pass content/snapshot image timeline observations.
- Rust host-side CRI/Kubelet event streaming is available as optional `kubelet-events`/`host-kubelet-events`, using a configurable JSONL command such as `crictl events --output json` and converting Kubernetes container lifecycle events into RuntimePulse sandbox updates.
- P1 runtime priority is Kubernetes on containerd first. Docker remains the single-node/local validation path, while CRI/Kubelet event ingestion enriches Kubernetes lifecycle context around containerd inventory and events.
- Kubernetes regular resource metrics are imported from the existing metrics platform through the optional `kubernetes-metrics` Prometheus adapter instead of re-sampling the same cgroups from RuntimePulse.
- Containerd inventory/events in the `k8s.io` namespace now derive the same `k8s-{namespace}-{pod}-{container}` sandbox id used by the Prometheus adapter, while preserving the raw containerd id in attributes.
- Containerd startup trace spans now use the same Kubernetes sandbox id as inventory, events, and Prometheus metrics, so K8s sandbox detail pages can correlate lifecycle trace timing with runtime metadata and resource curves.
- CRI/Kubelet enrichment events now use the same Kubernetes sandbox id when pod/container labels are present, so lifecycle enrichment does not split from containerd inventory or Prometheus metric series.
- The host-agent default source set is `procfs,psi,cgroupfs,docker-inventory,docker-events,docker-sandbox-cgroupfs`; Docker sandbox cgroupfs sampling is driven by startup inventory plus lifecycle-maintained active container ids, not broad cgroup scanning.
- Host-agent self-observability reports node-level metrics for queue depth, enqueued/dropped reports, collector errors, sender success/failure counters, and runtime event stream health.
- Host-agent sender persists failed batches as local JSON spool files and replays them before later in-memory batches after the outlet recovers.
- Host-agent can run optional `command` and `http` adapter sources, so third-party host tools that emit RuntimePulse partial output join the same queue, batching, spool, and outlet path.
- Host-agent can run `image-cache`/`host-image-cache` to ingest real snapshotter/exporter JSON or JSONL reports for precise image stage spans and lazy block-cache hit curves.
- Node detail page consumes node-level metric series directly, so host-agent health and node pressure are visible without relying on sandbox-level series.

Collector candidates:

- Host metrics collector. `procfs` now reports node CPU, memory, IO, process, and load metrics; `psi` reports node pressure metrics as an independent host source.
- Cgroup metrics collector.
- Host-agent queue-backed scheduler/sender that runs host plugins in one process and keeps collector threads off the synchronous HTTP path. First implementation, self-observability, and local spool replay are in place.
- Third-party collector adapters. `command` and `http` adapters work in the outlet path and can also be enabled as host-agent sources for host-visible tools.
- Kubernetes/containerd event collector. Containerd inventory and container/task/content/snapshot event streaming are the P1 path for Kubernetes, especially the `k8s.io` namespace. CRI/Kubelet events are used as lifecycle enrichment.
- Kubernetes metrics adapter. Prometheus vector queries for kubelet/cAdvisor container CPU, memory, network, and filesystem metrics are mapped into RuntimePulse sandbox metric series.
- Docker event collector. Docker container lifecycle, Docker image event dispatch, and Docker startup trace spans are implemented and kept as the single-node/local validation path.
- Lifecycle-triggered sandbox cgroup collector that starts sampling only after a sandbox/container `started` event and stops sampling after the matching `stopped` event. Docker startup reconciliation, active-set management, periodic cgroup sampling, lifecycle event streaming, and stopped-container cleanup are integrated into `host-agent`. Containerd/Kubernetes task-driven sampling is now available as `containerd-sandbox-cgroupfs`, using task event PIDs to resolve exact cgroup paths without broad scanning.
- image metadata and cache collector. Docker image metadata/layer breakdown and containerd content-store image enrichment are implemented; Docker image pull/tag/delete and containerd content/snapshot timeline observations come from runtime event dispatch; precise pull sub-stage durations and lazy block-cache hit curves are ingested from real snapshotter/exporter JSON reports through `image-cache`.
- Kata collector.
- Firecracker collector.
- eBPF/profile collector.
- gVisor collector. Defer until the common containerd/Kubernetes, Kata, Firecracker, and profiling paths are usable.

Collector output should map to the same domain concepts already used by the frontend:

- `Sandbox`
- `MetricSeries`
- `EventRecord`
- `TraceSpan`
- `ProfileArtifact`

## Phase 4: Expert Analysis Enhancements

Goals:

- Keep expert-led diagnosis as the main workflow.
- Add richer comparison and correlation tools before automation.

Potential features:

- Correlate event timestamp with metrics window. Implemented via event-to-metrics jump marker.
- Compare startup waterfall across multiple runs. Implemented in selected run comparison panel.
- Compare runtime overhead across images and nodes. Implemented in Runtime Comparison scope switcher.
- Compare cold start vs warm start. Implemented in Runtime Comparison with mock cache buckets.
- Add node-level IO pressure and PSI overlays. Implemented in run overview with IO/PSI overlay chart.
- Add image layer breakdown and cache hit visualizations. Implemented in run overview with mock layer timings.

## Phase 5: Rules and AI Analysis

Goals:

- Add automated analysis only after data model and query API stabilize.
- Use AI as an assistant for expert workflows, not as a replacement for raw data visibility.

Implemented:

- Sandbox rule analysis endpoint: `GET /api/sandboxes/{id}/analysis`.
- Rule findings combine sandbox metadata, metrics, events, trace spans, image details, and profile indexes.
- Sandbox detail overview now shows rule findings with evidence, recommended actions, and trace-span drill-down links.

Potential architecture:

```text
AI Analysis Service
  |
  +-- Context Builder
        |
        +-- Query API
              |
              +-- Metrics
              +-- Events
              +-- Trace spans
              +-- Profiles
              +-- Metadata
```

Potential capabilities:

- Generate run summaries.
- Identify likely startup bottlenecks.
- Explain runtime overhead differences.
- Compare regressions across runtime versions.
- Suggest next metrics or traces to inspect.

## Container Development Workflow

Build the frontend image:

```bash
docker compose build
```

Run the frontend:

```bash
docker compose up -d
```

Check status:

```bash
docker compose ps
```

Open the UI:

```text
http://localhost:8080
```

Stop the stack:

```bash
docker compose down
```

## Local Node Policy

The default workflow should not rely on host `npm install`, `npm run dev`, or host Node build validation.

If local Node is used temporarily for troubleshooting, document it in the PR or commit message. Container build remains the source of truth.
