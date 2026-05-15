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
- Rust host-side Docker lifecycle tool reports Docker container create/start/stop/die/kill/oom/destroy events as sandbox lifecycle events through the local outlet.
- Query API live store applies Docker lifecycle mutations: stop/die update sandbox status and destroy removes the sandbox from the current live container set while preserving event records.
- Rust host-side Docker sandbox cgroupfs tool resolves cgroup paths from Docker running-container inventory and samples per-sandbox CPU, memory, IO, and process metrics without broad host cgroup scanning.

Collector candidates:

- Host metrics collector.
- Cgroup metrics collector.
- Docker/containerd lifecycle collector. Docker lifecycle is implemented; containerd remains pending.
- Lifecycle-triggered sandbox cgroup collector that starts sampling only after a sandbox/container `started` event and stops sampling after the matching `stopped` event. Docker inventory-based sandbox cgroup sampling is implemented; event-managed sampler lifetime remains pending.
- image metadata and cache collector.
- gVisor collector.
- Kata collector.
- Firecracker collector.
- eBPF/profile collector.

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
