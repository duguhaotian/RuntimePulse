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
- a Docker inventory probe using the host Docker CLI or Docker socket
- a containerd event watcher using `/run/containerd/containerd.sock`
- an image-cache probe reading snapshotter state
- an eBPF profiler that needs host privileges

For the current implementation these host-side sources are normally run by one
`host-agent` process:

```bash
runtimepulse-collector host-agent
```

The default source set is configured with:

```text
RUNTIMEPULSE_HOST_AGENT_SOURCES=procfs,cgroupfs,docker-inventory,docker-events,docker-sandbox-cgroupfs
```

Enable snapshotter/cache report ingestion when a node has a real exporter:

```text
RUNTIMEPULSE_HOST_AGENT_SOURCES=procfs,cgroupfs,docker-inventory,docker-events,docker-sandbox-cgroupfs,image-cache
RUNTIMEPULSE_IMAGE_CACHE_REPORT_PATH=/var/lib/runtimepulse/image-cache-report.json
```

Optional third-party host tools can be pulled by the same host-agent process:

```text
RUNTIMEPULSE_HOST_AGENT_SOURCES=procfs,cgroupfs,command,http
RUNTIMEPULSE_COMMAND_PLUGIN_NAME=my-host-tool
RUNTIMEPULSE_COMMAND_PLUGIN_CMD='my-host-tool --format runtimepulse-json'
RUNTIMEPULSE_HTTP_PLUGIN_NAME=my-side-service
RUNTIMEPULSE_HTTP_PLUGIN_URL=http://127.0.0.1:19090/runtimepulse
RUNTIMEPULSE_ADAPTER_TIMEOUT_MS=3000
```

The command stdout or HTTP response body must be a RuntimePulse partial output.
These adapter sources use the host-agent queue, batching sender, and spool just
like native sources. Multiple adapters can be configured with indexed variables
such as `RUNTIMEPULSE_COMMAND_PLUGIN_0_CMD` and
`RUNTIMEPULSE_HTTP_PLUGIN_0_URL`. Use `RUNTIMEPULSE_ADAPTER_TIMEOUT_MS` or
plugin-specific timeout variables to keep slow third-party tools from blocking
the host-agent collection loop.

Each host-agent source keeps its own local report source label through the
outlet. The outlet submits separate central ingest batches for labels such as
`host-procfs`, `host-docker`, `host-image-cache`, and `host-profile-report`, so
Collector Status can identify which collector produced the rows.

The host-agent is expected to run as root in the systemd deployment because it
needs host-wide `/proc`, `/proc/pressure/*`, cgroupfs, Docker, containerd, and
future eBPF/profile visibility. `containerd-inventory` can be added to the source list when
containerd metadata should be collected. `containerd-events` can be added when
container/task lifecycle updates should come from containerd's event service.
Both sources connect directly to containerd's gRPC API over
`RUNTIMEPULSE_CONTAINERD_SOCKET`; they do not shell out to `ctr`.
For Kubernetes sandbox lifecycle, also enable `cri-events`: containerd events
describe low-level task/content/snapshot changes, while the CRI stream carries
pod-sandbox lifecycle state (`SANDBOX_CREATED`, `SANDBOX_READY`, stop/delete).
The default command is `crictl events --output json`; RuntimePulse accepts both
JSONL and crictl's pretty-printed multi-line JSON event stream.

```text
RUNTIMEPULSE_CONTAINERD_SOCKET=/run/containerd/containerd.sock
RUNTIMEPULSE_CONTAINERD_NAMESPACES=k8s.io
```

If `RUNTIMEPULSE_CONTAINERD_NAMESPACES` is empty, namespaces are discovered from
the containerd namespace service for inventory. Event streaming subscribes to
all namespaces unless this variable is set, in which case it applies
containerd namespace filters. When Docker collectors are enabled on the same
node, avoid collecting Docker's internal containerd `moby` namespace through
the containerd sources; those rows represent the same Docker containers and
will duplicate the Docker sandbox rows. For a Docker + CRI/containerd host,
point `RUNTIMEPULSE_CONTAINERD_SOCKET` at the CRI containerd socket and set
`RUNTIMEPULSE_CONTAINERD_NAMESPACES=k8s.io`.

For the current RuntimePulse production target, Docker is only a local
validation path and the real runtime scope is CRI + containerd. Keep Docker
sources disabled unless validating Docker-specific plumbing. See
[`CRI_CONTAINERD_COLLECTION.md`](CRI_CONTAINERD_COLLECTION.md) for the runtime
collector trigger model and deployment policy.

The Docker sandbox cgroupfs source initializes from Docker's running-container
inventory and then uses Docker lifecycle events to maintain the active set.
Only the active set is sampled.

The host-agent also reports its own node-level health metrics:

```text
host_agent.up
host_agent.queue.depth
host_agent.reports.enqueued_total
host_agent.reports.dropped_total
host_agent.collect.errors_total
host_agent.sender.batches_sent_total
host_agent.sender.batches_failed_total
host_agent.sender.reports_sent_total
host_agent.spool.files
host_agent.spool.batches_spooled_total
host_agent.spool.batches_replayed_total
host_agent.source.collect.duration_ms
host_agent.source.collect.success
host_agent.source.collect.errors_total
```

When the outlet is unavailable, the sender writes failed batches as JSON files
under `RUNTIMEPULSE_HOST_AGENT_SPOOL_DIR` and replays them before later
in-memory batches. For systemd deployments the starter service uses
`StateDirectory=runtimepulse`, and the example env points the spool to:

```text
/var/lib/runtimepulse/host-agent-spool
```

### 3. Optional Host Mounts

If a plugin can run safely inside the collector container but needs host files, mount only the required paths:

- `/proc`
- `/proc/pressure`
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

- host-side `procfs` for node CPU, memory, IO, process, and PSI metrics.
- host-side `cgroupfs` for host/root cgroup v2 CPU, memory, IO, and process samples.
- host-side `docker` for container and image inventory metadata.
- host-side `image-cache` for real snapshotter/exporter image stage timing and lazy block-cache curves.
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
- Run node-wide `procfs`, host/root cgroupfs, PSI, and Docker inventory collection as host-side tools.
- Add a local HTTP listener on `0.0.0.0:9091`.
- Accept partial plugin output at `POST /api/local/ingest`.
- Let host tools send JSON payloads to the collector container IP and port.
- Merge local HTTP payloads with plugin output.
- Keep central delivery through `POST /api/ingest/batch`.

This keeps the system small and still supports host-only collectors.

### systemd Host Agent

The repository includes starter units and a shared environment file:

```text
deploy/systemd/runtimepulse-host-agent.service
deploy/systemd/runtimepulse-startup-callchain.service
deploy/systemd/runtimepulse-host-agent.env
```

Suggested local install path:

```bash
sudo install -m 0755 rust-collector/target/release/runtimepulse-collector /usr/local/bin/runtimepulse-collector
sudo install -d -m 0755 /etc/runtimepulse
sudo install -m 0644 deploy/systemd/runtimepulse-host-agent.env /etc/runtimepulse/host-agent.env
sudo install -m 0644 deploy/systemd/runtimepulse-host-agent.service /etc/systemd/system/runtimepulse-host-agent.service
sudo systemctl daemon-reload
sudo systemctl enable --now runtimepulse-host-agent
```

The starter unit sets `User=root` and `Group=root`.

### systemd Startup Call-Chain Collector

For high-fidelity CRI+containerd startup attribution, run the continuous
eBPF startup call-chain probe as a separate producer service instead of adding
`startup-callchain` to `RUNTIMEPULSE_HOST_AGENT_SOURCES`.  The normal
`runtimepulse-host-agent` should keep lightweight inventory, cgroup, image, and
event sources; `runtimepulse-startup-probe` keeps probes attached and writes
completed reports into a durable spool, while `runtimepulse-startup-callchain`
consumes that spool and posts only non-empty startup reports to the same local
outlet.

Install the probe and the dedicated unit:

```bash
sudo install -m 0755 tools/runtimepulse-startup-probe/runtimepulse-startup-probe /usr/local/bin/runtimepulse-startup-probe
sudo install -m 0644 deploy/systemd/runtimepulse-startup-callchain.service /etc/systemd/system/runtimepulse-startup-callchain.service
sudo install -m 0644 deploy/systemd/runtimepulse-startup-probe.service /etc/systemd/system/runtimepulse-startup-probe.service
sudo systemctl daemon-reload
sudo systemctl enable --now runtimepulse-startup-probe runtimepulse-startup-callchain
```

The service uses the same `/etc/runtimepulse/host-agent.env` file.  A typical
CRI/containerd configuration is:

```text
RUNTIMEPULSE_HOST_AGENT_SOURCES=procfs,psi,cgroupfs,containerd-inventory,containerd-events,cri-events,cri-startup-trace,containerd-sandbox-cgroupfs,image-cache,profile-report,perf,ebpf
RUNTIMEPULSE_CONTAINERD_SOCKET=/run/containerd/containerd.sock
RUNTIMEPULSE_CONTAINERD_NAMESPACES=k8s.io
RUNTIMEPULSE_CRI_EVENTS_CMD=crictl events --output json
RUNTIMEPULSE_STARTUP_CALLCHAIN_SPOOL_DIR=/var/lib/runtimepulse/startup-callchain
RUNTIMEPULSE_STARTUP_CALLCHAIN_INTERVAL_MS=1000
RUNTIMEPULSE_STARTUP_PROBE_CONTAINERD_TREE=true
RUNTIMEPULSE_STARTUP_PROBE_CONTAINERD_NAMESPACES=k8s.io
RUNTIMEPULSE_STARTUP_PROBE_INCLUDE_HELPERS=true
RUNTIMEPULSE_STARTUP_PROBE_ENABLE_GO_UPROBES=true
RUNTIMEPULSE_STARTUP_PROBE_CONTAINERD_BINARY=/usr/bin/containerd
RUNTIMEPULSE_CONTAINERD_CONFIG=/etc/containerd/config.toml
```

Important deployment notes:

- `runtimepulse-startup-probe` runs as root because `bpftrace`/eBPF needs host
  kernel privileges and access to host `/proc`.
- The probe is a long-running producer and writes atomic JSON reports into the
  spool directory. The collector service only consumes the spool, so short-lived
  startup events are not missed by periodic snapshot windows.
- `RUNTIMEPULSE_STARTUP_PROBE_CONTAINERD_TREE=true` tracks the current containerd process tree, groups CNI
  plugin helper children such as `iptables` under the CNI root plugin, groups
  runtime shim/runtime children separately, ignores unrelated containerd child
  processes, and refreshes the target when containerd restarts.
- Pass `RUNTIMEPULSE_CONTAINERD_CONFIG` whenever the target containerd uses non-default
  `root`/`state` directories so OCI bundle metadata can be found.

After changing `/etc/runtimepulse/host-agent.env`, reload both services:

```bash
sudo systemctl restart runtimepulse-host-agent runtimepulse-startup-probe runtimepulse-startup-callchain
journalctl -u runtimepulse-startup-probe -u runtimepulse-startup-callchain -f
```
