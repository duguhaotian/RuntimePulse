# runtimepulse-startup-probe

`runtimepulse-startup-probe` is the minimal host-side exporter for the
RuntimePulse CRI/containerd startup call-chain path. It does **not** proxy CRI.
It captures real kernel `execve`/process-exit tracepoints through `bpftrace`,
optionally attaches high-precision containerd/runtime-shim Go uprobes for CRI
`RunPodSandbox`, CNI setup, and runtime boundaries, and emits the raw JSON event
shape consumed by the Rust
`startup-callchain` source.

Current coverage:

- CNI plugin binaries discovered from CNI bin dirs.
- OCI/runtime binaries such as `runc`, `crun`, `kata-runtime`, `runsc`.
- Kata/containerd shim and selected hypervisor helpers when their paths exist or
  are passed through `--include-binary`.
- Optional helper binaries such as `iptables`, `nft`, `ip`, and `tc`.
- Optional Go uprobe entry capture for CRI `RunPodSandbox`
  (`--enable-go-uprobes`), containerd/go-cni setup boundaries
  (`--enable-cni-go-uprobes`), and OCI/Kata runtime shim boundaries
  (`--enable-runtime-go-uprobes`), discovered from Go pclntab symbols even when
  the ELF is stripped. Go return probes are available behind the explicit
  experimental `--enable-go-uretprobes` flag only.
- Containerd process-tree routing (`--containerd-tree`, enabled by default):
  exec/fork events are accepted only when they descend from the current
  containerd process, CNI plugin subtrees include helper children such as
  `iptables`, and runtime shim/runtime subtrees are grouped separately.  The
  exporter re-discovers containerd by executable/start time when a new
  containerd process appears, so containerd restarts do not leave the probe
  pinned to a stale PID.
- Correlation by `CNI_CONTAINERID`, CNI netns inode id, Kubernetes CNI args,
  containerd shim `-id`, or OCI/containerd bundle path; RunPodSandbox uprobe
  observations are joined to the sandbox report by the observed startup event
  window, while runtime-shim uprobes are joined by exact shim PID.
- Per-sandbox report grouping when one capture window observes multiple sandbox
  ids, so concurrent pod starts do not merge their CNI/helper/runtime metrics.

Example with host-agent:

```bash
sudo install -m 0755 tools/runtimepulse-startup-probe/runtimepulse-startup-probe /usr/local/bin/runtimepulse-startup-probe

RUNTIMEPULSE_HOST_AGENT_SOURCES=containerd-events,cri-events,cri-startup-trace,startup-callchain \
RUNTIMEPULSE_CONTAINERD_NAMESPACES=k8s.io \
RUNTIMEPULSE_CRI_EVENTS_CMD='crictl events --output json' \
RUNTIMEPULSE_STARTUP_CALLCHAIN_SPOOL_DIR=/var/lib/runtimepulse/startup-callchain \
runtimepulse-collector host-agent
```

Use the dedicated producer/consumer units in `deploy/systemd/` and keep
`startup-callchain` out of `RUNTIMEPULSE_HOST_AGENT_SOURCES`; the probe daemon
captures continuously and the collector service consumes completed reports from
the spool:

```bash
sudo install -m 0755 tools/runtimepulse-startup-probe/runtimepulse-startup-probe /usr/local/bin/runtimepulse-startup-probe
sudo install -m 0644 deploy/systemd/runtimepulse-startup-callchain.service /etc/systemd/system/runtimepulse-startup-callchain.service
sudo install -m 0644 deploy/systemd/runtimepulse-startup-probe.service /etc/systemd/system/runtimepulse-startup-probe.service
sudo systemctl daemon-reload
sudo systemctl enable --now runtimepulse-startup-probe runtimepulse-startup-callchain
```

Recommended shared env settings for those units:

```text
RUNTIMEPULSE_STARTUP_CALLCHAIN_SPOOL_DIR=/var/lib/runtimepulse/startup-callchain
RUNTIMEPULSE_STARTUP_CALLCHAIN_INTERVAL_MS=1000
RUNTIMEPULSE_STARTUP_PROBE_CONTAINERD_TREE=true
RUNTIMEPULSE_STARTUP_PROBE_CONTAINERD_NAMESPACES=k8s.io
RUNTIMEPULSE_STARTUP_PROBE_INCLUDE_HELPERS=true
RUNTIMEPULSE_STARTUP_PROBE_ENABLE_GO_UPROBES=true
RUNTIMEPULSE_STARTUP_PROBE_CONTAINERD_BINARY=/usr/bin/containerd
RUNTIMEPULSE_CONTAINERD_CONFIG=/etc/containerd/config.toml
```

The legacy `export --once` path remains available for parser/debug validation,
but normal deployment should use `runtimepulse-startup-probe daemon` plus the
spool consumer so short-lived startup events are not missed between polls.

For local parser validation without attaching BPF:

```bash
tools/runtimepulse-startup-probe/runtimepulse-startup-probe self-test
```

By default the exporter groups captured events into one report per observed
`sandboxId`/`criSandboxId` and emits a top-level `reports` array when more than
one sandbox is seen. Each report includes `startTime`, `endTime`, and `durationMs`
computed from its own event window, allowing RuntimePulse to render a
`sandbox.startup.callchain` root span per sandbox. Use `--no-group-by-sandbox`
only for legacy debugging where a single capture-window report is required.


## CRI/containerd E2E validation helper

Use `validate-cri-containerd-startup.sh` to make the current runc/Kata startup
call-chain path repeatable. It can validate an existing probe JSON report, or run
a timed probe capture window and then normalize the report through
`host-startup-callchain`.

```bash
# Validate report shape only.
tools/runtimepulse-startup-probe/validate-cri-containerd-startup.sh /tmp/startup-probe.json

# Add assertions for repeatable runc/Kata checks.
EXPECT_RUNTIME_TYPE=kata EXPECT_ROLES=cni,kata \
tools/runtimepulse-startup-probe/validate-cri-containerd-startup.sh /tmp/startup-probe.json

# Also push normalized traces/metrics to the local collector outlet.
VALIDATE_INGEST=true \
LOCAL_REPORT_URL=http://127.0.0.1:9091/api/local/ingest \
tools/runtimepulse-startup-probe/validate-cri-containerd-startup.sh /tmp/startup-probe.json

# Verify that the Query API can read back the ingested sandbox trace/metrics.
VALIDATE_QUERY_API=true QUERY_API_URL=http://127.0.0.1:8081/api \
tools/runtimepulse-startup-probe/validate-cri-containerd-startup.sh /tmp/startup-probe.json

# Optionally require waterfall hierarchy and analysis findings after Query API ingest.
VALIDATE_QUERY_API=true EXPECT_PARENT_LINKS=true EXPECT_ANALYSIS=true \
tools/runtimepulse-startup-probe/validate-cri-containerd-startup.sh /tmp/startup-probe.json

# Capture mode: start this, wait for the ready message, then run crictl runp in
# another shell before the capture window ends.
CAPTURE=true RUNTIME_TYPE=kata DURATION_MS=12000 \
tools/runtimepulse-startup-probe/validate-cri-containerd-startup.sh

# Fully automated runp capture when a standalone/containerd CRI endpoint and pod
# config are available. The script starts the probe, runs crictl runp, cleans up
# the sandbox, and validates collector ingestion.
CAPTURE=true VALIDATE_QUERY_API=true RUNTIME_TYPE=kata CRI_RUNTIME_HANDLER=kata \
ENABLE_GO_UPROBES=true CONTAINERD_BINARY=/usr/bin/containerd \
CONTAINERD_CONFIG=/tmp/runtimepulse-containerd-cri-test/config.toml \
EXPECT_ROLES=cri,cni,kata \
CRI_ENDPOINT=unix:///tmp/runtimepulse-containerd-cri-test/containerd.sock \
POD_CONFIG=/tmp/runtimepulse-containerd-cri-test/pod-config-kata.json \
tools/runtimepulse-startup-probe/validate-cri-containerd-startup.sh

# Concurrent RunPodSandbox capture: verify reports stay isolated when two pods
# start inside the same probe window. Works for runc or Kata by switching
# RUNTIME_TYPE/CRI_RUNTIME_HANDLER and POD_CONFIG. The request-identity
# assertion verifies RunPodSandbox request metadata is decoded and matched to
# the real sandbox id through CNI env identity.
CAPTURE=true VALIDATE_INGEST=true CONCURRENT_RUNPODS=2 \
EXPECT_PROCESS_BINARY_METRICS=true EXPECT_CNI_CONTAINER_ID=true \
EXPECT_RUNPOD_REQUEST_IDENTITY=true EXPECT_TOP_LEVEL_IDENTITY=true \
EXPECT_CNISETUP_DEBUG_PENDING=true EXPECT_RUNTIME_BOUNDARY_CORRELATION=true EXPECT_ROLES=cni,kata \
RUNTIME_TYPE=kata CRI_RUNTIME_HANDLER=kata \
ENABLE_GO_UPROBES=true ENABLE_CNI_GO_UPROBES=true ENABLE_RUNTIME_GO_UPROBES=true \
CONTAINERD_BINARY=/usr/bin/containerd \
CONTAINERD_CONFIG=/tmp/runtimepulse-containerd-cri-test/config.toml \
CRI_ENDPOINT=unix:///tmp/runtimepulse-containerd-cri-test/containerd.sock \
POD_CONFIG=/tmp/runtimepulse-containerd-cri-test/pod-config-kata.json \
tools/runtimepulse-startup-probe/validate-cri-containerd-startup.sh

# Workload-container startup capture: after RunPodSandbox, also run
# crictl create/start inside the sandbox and assert that later OCI/Kata runtime
# events remain linked to the pod sandbox while preserving the raw workload task
# id. Use a unique pod name when validating Query API readback repeatedly.
CAPTURE=true VALIDATE_INGEST=true VALIDATE_QUERY_API=true \
EXPECT_WORKLOAD_CONTAINER_RUNTIME=true EXPECT_RUNTIME_BOUNDARY_CORRELATION=true \
EXPECT_TOP_LEVEL_IDENTITY=true EXPECT_CNI_CONTAINER_ID=true \
RUNTIME_TYPE=runc CRI_RUNTIME_HANDLER= \
ENABLE_GO_UPROBES=true ENABLE_CNI_GO_UPROBES=true ENABLE_RUNTIME_GO_UPROBES=true \
CONTAINERD_BINARY=/usr/bin/containerd \
CONTAINERD_CONFIG=/tmp/runtimepulse-containerd-cri-test/config.toml \
RUNTIME_UPROBE_BINARIES=/usr/bin/containerd-shim-runc-v2 \
CRI_ENDPOINT=unix:///tmp/runtimepulse-containerd-cri-test/containerd.sock \
POD_CONFIG=/tmp/runtimepulse-containerd-cri-test/pod-config-runc-workload.json \
CONTAINER_CONFIG=/tmp/runtimepulse-containerd-cri-test/container-config-pause.json \
tools/runtimepulse-startup-probe/validate-cri-containerd-startup.sh
```

High-precision RunPodSandbox uprobe mode:

```bash
sudo RUNTIMEPULSE_STARTUP_CALLCHAIN_SPOOL_DIR=/var/lib/runtimepulse/startup-callchain \
  runtimepulse-startup-probe daemon \
  --containerd-namespace k8s.io \
  --include-helpers \
  --enable-go-uprobes \
  --enable-cni-go-uprobes \
  --enable-runtime-go-uprobes \
  --containerd-binary /usr/bin/containerd \
  --containerd-config /etc/containerd/config.toml
```

The default uprobe profile attaches to the first matching containerd
`RunPodSandbox` Go symbol and emits a `cri.run_pod_sandbox` span after Rust
normalization. The authoritative CNI plugin timing comes from `execve` events
for the real CNI plugin binaries and their `CNI_CONTAINERID`/`CNI_ARGS` env.
`--enable-cni-go-uprobes` is a debug-only containerd internal boundary for
`setupPodNetwork`/`go-cni`; it is not used as the primary CNI cost source and
remains pending when it cannot be attributed exactly. With
`--enable-runtime-go-uprobes`,
it also emits `oci.shim.*`, `oci.runc.*`, `kata.shim.*`, and
`kata.sandbox.*` boundary spans from runtime shim binaries. Runtime-shim uprobe
events are attributed by exact shim PID first to avoid pulling unrelated host
runtime activity into the active RunPodSandbox window. The default is
entry-only capture with visible synthetic exits. `--enable-go-uretprobes` can
also attach Go return probes for real return timestamps, but it is explicit and
experimental because some Go/runtime combinations cannot unwind safely with
uretprobes attached. Extra symbols can be
supplied with repeated `--go-uprobe-symbol` / `--cni-go-uprobe-symbol` /
`--runtime-go-uprobe-symbol` (or the matching
`RUNTIMEPULSE_STARTUP_PROBE_*_SYMBOLS` env vars) when investigating specific
containerd builds. After normalization, each observed process binary also gets
`sandbox.startup.process.binary.<binary>_{count,duration_ms}` metrics with roles such as
`cni`, `oci`, `kata`, or `helper`; use
`EXPECT_PROCESS_BINARY_METRICS=true` and `EXPECT_HELPER_BINARY_METRICS=true` in
`validate-cri-containerd-startup.sh` to assert that this attribution path is
present in real captures. Use `EXPECT_RUNPOD_REQUEST_IDENTITY=true` to assert
that RunPodSandbox request metadata was decoded and matched back to the real
sandbox id through CNI identity. Decoded RunPod and CNI_ARGS identity is emitted
as both attributes and top-level `namespace`/`podName`/`podUid`/`runtimeHandler`
fields so downstream normalizers can derive stable `k8s-{namespace}-{pod}-pod`
ids even when a report is consumed outside the bundled Rust collector. Use
`EXPECT_TOP_LEVEL_IDENTITY=true` to assert that those top-level identity fields
are present in real captures. Use
`EXPECT_CNISETUP_DEBUG_PENDING=true` in concurrent CNI uprobe captures to assert
CNISetup remains a pending debug boundary instead of being used for core CNI
cost attribution. Use
`EXPECT_RUNTIME_BOUNDARY_CORRELATION=true` to assert OCI/Kata exec/uprobe
runtime boundaries carry an exact sandbox correlation marker instead of relying
on broad time-window attribution. Use `CONTAINER_CONFIG=...` plus
`EXPECT_WORKLOAD_CONTAINER_RUNTIME=true` to assert a real workload
`crictl create/start` path after RunPodSandbox. For workload-container runtime
events, OCI bundle metadata keeps `startup.stable_sandbox_id` at the pod
sandbox level and adds `startup.stable_container_id` for the workload container
when Kubernetes labels are present, while preserving
`containerd.sandbox_container_id` and the raw workload `containerdId`. If a CRI
bundle only contains the raw pod sandbox id, the probe still marks
`startup.phase=container`, sets `startup.workload_container_id`, and groups the
runtime event under the raw pod sandbox id. This lets later container events
remain linked to their pod sandbox without losing the workload task id.
`--containerd-config` (or `RUNTIMEPULSE_CONTAINERD_CONFIG`) points the probe at
the same `config.toml` used by the target containerd so it can infer
`state`/`root` task bundle directories even when the shim argv only contains
`-namespace`/`-id` plus the runtime root. This is especially useful for
standalone CRI+containerd test deployments whose task root is not `/run`.

Limitations:

- The bundled uprobe mode captures RunPodSandbox/runtime-shim entries and can
  optionally capture containerd CNI setup debug boundaries. CNI plugin cost is
  measured from real plugin `execve` events, not from CNISetup uprobes. Go
  uretprobes can capture real returns, but remain an opt-in experimental mode
  until request decoding provides exact end attribution without perturbing Go
  stacks.
- Concurrent sandbox starts are correlated by stable sandbox ids when those ids
  are available in CNI env, CNI netns inode id, shim args, or bundle paths. The
  CNI path uses the standard `CNI_CONTAINERID` and `CNI_ARGS`
  `K8S_POD_INFRA_CONTAINER_ID` values; set `EXPECT_CNI_CONTAINER_ID=true` in the
  validation helper to assert that each report has a CNI plugin event tied to
  the same sandbox id. RunPodSandbox uprobes on amd64 try to decode the CRI
  request pointer and attach `k8s.namespace`, `k8s.pod`, `k8s.pod_uid`, and
  `cri.runtime_handler` directly to the CRI span. Containerd CNI setup uprobes
  that do not expose request identity stay pending in concurrent captures and
  are reported through the pending-go-uprobe quality counters.
