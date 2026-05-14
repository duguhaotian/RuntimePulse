# RuntimePulse Collector Deployment

Collector packaging should stay container-first. For personal use, the collector container is also the single node-local outlet for both container-side and host-side tools.

## Short Answer

- Use one collector container as the node-local reporting outlet.
- Container-side tools run as in-container plugins only for container-scoped data, or report to the collector container over HTTP.
- Host-side tools report to the collector container by container IP and port.
- Some data still requires host visibility, but the reporting path stays unified.

## What Works Well In Containers

These sources are usually safe to package as normal containers:

- Application and process metrics that come from the workload container itself.
- cgroup-scoped resource metrics.
- Tool adapters that read JSON from a binary.
- Tool adapters that call an HTTP API.
- Profile upload, conversion, and batching logic.
- Synthetic validation collectors used for local smoke tests.

Examples:

- `procfs` inside the container namespace for container-local views only.
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

## Recommended Personal Deployment

### 1. Collector Outlet Container

Run one RuntimePulse collector container per node. This container owns the central ingest URL and is the only process that posts to RuntimePulse Query API.

The container is responsible for:

- running outlet-safe plugins such as `command` and `http`
- avoiding node-wide procfs/cgroupfs/PSI collection inside the container
- managing plugin lifecycle
- exposing a local HTTP report endpoint
- receiving host-side and container-side reports
- normalizing partial payloads into RuntimePulse ingest batches
- adding node identity and timestamps when missing
- posting batches to central ingest

Default local report endpoint:

```text
POST http://<collector-container-ip>:9091/api/local/ingest
```

Containers in the same Docker Compose network can use the service name:

```text
POST http://runtimepulse-rust-collector:9091/api/local/ingest
```

### 2. Host Tools

Host-side tools stay outside the collector container when they need host namespaces, host filesystem paths, runtime sockets, or kernel privileges.

They do not post to central ingest directly. Instead, they submit local JSON payloads to the collector container:

```bash
curl -X POST \
  http://<collector-container-ip>:9091/api/local/ingest \
  -H 'content-type: application/json' \
  -d @payload.json
```

Examples:

- a host binary reading real `/proc`, `/proc/pressure/*`, and cgroupfs
- a containerd event watcher using `/run/containerd/containerd.sock`
- an image-cache probe reading snapshotter state
- an eBPF profiler that needs host privileges

### 3. Optional Host Mounts

If a plugin can run safely inside the collector container but needs host files, mount only the required paths:

- `/proc`
- `/sys`
- `/run/containerd/containerd.sock`
- image store or snapshotter paths

This is useful for personal/local deployment, but the default design should still allow a host binary to report through the collector HTTP endpoint instead.

## Mapping By Data Type

| Data type | Collector container | Host tool over HTTP |
| --- | --- | --- |
| App CPU / memory / IO | yes | optional |
| Workload lifecycle spans | yes | optional |
| HTTP tool integrations | yes | optional |
| Command-line tool integrations | yes | optional |
| Node PSI | no, unless explicitly host-mounted | yes |
| Disk / network saturation | no, unless explicitly host-mounted | yes |
| containerd / kubelet events | possible with runtime mount | yes |
| Image layer unpack / cache | possible with host mount | yes |
| eBPF / perf / kernel profiles | usually no | yes |

## RuntimePulse Recommendation

Use one containerized collector outlet with pluginized backends:

- host-side `procfs` for node CPU, memory, IO, process, cgroupfs, and PSI metrics.
- `command` for existing binaries.
- `http` for API-based tools that the collector pulls.
- `local-http` input for host-side and sidecar tools that push reports.

That keeps personal deployment simple: one container owns reporting, while host tools can still collect host-only data.

## Node Unified Outlet

Each node should expose one local collector outlet. In the personal-use design, that outlet is the Rust collector container.

Recommended name:

```text
Node Collector Outlet
```

The outlet is the only component on a node that talks to the central RuntimePulse ingest API.

```text
Host tools
  |
  +-- procfs / cgroupfs / PSI
  +-- containerd / kubelet events
  +-- image cache / unpack probes
  +-- eBPF / perf profilers
        |
        | HTTP POST to container IP:9091
        v
RuntimePulse Collector Container  --->  RuntimePulse Ingest API
        ^
        |
  +-- in-container plugins
  +-- command plugins
  +-- HTTP pull adapters
  +-- container-local tools
```

### Why A Single Outlet

Without a node-local outlet, every collector needs to know how to talk to central ingest.

- central ingest URL
- retry behavior
- timestamp normalization
- node identity enrichment
- batch format
- source naming

Putting this in one collector container keeps host tools and container tools small.

### Outlet Responsibilities

The outlet should own:

- Node identity: cluster id, node id, hostname, kernel, labels.
- Source naming: plugin name, tool kind, and version when available.
- Payload normalization: convert tool-specific output into the RuntimePulse ingest schema.
- Local enrichment: attach node id, runtime type, sandbox id, image id, source, and timestamps when missing.
- Validation: reject malformed local payloads before central ingest.
- Batching: merge small local records into bounded ingest batches.
- Basic retry: retry central ingest failures.
- Health logs: show which local source is producing data.

### Local Input Interfaces

The outlet should support these input shapes:

| Input | Purpose | Example |
| --- | --- | --- |
| Local HTTP push | host tools, sidecars, and local scripts | `POST /api/local/ingest` |
| Command plugin | existing binaries | `containerd-exporter --format runtimepulse-json` |
| Pull plugin | API-based tools | image cache agent HTTP endpoint |

For the first implementation, the local HTTP push endpoint is enough for host-side reports. It avoids filesystem mounts just for reporting and works for both host tools and sidecar containers.

### Local Payload Contract

Local tools submit partial plugin outputs. The collector outlet wraps them into the central RuntimePulse ingest batch.

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

The outlet fills missing `source`, `nodeId`, `observedAt`, and related labels when it can do so safely.

### Delivery Flow

Recommended first flow:

1. Local source submits data to `POST /api/local/ingest`.
2. Collector outlet validates and queues the local payload.
3. Collector outlet merges plugin and local HTTP data into the next batch.
4. Collector outlet posts to central ingest.
5. Collector outlet logs success or failure.

Advanced disk buffering can wait until it is actually needed.

### Recommended First Implementation

Use the existing Rust collector as the outlet container:

- Keep `command` and `http` as in-process plugins when they collect container-safe data.
- Run node-wide `procfs`, cgroupfs, and PSI collection as a host-side tool.
- Add a local HTTP listener on `0.0.0.0:9091`.
- Accept partial plugin output at `POST /api/local/ingest`.
- Let host tools send JSON payloads to the collector container IP and port.
- Merge local HTTP payloads with plugin output.
- Keep central delivery through `POST /api/ingest/batch`.

This keeps the system small and still supports host-only collectors.
