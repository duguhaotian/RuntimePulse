# CRI + containerd Collection Scope

RuntimePulse's production runtime collection target is **CRI + containerd**. Docker
support exists only to validate collector plumbing on a simple local machine and
to keep a convenient demo path. We do not optimize the Docker path beyond keeping
it configurable. For real runtime work, keep Docker collectors disabled and focus
on CRI/containerd data quality.

## Target deployment

The intended host has one CRI runtime backed by containerd:

```text
kubelet / CRI client
  -> CRI API
  -> containerd namespace k8s.io
  -> runc / Kata / other OCI runtime
```

The host may also have Docker installed for local development, but Docker is not
part of the production collection target. If Docker is present, Docker collectors
must be explicitly enabled; otherwise RuntimePulse should ignore Docker-managed
containers.

Recommended CRI/containerd host-agent sources:

```bash
RUNTIMEPULSE_HOST_AGENT_SOURCES=procfs,psi,cgroupfs,containerd-inventory,containerd-events,containerd-sandbox-cgroupfs,image-cache,profile-report,perf,ebpf
RUNTIMEPULSE_CONTAINERD_SOCKET=/run/containerd/containerd.sock
RUNTIMEPULSE_CONTAINERD_NAMESPACES=k8s.io
RUNTIMEPULSE_CGROUP_ROOT=/sys/fs/cgroup
```

For a standalone CRI/containerd validation environment, point the socket at that
containerd instance instead:

```bash
RUNTIMEPULSE_CONTAINERD_SOCKET=/tmp/runtimepulse-containerd-cri-test/containerd.sock
RUNTIMEPULSE_CONTAINERD_NAMESPACES=k8s.io
```

## Optional Docker validation mode

Docker collectors are optional and should only be used for local validation or
UI/demo data:

```bash
RUNTIMEPULSE_HOST_AGENT_SOURCES=procfs,psi,cgroupfs,docker-inventory,docker-events,docker-sandbox-cgroupfs
```

If Docker collectors are enabled together with containerd collectors, do **not**
collect Docker's internal containerd `moby` namespace through the containerd
source. Those rows describe the same Docker containers and will appear as
`containerd-moby-*` duplicates next to `docker-*` rows.

Correct mixed validation mode:

```bash
# Docker rows from Docker collectors.
# CRI rows from containerd k8s.io only.
RUNTIMEPULSE_HOST_AGENT_SOURCES=procfs,psi,cgroupfs,docker-inventory,docker-events,docker-sandbox-cgroupfs,containerd-inventory,containerd-events,containerd-sandbox-cgroupfs
RUNTIMEPULSE_CONTAINERD_SOCKET=<CRI containerd socket>
RUNTIMEPULSE_CONTAINERD_NAMESPACES=k8s.io
```

## How runtime collection is triggered

Runtime collectors are not auto-discovered from installed runtimes. They are
controlled by `RUNTIMEPULSE_HOST_AGENT_SOURCES`.

The host-agent starts two kinds of work:

1. **Periodic collectors**, once per `RUNTIMEPULSE_COLLECTOR_INTERVAL_MS`:
   - `containerd-inventory`
   - `containerd-sandbox-cgroupfs`
   - node collectors such as `procfs`, `psi`, and `cgroupfs`
   - report/file/command collectors such as `image-cache`, `profile-report`,
     `perf`, and `ebpf`
2. **Event-stream collectors**, long-running worker threads:
   - `containerd-events`
   - optional `docker-events` in Docker validation mode

`containerd-events` feeds lifecycle metadata and keeps the active containerd task
set updated. `containerd-sandbox-cgroupfs` samples only active targets resolved
from containerd tasks; it does not scan the cgroup tree broadly.

## What the CRI/containerd collectors depend on

The main CRI/containerd inventory/event/cgroup collectors depend on runtime APIs
and process metadata, not on direct metadata directory scans:

| Collector | Runtime dependency | Filesystem dependency |
| --- | --- | --- |
| `containerd-inventory` | containerd gRPC socket | none for metadata directories |
| `containerd-events` | containerd event service | none for metadata directories |
| `containerd-sandbox-cgroupfs` | containerd task PID | `/proc/<pid>/cgroup` and `RUNTIMEPULSE_CGROUP_ROOT` |
| `cri-startup-trace` | CRI/containerd lifecycle events | none for metadata directories |
| `startup-callchain` / uprobe exporter | RunPod/CNI/OCI/Kata events | OCI bundle `config.json`; may need containerd `config.toml` for state/root discovery |

Important details:

- `RUNTIMEPULSE_CONTAINERD_SOCKET` selects the containerd instance.
- `RUNTIMEPULSE_CONTAINERD_NAMESPACES=k8s.io` selects CRI-managed containers and
  excludes Docker's `moby` namespace.
- `RUNTIMEPULSE_CGROUP_ROOT` defaults to `/sys/fs/cgroup`.
- Cgroup sampling resolves exact paths from task PIDs via `/proc/<pid>/cgroup`.
- The high-fidelity startup call-chain path is the exception: it may need the
  target containerd `config.toml` so the probe can locate OCI bundle roots when
  the runtime state directory differs from `/run/containerd`.

## Current policy

- Keep Docker collectors **off** for CRI/containerd development and validation.
- Keep Docker support configurable for local demos only.
- Do not spend design effort optimizing Docker/containerd duplicate handling
  beyond clear configuration and documentation.
- Prioritize CRI/containerd runc and Kata end-to-end collection quality.
