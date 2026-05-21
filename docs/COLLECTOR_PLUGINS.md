# RuntimePulse Collector Plugins

The Rust collector is the preferred path for real data collection. It keeps collection independent from the frontend and sends the same `POST /api/ingest/batch` payload as other collectors.

For personal node deployments, the Rust collector container acts as the node-local outlet. Host tools and container tools submit locally to the collector container over HTTP, and only the outlet posts to the central RuntimePulse ingest API.

## Plugin Model

Each plugin returns partial ingest data:

- `metadata`: clusters, nodes, images, sandboxes
- `metrics`: timestamped numeric samples
- `events`: lifecycle or collector events
- `traces`: startup or runtime spans
- `profiles`: profile artifact indexes

The collector merges plugin outputs into one batch, adds the collector source, and posts it to the Query API ingest endpoint.

For node deployments, plugin output should flow through the local outlet path even when the plugin runs in a separate container or process. This keeps node identity, batching, retry, and central ingest configuration in one place.

## Outlet Container Plugins

- `command`: runs an external binary or shell command inside the outlet container and parses JSON from stdout.
- `http`: calls an HTTP API from inside the outlet container and parses JSON from the response body.

Do not use container-side `procfs` or `cgroupfs` plugins for node-wide metrics. A normal container sees container namespaces, so those readings describe the collector container or container-visible subset, not the host node.

## Host-Side Tools

- `host-procfs`: runs on the host, reads `/proc`, `/proc/pressure/*`, and cgroup-like process signals from the host view, then pushes partial ingest JSON to the outlet over HTTP.
- `host-cgroupfs`: runs on the host, reads only host/root cgroup v2 CPU, memory, IO, and process counts from `/sys/fs/cgroup`, and reports node-level metrics.
- `host-docker`: runs on the host, reads Docker container/image inventory through the Docker CLI, then pushes sandbox and image metadata to the outlet over HTTP.
- `host-docker-events`: runs on the host, follows Docker lifecycle events, and pushes sandbox lifecycle event records to the outlet.
- `host-docker-cgroupfs`: runs on the host, resolves cgroup paths from Docker running-container PIDs, and reports per-sandbox CPU, memory, IO, network, and process metrics without scanning the whole cgroup tree.
- `host-image-cache`: runs on the host, reads a RuntimePulse JSON/JSONL report emitted by a real snapshotter or image-cache exporter, and reports lazy block-cache metrics plus precise image stage spans.
- `command`: runs an external binary or shell command and parses JSON from stdout.
- `http`: calls an HTTP API and parses JSON from the response body.

```bash
cd rust-collector
RUNTIMEPULSE_COLLECTOR_NODE_ID="$(hostname)" \
RUNTIMEPULSE_LOCAL_REPORT_URL=http://localhost:9091/api/local/ingest \
cargo run -- host-procfs
```

```bash
cd rust-collector
RUNTIMEPULSE_COLLECTOR_NODE_ID="$(hostname)" \
RUNTIMEPULSE_CGROUP_MAX_ENTRIES=200 \
RUNTIMEPULSE_LOCAL_REPORT_URL=http://localhost:9091/api/local/ingest \
cargo run -- host-cgroupfs
```

```bash
cd rust-collector
RUNTIMEPULSE_COLLECTOR_NODE_ID="$(hostname)" \
RUNTIMEPULSE_LOCAL_REPORT_URL=http://localhost:9091/api/local/ingest \
cargo run -- host-docker
```

```bash
cd rust-collector
RUNTIMEPULSE_COLLECTOR_NODE_ID="$(hostname)" \
RUNTIMEPULSE_LOCAL_REPORT_URL=http://localhost:9091/api/local/ingest \
cargo run -- host-docker-cgroupfs
```

Useful host tool settings:

| Variable | Default | Purpose |
| --- | --- | --- |
| `RUNTIMEPULSE_COLLECTOR_NODE_ID` | `rust-node-a` | Node id attached to host samples. |
| `RUNTIMEPULSE_LOCAL_REPORT_URL` | `http://localhost:9091/api/local/ingest` | Collector outlet endpoint. |
| `RUNTIMEPULSE_COLLECTOR_INTERVAL_MS` | `5000` | Sampling interval for loop mode. |
| `RUNTIMEPULSE_COLLECTOR_ONCE` | `false` | Set `true` to collect once and exit. |
| `RUNTIMEPULSE_CGROUP_ROOT` | `/sys/fs/cgroup` | cgroup v2 root to scan. |
| `RUNTIMEPULSE_CGROUP_MAX_ENTRIES` | `200` | Maximum cgroups sampled per tick. |

## Command Plugin

Use this for existing tools that already expose useful data.

```bash
RUNTIMEPULSE_COLLECTOR_PLUGINS=command
RUNTIMEPULSE_COMMAND_PLUGIN_NAME=containerd-exporter
RUNTIMEPULSE_COMMAND_PLUGIN_CMD='containerd-exporter --format runtimepulse-json'
RUNTIMEPULSE_COMMAND_PLUGIN_TIMEOUT_MS=3000
```

The command must write JSON shaped like `rust-collector/examples/command-plugin-output.json`.
Multiple command plugins can be configured with indexed variables:

```bash
RUNTIMEPULSE_COLLECTOR_PLUGINS=command
RUNTIMEPULSE_COMMAND_PLUGIN_0_NAME=snapshotter-exporter
RUNTIMEPULSE_COMMAND_PLUGIN_0_CMD='snapshotter-exporter --format runtimepulse-json'
RUNTIMEPULSE_COMMAND_PLUGIN_0_TIMEOUT_MS=5000
RUNTIMEPULSE_COMMAND_PLUGIN_1_NAME=perf-summary
RUNTIMEPULSE_COMMAND_PLUGIN_1_CMD='perf-summary --runtimepulse-json'
```

## HTTP Plugin

Use this for tools that expose a local or remote API.

```bash
RUNTIMEPULSE_COLLECTOR_PLUGINS=http
RUNTIMEPULSE_HTTP_PLUGIN_NAME=image-cache-agent
RUNTIMEPULSE_HTTP_PLUGIN_URL=http://image-cache-agent:9090/runtimepulse
RUNTIMEPULSE_HTTP_PLUGIN_TIMEOUT_MS=3000
```

The endpoint must return the same partial ingest JSON shape as the command plugin.
Multiple HTTP plugins can be configured the same way:

```bash
RUNTIMEPULSE_COLLECTOR_PLUGINS=http
RUNTIMEPULSE_HTTP_PLUGIN_0_NAME=image-cache-agent
RUNTIMEPULSE_HTTP_PLUGIN_0_URL=http://127.0.0.1:19090/runtimepulse
RUNTIMEPULSE_HTTP_PLUGIN_0_TIMEOUT_MS=1000
RUNTIMEPULSE_HTTP_PLUGIN_1_NAME=sandbox-profiler
RUNTIMEPULSE_HTTP_PLUGIN_1_URL=http://127.0.0.1:19091/runtimepulse
```

`RUNTIMEPULSE_ADAPTER_TIMEOUT_MS` sets the default adapter timeout when a
plugin-specific timeout is not provided.

## Image Cache and Snapshotter Reports

Use `image-cache` when a real snapshotter/cache tool can export precise image
stage timing or lazy-loading block cache counters. RuntimePulse does not infer
these values from Docker metadata and does not run `docker pull` just to create
measurements.

```bash
RUNTIMEPULSE_HOST_AGENT_SOURCES=procfs,psi,cgroupfs,docker-inventory,docker-events,docker-sandbox-cgroupfs,image-cache
RUNTIMEPULSE_IMAGE_CACHE_REPORT_PATH=/var/lib/runtimepulse/image-cache-report.json
runtimepulse-collector host-agent
```

The file can be a JSON object, a JSON array, or JSONL. Each row describes one
image observation:

```json
{
  "imageId": "docker-image-registry-local-ml-heavy-v8",
  "imageRef": "registry.local/ml-heavy:v8",
  "imageDigest": "sha256:mlheavy8",
  "loadingMode": "lazy",
  "timestamp": "2026-05-21T00:00:00.000Z",
  "snapshotter": "nydus",
  "cache": {
    "requestedBlocks": 12000,
    "hitBlocks": 9800,
    "localReadBytes": 1284505600,
    "remoteReadBytes": 288358400,
    "blockSizeBytes": 131072
  },
  "downloadTimeline": [
    {
      "name": "Prefetch bootstrap",
      "phase": "pull",
      "durationMs": 1270,
      "bytes": 104857600,
      "timestamp": "2026-05-21T00:00:00.084Z"
    }
  ]
}
```

See `rust-collector/examples/image-cache-report.json` for a complete example.

## Container Cgroup Collection

Container cgroupfs metrics should be collected by a lifecycle-aware runtime collector, not by `host-cgroupfs`.

Expected flow:

1. A Docker/containerd/Kubernetes/runtime-specific collector observes a sandbox/container `started` event or reconciles already-running sandboxes from runtime inventory.
2. The collector resolves the exact runtime cgroup path for that sandbox shape, such as Docker PID `/proc/<pid>/cgroup` resolution.
3. A per-sandbox cgroup sampler reports `sandbox.*` metrics against the sandbox id created by the lifecycle event.
4. The sampler stops or expires when the corresponding `stopped` event is observed.

## Local HTTP Reports

Use this for host-side tools or sidecar containers that push data into the collector outlet.

```bash
curl -X POST \
  http://<collector-container-ip>:9091/api/local/ingest \
  -H 'content-type: application/json' \
  -d @rust-collector/examples/command-plugin-output.json
```

Containers in the same Compose network can use:

```text
http://runtimepulse-rust-collector:9091/api/local/ingest
```

The request body uses the same partial ingest JSON shape as the command plugin.

## Development Validation

The current Dockerfile packages the locally built collector binary for fast validation:

```bash
cd rust-collector
cargo build
cd ..
docker compose build runtimepulse-rust-collector
docker compose up -d runtimepulse-rust-collector
```

The production image pipeline should switch this to a fully containerized release build once the collector crate layout and native dependencies settle.
