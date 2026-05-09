# RuntimePulse

RuntimePulse is an early-stage sandbox runtime metrics platform prototype. Phase 1 is frontend-first: it uses mock data to validate expert workflows before backend storage and collectors are implemented.

## Phase 1 Scope

- Sandbox Explorer with runtime/status/search filters.
- Sandbox Detail with metrics, lifecycle events, startup trace, profiles, and raw data.
- Runtime Comparison for runc, gVisor, Kata, and Firecracker.
- Mock API adapter that can later be replaced by a real HTTP API without rewriting pages.
- Container-first deployment for local validation and later platform packaging.

## Documentation

- [Architecture](docs/ARCHITECTURE.md)
- [Development Plan](docs/DEVELOPMENT_PLAN.md)
- [Git Workflow](docs/GIT_WORKFLOW.md)

## Container Deployment

Run the frontend prototype with Docker Compose:

```bash
docker compose up --build
```

Open:

```text
http://localhost:8080
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
```

## Mock Scenarios

The current mock dataset includes slow image unpack, high node IO pressure, gVisor sentry CPU overhead, Kata MicroVM slow boot, and Firecracker guest agent timeout cases.
