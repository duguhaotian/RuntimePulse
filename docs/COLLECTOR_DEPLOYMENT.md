# RuntimePulse Collector Deployment

Collector packaging should stay container-friendly, but the data source decides whether the collector must run with host access.

## Short Answer

- Container deployment is enough for many workload, process, and API-based metrics.
- Host-side access is required for a slice of node, runtime, kernel, and image-cache data.
- The practical model is mixed: ship collectors as containers, run some as privileged node agents.

## What Works Well In Containers

These sources are usually safe to package as normal containers:

- Application and process metrics that come from the workload container itself.
- cgroup-scoped resource metrics.
- Tool adapters that read JSON from a binary.
- Tool adapters that call an HTTP API.
- Profile upload, conversion, and batching logic.
- Synthetic validation collectors used for local smoke tests.

Examples:

- `procfs` inside the container namespace for container-local views.
- `command` plugins for existing tools such as exporters, profilers, or snapshotters.
- `http` plugins for local agents or side services that already expose metrics.

## What Usually Needs Host Access

These sources often need host visibility or elevated privileges:

- Node PSI and system-wide pressure signals.
- Real disk and network saturation across the node.
- containerd or kubelet lifecycle events.
- Image unpack, remote read, and cache-hit behavior across all containers.
- eBPF, perf, and kernel profile collection.
- Host namespace process trees and system-level cgroup traversal.
- Runtime-specific data that lives outside the workload container namespace.

## Recommended Deployment Modes

### 1. Container Agent

Use a normal container when the plugin reads in-container data or talks to APIs.

Good for:

- app metrics
- command adapters
- HTTP adapters
- profile transport

### 2. Host Agent

Use a privileged container or host binary when the collector needs node-wide visibility.

Typical settings:

- `hostPID: true`
- `hostNetwork: true`
- `privileged: true` or targeted Linux capabilities
- host mounts for `/proc`, `/sys`, `/var/lib/containerd`, runtime sockets, or image stores

### 3. DaemonSet or Per-Node Service

Use one collector per node when the data is node-scoped.

Best fit for:

- PSI
- cgroup tree inspection
- containerd events
- image cache and unpack metrics
- eBPF/profile collectors

## Mapping By Data Type

| Data type | Container agent | Host agent |
| --- | --- | --- |
| App CPU / memory / IO | yes | optional |
| Workload lifecycle spans | yes | optional |
| HTTP tool integrations | yes | optional |
| Command-line tool integrations | yes | optional |
| Node PSI | limited | yes |
| Disk / network saturation | limited | yes |
| containerd / kubelet events | no | yes |
| Image layer unpack / cache | no | yes |
| eBPF / perf / kernel profiles | no | yes |

## RuntimePulse Recommendation

Use one collector framework with pluginized backends:

- `procfs` for basic local and host-visible metrics.
- `command` for existing binaries.
- `http` for API-based tools.
- later host plugins for runtime, image, and kernel sources.

That keeps the codebase unified while letting deployment vary by source.

## Node Unified Outlet

Each node should expose one local collector outlet. Host-side tools and container-side tools should report to this local outlet instead of posting directly to the central ingest API.

Recommended name:

```text
Node Collector Gateway
```

The gateway is the only component on a node that talks to the central RuntimePulse ingest API.

```text
Host tools
  |
  +-- procfs / cgroup / PSI
  +-- containerd / kubelet events
  +-- image cache / unpack probes
  +-- eBPF / perf profilers
        |
        v
Node Collector Gateway  --->  RuntimePulse Ingest API
        ^
        |
  +-- workload sidecars
  +-- container-local agents
  +-- command plugins
  +-- HTTP plugin adapters
```

### Why A Node Gateway

Without a node gateway, every collector needs to solve the same operational problems:

- central ingest authentication
- retry and backoff
- local buffering when the network is unavailable
- duplicate event detection
- timestamp normalization
- node identity enrichment
- batch sizing and rate limits
- source health reporting

Putting these responsibilities in one local outlet makes host and container collectors simpler.

### Gateway Responsibilities

The gateway should own:

- Node identity: cluster id, node id, hostname, kernel, labels.
- Source registration: plugin name, tool kind, version, privilege level.
- Payload normalization: convert tool-specific output into the RuntimePulse ingest schema.
- Local enrichment: attach node id, runtime type, sandbox id, image id, source, and timestamps when missing.
- Validation: reject malformed local payloads before they reach central ingest.
- Batching: merge small local records into bounded ingest batches.
- De-duplication: collapse repeated events, trace spans, and profile indexes by stable ids.
- Backpressure: rate limit noisy tools and drop or downsample low-priority metrics first.
- Buffering: spool accepted local records when central ingest is unavailable.
- Health: expose local source status for debugging.

### Local Input Interfaces

The gateway should support more than one input shape:

| Input | Purpose | Example |
| --- | --- | --- |
| Unix domain socket | host-local trusted tools | `/run/runtimepulse/node-gateway.sock` |
| Local HTTP | container tools and sidecars | `http://runtimepulse-node-gateway:19091/ingest` |
| Command plugin | existing binaries | `containerd-exporter --format runtimepulse-json` |
| Pull plugin | API-based tools | image cache agent HTTP endpoint |
| File spool | crash-safe handoff | `/var/lib/runtimepulse/spool/*.jsonl` |

For Kubernetes, expose the gateway to local pods through a ClusterIP service, hostNetwork port, or mounted Unix socket depending on the trust boundary.

### Local Payload Contract

Local tools can submit either full RuntimePulse ingest batches or partial plugin outputs.

Full batch:

```json
{
  "source": "containerd-agent/node-a",
  "observedAt": "2026-05-13T00:00:00.000Z",
  "metadata": {},
  "metrics": [],
  "events": [],
  "traces": [],
  "profiles": []
}
```

Partial output:

```json
{
  "metrics": [
    {
      "timestamp": "2026-05-13T00:00:00.000Z",
      "name": "node.psi.io.some",
      "value": 0.34,
      "unit": "ratio",
      "group": "pressure"
    }
  ]
}
```

The gateway fills missing `source`, `nodeId`, `observedAt`, and related labels when it can do so safely.

### Security Boundary

The gateway should distinguish trusted host sources from less-trusted container sources.

Recommended rules:

- Host tools can use a Unix socket with filesystem permissions.
- Container tools should use a local HTTP endpoint with a per-source token.
- Only the gateway stores central ingest credentials.
- Privileged plugins stay in the host agent process or a privileged companion container.
- Container plugins should not receive host mounts unless their data source requires it.

### Buffering And Delivery

The gateway should acknowledge local submissions after local validation and durable enqueue, not after central ingest succeeds.

Recommended flow:

1. Local source submits data.
2. Gateway validates and enriches.
3. Gateway writes to an in-memory queue plus optional disk spool.
4. Gateway batches by size, time, and priority.
5. Gateway posts to central ingest with retry and backoff.
6. Gateway updates local source health.

This prevents short central outages from breaking host-side collection.

### Recommended First Implementation

Use the existing Rust collector as the gateway process:

- Keep `procfs`, `command`, and `http` as in-process plugins.
- Add a local HTTP `POST /local/ingest` endpoint for container tools.
- Add a Unix socket listener for host tools.
- Add a bounded local queue.
- Add optional disk spool under `/var/lib/runtimepulse`.
- Keep central delivery through `POST /api/ingest/batch`.

This lets both deployment styles share one node-local outlet while preserving the plugin model.
