const ingestUrl = process.env.INGEST_URL ?? 'http://runtimepulse-query-api:8081/api/ingest/batch';
const nodeId = process.env.COLLECTOR_NODE_ID ?? 'node-a';
const intervalMs = Number(process.env.COLLECTOR_INTERVAL_MS ?? 15_000);
const startSequence = Number(process.env.COLLECTOR_START_SEQUENCE ?? 0);
const source = `mock-node-collector/${nodeId}`;

let sequence = Number.isFinite(startSequence) && startSequence >= 0 ? Math.floor(startSequence) : 0;

await sendLoop();
setInterval(() => {
  sendLoop().catch((error) => {
    console.error(JSON.stringify({
      level: 'error',
      message: 'collector_tick_failed',
      error: error instanceof Error ? error.message : String(error),
    }));
  });
}, Number.isFinite(intervalMs) && intervalMs > 0 ? intervalMs : 15_000);

async function sendLoop() {
  const batch = buildBatch(new Date(), sequence++);
  const response = await fetch(ingestUrl, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(batch),
  });

  const body = await response.text();
  if (!response.ok) {
    throw new Error(`ingest failed with ${response.status}: ${body}`);
  }

  console.log(JSON.stringify({
    level: 'info',
    message: 'ingest_batch_accepted',
    status: response.status,
    sequence: sequence - 1,
    nodeId,
    body: safeJson(body),
  }));
}

function buildBatch(now, index) {
  const timestamp = now.toISOString();
  const sandboxId = `collector-${nodeId}-${String(index % 12).padStart(3, '0')}`;
  const runtimeType = index % 5 === 0 ? 'kata' : index % 3 === 0 ? 'gvisor' : 'runc';
  const imageId = runtimeType === 'kata' ? 'img-worker' : 'img-api';
  const imageRef = runtimeType === 'kata' ? 'registry.local/worker:v17' : 'registry.local/api:v42';
  const containerCount = 18 + (index % 7) + (runtimeType === 'kata' ? 2 : 0);
  const ioPressure = Number((0.08 + Math.sin(index / 3) * 0.04 + (runtimeType === 'kata' ? 0.12 : 0)).toFixed(3));
  const startupDurationMs = runtimeType === 'kata' ? 6400 + (index % 4) * 900 : runtimeType === 'gvisor' ? 2800 + (index % 3) * 360 : 940 + (index % 5) * 110;
  const cpuAvg = runtimeType === 'gvisor' ? 0.28 : runtimeType === 'kata' ? 0.18 : 0.11;
  const memoryPeakBytes = runtimeType === 'kata' ? 1.1 * 1024 ** 3 : runtimeType === 'gvisor' ? 780 * 1024 ** 2 : 360 * 1024 ** 2;
  const ioReadBytes = runtimeType === 'kata' ? 38 * 1024 ** 2 : runtimeType === 'gvisor' ? 22 * 1024 ** 2 : 9 * 1024 ** 2;
  const networkRxBytes = runtimeType === 'gvisor' ? 11 * 1024 ** 2 : 5 * 1024 ** 2;
  const createdAt = new Date(now.getTime() - startupDurationMs).toISOString();
  const phaseDurations = lifecyclePhases(runtimeType, startupDurationMs);

  return {
    source,
    observedAt: timestamp,
    metadata: {
      nodes: [
        {
          id: nodeId,
          clusterId: 'cluster-prod',
          name: `rp-${nodeId}`,
          status: ioPressure > 0.18 ? 'degraded' : 'ready',
          labels: { collector: 'mock-node-collector' },
        },
      ],
      sandboxes: [
        {
          id: sandboxId,
          clusterId: 'cluster-prod',
          nodeId,
          namespace: 'collector',
          workloadId: `collector-smoke-${runtimeType}`,
          runtimeType,
          runtimeVersion: runtimeVersion(runtimeType),
          imageId,
          imageRef,
          workloadName: `collector-smoke-${runtimeType}`,
          status: 'running',
          createdAt,
          startedAt: timestamp,
          startupDurationMs,
          cpuAvg,
          memoryPeakBytes,
          labels: { app: 'collector-smoke', runtime: runtimeType },
          attributes: { source },
        },
      ],
    },
    metrics: [
      metric(timestamp, 'node.container.count', containerCount, 'count', 'lifecycle', { nodeId }),
      metric(timestamp, 'node.psi.io.some', ioPressure, 'ratio', 'pressure', { nodeId, sandboxId, runtimeType }),
      metric(timestamp, 'sandbox.cpu.usage_ratio', cpuAvg, 'ratio', 'cpu', { nodeId, sandboxId, runtimeType }),
      metric(timestamp, 'sandbox.memory.working_set_bytes', memoryPeakBytes, 'bytes', 'memory', { nodeId, sandboxId, runtimeType }),
      metric(timestamp, 'sandbox.io.read_bytes', ioReadBytes, 'bytes/s', 'io', { nodeId, sandboxId, runtimeType }),
      metric(timestamp, 'sandbox.network.rx_bytes', networkRxBytes, 'bytes/s', 'network', { nodeId, sandboxId, runtimeType }),
      metric(timestamp, 'sandbox.startup.duration_ms', startupDurationMs, 'ms', 'startup', { nodeId, sandboxId, runtimeType }),
      ...phaseDurations.map((phase) => metric(timestamp, `sandbox.lifecycle.${phase.name}.duration_ms`, phase.durationMs, 'ms', 'lifecycle', {
        nodeId,
        sandboxId,
        runtimeType,
        attributes: { stageName: phase.name },
      })),
    ],
    events: [
      {
        id: `${sandboxId}-created-${index}`,
        timestamp,
        severity: ioPressure > 0.18 ? 'warning' : 'info',
        eventType: 'lifecycle',
        eventName: 'sandbox.created',
        sandboxId,
        nodeId,
        runtimeType,
        message: `Mock collector observed ${runtimeType} sandbox startup`,
        source,
        attributes: { containerCount, ioPressure },
      },
    ],
    traces: traceSpans(timestamp, sandboxId, runtimeType, phaseDurations),
    profiles: profileArtifacts(timestamp, sandboxId, runtimeType, index, startupDurationMs),
  };
}

function lifecyclePhases(runtimeType, totalMs) {
  const imagePullRatio = runtimeType === 'runc' ? 0.18 : 0.22;
  const snapshotRatio = runtimeType === 'gvisor' ? 0.16 : 0.1;
  const runtimeCreateRatio = runtimeType === 'kata' ? 0.18 : 0.24;
  const vmBootRatio = runtimeType === 'kata' ? 0.32 : 0.05;

  return [
    { name: 'image.pull', durationMs: Math.round(totalMs * imagePullRatio) },
    { name: 'snapshot.prepare', durationMs: Math.round(totalMs * snapshotRatio) },
    { name: 'runtime.create', durationMs: Math.round(totalMs * runtimeCreateRatio) },
    { name: 'microvm.boot', durationMs: Math.round(totalMs * vmBootRatio) },
    { name: 'container.start', durationMs: Math.max(1, Math.round(totalMs * (1 - imagePullRatio - snapshotRatio - runtimeCreateRatio - vmBootRatio))) },
  ];
}

function traceSpans(timestamp, sandboxId, runtimeType, phases) {
  let cursor = Date.parse(timestamp);
  return phases.map((phase, index) => {
    const startTime = new Date(cursor).toISOString();
    cursor += phase.durationMs;
    const endTime = new Date(cursor).toISOString();

    return {
      traceId: `trace-${sandboxId}`,
      spanId: `span-${index + 1}`,
      parentSpanId: index === 0 ? undefined : 'span-1',
      sandboxId,
      spanName: phase.name,
      startTime,
      endTime,
      durationMs: phase.durationMs,
      status: 'ok',
      attributes: { runtimeType, collector: source },
    };
  });
}

function runtimeVersion(runtimeType) {
  if (runtimeType === 'gvisor') return 'runsc-collector';
  if (runtimeType === 'kata') return 'kata-collector';
  return 'runc-collector';
}

function profileArtifacts(timestamp, sandboxId, runtimeType, index, startupDurationMs) {
  const profileType = index % 4 === 0 ? 'block_io' : runtimeType === 'kata' ? 'off_cpu' : 'cpu';
  const sampleCount = profileType === 'block_io'
    ? 300 + (index % 9) * 24
    : runtimeType === 'kata'
      ? 620 + (index % 6) * 55
      : 420 + (index % 7) * 31;

  return [
    {
      id: `${sandboxId}-${profileType}-${index}`,
      timestamp,
      sandboxId,
      profileType,
      processRole: runtimeType === 'kata' ? 'shim-v2' : 'container-init',
      durationMs: Math.max(500, Math.round(startupDurationMs * 0.6)),
      sampleCount,
      objectUri: `s3://runtimepulse/mock-profiles/${nodeId}/${sandboxId}/${profileType}.pprof`,
    },
  ];
}

function metric(timestamp, name, value, unit, group, dimensions) {
  return {
    timestamp,
    name,
    value,
    unit,
    group,
    ...dimensions,
  };
}

function safeJson(body) {
  try {
    return JSON.parse(body);
  } catch {
    return body;
  }
}
