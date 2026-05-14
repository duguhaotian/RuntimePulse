# RuntimePulse

RuntimePulse is an early-stage sandbox runtime metrics platform prototype. The current default path is collector-first: the UI reads Query API data backed by accepted collector batches.

## Current Scope

- Sandbox Explorer with runtime/status/search filters.
- Sandbox Detail with metrics, lifecycle events, startup trace, profiles, and raw data.
- Runtime Comparison for runc, gVisor, Kata, and Firecracker.
- Collector status page for ingest acceptance counters, source health, short activity trends, and per-refresh deltas.
- HTTP API adapter used by the frontend by default.
- Lightweight Query API container that serves live in-memory telemetry from collector ingest.
- Rust collector container as the node-local outlet, plus host-side collector tools for procfs, cgroupfs, Docker inventory, PSI, and other node-scoped data.
- Optional mock node collector containers for UI/demo validation.
- Container-first deployment for local validation and later platform packaging.

## Documentation

- [Architecture](docs/ARCHITECTURE.md)
- [Development Plan](docs/DEVELOPMENT_PLAN.md)
- [Git Workflow](docs/GIT_WORKFLOW.md)
- [OpenAPI Contract](docs/openapi.yaml)
- [Storage Design](docs/STORAGE_DESIGN.md)
- [Collector Plugins](docs/COLLECTOR_PLUGINS.md)
- [Collector Deployment](docs/COLLECTOR_DEPLOYMENT.md)

## Container Deployment

Run the frontend prototype with Docker Compose:

```bash
docker compose up --build
```

Open:

```text
http://localhost:8080
```

The frontend proxies `/api/*` to the local Query API container. By default, Query API responses come from accepted collector batches only. The Query API is also exposed directly for endpoint checks:

```text
http://localhost:8081/health
http://localhost:8081/api/sandboxes
http://localhost:8081/api/ingest/batch
http://localhost:8081/api/ingest/status
http://localhost:8081/api/ingest/recent
```

Stop it with:

```bash
docker compose down
```

## Local Frontend Development

The preferred workflow is container-first. Use local Node only for troubleshooting.

```bash
cd frontend
npm install
npm run dev
```

Build check:

```bash
cd frontend
npm run build
```

## Frontend Structure

```text
frontend/src/api        API interface and HTTP adapter
frontend/src/domain     Shared TypeScript domain models
frontend/src/mock       Scenario-based mock telemetry data
frontend/src/components Reusable visualization/layout components
frontend/src/pages      Expert analysis pages
frontend/src/utils      Time, unit, and color helpers
query-api               Minimal HTTP Query API backed by live ingest telemetry
collector               Optional mock node collector image used for validation
rust-collector          Rust collector framework with outlet, host-procfs, host-cgroupfs, host-docker, command, HTTP, and local report inputs
```

## Rust Collector Plugins

The Rust collector sends the same ingest batch contract as every other collector.

```bash
cd rust-collector
cargo build
cd ..
docker compose up -d runtimepulse-rust-collector
```

Default container role:

- `outlet`: exposes `POST /api/local/ingest` and forwards accepted reports to Query API.

Optional plugins:

- `command`: set `RUNTIMEPULSE_COLLECTOR_PLUGINS=command` and `RUNTIMEPULSE_COMMAND_PLUGIN_CMD='your-tool --json'`.
- `http`: set `RUNTIMEPULSE_COLLECTOR_PLUGINS=http` and `RUNTIMEPULSE_HTTP_PLUGIN_URL=http://tool:port/metrics/runtimepulse`.

Command/API plugins should return JSON shaped like `rust-collector/examples/command-plugin-output.json`; the collector merges it into one ingest batch.

Node-scoped procfs, cgroupfs, and PSI data should be collected from the host, not from inside the outlet container:

```bash
cd rust-collector
RUNTIMEPULSE_COLLECTOR_NODE_ID="$(hostname)" \
RUNTIMEPULSE_LOCAL_REPORT_URL=http://localhost:9091/api/local/ingest \
cargo run -- host-procfs
```

For cgroup v2 resource samples:

```bash
cd rust-collector
RUNTIMEPULSE_COLLECTOR_NODE_ID="$(hostname)" \
RUNTIMEPULSE_CGROUP_MAX_ENTRIES=200 \
RUNTIMEPULSE_LOCAL_REPORT_URL=http://localhost:9091/api/local/ingest \
cargo run -- host-cgroupfs
```

For Docker container and image inventory:

```bash
cd rust-collector
RUNTIMEPULSE_COLLECTOR_NODE_ID="$(hostname)" \
RUNTIMEPULSE_LOCAL_REPORT_URL=http://localhost:9091/api/local/ingest \
cargo run -- host-docker
```

For one-shot validation, add `RUNTIMEPULSE_COLLECTOR_ONCE=true`.

By default this host-side tool posts to:

```text
http://localhost:9091/api/local/ingest
```

Host-side tools and sidecar containers can also push partial collector output to the Rust collector outlet:

```bash
curl -X POST \
  http://localhost:9091/api/local/ingest \
  -H 'content-type: application/json' \
  -d @rust-collector/examples/command-plugin-output.json
```

The collector queues local reports and merges them into the next `POST /api/ingest/batch` delivery.

## Optional Mock Scenarios

Mock data is not part of the default data path. To run the old synthetic collectors for UI validation:

```bash
docker compose --profile mock up --build
```

The optional mock dataset includes slow image unpack, high node IO pressure, gVisor sentry CPU overhead, Kata MicroVM slow boot, and Firecracker guest agent timeout cases.
