# Collector Architecture

RuntimePulse collectors are organized around one rule: each collector belongs to one of three runtime layers, and all collectors are managed from one place.

## Top-Level Layers

### 1. Outlet Layer

Runs inside the node-local collector container.

Responsibilities:

- accept local reports over HTTP
- normalize partial payloads
- batch and forward to Query API ingest
- host `command` and `http` adapters
- act as the central process for third-party adapters that do not need host or sandbox namespaces

Typical collectors:

- outlet HTTP receiver
- command adapter
- HTTP pull adapter
- payload normalizer
- batch sender

### 2. Host Layer

Runs on the node host or with host-level access.

Responsibilities:

- read host-only kernel and filesystem state
- collect node-wide metrics and events
- discover runtime and image inventory
- watch sandbox lifecycle events
- feed the outlet container through local HTTP

Typical collectors:

- host `procfs`
- host root `cgroupfs`
- PSI/pressure
- Docker inventory
- containerd or kubelet lifecycle watcher
- image cache / unpack probe
- eBPF / perf / kernel profile collectors

### 3. Sandbox Layer

Runs per sandbox shape or per sandbox lifecycle domain.

Responsibilities:

- attach to a specific sandbox after `started`
- sample sandbox-specific cgroups and runtime state
- stop or expire after `stopped`
- support multiple sandbox shapes through adapters

Typical collectors:

- container sandbox cgroup sampler
- gVisor sandbox sampler
- Kata sandbox sampler
- Firecracker sandbox sampler
- sandbox trace / profile collector

## Third-Party Collectors

Third-party collectors are not a separate runtime layer. They are adapters that plug into one of the three layers above.

Preferred integration paths:

- outlet layer via `command`
- outlet layer via `http`
- host layer via host binary + local HTTP push
- sandbox layer via lifecycle-triggered sampler

## Project Split And Runtime Grouping

The repo should treat collector code as one project with multiple runtime groups.

| Runtime group | Layer | Runs with | Examples |
| --- | --- | --- | --- |
| `collector-outlet` | outlet | collector container | local HTTP ingress, command/http adapters, batching, central ingest forwarding |
| `host-agent` | host | host process or privileged host-visible deployment | procfs, host root cgroupfs, Docker inventory, lifecycle watchers, image probes, eBPF |
| `sandbox-agent` | sandbox | lifecycle-triggered worker or runtime-specific process | sandbox cgroup sampler, gVisor/Kata/Firecracker samplers, sandbox traces/profiles |
| `third-party-adapters` | adapter | outlet, host agent, or sandbox agent | external binary adapter, third-party HTTP metric source |

First implementation can keep these runtime groups in one Rust crate and one binary with subcommands. As the code grows, the groups can split into separate binaries without changing the layer model.

## What Runs Together

### Collector Container

Run together:

- outlet HTTP ingress
- command adapters
- HTTP adapters
- batch normalization and forwarding

### Host Agent

Run together:

- host `procfs`
- host root `cgroupfs`
- PSI
- Docker inventory
- lifecycle watchers
- image cache probes
- host eBPF / perf collectors

### Sandbox Agent

Run together when the runtime needs per-sandbox attachment:

- sandbox lifecycle listener
- runtime-specific cgroup sampler
- sandbox trace collector
- sandbox profile collector

## Directory Shape

All collectors should live under one tree in `rust-collector/src/collectors/`:

```text
collectors/
  outlet/
  host/
  sandbox/
  third_party/
```

The goal is to keep collector ownership obvious:

- outlet code stays together
- host collectors stay together
- sandbox collectors stay together
- third-party adapters stay together

## Design Rule

If a collector needs:

- host namespaces or host filesystem access, put it in the host layer
- a sandbox-specific lifecycle or cgroup, put it in the sandbox layer
- only normalization, batching, or generic integration, put it in the outlet layer
- external binary or HTTP integration, treat it as third-party and adapt it into one of the layers above
