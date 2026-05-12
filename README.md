# RuntimePulse

RuntimePulse is an early-stage sandbox runtime metrics platform prototype. Phase 1 is frontend-first: it uses mock data to validate expert workflows before backend storage and collectors are implemented.

## Phase 1 Scope

- Sandbox Explorer with runtime/status/search filters.
- Sandbox Detail with metrics, lifecycle events, startup trace, profiles, and raw data.
- Runtime Comparison for runc, gVisor, Kata, and Firecracker.
- Collector status page for ingest acceptance counters and source health.
- Mock API adapter that can later be replaced by a real HTTP API without rewriting pages.
- Lightweight Query API container that serves the same mock telemetry over HTTP for frontend/backend contract validation.
- Mock node collector container that periodically validates collector payloads through the ingest API.
- Container-first deployment for local validation and later platform packaging.

## Documentation

- [Architecture](docs/ARCHITECTURE.md)
- [Development Plan](docs/DEVELOPMENT_PLAN.md)
- [Git Workflow](docs/GIT_WORKFLOW.md)
- [OpenAPI Contract](docs/openapi.yaml)
- [Storage Design](docs/STORAGE_DESIGN.md)

## Container Deployment

Run the frontend prototype with Docker Compose:

```bash
docker compose up --build
```

Open:

```text
http://localhost:8080
```

The frontend proxies `/api/*` to the local Query API container. The Query API is also exposed directly for endpoint checks:

```text
http://localhost:8081/health
http://localhost:8081/api/sandboxes
http://localhost:8081/api/ingest/batch
http://localhost:8081/api/ingest/status
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
frontend/src/api        API interface and mock adapter
frontend/src/domain     Shared TypeScript domain models
frontend/src/mock       Scenario-based mock telemetry data
frontend/src/components Reusable visualization/layout components
frontend/src/pages      Expert analysis pages
frontend/src/utils      Time, unit, and color helpers
query-api               Minimal HTTP Query API backed by mock telemetry
collector               Mock node collector that posts validation-only ingest batches
```

## Mock Scenarios

The current mock dataset includes slow image unpack, high node IO pressure, gVisor sentry CPU overhead, Kata MicroVM slow boot, and Firecracker guest agent timeout cases.
