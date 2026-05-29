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
CONCURRENT_RUNPODS="${CONCURRENT_RUNPODS:-1}"
CRI_RUNTIME_HANDLER="${CRI_RUNTIME_HANDLER:-$RUNTIME_TYPE}"
CLEANUP_POD="${CLEANUP_POD:-true}"
EXPECT_RUNTIME_TYPE="${EXPECT_RUNTIME_TYPE:-$RUNTIME_TYPE}"
EXPECT_ROLES="${EXPECT_ROLES:-}"
EXPECT_SANDBOX_ID="${EXPECT_SANDBOX_ID:-}"
EXPECT_ANALYSIS="${EXPECT_ANALYSIS:-false}"
EXPECT_PARENT_LINKS="${EXPECT_PARENT_LINKS:-false}"
EXPECT_PROCESS_BINARY_METRICS="${EXPECT_PROCESS_BINARY_METRICS:-false}"
EXPECT_HELPER_BINARY_METRICS="${EXPECT_HELPER_BINARY_METRICS:-false}"
EXPECT_CNI_CONTAINER_ID="${EXPECT_CNI_CONTAINER_ID:-false}"
EXPECT_RUNPOD_REQUEST_IDENTITY="${EXPECT_RUNPOD_REQUEST_IDENTITY:-false}"
EXPECT_CNISETUP_DEBUG_PENDING="${EXPECT_CNISETUP_DEBUG_PENDING:-false}"
EXPECT_RUNTIME_BOUNDARY_CORRELATION="${EXPECT_RUNTIME_BOUNDARY_CORRELATION:-false}"
ENABLE_GO_UPROBES="${ENABLE_GO_UPROBES:-false}"
CONTAINERD_BINARY="${CONTAINERD_BINARY:-}"
CONTAINERD_CONFIG="${CONTAINERD_CONFIG:-${RUNTIMEPULSE_CONTAINERD_CONFIG:-}}"
GO_UPROBE_SYMBOLS="${GO_UPROBE_SYMBOLS:-}"
ENABLE_CNI_GO_UPROBES="${ENABLE_CNI_GO_UPROBES:-false}"
CNI_GO_UPROBE_SYMBOLS="${CNI_GO_UPROBE_SYMBOLS:-}"
ENABLE_RUNTIME_GO_UPROBES="${ENABLE_RUNTIME_GO_UPROBES:-false}"
RUNTIME_UPROBE_BINARIES="${RUNTIME_UPROBE_BINARIES:-}"
RUNTIME_GO_UPROBE_SYMBOLS="${RUNTIME_GO_UPROBE_SYMBOLS:-}"
ENABLE_GO_URETPROBES="${ENABLE_GO_URETPROBES:-false}"

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
  CONCURRENT_RUNPODS=3         Run this many POD_CONFIG variants during one capture.
  CRI_ENDPOINT=unix://...      Runtime endpoint used by crictl.
  CRI_RUNTIME_HANDLER=kata     Optional --runtime passed to crictl runp.
  CLEANUP_POD=true|false       Stop/remove the sandbox after automatic RunPod.
  EXPECT_RUNTIME_TYPE=kata     Optional report runtime assertion. Defaults to RUNTIME_TYPE.
  EXPECT_ROLES=cni,kata        Optional required enter-event roles.
  EXPECT_SANDBOX_ID=...        Optional required sandbox id; auto-filled from runp.
  EXPECT_PARENT_LINKS=true     Require at least one non-root trace span parent link.
  EXPECT_PROCESS_BINARY_METRICS=true Require per-process-binary startup metrics.
  EXPECT_HELPER_BINARY_METRICS=true Require helper process-binary metrics.
  EXPECT_CNI_CONTAINER_ID=true Require CNI_CONTAINERID/CNI_ARGS infra id matches report sandbox id.
  EXPECT_RUNPOD_REQUEST_IDENTITY=true Require RunPodSandbox uprobe identity decoded and matched to sandbox.
  EXPECT_CNISETUP_DEBUG_PENDING=true Require CNISetup uprobes remain pending debug boundaries in concurrent reports.
  EXPECT_RUNTIME_BOUNDARY_CORRELATION=true Require OCI/Kata exec/uprobe boundaries to carry exact correlation markers.
  EXPECT_ANALYSIS=true         Require Query API analysis findings for each sandbox.
  VALIDATE_INGEST=true         Also send normalized output to LOCAL_REPORT_URL.
  VALIDATE_QUERY_API=true      Verify sandbox trace/metrics via QUERY_API_URL.
  LOCAL_REPORT_URL=http://127.0.0.1:9091/api/local/ingest
  QUERY_API_URL=http://127.0.0.1:8081/api
  QUERY_API_RETRY_SECONDS=10   Wait for collector outlet to flush to Query API.
  ENABLE_GO_UPROBES=true       Attach containerd RunPodSandbox Go uprobes.
  CONTAINERD_BINARY=/usr/bin/containerd  Optional containerd binary for symbol discovery.
  CONTAINERD_CONFIG=/etc/containerd/config.toml Optional config for task bundle root inference.
  GO_UPROBE_SYMBOLS=pattern    Optional comma-separated RunPodSandbox Go symbol/pattern list.
  ENABLE_CNI_GO_UPROBES=true   Also attach containerd/go-cni setup Go uprobes.
  CNI_GO_UPROBE_SYMBOLS=pattern Optional comma-separated CNI setup Go symbol/pattern list.
  ENABLE_RUNTIME_GO_UPROBES=true Also attach OCI/Kata runtime shim Go uprobes.
  RUNTIME_UPROBE_BINARIES=/usr/bin/containerd-shim-runc-v2 Optional comma-separated runtime shim binaries.
  RUNTIME_GO_UPROBE_SYMBOLS=pattern Optional comma-separated runtime Go symbol/pattern list.
  ENABLE_GO_URETPROBES=true Experimental: attach Go return probes for real exits.
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
  while IFS= read -r sandbox_id; do
    [[ -n "$sandbox_id" ]] || continue
    run_crictl stopp "$sandbox_id" >/dev/null 2>&1 || true
    run_crictl rmp "$sandbox_id" >/dev/null 2>&1 || true
  done < "$SANDBOX_ID_FILE"
}

pod_config_variant() {
  local source="$1"
  local index="$2"
  if [[ "$index" -le 1 ]]; then
    printf '%s' "$source"
    return 0
  fi
  local target="$OUT_DIR/pod-config-$index.json"
  python3 - "$source" "$target" "$index" <<'PY'
import json, sys
source, target, index = sys.argv[1:4]
with open(source, encoding='utf-8') as handle:
    pod = json.load(handle)
suffix = f"concurrent-{index}"
metadata = pod.setdefault('metadata', {})
base_name = metadata.get('name') or 'runtimepulse-runpod'
metadata['name'] = f"{base_name}-{suffix}"
metadata['uid'] = f"{metadata.get('uid') or base_name}-{suffix}"
metadata['attempt'] = int(metadata.get('attempt') or 1)
pod['log_directory'] = f"{pod.get('log_directory') or '/tmp/runtimepulse-runpod'}/{suffix}"
labels = pod.setdefault('labels', {})
labels['io.kubernetes.pod.name'] = metadata['name']
labels['io.kubernetes.pod.uid'] = metadata['uid']
with open(target, 'w', encoding='utf-8') as handle:
    json.dump(pod, handle)
PY
  printf '%s' "$target"
}

run_one_pod_sandbox() {
  local config="$1"
  local index="$2"
  local log_file="$OUT_DIR/crictl-runp-$index.log"
  local runtime_args=()
  if [[ -n "$CRI_RUNTIME_HANDLER" ]]; then
    runtime_args+=(--runtime "$CRI_RUNTIME_HANDLER")
  fi

  echo "running crictl runp[$index]: $config" >&2
  set +e
  run_crictl runp "${runtime_args[@]}" "$config" > "$log_file" 2>&1
  local rc=$?
  set -e
  if [[ $rc -ne 0 ]]; then
    cat "$log_file" >&2 || true
    return $rc
  fi
  local sandbox_id
  sandbox_id="$(tail -n 1 "$log_file" | tr -d '[:space:]')"
  if [[ -z "$sandbox_id" ]]; then
    echo "crictl runp[$index] did not return a sandbox id" >&2
    cat "$log_file" >&2 || true
    return 1
  fi
  echo "$sandbox_id" >> "$SANDBOX_ID_FILE"
  echo "created sandbox[$index]: $sandbox_id" >&2
}

run_pod_sandbox() {
  [[ -n "$POD_CONFIG" ]] || return 0
  require_file "$POD_CONFIG"
  if ! command -v crictl >/dev/null 2>&1; then
    echo "crictl not found; install crictl or unset POD_CONFIG" >&2
    exit 1
  fi

  local count="$CONCURRENT_RUNPODS"
  if ! [[ "$count" =~ ^[0-9]+$ ]] || [[ "$count" -lt 1 ]]; then
    echo "CONCURRENT_RUNPODS must be a positive integer: $CONCURRENT_RUNPODS" >&2
    exit 1
  fi

  : > "$SANDBOX_ID_FILE"
  : > "$RUNP_LOG"
  local pids=()
  local idx config
  for idx in $(seq 1 "$count"); do
    config="$(pod_config_variant "$POD_CONFIG" "$idx")"
    ( run_one_pod_sandbox "$config" "$idx" ) &
    pids+=("$!")
  done

  local rc=0
  for pid in "${pids[@]}"; do
    if ! wait "$pid"; then
      rc=1
    fi
  done
  cat "$OUT_DIR"/crictl-runp-*.log > "$RUNP_LOG" 2>/dev/null || true
  if [[ $rc -ne 0 ]]; then
    exit $rc
  fi
  if [[ ! -s "$SANDBOX_ID_FILE" ]]; then
    echo "crictl runp did not create any sandboxes" >&2
    exit 1
  fi
  if [[ -z "$EXPECT_SANDBOX_ID" && "$count" -eq 1 ]]; then
    EXPECT_SANDBOX_ID="$(head -n 1 "$SANDBOX_ID_FILE")"
    export EXPECT_SANDBOX_ID
  fi
}

capture_probe() {
  local helper_args=()
  local runtime_args=()
  local uprobe_args=()
  if [[ "$INCLUDE_HELPERS" == "true" ]]; then
    helper_args+=(--include-helpers)
  fi
  if [[ -n "$RUNTIME_TYPE" ]]; then
    runtime_args+=(--runtime-type "$RUNTIME_TYPE")
  fi
  if [[ "$ENABLE_GO_UPROBES" == "true" ]]; then
    uprobe_args+=(--enable-go-uprobes)
    if [[ -n "$CONTAINERD_BINARY" ]]; then
      uprobe_args+=(--containerd-binary "$CONTAINERD_BINARY")
    fi
    if [[ -n "$CONTAINERD_CONFIG" ]]; then
      uprobe_args+=(--containerd-config "$CONTAINERD_CONFIG")
    fi
    if [[ -n "$GO_UPROBE_SYMBOLS" ]]; then
      IFS=',' read -r -a _go_uprobe_symbols <<< "$GO_UPROBE_SYMBOLS"
      for _symbol in "${_go_uprobe_symbols[@]}"; do
        [[ -n "$_symbol" ]] && uprobe_args+=(--go-uprobe-symbol "$_symbol")
      done
    fi
    if [[ "$ENABLE_CNI_GO_UPROBES" == "true" ]]; then
      uprobe_args+=(--enable-cni-go-uprobes)
    fi
    if [[ -n "$CNI_GO_UPROBE_SYMBOLS" ]]; then
      IFS=',' read -r -a _cni_go_uprobe_symbols <<< "$CNI_GO_UPROBE_SYMBOLS"
      for _symbol in "${_cni_go_uprobe_symbols[@]}"; do
        [[ -n "$_symbol" ]] && uprobe_args+=(--cni-go-uprobe-symbol "$_symbol")
      done
    fi
    if [[ "$ENABLE_RUNTIME_GO_UPROBES" == "true" ]]; then
      uprobe_args+=(--enable-runtime-go-uprobes)
    fi
    if [[ -n "$RUNTIME_UPROBE_BINARIES" ]]; then
      IFS=',' read -r -a _runtime_uprobe_binaries <<< "$RUNTIME_UPROBE_BINARIES"
      for _binary in "${_runtime_uprobe_binaries[@]}"; do
        [[ -n "$_binary" ]] && uprobe_args+=(--runtime-uprobe-binary "$_binary")
      done
    fi
    if [[ -n "$RUNTIME_GO_UPROBE_SYMBOLS" ]]; then
      IFS=',' read -r -a _runtime_go_uprobe_symbols <<< "$RUNTIME_GO_UPROBE_SYMBOLS"
      for _symbol in "${_runtime_go_uprobe_symbols[@]}"; do
        [[ -n "$_symbol" ]] && uprobe_args+=(--runtime-go-uprobe-symbol "$_symbol")
      done
    fi
    if [[ "$ENABLE_GO_URETPROBES" == "true" ]]; then
      uprobe_args+=(--enable-go-uretprobes)
    fi
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
    "${uprobe_args[@]}" \
    > "$REPORT_PATH" &
  local probe_pid=$!

  for _ in {1..400}; do
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
  CONCURRENT_RUNPODS="$CONCURRENT_RUNPODS" \
  EXPECT_CNI_CONTAINER_ID="$EXPECT_CNI_CONTAINER_ID" \
  EXPECT_RUNPOD_REQUEST_IDENTITY="$EXPECT_RUNPOD_REQUEST_IDENTITY" \
  EXPECT_CNISETUP_DEBUG_PENDING="$EXPECT_CNISETUP_DEBUG_PENDING" \
  EXPECT_RUNTIME_BOUNDARY_CORRELATION="$EXPECT_RUNTIME_BOUNDARY_CORRELATION" \
  python3 - "$REPORT_PATH" <<'PY'
import json, os, sys
path = sys.argv[1]
expect_runtime = os.environ.get('EXPECT_RUNTIME_TYPE', '').strip().lower()
expect_roles = {item.strip().lower() for item in os.environ.get('EXPECT_ROLES', '').split(',') if item.strip()}
expect_sandbox = os.environ.get('EXPECT_SANDBOX_ID', '').strip()
expected_report_count = int(os.environ.get('CONCURRENT_RUNPODS', '1') or '1')
expect_cni_container_id = os.environ.get('EXPECT_CNI_CONTAINER_ID', '').lower() == 'true'
expect_runpod_request_identity = os.environ.get('EXPECT_RUNPOD_REQUEST_IDENTITY', '').lower() == 'true'
expect_cnisetup_debug_pending = os.environ.get('EXPECT_CNISETUP_DEBUG_PENDING', '').lower() == 'true'
expect_runtime_boundary_correlation = os.environ.get('EXPECT_RUNTIME_BOUNDARY_CORRELATION', '').lower() == 'true'

def parse_cni_args(value):
    result = {}
    for item in str(value or '').split(';'):
        if '=' not in item:
            continue
        key, val = item.split('=', 1)
        key = key.strip()
        if key:
            result[key] = val
    return result

def cni_identity_candidates(event):
    candidates = set()
    for key in ('cniContainerId', 'criSandboxId', 'containerdId', 'sandboxId'):
        value = str(event.get(key) or '').strip()
        if value:
            candidates.add(value)
    attrs = event.get('attributes') or {}
    if isinstance(attrs, dict):
        for key in ('cni.container_id', 'cni.args.K8S_POD_INFRA_CONTAINER_ID'):
            value = str(attrs.get(key) or '').strip()
            if value:
                candidates.add(value)
    env = event.get('env') or {}
    if isinstance(env, dict):
        value = str(env.get('CNI_CONTAINERID') or '').strip()
        if value:
            candidates.add(value)
        infra = parse_cni_args(env.get('CNI_ARGS', '')).get('K8S_POD_INFRA_CONTAINER_ID', '').strip()
        if infra:
            candidates.add(infra)
    return candidates
with open(path, encoding='utf-8') as handle:
    payload = json.load(handle)
reports = payload.get('reports') if isinstance(payload, dict) else None
if reports is None:
    reports = [payload]
if not reports:
    raise SystemExit('no reports found')
if expected_report_count > 1 and len(reports) < expected_report_count:
    raise SystemExit(f'expected at least {expected_report_count} reports for concurrent runpods, observed {len(reports)}')
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
    sandbox = str(sandbox)
    sandboxes.append(sandbox)
    event_ids = set()
    for event in events:
        for key in ('sandboxId', 'criSandboxId', 'containerdId'):
            value = str(event.get(key) or '').strip()
            if value:
                event_ids.add(value)
    if sandbox not in event_ids:
        raise SystemExit(f'report {idx} sandbox id {sandbox} is not present in event identities {sorted(event_ids)}')
    foreign_ids = sorted(event_ids - {sandbox})
    if foreign_ids:
        raise SystemExit(f'report {idx} for sandbox {sandbox} contains foreign sandbox/container ids {foreign_ids}')
    report_runtime = str(report.get('runtimeType') or '').lower()
    if report_runtime:
        runtime_types.add(report_runtime)
    roles = {str(event.get('role') or '').lower() for event in events if event.get('eventType') == 'enter'}
    roles.discard('')
    all_roles |= roles
    runtime_types |= {str(event.get('runtimeType') or '').lower() for event in events if event.get('runtimeType')}
    if not roles & {'cni', 'oci', 'kata', 'helper'}:
        raise SystemExit(f'report {idx} has no startup roles: {sorted(roles)}')
    if expect_cni_container_id:
        matching_cni = []
        mismatched_cni = []
        for event in events:
            if str(event.get('role') or '').lower() != 'cni':
                continue
            candidates = cni_identity_candidates(event)
            if sandbox in candidates:
                matching_cni.append(event)
            elif candidates:
                mismatched_cni.append(sorted(candidates))
        if not matching_cni:
            raise SystemExit(
                f'report {idx} sandbox {sandbox} has no CNI event with CNI_CONTAINERID/'
                f'CNI_ARGS infra id matching sandbox; observed CNI ids {mismatched_cni}'
            )
    if expect_runpod_request_identity:
        matching_runpod = []
        decoded_unmatched = []
        for event in events:
            if str(event.get('function') or '') != 'RunPodSandbox':
                continue
            attrs = event.get('attributes') or {}
            if not isinstance(attrs, dict):
                attrs = {}
            if attrs.get('startup.probe.req_identity') != 'runpod-request':
                continue
            pod_identity = (attrs.get('k8s.namespace'), attrs.get('k8s.pod'), attrs.get('k8s.pod_uid'))
            if str(event.get('sandboxId') or '') == sandbox and all(pod_identity[:2]):
                matching_runpod.append(event)
            else:
                decoded_unmatched.append({
                    'sandboxId': event.get('sandboxId'),
                    'correlation': attrs.get('startup.probe.correlation'),
                    'pod': pod_identity,
                })
        if not matching_runpod:
            raise SystemExit(
                f'report {idx} sandbox {sandbox} has no RunPodSandbox event with decoded request identity; '
                f'observed decoded RunPod events {decoded_unmatched}'
            )
    if expect_cnisetup_debug_pending:
        assigned_cnisetup = [
            event for event in events
            if str(event.get('function') or '') == 'CNISetup'
        ]
        pending_functions = {str(item) for item in (report.get('summary') or {}).get('pendingGoUprobeFunctions') or []}
        pending_debug_count = float((report.get('summary') or {}).get('pendingDebugGoUprobeEventCount') or 0.0)
        if assigned_cnisetup:
            raise SystemExit(
                f'report {idx} sandbox {sandbox} has assigned CNISetup uprobe events; '
                'CNISetup is debug-only and should stay pending in concurrent captures'
            )
        if 'CNISetup' not in pending_functions or pending_debug_count <= 0:
            raise SystemExit(
                f'report {idx} sandbox {sandbox} missing pending debug CNISetup quality markers; '
                f'functions={sorted(pending_functions)} pendingDebug={pending_debug_count}'
            )
    if expect_runtime_boundary_correlation:
        runtime_events = [
            event for event in events
            if event.get('eventType') == 'enter'
            and str(event.get('role') or '').lower() in {'oci', 'kata'}
        ]
        if not runtime_events:
            raise SystemExit(f'report {idx} sandbox {sandbox} has no OCI/Kata runtime boundary enter events')
        missing = []
        for event in runtime_events:
            attrs = event.get('attributes') or {}
            if not isinstance(attrs, dict):
                attrs = {}
            corr = str(attrs.get('startup.probe.correlation') or '').strip()
            corr_id = str(attrs.get('startup.probe.correlation_sandbox_id') or '').strip()
            legacy_exact = str(event.get('sandboxId') or '').strip() == sandbox and str(event.get('containerdId') or '').strip() == sandbox
            if not corr or corr_id != sandbox:
                if legacy_exact and str(event.get('function') or '') == 'execve':
                    attrs['startup.probe.correlation'] = 'legacy-containerd-task-id'
                    attrs['startup.probe.correlation_sandbox_id'] = sandbox
                    event['attributes'] = attrs
                else:
                    missing.append({
                        'function': event.get('function'),
                        'binary': event.get('binary'),
                        'role': event.get('role'),
                        'correlation': corr,
                        'correlationSandboxId': corr_id,
                    })
        if missing:
            raise SystemExit(
                f'report {idx} sandbox {sandbox} has runtime boundary events without exact correlation: {missing[:8]}'
            )
if expected_report_count > 1 and len(set(sandboxes)) != len(sandboxes):
    raise SystemExit(f'concurrent reports contain duplicate sandbox ids: {sandboxes}')
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
    'validatedCniContainerId': expect_cni_container_id,
    'validatedRunPodRequestIdentity': expect_runpod_request_identity,
    'validatedCniSetupDebugPending': expect_cnisetup_debug_pending,
    'validatedRuntimeBoundaryCorrelation': expect_runtime_boundary_correlation,
}, separators=(',', ':')))
PY
}

validate_collector_output() {
  EXPECT_PROCESS_BINARY_METRICS="$EXPECT_PROCESS_BINARY_METRICS" \
  EXPECT_HELPER_BINARY_METRICS="$EXPECT_HELPER_BINARY_METRICS" \
  python3 - "$COLLECTOR_LOG" "$REPORT_PATH" <<'PY'
import json, os, sys
path, report_path = sys.argv[1:3]
expect_process_binary_metrics = os.environ.get('EXPECT_PROCESS_BINARY_METRICS', '').lower() == 'true'
expect_helper_binary_metrics = os.environ.get('EXPECT_HELPER_BINARY_METRICS', '').lower() == 'true'
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
if expect_process_binary_metrics or expect_helper_binary_metrics:
    with open(report_path, encoding='utf-8') as handle:
        payload = json.load(handle)
    reports = payload.get('reports') if isinstance(payload, dict) else None
    if reports is None:
        reports = [payload]
    process_binary_names = set()
    helper_binary_names = set()
    for report in reports:
        for event in report.get('events') or []:
            if event.get('eventType') != 'enter':
                continue
            binary = str(event.get('binary') or event.get('command') or '').split('/')[-1].split()[0]
            role = str(event.get('role') or '').lower()
            if binary:
                process_binary_names.add(binary)
            if role == 'helper' and binary:
                helper_binary_names.add(binary)
    if expect_process_binary_metrics and not process_binary_names:
        raise SystemExit('report has no process binaries to derive process-binary metrics')
    if expect_helper_binary_metrics and not helper_binary_names:
        raise SystemExit('report has no helper binaries to derive helper process-binary metrics')
    last = dict(last)
    last['validatedProcessBinaryInputs'] = sorted(process_binary_names)[:8] if expect_process_binary_metrics else []
    last['validatedHelperBinaryInputs'] = sorted(helper_binary_names)[:8] if expect_helper_binary_metrics else []
print(json.dumps(last, separators=(',', ':')))
PY
}

query_api_sandboxes() {
  python3 - "$REPORT_PATH" <<'PY'
import json, sys

def sanitize_id(value):
    out = []
    last_dash = False
    for ch in str(value or '').strip().lower():
        if ch.isalnum():
            out.append(ch)
            last_dash = False
        elif not last_dash:
            out.append('-')
            last_dash = True
    return ''.join(out).strip('-') or 'unknown'

def attrs_from_event(event):
    attrs = event.get('attributes') or {}
    return attrs if isinstance(attrs, dict) else {}

def first_report_value(report, keys):
    for key in keys:
        value = str(report.get(key) or '').strip()
        if value:
            return value
    for event in report.get('events') or []:
        attrs = attrs_from_event(event)
        for key in keys:
            value = str(event.get(key) or attrs.get(key) or '').strip()
            if value:
                return value
    return ''

def stable_query_sandbox_id(report):
    explicit_stable = first_report_value(report, ['startup.stable_sandbox_id'])
    if explicit_stable:
        return explicit_stable
    namespace = first_report_value(report, ['namespace', 'k8s_namespace', 'podNamespace', 'k8s.namespace', 'cni.args.K8S_POD_NAMESPACE'])
    pod = first_report_value(report, ['podName', 'pod_name', 'k8s.pod', 'cni.args.K8S_POD_NAME'])
    container = first_report_value(report, ['containerName', 'container_name', 'k8s.container']) or 'pod'
    if namespace and pod:
        return f'k8s-{sanitize_id(namespace)}-{sanitize_id(pod)}-{sanitize_id(container)}'
    return str(report.get('sandboxId') or report.get('criSandboxId') or '').strip()

with open(sys.argv[1], encoding='utf-8') as handle:
    payload = json.load(handle)
reports = payload.get('reports') if isinstance(payload, dict) else None
if reports is None:
    reports = [payload]
for report in reports:
    sandbox = stable_query_sandbox_id(report)
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
    EXPECT_PROCESS_BINARY_METRICS="$EXPECT_PROCESS_BINARY_METRICS" \
    EXPECT_HELPER_BINARY_METRICS="$EXPECT_HELPER_BINARY_METRICS" \
    EXPECT_ANALYSIS="$EXPECT_ANALYSIS" \
    python3 - "$trace_file" "$metrics_file" "$analysis_file" "$sandbox_id" <<'PY'
import json, os, sys
trace_path, metrics_path, analysis_path, sandbox_id = sys.argv[1:5]
expect_roles = {item.strip().lower() for item in os.environ.get('EXPECT_ROLES', '').split(',') if item.strip()}
expect_runtime = os.environ.get('EXPECT_RUNTIME_TYPE', '').strip().lower()
expect_parent_links = os.environ.get('EXPECT_PARENT_LINKS', '').lower() == 'true'
expect_analysis = os.environ.get('EXPECT_ANALYSIS', '').lower() == 'true'
expect_process_binary_metrics = os.environ.get('EXPECT_PROCESS_BINARY_METRICS', '').lower() == 'true'
expect_helper_binary_metrics = os.environ.get('EXPECT_HELPER_BINARY_METRICS', '').lower() == 'true'
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
if expect_process_binary_metrics and not any(str(name or '').startswith('sandbox.startup.process.binary.') for name in metric_names):
    raise SystemExit(f'missing process binary metrics for {sandbox_id}')
if expect_helper_binary_metrics:
    helper_metrics = [
        series for series in metrics
        if str(series.get('name') or '').startswith('sandbox.startup.process.binary.')
        and 'helper' in ((series.get('attributes') or {}).get('process.roles') or [])
    ]
    if not helper_metrics:
        raise SystemExit(f'missing helper process binary metrics for {sandbox_id}')
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
    'processBinaryMetrics': sum(1 for name in metric_names if str(name or '').startswith('sandbox.startup.process.binary.')),
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
