#!/usr/bin/env bash
# Validate RuntimePulse CRI+containerd startup call-chain ingestion from a real
# runtimepulse-startup-probe capture or an existing report file.
#
# Existing report mode validates a probe JSON report. Capture mode starts the
# probe and can optionally run `crictl runp` automatically when POD_CONFIG is
# provided. With VALIDATE_INGEST=true it normalizes the report through the Rust
# startup-callchain path and verifies traces/metrics/events.
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
QUERY_API_URL="${QUERY_API_URL:-http://127.0.0.1:8081/api}"
QUERY_API_RETRY_SECONDS="${QUERY_API_RETRY_SECONDS:-10}"
VALIDATE_INGEST="${VALIDATE_INGEST:-false}"
VALIDATE_QUERY_API="${VALIDATE_QUERY_API:-false}"
CRI_ENDPOINT="${CRI_ENDPOINT:-}"
IMAGE_ENDPOINT="${IMAGE_ENDPOINT:-$CRI_ENDPOINT}"
POD_CONFIG="${POD_CONFIG:-}"
CRI_RUNTIME_HANDLER="${CRI_RUNTIME_HANDLER:-$RUNTIME_TYPE}"
CLEANUP_POD="${CLEANUP_POD:-true}"
EXPECT_RUNTIME_TYPE="${EXPECT_RUNTIME_TYPE:-$RUNTIME_TYPE}"
EXPECT_ROLES="${EXPECT_ROLES:-}"
EXPECT_SANDBOX_ID="${EXPECT_SANDBOX_ID:-}"
EXPECT_ANALYSIS="${EXPECT_ANALYSIS:-false}"
EXPECT_PARENT_LINKS="${EXPECT_PARENT_LINKS:-false}"

mkdir -p "$OUT_DIR"
REPORT_PATH="${REPORT_PATH:-$OUT_DIR/startup-probe-report.json}"
COLLECTOR_LOG="$OUT_DIR/startup-callchain-collector.log"
READY_FILE="$OUT_DIR/startup-probe.ready"
RUNP_LOG="$OUT_DIR/crictl-runp.log"
SANDBOX_ID_FILE="$OUT_DIR/crictl-sandbox-id"

usage() {
  cat <<USAGE
Usage:
  REPORT_PATH=/path/to/probe.json $0
  $0 /path/to/probe.json
  CAPTURE=true RUNTIME_TYPE=kata $0
  CAPTURE=true POD_CONFIG=/path/to/pod.json CRI_ENDPOINT=unix:///run/containerd/containerd.sock $0

Modes:
  Existing report (default): validate REPORT_PATH shape.
  Capture mode: CAPTURE=true runs runtimepulse-startup-probe and writes REPORT_PATH.
  Automatic RunPod: set POD_CONFIG and CRI_ENDPOINT to run crictl runp during capture.
  Ingest validation: VALIDATE_INGEST=true also normalizes through host-startup-callchain.

Common env:
  RUNTIME_TYPE=runc|kata       Expected runtime type hint for capture mode.
  CONTAINERD_NAMESPACE=k8s.io  Namespace filter for runtime/shim events.
  INCLUDE_HELPERS=true|false   Include helper binaries such as iptables/nft/ip/tc.
  POD_CONFIG=/path/pod.json    Optional crictl runp pod config for automatic E2E.
  CRI_ENDPOINT=unix://...      Runtime endpoint used by crictl.
  CRI_RUNTIME_HANDLER=kata     Optional --runtime passed to crictl runp.
  CLEANUP_POD=true|false       Stop/remove the sandbox after automatic RunPod.
  EXPECT_RUNTIME_TYPE=kata     Optional report runtime assertion. Defaults to RUNTIME_TYPE.
  EXPECT_ROLES=cni,kata        Optional required enter-event roles.
  EXPECT_SANDBOX_ID=...        Optional required sandbox id; auto-filled from runp.
  EXPECT_PARENT_LINKS=true     Require at least one non-root trace span parent link.
  EXPECT_ANALYSIS=true         Require Query API analysis findings for each sandbox.
  VALIDATE_INGEST=true         Also send normalized output to LOCAL_REPORT_URL.
  VALIDATE_QUERY_API=true      Verify sandbox trace/metrics via QUERY_API_URL.
  LOCAL_REPORT_URL=http://127.0.0.1:9091/api/local/ingest
  QUERY_API_URL=http://127.0.0.1:8081/api
  QUERY_API_RETRY_SECONDS=10   Wait for collector outlet to flush to Query API.
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

crictl_args() {
  local args=()
  if [[ -n "$CRI_ENDPOINT" ]]; then
    args+=(--runtime-endpoint "$CRI_ENDPOINT")
  fi
  if [[ -n "$IMAGE_ENDPOINT" ]]; then
    args+=(--image-endpoint "$IMAGE_ENDPOINT")
  fi
  printf '%s\0' "${args[@]}"
}

run_crictl() {
  local args=()
  while IFS= read -r -d '' item; do
    args+=("$item")
  done < <(crictl_args)
  crictl "${args[@]}" "$@"
}

cleanup_sandbox() {
  [[ "$CLEANUP_POD" == "true" ]] || return 0
  [[ -s "$SANDBOX_ID_FILE" ]] || return 0
  local sandbox_id
  sandbox_id="$(cat "$SANDBOX_ID_FILE")"
  [[ -n "$sandbox_id" ]] || return 0
  run_crictl stopp "$sandbox_id" >/dev/null 2>&1 || true
  run_crictl rmp "$sandbox_id" >/dev/null 2>&1 || true
}

run_pod_sandbox() {
  [[ -n "$POD_CONFIG" ]] || return 0
  require_file "$POD_CONFIG"
  if ! command -v crictl >/dev/null 2>&1; then
    echo "crictl not found; install crictl or unset POD_CONFIG" >&2
    exit 1
  fi

  local runtime_args=()
  if [[ -n "$CRI_RUNTIME_HANDLER" ]]; then
    runtime_args+=(--runtime "$CRI_RUNTIME_HANDLER")
  fi

  echo "running crictl runp: $POD_CONFIG" >&2
  set +e
  run_crictl runp "${runtime_args[@]}" "$POD_CONFIG" > "$RUNP_LOG" 2>&1
  local rc=$?
  set -e
  if [[ $rc -ne 0 ]]; then
    cat "$RUNP_LOG" >&2 || true
    exit $rc
  fi
  local sandbox_id
  sandbox_id="$(tail -n 1 "$RUNP_LOG" | tr -d '[:space:]')"
  if [[ -z "$sandbox_id" ]]; then
    echo "crictl runp did not return a sandbox id" >&2
    cat "$RUNP_LOG" >&2 || true
    exit 1
  fi
  echo "$sandbox_id" > "$SANDBOX_ID_FILE"
  if [[ -z "$EXPECT_SANDBOX_ID" ]]; then
    EXPECT_SANDBOX_ID="$sandbox_id"
    export EXPECT_SANDBOX_ID
  fi
  echo "created sandbox: $sandbox_id" >&2
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

  rm -f "$READY_FILE" "$REPORT_PATH" "$RUNP_LOG" "$SANDBOX_ID_FILE"
  trap cleanup_sandbox EXIT
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

  if [[ -e "$READY_FILE" ]]; then
    if [[ -n "$POD_CONFIG" ]]; then
      run_pod_sandbox
    else
      echo "probe is ready; run crictl runp now before the capture window ends" >&2
    fi
  else
    echo "probe did not report ready before timeout" >&2
  fi

  wait "$probe_pid"
  cleanup_sandbox
  trap - EXIT
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
  EXPECT_RUNTIME_TYPE="$EXPECT_RUNTIME_TYPE" \
  EXPECT_ROLES="$EXPECT_ROLES" \
  EXPECT_SANDBOX_ID="$EXPECT_SANDBOX_ID" \
  python3 - "$REPORT_PATH" <<'PY'
import json, os, sys
path = sys.argv[1]
expect_runtime = os.environ.get('EXPECT_RUNTIME_TYPE', '').strip().lower()
expect_roles = {item.strip().lower() for item in os.environ.get('EXPECT_ROLES', '').split(',') if item.strip()}
expect_sandbox = os.environ.get('EXPECT_SANDBOX_ID', '').strip()
with open(path, encoding='utf-8') as handle:
    payload = json.load(handle)
reports = payload.get('reports') if isinstance(payload, dict) else None
if reports is None:
    reports = [payload]
if not reports:
    raise SystemExit('no reports found')
all_roles = set()
runtime_types = set()
sandboxes = []
for idx, report in enumerate(reports):
    events = report.get('events') or []
    if not events:
        raise SystemExit(f'report {idx} has no events')
    sandbox = report.get('sandboxId') or report.get('criSandboxId')
    if not sandbox:
        raise SystemExit(f'report {idx} has no sandbox id')
    sandboxes.append(str(sandbox))
    report_runtime = str(report.get('runtimeType') or '').lower()
    if report_runtime:
        runtime_types.add(report_runtime)
    roles = {str(event.get('role') or '').lower() for event in events if event.get('eventType') == 'enter'}
    roles.discard('')
    all_roles |= roles
    runtime_types |= {str(event.get('runtimeType') or '').lower() for event in events if event.get('runtimeType')}
    if not roles & {'cni', 'oci', 'kata', 'helper'}:
        raise SystemExit(f'report {idx} has no startup roles: {sorted(roles)}')
if expect_runtime and expect_runtime not in runtime_types:
    raise SystemExit(f'expected runtimeType {expect_runtime}, observed {sorted(runtime_types)}')
missing_roles = sorted(expect_roles - all_roles)
if missing_roles:
    raise SystemExit(f'missing expected roles {missing_roles}, observed {sorted(all_roles)}')
if expect_sandbox and expect_sandbox not in sandboxes:
    raise SystemExit(f'expected sandbox {expect_sandbox}, observed {sandboxes}')
print(json.dumps({
    'reports': len(reports),
    'events': sum(len(report.get('events') or []) for report in reports),
    'sandboxes': sandboxes,
    'runtimeTypes': sorted(runtime_types),
    'roles': sorted(all_roles),
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

query_api_sandboxes() {
  python3 - "$REPORT_PATH" <<'PY'
import json, sys
with open(sys.argv[1], encoding='utf-8') as handle:
    payload = json.load(handle)
reports = payload.get('reports') if isinstance(payload, dict) else None
if reports is None:
    reports = [payload]
for report in reports:
    sandbox = report.get('sandboxId') or report.get('criSandboxId')
    if sandbox:
        print(sandbox)
PY
}

query_api_get() {
  local url="$1"
  local output="$2"
  local deadline=$((SECONDS + QUERY_API_RETRY_SECONDS))
  while true; do
    if curl -fsS "$url" > "$output" 2>/dev/null; then
      return 0
    fi
    if (( SECONDS >= deadline )); then
      curl -fsS "$url" > "$output"
      return $?
    fi
    sleep 1
  done
}

validate_query_api_output() {
  if ! command -v curl >/dev/null 2>&1; then
    echo "curl not found; cannot validate Query API" >&2
    exit 1
  fi

  local sandbox_id
  local trace_file
  local metrics_file
  while IFS= read -r sandbox_id; do
    [[ -n "$sandbox_id" ]] || continue
    trace_file="$OUT_DIR/query-trace-${sandbox_id}.json"
    metrics_file="$OUT_DIR/query-metrics-${sandbox_id}.json"
    analysis_file="$OUT_DIR/query-analysis-${sandbox_id}.json"
    query_api_get "$QUERY_API_URL/sandboxes/$sandbox_id/trace" "$trace_file"
    query_api_get "$QUERY_API_URL/sandboxes/$sandbox_id/metrics" "$metrics_file"
    if [[ "$EXPECT_ANALYSIS" == "true" ]]; then
      query_api_get "$QUERY_API_URL/sandboxes/$sandbox_id/analysis" "$analysis_file"
    else
      printf '{"data":null}' > "$analysis_file"
    fi
    EXPECT_ROLES="$EXPECT_ROLES" \
    EXPECT_RUNTIME_TYPE="$EXPECT_RUNTIME_TYPE" \
    EXPECT_PARENT_LINKS="$EXPECT_PARENT_LINKS" \
    EXPECT_ANALYSIS="$EXPECT_ANALYSIS" \
    python3 - "$trace_file" "$metrics_file" "$analysis_file" "$sandbox_id" <<'PY'
import json, os, sys
trace_path, metrics_path, analysis_path, sandbox_id = sys.argv[1:5]
expect_roles = {item.strip().lower() for item in os.environ.get('EXPECT_ROLES', '').split(',') if item.strip()}
expect_runtime = os.environ.get('EXPECT_RUNTIME_TYPE', '').strip().lower()
expect_parent_links = os.environ.get('EXPECT_PARENT_LINKS', '').lower() == 'true'
expect_analysis = os.environ.get('EXPECT_ANALYSIS', '').lower() == 'true'
with open(trace_path, encoding='utf-8') as handle:
    traces = json.load(handle).get('data') or []
with open(metrics_path, encoding='utf-8') as handle:
    metrics = json.load(handle).get('data') or []
with open(analysis_path, encoding='utf-8') as handle:
    analysis = json.load(handle).get('data')
if not traces:
    raise SystemExit(f'Query API returned no traces for {sandbox_id}')
if not metrics:
    raise SystemExit(f'Query API returned no metrics for {sandbox_id}')
span_names = {str(span.get('spanName') or '') for span in traces}
if 'sandbox.startup.callchain' not in span_names:
    raise SystemExit(f'missing sandbox.startup.callchain span for {sandbox_id}: {sorted(span_names)}')
roles = {str((span.get('attributes') or {}).get('process.role') or '').lower() for span in traces}
roles.discard('')
missing_roles = sorted(expect_roles - roles)
if missing_roles:
    raise SystemExit(f'Query API missing roles {missing_roles} for {sandbox_id}, observed {sorted(roles)}')
runtime_types = {str(span.get('runtimeType') or '').lower() for span in traces if span.get('runtimeType')}
if expect_runtime and expect_runtime not in runtime_types:
    raise SystemExit(f'Query API expected runtime {expect_runtime} for {sandbox_id}, observed {sorted(runtime_types)}')
if expect_parent_links and not any(span.get('parentSpanId') for span in traces):
    raise SystemExit(f'Query API returned no parentSpanId links for {sandbox_id}')
metric_names = {series.get('name') for series in metrics}
if 'sandbox.startup.callchain_duration_ms' not in metric_names:
    raise SystemExit(f'missing sandbox.startup.callchain_duration_ms metric for {sandbox_id}')
finding_count = 0
if expect_analysis:
    findings = (analysis or {}).get('findings') or []
    finding_count = len(findings)
    if finding_count <= 0:
        raise SystemExit(f'Query API returned no analysis findings for {sandbox_id}')
print(json.dumps({
    'sandboxId': sandbox_id,
    'traces': len(traces),
    'metrics': len(metrics),
    'roles': sorted(roles),
    'runtimeTypes': sorted(runtime_types),
    'parentLinks': sum(1 for span in traces if span.get('parentSpanId')),
    'analysisFindings': finding_count,
}, separators=(',', ':')))
PY
  done < <(query_api_sandboxes)
}

if [[ "${CAPTURE:-false}" == "true" ]]; then
  capture_probe
else
  require_file "$REPORT_PATH"
fi

validate_report_shape
if [[ "$VALIDATE_INGEST" == "true" || "$VALIDATE_QUERY_API" == "true" ]]; then
  normalize_report
  validate_collector_output
else
  echo "report shape validated; set VALIDATE_INGEST=true to send through host-startup-callchain" >&2
fi

if [[ "$VALIDATE_QUERY_API" == "true" ]]; then
  validate_query_api_output
fi
