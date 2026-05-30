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

The collector merges plugin outputs into one batch, adds the collector source, and posts it to the Query API ingest endpoint. Local reports sent by host-agent or host-visible tools may include an optional top-level `source`; the outlet preserves that label as the central ingest source so Collector Status can identify the exact local collector.

For node deployments, plugin output should flow through the local outlet path even when the plugin runs in a separate container or process. This keeps node identity, batching, retry, and central ingest configuration in one place.

## Outlet Container Plugins

- `command`: runs an external binary or shell command inside the outlet container and parses JSON from stdout.
- `http`: calls an HTTP API from inside the outlet container and parses JSON from the response body.

Do not use container-side `procfs` or `cgroupfs` plugins for node-wide metrics. A normal container sees container namespaces, so those readings describe the collector container or container-visible subset, not the host node.

## Host-Side Tools

- `host-procfs`: runs on the host, reads `/proc`, `/proc/pressure/*`, and cgroup-like process signals from the host view, then pushes partial ingest JSON to the outlet over HTTP.
- `host-cgroupfs`: runs on the host, reads only host/root cgroup v2 CPU, memory, IO, and process counts from `/sys/fs/cgroup`, and reports node-level metrics.
- `host-containerd`: runs on the host, reads containerd inventory through the containerd client, and is the preferred Kubernetes path when pointed at the `k8s.io` namespace.
- `host-containerd-events`: runs on the host, follows containerd events, and is the preferred Kubernetes lifecycle stream.
- `host-containerd-cgroupfs`: runs on the host, samples only containerd containers made active by task events, and resolves cgroup paths from the task PID. It is the Kubernetes/containerd fallback when the standard metrics platform does not expose a needed sandbox metric.
- `host-kubelet-events`: runs on the host, follows CRI/Kubelet lifecycle events from a configurable JSONL command such as `crictl events --output json`, and enriches Kubernetes sandbox lifecycle records around the containerd path.
- `host-startup-callchain`: runs on the host, reads or invokes an external RunPodSandbox/CNI/OCI/Kata uprobe exporter through `RUNTIMEPULSE_STARTUP_CALLCHAIN_REPORT_PATH` or `RUNTIMEPULSE_STARTUP_CALLCHAIN_REPORT_CMD`, and pushes normalized startup spans/metrics to the outlet.
- `host-docker`: runs on the host, reads Docker container/image inventory through the Docker CLI, then pushes sandbox and image metadata to the outlet over HTTP. This is the single-node/local validation path, not the Kubernetes path.
- `host-docker-events`: runs on the host, follows Docker lifecycle events, and pushes sandbox lifecycle event records to the outlet for single-node Docker scenarios.
- `host-docker-cgroupfs`: runs on the host, resolves cgroup paths from Docker running-container PIDs, and reports per-sandbox CPU, memory, IO, network, and process metrics without scanning the whole cgroup tree.
- `host-image-cache`: runs on the host, reads a RuntimePulse JSON/JSONL report emitted by a real snapshotter or image-cache exporter, and reports lazy block-cache metrics plus precise image stage spans.
- `host-kata`: runs on the host, reads a real Kata exporter JSON/JSONL report from `RUNTIMEPULSE_KATA_REPORT_PATH`, and reports Kata sandbox metadata, VM sizing metrics, observed events, and VM boot spans.
- `host-firecracker`: runs on the host, reads a real Firecracker/jailer exporter JSON/JSONL report from `RUNTIMEPULSE_FIRECRACKER_REPORT_PATH`, and reports microVM metadata, sizing/ready metrics, observed events, exit codes, and VM boot spans.
- `host-gvisor`: runs on the host, reads a real runsc/gVisor exporter JSON/JSONL report from `RUNTIMEPULSE_GVISOR_REPORT_PATH`, and reports gVisor sandbox metadata, syscall/gofer/fault metrics, observed events, and sandbox boot spans.
- `host-sandbox-reconcile`: runs on the host, reads a real runtime snapshot report from `RUNTIMEPULSE_SANDBOX_RECONCILE_REPORT_PATH`, and emits snapshot metadata used by the Query API to remove stale live sandboxes for a runtime scope.
- `host-diagnostic-report`: runs on the host, reads runtime diagnostic bundle indexes and emits diagnostic events, issue metrics, and capture spans while keeping raw bundles in external storage.
- `host-perf`: runs on the host, reads a perf profile artifact report or executes a configured perf exporter command that writes RuntimePulse-compatible JSON to stdout.
- `host-perf-script`: runs on the host, converts plain `perf script` text from a file or command into RuntimePulse profile artifacts with inline flamegraph trees.
- `host-ebpf`: runs on the host, reads an eBPF profile artifact report or executes a configured eBPF exporter command that writes RuntimePulse-compatible JSON to stdout.
- `command`: runs an external binary or shell command and parses JSON from stdout.
- `http`: calls an HTTP API and parses JSON from the response body.

```bash
cd rust-collector
RUNTIMEPULSE_COLLECTOR_NODE_ID="$(hostname)" \
RUNTIMEPULSE_LOCAL_REPORT_URL=http://localhost:9091/api/local/ingest \
cargo run -- host-procfs
```

For Kubernetes, prefer containerd inventory/events and add CRI/Kubelet events as
extra lifecycle context. Regular container resource metrics should come from the
existing Kubernetes metrics platform, typically Prometheus fed by kubelet/cAdvisor:

```bash
RUNTIMEPULSE_HOST_AGENT_SOURCES=procfs,psi,cgroupfs,containerd-inventory,containerd-events,containerd-sandbox-cgroupfs,kubelet-events,kubernetes-metrics,image-cache \
RUNTIMEPULSE_CONTAINERD_NAMESPACES=k8s.io \
RUNTIMEPULSE_PROMETHEUS_URL=http://prometheus.monitoring.svc:9090 \
RUNTIMEPULSE_CRI_EVENTS_CMD='crictl events --output json' \
runtimepulse-collector host-agent
```

The `kubernetes-metrics` source imports Prometheus vector query results and maps
common kubelet/cAdvisor metrics into RuntimePulse sandbox series such as
`sandbox.cpu.usage_ratio`, `sandbox.memory.working_set_bytes`,
`sandbox.network.rx_bytes`, `sandbox.network.tx_bytes`, `sandbox.io.read_bytes`,
and `sandbox.io.write_bytes`. Query strings can be overridden with
`RUNTIMEPULSE_PROMETHEUS_QUERY_*` variables when a cluster uses different metric
labels.

Local adapter smoke test:

```bash
node rust-collector/examples/mock-prometheus.mjs &
RUNTIMEPULSE_COLLECTOR_ONCE=true \
RUNTIMEPULSE_HOST_AGENT_SOURCES=kubernetes-metrics \
RUNTIMEPULSE_PROMETHEUS_URL=http://127.0.0.1:19090 \
cargo run --manifest-path rust-collector/Cargo.toml -- host-agent
```

CRI/Kubelet event collection can also be enabled by itself through the unified
host-agent:

```bash
RUNTIMEPULSE_HOST_AGENT_SOURCES=kubelet-events \
RUNTIMEPULSE_CRI_EVENTS_CMD='crictl events --output json' \
runtimepulse-collector host-agent
```

For local validation without a Kubernetes node, stream the example JSONL file:

```bash
RUNTIMEPULSE_COLLECTOR_ONCE=true \
RUNTIMEPULSE_CRI_EVENTS_CMD='cat rust-collector/examples/cri-events.jsonl' \
cargo run --manifest-path rust-collector/Cargo.toml -- host-kubelet-events
```

For high-fidelity CRI+containerd startup attribution, pair the lightweight CRI
startup trace with the startup call-chain source. The call-chain source accepts
either already-normalized spans or raw uprobe/eBPF enter/exit events from a
real exporter. Raw events are paired by request/correlation id and converted
into RunPodSandbox, CNI plugin binary, OCI runtime, Kata, and helper spans
before metrics are derived.

For production-like host deployments, prefer the dedicated
`runtimepulse-startup-callchain` systemd unit rather than adding
`startup-callchain` to the multi-source host-agent.  Long BPF capture windows
can otherwise delay normal inventory/event sources in the host-agent collection
loop:

```bash
sudo install -m 0755 tools/runtimepulse-startup-probe/runtimepulse-startup-probe /usr/local/bin/runtimepulse-startup-probe
sudo install -m 0644 deploy/systemd/runtimepulse-startup-callchain.service /etc/systemd/system/runtimepulse-startup-callchain.service
sudo systemctl daemon-reload
sudo systemctl enable --now runtimepulse-startup-callchain
```

Use the shared `/etc/runtimepulse/host-agent.env` file to configure both the
normal host-agent and the dedicated startup call-chain service:

```text
RUNTIMEPULSE_HOST_AGENT_SOURCES=procfs,psi,cgroupfs,containerd-inventory,containerd-events,cri-startup-trace,containerd-sandbox-cgroupfs,image-cache,profile-report,perf,ebpf
RUNTIMEPULSE_CONTAINERD_NAMESPACES=k8s.io
RUNTIMEPULSE_CRI_EVENTS_CMD=crictl events --output json
RUNTIMEPULSE_STARTUP_CALLCHAIN_INTERVAL_MS=9000
RUNTIMEPULSE_STARTUP_CALLCHAIN_REPORT_TIMEOUT_MS=45000
RUNTIMEPULSE_STARTUP_CALLCHAIN_REPORT_CMD=/usr/local/bin/runtimepulse-startup-probe export --once --duration-ms 8000 --containerd-tree --containerd-namespace k8s.io --include-helpers --containerd-binary /usr/bin/containerd --containerd-config /etc/containerd/config.toml
```

The repository includes a minimal exporter at
`tools/runtimepulse-startup-probe/runtimepulse-startup-probe`. It uses real
`bpftrace` exec/exit tracepoints to emit raw startup-callchain events for CNI
plugin binaries, OCI/Kata runtime binaries, containerd shims, and optional helper
binaries. With `--containerd-tree`, it traces current containerd descendants,
routes only CNI plugin subtrees and runtime shim/runtime subtrees into startup
reports, and ignores unrelated child processes.  CNI helper binaries such as
`iptables` inherit the root CNI plugin through process parentage, so concurrent
sandbox starts do not split helper cost away from the plugin that invoked it.
The exporter also re-discovers containerd by executable and `/proc` start time
when a new containerd process appears, allowing collection to continue after a
containerd restart.

When a capture window observes multiple sandbox ids, the exporter emits one
lightweight report per sandbox under `reports` so concurrent starts keep
separate CNI/helper/runtime metrics. Each generated report carries its own
`startTime`/`endTime`/`durationMs` event window for the call-chain root span.
The normalizer derives aggregate startup metrics, per-CNI-plugin metrics,
process-binary breakdowns, and CNI subtree metrics such as
`sandbox.startup.cni.group.<plugin>_{count,duration_ms,helper_duration_ms,process_sum_duration_ms}`.
`validate-cri-containerd-startup.sh` can set `CONCURRENT_RUNPODS` to start
multiple CRI sandboxes in one capture and assert that each emitted report
contains only its own sandbox/container ids; this has been validated for both
runc and Kata on the standalone CRI+containerd test setup.

Local one-shot validation can read the bundled raw-event example without a real
uprobe exporter:

```bash
RUNTIMEPULSE_COLLECTOR_ONCE=true \
RUNTIMEPULSE_STARTUP_CALLCHAIN_REPORT_PATH=rust-collector/examples/startup-uprobe-events.json \
cargo run --manifest-path rust-collector/Cargo.toml -- host-startup-callchain
```

The same normalizer is also available as an outlet-container plugin when the
exporter runs inside the outlet namespace:

```bash
RUNTIMEPULSE_COLLECTOR_ONCE=true \
RUNTIMEPULSE_COLLECTOR_PLUGINS=startup-callchain \
RUNTIMEPULSE_STARTUP_CALLCHAIN_REPORT_PATH=rust-collector/examples/startup-uprobe-events.json \
cargo run --manifest-path rust-collector/Cargo.toml
```

Local containerd validation can use an isolated containerd instance instead of
the system socket:

```bash
mkdir -p /tmp/runtimepulse-containerd-test/{root,state,logs}
containerd config default > /tmp/runtimepulse-containerd-test/config.toml
# Edit root/state/grpc.address to point at /tmp/runtimepulse-containerd-test.
sudo containerd --config /tmp/runtimepulse-containerd-test/config.toml \
  > /tmp/runtimepulse-containerd-test/logs/containerd.log 2>&1 &
docker save ubuntu:24.04 -o /tmp/runtimepulse-containerd-test/ubuntu.tar
sudo ctr --address /tmp/runtimepulse-containerd-test/containerd.sock \
  --namespace k8s.io images import /tmp/runtimepulse-containerd-test/ubuntu.tar
sudo ctr --address /tmp/runtimepulse-containerd-test/containerd.sock \
  --namespace k8s.io run --detach \
  --label io.kubernetes.pod.namespace=default \
  --label io.kubernetes.pod.name=runtimepulse-ctrd-demo \
  --label io.kubernetes.container.name=app \
  --label io.kubernetes.pod.uid=runtimepulse-ctrd-demo-uid \
  docker.io/library/ubuntu:24.04 runtimepulse-ctrd-demo sleep 600
RUNTIMEPULSE_COLLECTOR_ONCE=true \
RUNTIMEPULSE_HOST_AGENT_SOURCES=containerd-inventory,containerd-sandbox-cgroupfs \
RUNTIMEPULSE_CONTAINERD_SOCKET=/tmp/runtimepulse-containerd-test/containerd.sock \
RUNTIMEPULSE_CONTAINERD_NAMESPACES=k8s.io \
runtimepulse-collector host-agent
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
| `RUNTIMEPULSE_COLLECTOR_NODE_ID` | `runtimepulse-host` | Node id attached to host samples. |
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
plugin-specific timeout is not provided. It is also the default timeout for
image-cache exporter commands unless `RUNTIMEPULSE_IMAGE_CACHE_REPORT_TIMEOUT_MS`
is set.

## Kata, Firecracker, and gVisor Sandbox Reports

Use `kata`/`host-kata`, `firecracker`/`host-firecracker`, and `gvisor`/`host-gvisor` when a real runtime-side exporter can write observed VM or sandbox state to JSON or JSONL. RuntimePulse does not invent runtime state; if the report path is unset or missing, these collectors emit no data.

```bash
RUNTIMEPULSE_HOST_AGENT_SOURCES=procfs,psi,cgroupfs,kata,firecracker,gvisor \
RUNTIMEPULSE_KATA_REPORT_PATH=/var/lib/runtimepulse/kata-report.jsonl \
RUNTIMEPULSE_FIRECRACKER_REPORT_PATH=/var/lib/runtimepulse/firecracker-report.jsonl \
RUNTIMEPULSE_GVISOR_REPORT_PATH=/var/lib/runtimepulse/gvisor-report.jsonl \
runtimepulse-collector host-agent
```

A lightweight report can be an object with a `sandboxes` array, a JSON array of sandbox rows, or JSONL rows. Example Kata row:

```json
{
  "timestamp": "2026-05-25T00:00:00.000Z",
  "sandboxes": [
    {
      "id": "kata-demo",
      "workloadName": "demo/app",
      "namespace": "default",
      "imageRef": "registry.example/demo:v1",
      "status": "running",
      "runtimeVersion": "kata-3.3.0",
      "vmId": "vm-1",
      "hypervisor": "qemu",
      "vcpus": 2,
      "memoryBytes": 536870912,
      "bootTimeMs": 1234
    }
  ]
}
```

Firecracker rows accept the same common fields plus `machineId`, `jailerPid`, `apiSocket`, `guestReadyMs`, `exitCode`, and `reason`. gVisor rows accept common fields plus `platform`, `sandboxPid`, `goferPid`, `sentryPid`, `bootTimeMs`, `syscallCount`, `syscallLatencyMs`, `goferIoBytes`, and `faults`. The collectors also accept full RuntimePulse partial ingest JSON, so a richer exporter can bypass lightweight normalization while still using the same host-agent queue/spool/outlet path.

Example reports are available at:

- `rust-collector/examples/kata-report.json`
- `rust-collector/examples/firecracker-report.json`
- `rust-collector/examples/gvisor-report.json`

## Sandbox Reconcile Reports

Use `sandbox-reconcile`/`host-sandbox-reconcile` when a report-only runtime exporter can list the currently live sandbox ids. This closes the lifecycle gap for runtimes that do not have an event stream wired yet: the Query API removes stale live sandboxes in the same `snapshot.scope` for the same node.

```bash
RUNTIMEPULSE_HOST_AGENT_SOURCES=kata,firecracker,gvisor,sandbox-reconcile \
RUNTIMEPULSE_KATA_REPORT_PATH=/var/lib/runtimepulse/kata-report.jsonl \
RUNTIMEPULSE_FIRECRACKER_REPORT_PATH=/var/lib/runtimepulse/firecracker-report.jsonl \
RUNTIMEPULSE_GVISOR_REPORT_PATH=/var/lib/runtimepulse/gvisor-report.jsonl \
RUNTIMEPULSE_SANDBOX_RECONCILE_REPORT_PATH=/var/lib/runtimepulse/sandbox-snapshot.jsonl \
runtimepulse-collector host-agent
```

Lightweight snapshot example:

```json
{
  "timestamp": "2026-05-25T00:00:00.000Z",
  "runtimeType": "kata",
  "scope": "kata-running",
  "sandboxIds": ["kata-live-1", "kata-live-2"]
}
```

If `scope` is omitted it defaults to `<runtimeType>-running`. The same report may use `sandboxes: [{ "id": "..." }]` instead of `sandboxIds`. See `rust-collector/examples/sandbox-reconcile-report.json` for a complete example.

## Image Cache and Snapshotter Reports

Use `image-cache` when a real snapshotter/cache tool can export precise image
stage timing or lazy-loading block cache counters. RuntimePulse does not infer
these values from Docker metadata and does not run `docker pull` just to create
measurements.

Runtime collection is currently optimized for CRI + containerd. Docker sources
are optional validation/demo sources and can be disabled entirely; see
[`CRI_CONTAINERD_COLLECTION.md`](CRI_CONTAINERD_COLLECTION.md) for the
recommended source set and why the Docker `moby` namespace should not be
collected through containerd when Docker validation is enabled.

```bash
RUNTIMEPULSE_HOST_AGENT_SOURCES=procfs,psi,cgroupfs,containerd-inventory,containerd-events,containerd-sandbox-cgroupfs,image-cache
RUNTIMEPULSE_CONTAINERD_NAMESPACES=k8s.io
RUNTIMEPULSE_IMAGE_CACHE_REPORT_PATH=/var/lib/runtimepulse/image-cache-report.json
runtimepulse-collector host-agent
```

For exporters that should be invoked on each host-agent collection tick, use a
command hook instead of a file path. The command must write the same lightweight
JSON/JSONL report to stdout:

```bash
RUNTIMEPULSE_HOST_AGENT_SOURCES=image-cache \
RUNTIMEPULSE_IMAGE_CACHE_REPORT_CMD='nydus-cache-exporter --runtimepulse-json' \
RUNTIMEPULSE_IMAGE_CACHE_REPORT_TIMEOUT_MS=5000 \
runtimepulse-collector host-agent
```

The file or command output can be RuntimePulse JSON, common snapshotter state/index JSON, RuntimePulse or common-state JSONL, or a
Prometheus text exposition from a snapshotter/cache exporter. JSON fields may use
either RuntimePulse camelCase names or common exporter-style snake_case aliases
such as `image_ref`, `requested_blocks`, `hit_blocks`, `duration_ms`, and
`download_timeline`. Prometheus samples are grouped by labels such as
`image_ref`, `image`, `digest`, `snapshotter`, `layer`, `phase`, or `stage`;
metrics with names containing `image_cache`, `snapshotter`, `nydus`, `stargz`,
or `overlaybd` are normalized into the same cache metrics, layer counters,
prefetch records, and timeline spans. If an exporter only reports per-layer
cache counters, RuntimePulse derives the image-level lazy cache ratio and
read-byte metrics from the layer totals and marks those metrics with
`snapshotter.metricSource=layers`. RuntimePulse-shaped JSON rows describe one image
observation:

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
  "layers": [
    {
      "id": "bootstrap",
      "digest": "sha256:bootstrap",
      "sizeBytes": 104857600,
      "requestedBlocks": 2400,
      "hitBlocks": 2100,
      "localReadBytes": 275251200,
      "remoteReadBytes": 39321600,
      "blockSizeBytes": 131072
    }
  ],
  "prefetches": [
    {
      "id": "hot-blocks",
      "name": "Prefetch hot blocks",
      "phase": "prefetch",
      "startedAt": "2026-05-21T00:00:00.084Z",
      "durationMs": 1270,
      "bytes": 104857600,
      "requestedBlocks": 800,
      "hitBlocks": 650
    }
  ],
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

See `rust-collector/examples/image-cache-report.json` for a complete RuntimePulse JSON example, `rust-collector/examples/image-cache-state.json` for common snapshotter state/index JSON, and `rust-collector/examples/image-cache-prometheus.prom` for Prometheus text input.


## Runtime Diagnostic Reports

Use `diagnostic-report` when a host-side tool creates a runtime support bundle,
log archive, inspect dump, or other diagnostic artifact. RuntimePulse records a
small index with the bundle URI, summary counters, issue events, metrics, and an
optional capture span; the raw bundle remains in local/object storage.

```bash
RUNTIMEPULSE_HOST_AGENT_SOURCES=diagnostic-report
RUNTIMEPULSE_DIAGNOSTIC_REPORT_PATH=/var/lib/runtimepulse/diagnostic-report.jsonl
runtimepulse-collector host-agent
```

A host-side diagnostic exporter can also be executed every collection interval.
The command must write the same JSON/JSONL index to stdout; raw bundles should
still be written to local or object storage and referenced by `objectUri`:

```bash
RUNTIMEPULSE_HOST_AGENT_SOURCES=diagnostic-report
RUNTIMEPULSE_DIAGNOSTIC_REPORT_CMD='runtimepulse-collector docker-diagnostics'
RUNTIMEPULSE_DIAGNOSTIC_REPORT_TIMEOUT_MS=10000
runtimepulse-collector host-agent
```


The built-in Docker, CRI, and containerd diagnostic exporters write runtime metadata/artifact files
and print a RuntimePulse diagnostic JSONL index to stdout. They can be used
directly as the command hook above. Useful Docker settings:

| Variable | Default | Purpose |
| --- | --- | --- |
| `RUNTIMEPULSE_DOCKER_DIAGNOSTIC_CONTAINERS` | all containers | Comma-separated Docker container ids/names to export. |
| `RUNTIMEPULSE_DOCKER_DIAGNOSTIC_OUTPUT_DIR` | `/tmp/runtimepulse/diagnostics/docker` | Directory for raw inspect/log artifacts. |
| `RUNTIMEPULSE_DOCKER_DIAGNOSTIC_INCLUDE_LOGS` | `true` | Set `false` to skip `docker logs`. |
| `RUNTIMEPULSE_DOCKER_DIAGNOSTIC_TAIL_LINES` | `200` | Number of log lines captured per container. |

Useful CRI/crictl settings for Kubernetes/containerd nodes:

| Variable | Default | Purpose |
| --- | --- | --- |
| `RUNTIMEPULSE_CRICTL_BIN` | `crictl` | Path/name of the `crictl` binary. |
| `RUNTIMEPULSE_CRI_DIAGNOSTIC_CONTAINERS` | all containers | Comma-separated CRI container ids to export. |
| `RUNTIMEPULSE_CRI_DIAGNOSTIC_OUTPUT_DIR` | `/tmp/runtimepulse/diagnostics/cri` | Directory for raw `crictl inspect`/log artifacts. |
| `RUNTIMEPULSE_CRI_DIAGNOSTIC_INCLUDE_LOGS` | `true` | Set `false` to skip `crictl logs`. |
| `RUNTIMEPULSE_CRI_DIAGNOSTIC_TAIL_LINES` | `200` | Number of log lines captured per container. |

Useful native containerd settings:

| Variable | Default | Purpose |
| --- | --- | --- |
| `RUNTIMEPULSE_CONTAINERD_SOCKET` | `/run/containerd/containerd.sock` | containerd gRPC socket. |
| `RUNTIMEPULSE_CONTAINERD_NAMESPACES` | all namespaces | Comma-separated namespaces to inspect. Use `k8s.io` for CRI/containerd when Docker collectors are also enabled, so Docker's internal `moby` namespace is not reported twice. |
| `RUNTIMEPULSE_CONTAINERD_DIAGNOSTIC_CONTAINERS` | all containers | Comma-separated container ids, short ids, sandbox ids, or workload names to export. |
| `RUNTIMEPULSE_CONTAINERD_DIAGNOSTIC_OUTPUT_DIR` | `/tmp/runtimepulse/diagnostics/containerd` | Directory for raw native containerd metadata artifacts. |

For a one-shot local export without host-agent:

```bash
runtimepulse-collector docker-diagnostics > /var/lib/runtimepulse/docker-diagnostic-report.jsonl
runtimepulse-collector crictl-diagnostics > /var/lib/runtimepulse/cri-diagnostic-report.jsonl
runtimepulse-collector containerd-diagnostics > /var/lib/runtimepulse/containerd-diagnostic-report.jsonl
```

The report file or command output can be a RuntimePulse-shaped JSON object, JSON array, JSONL, or a common diagnostic artifact index with `reports` / `diagnostics` / `artifacts` / `files` / `items` rows. Artifact-index rows may use aliases such as `path`, `uri`, `type`, `bytes`, `duration_ms`, `sandbox_id`, nested `target` / `stats`, `findings`, and `attachments`:

```json
{
  "id": "diag-docker-runtimepulse-demo-001",
  "timestamp": "2026-05-23T00:00:00.000Z",
  "sandboxId": "docker-runtimepulse-demo",
  "runtimeType": "runc",
  "source": "docker-debug",
  "objectUri": "file:///var/lib/runtimepulse/diagnostics/docker-runtimepulse-demo/bundle.tar.zst",
  "sizeBytes": 7340032,
  "durationMs": 1850,
  "artifactType": "support_bundle",
  "summary": {"files": 42, "logs": 6, "warnings": 2, "errors": 0, "checks": 12, "failedChecks": 1},
  "issues": [{"severity": "warning", "category": "io", "message": "Container logs show delayed writes during startup."}]
}
```

Derived series include `diagnostic.artifact_size_bytes`,
`diagnostic.capture_duration_ms`, `diagnostic.issues_total`,
`diagnostic.files_total`, `diagnostic.logs_total`, `diagnostic.warnings_total`,
`diagnostic.errors_total`, and `diagnostic.failed_checks_total`. Issue rows are
emitted as `diagnostic.issue.<category>` events and capture duration is emitted
as a `diagnostic.<artifact_type>.capture` span.

See `rust-collector/examples/diagnostic-report.jsonl` for a RuntimePulse-shaped JSONL example and `rust-collector/examples/diagnostic-index.json` for a generic diagnostic artifact index example.

## Profile Artifact Reports

Use `profile-report` when a real perf/eBPF/third-party profiler can export
profile artifact indexes. RuntimePulse stores the profile metadata and object
URI; the profiler still owns the raw pprof/perf/flamegraph file.

```bash
RUNTIMEPULSE_HOST_AGENT_SOURCES=profile-report
RUNTIMEPULSE_PROFILE_REPORT_PATH=/var/lib/runtimepulse/profile-report.jsonl
runtimepulse-collector host-agent
```

For profiler-specific adapters, enable `perf` or `ebpf`. Each adapter can read
a report file, execute an exporter command, or merge both sources on each tick.
Commands run on the host through the host-agent and must write the same
RuntimePulse `PluginOutput` or lightweight profile JSON/JSONL to stdout:

```bash
RUNTIMEPULSE_HOST_AGENT_SOURCES=perf,ebpf
RUNTIMEPULSE_PERF_REPORT_PATH=/var/lib/runtimepulse/perf-report.jsonl
RUNTIMEPULSE_PERF_PROFILE_CMD='perf-summary --runtimepulse-json'
RUNTIMEPULSE_EBPF_REPORT_PATH=/var/lib/runtimepulse/ebpf-report.jsonl
RUNTIMEPULSE_EBPF_PROFILE_CMD='ebpf-profiler export --format runtimepulse-json'
RUNTIMEPULSE_PROFILE_COMMAND_TIMEOUT_MS=5000
runtimepulse-collector host-agent
```

The command timeout defaults to `RUNTIMEPULSE_ADAPTER_TIMEOUT_MS` or 3000 ms.
Timed-out profiler command process groups are terminated before the next
collection interval.

For a native `perf script` path, enable `perf-script` or run
`runtimepulse-collector perf-script`. It reads plain `perf script` text from a
file or command, groups identical call stacks, infers target command/PID and capture window from sample headers when available, and emits RuntimePulse profile
artifacts with inline flamegraph trees:

```bash
RUNTIMEPULSE_HOST_AGENT_SOURCES=perf-script
RUNTIMEPULSE_PERF_SCRIPT_PATH=/var/lib/runtimepulse/profiles/demo.perf-script
RUNTIMEPULSE_PERF_SCRIPT_SANDBOX_ID=docker-0123456789ab
runtimepulse-collector host-agent
```

Useful settings:

| Variable | Default | Purpose |
| --- | --- | --- |
| `RUNTIMEPULSE_PERF_SCRIPT_PATH` | unset | Single `perf script` text file to convert. |
| `RUNTIMEPULSE_PERF_SCRIPT_CMD` | unset | Command that prints `perf script` text to stdout. |
| `RUNTIMEPULSE_PERF_SCRIPT_TIMEOUT_MS` | adapter timeout | Timeout for `RUNTIMEPULSE_PERF_SCRIPT_CMD`. |
| `RUNTIMEPULSE_PERF_SCRIPT_SANDBOX_ID` | `host-perf-script` | Sandbox id for single-source mode. |
| `RUNTIMEPULSE_PERF_SCRIPT_OBJECT_URI` | generated `file://` URI | Optional URI for the raw `perf script` artifact. |
| `RUNTIMEPULSE_PERF_SCRIPT_PROFILE_TYPE` | `cpu` | Profile type. |
| `RUNTIMEPULSE_PERF_SCRIPT_PROCESS_ROLE` | `app` | Process role attached to the profile artifact. |
| `RUNTIMEPULSE_PERF_SCRIPT_DURATION_MS` | `0` | Capture window used to derive sample rate. |
| `RUNTIMEPULSE_PERF_SCRIPT_TARGETS` | unset | Semicolon-separated multi-target specs such as `sandbox=s1,path=/tmp/a.perf-script,role=app`. |
| `RUNTIMEPULSE_PERF_SCRIPT_OUTPUT_DIR` | `/tmp/runtimepulse/profiles/perf-script` | Directory for copied raw script artifacts when no object URI is supplied. |

See `rust-collector/examples/perf-script.txt` for a small input example.

For a lightweight native folded-stack perf path, enable the `perf-folded` source
or run `runtimepulse-collector perf-folded`; it converts folded stack files into
RuntimePulse profile artifacts with inline flamegraph trees. It is useful when hosts already run `perf script | stackcollapse-perf.pl`
or an equivalent exporter. Useful settings:

| Variable | Default | Purpose |
| --- | --- | --- |
| `RUNTIMEPULSE_PERF_FOLDED_PATH` | unset | Single folded stack file to convert. |
| `RUNTIMEPULSE_PERF_FOLDED_CMD` | unset | Command that prints folded stacks to stdout. |
| `RUNTIMEPULSE_PERF_FOLDED_TIMEOUT_MS` | `3000` | Timeout for `RUNTIMEPULSE_PERF_FOLDED_CMD`. |
| `RUNTIMEPULSE_PERF_FOLDED_SANDBOX_ID` | `host-perf-folded` | Sandbox id for the single-file mode. |
| `RUNTIMEPULSE_PERF_FOLDED_OBJECT_URI` | generated `file://` URI | Optional URI for the raw folded stack artifact. |
| `RUNTIMEPULSE_PERF_FOLDED_PROFILE_TYPE` | `cpu` | Profile type, for example `cpu` or `off_cpu`. |
| `RUNTIMEPULSE_PERF_FOLDED_PROCESS_ROLE` | `app` | Process role attached to the profile artifact. |
| `RUNTIMEPULSE_PERF_FOLDED_DURATION_MS` | `0` | Capture window used to derive sample rate. |
| `RUNTIMEPULSE_PERF_FOLDED_TARGETS` | unset | Semicolon-separated multi-target specs such as `sandbox=s1,path=/tmp/a.folded,role=runtime` or `sandbox=s1,command=profiler-export-folded`. |
| `RUNTIMEPULSE_PERF_FOLDED_OUTPUT_DIR` | `/tmp/runtimepulse/profiles/perf-folded` | Directory for copied folded stack artifacts when no object URI is supplied. |

Native host-agent source mode:

```bash
RUNTIMEPULSE_HOST_AGENT_SOURCES=perf-folded
RUNTIMEPULSE_PERF_FOLDED_PATH=/var/lib/runtimepulse/profiles/demo.folded
RUNTIMEPULSE_PERF_FOLDED_SANDBOX_ID=docker-0123456789ab
runtimepulse-collector host-agent
```

For eBPF profilers that export folded off-CPU/runtime stacks, use `ebpf-folded`
(or command `runtimepulse-collector ebpf-folded`). It uses the same folded-stack
format and defaults to `off_cpu` / `runtime` metadata:

```bash
RUNTIMEPULSE_HOST_AGENT_SOURCES=ebpf-folded
RUNTIMEPULSE_EBPF_FOLDED_PATH=/var/lib/runtimepulse/profiles/demo-offcpu.folded
RUNTIMEPULSE_EBPF_FOLDED_SANDBOX_ID=docker-0123456789ab
runtimepulse-collector host-agent
```

Additional eBPF folded settings mirror the perf names with `RUNTIMEPULSE_EBPF_FOLDED_*`, including `TARGETS`, `CMD`, `TIMEOUT_MS`, `OBJECT_URI`, `PROFILE_TYPE`, `PROCESS_ROLE`, `DURATION_MS`, and `OUTPUT_DIR`.

It can also be used through the existing perf command hook because `perf-folded`
prints a RuntimePulse-compatible `PluginOutput`:

```bash
RUNTIMEPULSE_HOST_AGENT_SOURCES=perf
RUNTIMEPULSE_PERF_PROFILE_CMD='runtimepulse-collector perf-folded'
runtimepulse-collector host-agent
```

The file or command output can be a RuntimePulse `PluginOutput`, a lightweight
RuntimePulse profile JSON object, a JSON array, JSONL, or a common artifact index
with `artifacts` / `files` / `items` rows. Artifact-index rows may use aliases
such as `path`, `uri`, `type`, `artifact_type`, `samples`, `sample_count`,
`duration_ms`, `sandbox_id`, and nested `target` / `stats` maps. Lightweight
RuntimePulse-shaped rows only need the target sandbox and profiles:

```json
{
  "sandboxId": "docker-0123456789ab",
  "source": "perf",
  "timestamp": "2026-05-22T00:00:00.000Z",
  "profiles": [
    {
      "profileType": "cpu",
      "processRole": "app",
      "durationMs": 10000,
      "sampleCount": 2451,
      "objectUri": "file:///var/lib/runtimepulse/profiles/docker-0123456789ab/cpu.perf",
      "target": {
        "pid": 4242,
        "command": "demo-app",
        "runtimeProcess": "workload"
      },
      "stats": {
        "sampleRateHz": 99.5,
        "cpuTimeMs": 8120,
        "lostSamples": 3,
        "kernelSamples": 540,
        "userSamples": 1911
      },
      "labels": {
        "runtime": "runc"
      }
    }
  ]
}
```

RuntimePulse preserves `target`, `stats`, `labels`, and custom `attributes` under profile metadata, emits profile observation events, and derives profile metric series such as `profile.samples_total`, `profile.duration_ms`, `profile.lost_samples_total`, `profile.sample_rate_hz`, `profile.cpu_time_ms`, `profile.kernel_samples_total`, and `profile.user_samples_total`.

See `rust-collector/examples/profile-report.jsonl` for a RuntimePulse-shaped JSONL example and `rust-collector/examples/profile-index.json` for a generic artifact index example.

## Container Cgroup Collection

Container cgroupfs metrics should be collected by a lifecycle-aware runtime collector, not by `host-cgroupfs`.

Expected flow:

1. A Docker/containerd/Kubernetes/runtime-specific collector observes a sandbox/container `started` event or reconciles already-running sandboxes from runtime inventory.
2. The collector resolves the exact runtime cgroup path for that sandbox shape, such as Docker PID or containerd task PID `/proc/<pid>/cgroup` resolution.
3. A per-sandbox cgroup sampler reports `sandbox.*` metrics against the sandbox id created by the lifecycle event.
4. The sampler stops or expires when the corresponding `stopped`/`exit`/`delete` event is observed.

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
