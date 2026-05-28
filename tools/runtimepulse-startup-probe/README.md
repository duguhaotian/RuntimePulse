# runtimepulse-startup-probe

`runtimepulse-startup-probe` is the minimal host-side exporter for the
RuntimePulse CRI/containerd startup call-chain path. It does **not** proxy CRI.
It captures real kernel `execve`/process-exit tracepoints through `bpftrace` and
emits the raw JSON event shape consumed by the Rust `startup-callchain` source.

Current coverage:

- CNI plugin binaries discovered from CNI bin dirs.
- OCI/runtime binaries such as `runc`, `crun`, `kata-runtime`, `runsc`.
- Kata/containerd shim and selected hypervisor helpers when their paths exist or
  are passed through `--include-binary`.
- Optional helper binaries such as `iptables`, `nft`, `ip`, and `tc`.
- Correlation by `CNI_CONTAINERID`, Kubernetes CNI args, containerd shim `-id`,
  or OCI/containerd bundle path.
- Per-sandbox report grouping when one capture window observes multiple sandbox
  ids, so concurrent pod starts do not merge their CNI/helper/runtime metrics.

Example with host-agent:

```bash
sudo install -m 0755 tools/runtimepulse-startup-probe/runtimepulse-startup-probe /usr/local/bin/runtimepulse-startup-probe

RUNTIMEPULSE_HOST_AGENT_SOURCES=containerd-events,cri-events,cri-startup-trace,startup-callchain \
RUNTIMEPULSE_CONTAINERD_NAMESPACES=k8s.io \
RUNTIMEPULSE_CRI_EVENTS_CMD='crictl events --output json' \
RUNTIMEPULSE_STARTUP_CALLCHAIN_REPORT_CMD='sudo runtimepulse-startup-probe export --once --duration-ms 3000 --containerd-namespace k8s.io --include-helpers' \
runtimepulse-collector host-agent
```

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

# Also push normalized traces/metrics to the local collector outlet.
VALIDATE_INGEST=true \
LOCAL_REPORT_URL=http://127.0.0.1:9091/api/local/ingest \
tools/runtimepulse-startup-probe/validate-cri-containerd-startup.sh /tmp/startup-probe.json

# Capture mode: start this, wait for the ready message, then run crictl runp in
# another shell before the capture window ends.
CAPTURE=true RUNTIME_TYPE=kata DURATION_MS=12000 \
tools/runtimepulse-startup-probe/validate-cri-containerd-startup.sh

# Fully automated runp capture when a standalone/containerd CRI endpoint and pod
# config are available. The script starts the probe, runs crictl runp, cleans up
# the sandbox, and validates collector ingestion.
CAPTURE=true VALIDATE_INGEST=true RUNTIME_TYPE=kata CRI_RUNTIME_HANDLER=kata \
CRI_ENDPOINT=unix:///tmp/runtimepulse-containerd-cri-test/containerd.sock \
POD_CONFIG=/tmp/runtimepulse-containerd-cri-test/pod-config-kata.json \
tools/runtimepulse-startup-probe/validate-cri-containerd-startup.sh
```

Limitations:

- This is an exec-level exporter. It provides concrete CNI/runtime/helper binary
  attribution, but it is not yet a Go uprobe on containerd CRI `RunPodSandbox`.
- Concurrent sandbox starts are correlated by stable sandbox ids when those ids
  are available in CNI env, shim args, or bundle paths. The future native uprobe
  backend should add RunPodSandbox request context directly.
