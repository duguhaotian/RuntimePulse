#!/usr/bin/env bash
# Validate RuntimePulse CRI+containerd startup call-chain ingestion from a real
# runtimepulse-startup-probe capture or an existing report file.
#
# This script intentionally does not create a CRI/containerd sandbox by itself.
# In capture mode it starts the probe; run `crictl runp` (runc or kata) in the
# capture window. With VALIDATE_INGEST=true it then normalizes the report through
# the Rust startup-callchain path and verifies traces/metrics/events.
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
PROBE_BIN="${PROBE_BIN:-$ROOT_DIR/tools/runtimepulse-startup-probe/runtimepulse-startup-probe}"
REPORT_PATH="${REPORT_PATH:-${1:-}}"
OUT_DIR="${OUT_DIR:-/tmp/runtimepulse-startup-e2e}"
DURATION_MS="${DURATION_MS:-12000}"
RUNTIME_TYPE="${RUNTIME_TYPE:-}"
CONTAINERD_NAMESPACE="${CONTAINERD_NAMESPACE:-k8s.io}"
INCLUDE_HELPERS="${INCLUDE_HELPERS:-true}"
LOCAL_REPORT_URL="${LOCAL_REPORT_URL:-http://127.0.0.1:9091/api/local/ingest}"
VALIDATE_INGEST="${VALIDATE_INGEST:-false}"

mkdir -p "$OUT_DIR"
REPORT_PATH="${REPORT_PATH:-$OUT_DIR/startup-probe-report.json}"
COLLECTOR_LOG="$OUT_DIR/startup-callchain-collector.log"
READY_FILE="$OUT_DIR/startup-probe.ready"

usage() {
  cat <<USAGE
Usage:
  REPORT_PATH=/path/to/probe.json $0
  $0 /path/to/probe.json
  CAPTURE=true RUNTIME_TYPE=kata $0

Modes:
  Existing report (default): validate REPORT_PATH shape.
  Capture mode: CAPTURE=true runs runtimepulse-startup-probe and writes REPORT_PATH.
  Ingest validation: VALIDATE_INGEST=true also normalizes through host-startup-callchain.

Common env:
  RUNTIME_TYPE=runc|kata       Expected runtime type hint for capture mode.
  CONTAINERD_NAMESPACE=k8s.io  Namespace filter for runtime/shim events.
  INCLUDE_HELPERS=true|false   Include helper binaries such as iptables/nft/ip/tc.
  VALIDATE_INGEST=true         Also send normalized output to LOCAL_REPORT_URL.
  LOCAL_REPORT_URL=http://127.0.0.1:9091/api/local/ingest
USAGE
}

if [[ "${1:-}" == "-h" || "${1:-}" == "--help" ]]; then
  usage
  exit 0
fi

require_file() {
  if [[ ! -s "$1" ]]; then
    echo "missing or empty file: $1" >&2
    exit 1
  fi
}

capture_probe() {
  local helper_args=()
  local runtime_args=()
  if [[ "$INCLUDE_HELPERS" == "true" ]]; then
    helper_args+=(--include-helpers)
  fi
  if [[ -n "$RUNTIME_TYPE" ]]; then
    runtime_args+=(--runtime-type "$RUNTIME_TYPE")
  fi

  rm -f "$READY_FILE" "$REPORT_PATH"
  echo "starting startup probe capture: $REPORT_PATH" >&2
  sudo "$PROBE_BIN" export \
    --once \
    --duration-ms "$DURATION_MS" \
    --containerd-namespace "$CONTAINERD_NAMESPACE" \
    --ready-file "$READY_FILE" \
    "${runtime_args[@]}" \
    "${helper_args[@]}" \
    > "$REPORT_PATH" &
  local probe_pid=$!

  for _ in {1..100}; do
    [[ -e "$READY_FILE" ]] && break
    if ! kill -0 "$probe_pid" 2>/dev/null; then
      wait "$probe_pid"
      break
    fi
    sleep 0.05
  done

  echo "probe is ready; run crictl runp now if not already running" >&2
  wait "$probe_pid"
  require_file "$REPORT_PATH"
}

normalize_report() {
  echo "normalizing startup-callchain report: $REPORT_PATH" >&2
  RUNTIMEPULSE_COLLECTOR_ONCE=true \
  RUNTIMEPULSE_STARTUP_CALLCHAIN_REPORT_PATH="$REPORT_PATH" \
  RUNTIMEPULSE_LOCAL_REPORT_URL="$LOCAL_REPORT_URL" \
  cargo run --manifest-path "$ROOT_DIR/rust-collector/Cargo.toml" --quiet -- host-startup-callchain \
    > "$COLLECTOR_LOG"
  require_file "$COLLECTOR_LOG"
}

validate_report_shape() {
  python3 - "$REPORT_PATH" <<'PY'
import json, sys
path = sys.argv[1]
with open(path, encoding='utf-8') as handle:
    payload = json.load(handle)
reports = payload.get('reports') if isinstance(payload, dict) else None
if reports is None:
    reports = [payload]
if not reports:
    raise SystemExit('no reports found')
for idx, report in enumerate(reports):
    events = report.get('events') or []
    if not events:
        raise SystemExit(f'report {idx} has no events')
    if not report.get('sandboxId') and not report.get('criSandboxId'):
        raise SystemExit(f'report {idx} has no sandbox id')
    roles = {event.get('role') for event in events if event.get('eventType') == 'enter'}
    if not roles & {'cni', 'oci', 'kata', 'helper'}:
        raise SystemExit(f'report {idx} has no startup roles: {sorted(roles)}')
print(json.dumps({
    'reports': len(reports),
    'events': sum(len(report.get('events') or []) for report in reports),
    'sandboxes': [report.get('sandboxId') or report.get('criSandboxId') for report in reports],
}, separators=(',', ':')))
PY
}

validate_collector_output() {
  python3 - "$COLLECTOR_LOG" <<'PY'
import json, sys
path = sys.argv[1]
accepted = []
with open(path, encoding='utf-8') as handle:
    for line in handle:
        line = line.strip()
        if not line:
            continue
        try:
            item = json.loads(line)
        except json.JSONDecodeError:
            continue
        if item.get('message') == 'host_startup_callchain_report_accepted':
            accepted.append(item)
if not accepted:
    raise SystemExit('collector did not accept startup-callchain report')
last = accepted[-1]
if int(last.get('traces') or 0) <= 0:
    raise SystemExit(f'collector emitted no traces: {last}')
if int(last.get('metrics') or 0) <= 0:
    raise SystemExit(f'collector emitted no metrics: {last}')
print(json.dumps(last, separators=(',', ':')))
PY
}

if [[ "${CAPTURE:-false}" == "true" ]]; then
  capture_probe
else
  require_file "$REPORT_PATH"
fi

validate_report_shape
if [[ "$VALIDATE_INGEST" == "true" ]]; then
  normalize_report
  validate_collector_output
else
  echo "report shape validated; set VALIDATE_INGEST=true to send through host-startup-callchain" >&2
fi
