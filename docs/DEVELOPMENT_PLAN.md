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
- Profile artifact preview panel.
- Saved views for common expert workflows. Implemented for explorer filters and selection state.

## Phase 2: Query API and Storage Design

Goals:

- Replace mock adapter with an HTTP adapter.
- Add a backend query API without implementing collectors yet.
- Define storage schema and query boundaries.

Planned backend endpoints:

```text
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

Candidate storage split:

- PostgreSQL for metadata and object relationships.
- ClickHouse for metrics, events, and trace spans.
- Object storage for profiles, flamegraphs, and diagnostic bundles.

## Phase 3: Collector Integration

Goals:

- Add real data ingestion after the UI and query model are stable.
- Keep collectors independent from the frontend.
- Support pluggable runtime collectors.

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
- Compare startup waterfall across multiple runs.
- Compare runtime overhead across images and nodes. Implemented in Runtime Comparison scope switcher.
- Compare cold start vs warm start. Implemented in Runtime Comparison with mock cache buckets.
- Add node-level IO pressure and PSI overlays.
- Add image layer breakdown and cache hit visualizations.

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
