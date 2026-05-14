# RuntimePulse Collector Plugins

The Rust collector is the preferred path for real data collection. It keeps collection independent from the frontend and sends the same `POST /api/ingest/batch` payload as other collectors.

For personal node deployments, the Rust collector also acts as the node-local outlet. Host tools and container tools submit locally to the collector container over HTTP, and only the outlet posts to the central RuntimePulse ingest API.

## Plugin Model

Each plugin returns partial ingest data:

- `metadata`: clusters, nodes, images, sandboxes
- `metrics`: timestamped numeric samples
- `events`: lifecycle or collector events
- `traces`: startup or runtime spans
- `profiles`: profile artifact indexes

The collector merges plugin outputs into one batch, adds the collector source, and posts it to the Query API ingest endpoint.

For node deployments, plugin output should flow through the local outlet path even when the plugin runs in a separate container or process. This keeps node identity, batching, retry, and central ingest configuration in one place.

## Built-In Plugins

- `procfs`: reads real CPU, memory, disk, load, process, and cgroup-like process signals from `/proc`.
- `command`: runs an external binary or shell command and parses JSON from stdout.
- `http`: calls an HTTP API and parses JSON from the response body.

## Command Plugin

Use this for existing tools that already expose useful data.

```bash
RUNTIMEPULSE_COLLECTOR_PLUGINS=procfs,command
RUNTIMEPULSE_COMMAND_PLUGIN_NAME=containerd-exporter
RUNTIMEPULSE_COMMAND_PLUGIN_CMD='containerd-exporter --format runtimepulse-json'
```

The command must write JSON shaped like `rust-collector/examples/command-plugin-output.json`.

## HTTP Plugin

Use this for tools that expose a local or remote API.

```bash
RUNTIMEPULSE_COLLECTOR_PLUGINS=procfs,http
RUNTIMEPULSE_HTTP_PLUGIN_NAME=image-cache-agent
RUNTIMEPULSE_HTTP_PLUGIN_URL=http://image-cache-agent:9090/runtimepulse
```

The endpoint must return the same partial ingest JSON shape as the command plugin.

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
