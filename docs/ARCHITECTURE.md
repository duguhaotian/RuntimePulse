# RuntimePulse Architecture

RuntimePulse is a sandbox runtime observability and metrics analysis platform for technical experts. The platform targets sandbox images, containers, MicroVMs, gVisor, Kata, Firecracker, and related runtime layers.

The first phase is frontend-first and mock-data-first. It intentionally does not implement collectors, storage, rule engines, or AI analysis yet. The goal is to validate expert workflows, visualizations, and data shape before binding the product to real ingestion pipelines.

## Product Goals

- Collect and visualize metrics for sandbox runtimes and related infrastructure.
- Help experts inspect performance, lifecycle events, startup latency, resource overhead, and bottlenecks manually.
- Keep frontend, data/query layer, and collectors separated.
- Support future extension to rule engines, AI-assisted analysis, and real collectors without rewriting the UI.

## Phase 1 Scope

Phase 1 focuses on the frontend experience:

- Sandbox run exploration.
- Sandbox detail pages.
- Metrics visualization.
- Lifecycle event timeline.
- Startup trace waterfall.
- Runtime comparison reports.
- Scenario-based mock telemetry.
- Container-first build and deployment.

Phase 1 intentionally excludes:

- Real collectors.
- Backend storage.
- Rule engine.
- Alerting engine.
- AI root cause analysis.
- Production authentication and authorization.

## Architecture Overview

```text
Frontend Prototype
  |
  | RuntimePulseApi interface
  |
  +-- MockRuntimePulseApi       Phase 1
  |
  +-- HttpRuntimePulseApi       Future
        |
        +-- Query API
              |
              +-- Metadata DB
              +-- Metrics/Event/Trace Store
              +-- Object Store
              +-- Future AI Analysis Service
```

The frontend depends on a stable TypeScript API contract rather than direct storage or collector details.

The planned Query API and storage boundaries are documented in [`STORAGE_DESIGN.md`](STORAGE_DESIGN.md). That document defines the PostgreSQL metadata model, ClickHouse analytical model, object storage layout, and per-endpoint query ownership.

## Frontend Structure

```text
frontend/src/api        API interface and adapters
frontend/src/domain     Shared TypeScript domain models
frontend/src/mock       Scenario-based telemetry data
frontend/src/components Reusable visualization and layout components
frontend/src/pages      Expert analysis pages
frontend/src/utils      Time, unit, and color helpers
```

## Current UI Model

The UI follows a Weights & Biases-inspired workspace model:

- `Runs`: sandbox instances treated like experiment runs.
- `Reports`: aggregated runtime comparison views.
- `Artifacts`: reserved for profiles, pprof files, flamegraphs, and diagnostics.
- `Launch queue`: reserved for future controlled profiling or capture jobs.

A sandbox is represented as a run with:

- Runtime type and version.
- Node and workload identity.
- Image metadata.
- Startup duration.
- CPU and memory summary.
- Lifecycle events.
- Startup trace spans.
- Profile artifacts.
- Raw mock payload for inspection.

## Domain Data Types

RuntimePulse separates observability data into these categories:

- `Metric`: numeric time series such as CPU, memory, IO, network, PSI, runtime process metrics.
- `Event`: lifecycle and error records such as sandbox start, image pull, guest agent timeout, OOM.
- `TraceSpan`: startup and lifecycle phases such as image pull, unpack, runtime create, MicroVM boot.
- `ProfileArtifact`: references to profiling outputs such as CPU, off-CPU, memory, block IO, or syscall profiles.
- `Metadata`: cluster, node, workload, image, runtime, sandbox, and process relationships.

## Mock Scenarios

The current frontend mock dataset includes:

- Normal `runc` sandbox.
- gVisor sentry CPU overhead.
- Kata MicroVM slow boot.
- Firecracker guest agent timeout.
- Large image unpack latency.
- Node IO pressure affecting sandbox startup.
- Stopped sandbox history.
- Runtime comparison aggregate data.

## Container-First Deployment

The project uses container-first validation.

```text
Docker Compose
  |
  +-- runtimepulse-frontend
        |
        +-- Node build stage
        +-- Nginx runtime stage
```

The frontend is built inside Docker and served by Nginx on port `8080`.

## Future Architecture

Future phases can add backend and collectors behind the existing frontend API contract:

```text
Collectors
  |
  +-- host collector
  +-- cgroup collector
  +-- containerd collector
  +-- gVisor collector
  +-- Kata collector
  +-- Firecracker collector
  +-- eBPF/profile collector
        |
        v
Ingest API
        |
        +-- Metadata DB
        +-- Metrics/Event/Trace Store
        +-- Object Store
        |
        v
Query API
        |
        v
Frontend
```

The future AI analysis service should consume structured context from the Query API rather than reading raw storage directly.
