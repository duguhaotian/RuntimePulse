# RuntimePulse Development Plan

## Current Principle

Development is frontend-first and container-first.

- Do not depend on real collectors during Phase 1.
- Do not bind pages directly to backend storage.
- Do not run host-level Node development as the default workflow.
- Build and verify the app through Docker Compose.
- Keep mock data realistic enough that real ingestion can later replace it.

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
- `MockRuntimePulseApi` adapter.
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
- Environment-based API switching with `VITE_RUNTIMEPULSE_API_BASE_URL`; empty value keeps the mock adapter.
- Minimal `runtimepulse-query-api` service that serves mock telemetry through the planned HTTP endpoints.
- Docker Compose wiring for frontend-to-query-api validation through `/api`.
- Storage model and Query API boundary design: [`docs/STORAGE_DESIGN.md`](STORAGE_DESIGN.md)

Query API contract:

- Machine-readable OpenAPI spec: [`docs/openapi.yaml`](openapi.yaml)
- Implemented mock Query API endpoints:

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

Collector candidates:

- Host metrics collector.
- Cgroup metrics collector.
- containerd lifecycle collector.
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
