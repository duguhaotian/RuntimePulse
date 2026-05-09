import type {
  EventRecord,
  Image,
  MetricSeries,
  Node,
  ProfileArtifact,
  RuntimeCompareRow,
  RuntimeType,
  Sandbox,
  TraceSpan,
} from '../domain/model';
import { minutesAgo } from '../utils/time';

export const nodes: Node[] = [
  { id: 'node-a', name: 'rp-node-a', clusterId: 'cluster-prod', kernelVersion: '6.8.0', cpuCores: 64, memoryBytes: 256 * 1024 ** 3, status: 'ready' },
  { id: 'node-b', name: 'rp-node-b', clusterId: 'cluster-prod', kernelVersion: '6.8.0', cpuCores: 48, memoryBytes: 192 * 1024 ** 3, status: 'degraded' },
  { id: 'node-c', name: 'rp-node-c', clusterId: 'cluster-prod', kernelVersion: '5.15.0', cpuCores: 32, memoryBytes: 128 * 1024 ** 3, status: 'ready' },
];

export const images: Image[] = [
  { id: 'img-api', ref: 'registry.local/api:v42', digest: 'sha256:api42', sizeBytes: 810 * 1024 ** 2, layerCount: 18 },
  { id: 'img-ml-heavy', ref: 'registry.local/ml-heavy:v8', digest: 'sha256:mlheavy8', sizeBytes: 4.6 * 1024 ** 3, layerCount: 91 },
  { id: 'img-worker', ref: 'registry.local/worker:v17', digest: 'sha256:worker17', sizeBytes: 1.4 * 1024 ** 3, layerCount: 34 },
  { id: 'img-edge', ref: 'registry.local/edge-proxy:v5', digest: 'sha256:edge5', sizeBytes: 380 * 1024 ** 2, layerCount: 12 },
];

export const sandboxes: Sandbox[] = [
  sandbox('sb-runc-001', 'runc', 'node-a', 'img-api', 'api-server', 'running', 980, 0.16, 620 * 1024 ** 2, 2),
  sandbox('sb-gvisor-014', 'gvisor', 'node-a', 'img-api', 'checkout-api', 'running', 2860, 0.42, 950 * 1024 ** 2, 5, { platform: 'ptrace', scenario: 'sentry cpu overhead' }),
  sandbox('sb-kata-021', 'kata', 'node-b', 'img-worker', 'batch-worker', 'running', 7420, 0.31, 1.8 * 1024 ** 3, 8, { hypervisor: 'qemu', scenario: 'microvm boot slow' }),
  sandbox('sb-fire-033', 'firecracker', 'node-b', 'img-edge', 'edge-proxy', 'failed', 31_200, 0.09, 420 * 1024 ** 2, 11, { scenario: 'guest agent timeout' }),
  sandbox('sb-kata-044', 'kata', 'node-c', 'img-ml-heavy', 'model-loader', 'running', 18_900, 0.56, 5.8 * 1024 ** 3, 9, { scenario: 'large image unpack slow' }),
  sandbox('sb-gvisor-052', 'gvisor', 'node-b', 'img-worker', 'job-runner', 'running', 5100, 0.68, 1.3 * 1024 ** 3, 7, { scenario: 'node io pressure' }),
  sandbox('sb-runc-063', 'runc', 'node-c', 'img-edge', 'edge-proxy', 'stopped', 1120, 0.13, 300 * 1024 ** 2, 3),
  sandbox('sb-fire-071', 'firecracker', 'node-a', 'img-worker', 'secure-job', 'running', 6840, 0.23, 1.2 * 1024 ** 3, 4),
];

export function metricsForSandbox(sandboxId: string): MetricSeries[] {
  const target = sandboxes.find((item) => item.id === sandboxId) ?? sandboxes[0];
  const slow = target.startupDurationMs > 7000;
  const pressure = target.nodeId === 'node-b';
  const runtimeBoost = target.runtimeType === 'gvisor' ? 0.18 : target.runtimeType === 'kata' ? 0.11 : 0.06;

  return [
    series(target, 'sandbox.cpu.usage_ratio', 'Sandbox CPU', 'ratio', 'cpu', target.cpuAvg, runtimeBoost, slow ? 0.16 : 0.08),
    series(target, 'runtime.process.cpu.usage_ratio', 'Runtime Process CPU', 'ratio', 'runtime', runtimeBoost, 0.04, target.runtimeType === 'gvisor' ? 0.22 : 0.08),
    series(target, 'sandbox.memory.working_set_bytes', 'Working Set', 'bytes', 'memory', target.memoryPeakBytes * 0.72, target.memoryPeakBytes * 0.05, target.memoryPeakBytes * 0.1),
    series(target, 'sandbox.io.read_bytes', 'IO Read Throughput', 'bytes/s', 'io', slow ? 72 * 1024 ** 2 : 14 * 1024 ** 2, 4 * 1024 ** 2, pressure ? 52 * 1024 ** 2 : 8 * 1024 ** 2),
    series(target, 'sandbox.network.rx_bytes', 'Network RX', 'bytes/s', 'network', 9 * 1024 ** 2, 3 * 1024 ** 2, target.runtimeType === 'gvisor' ? 8 * 1024 ** 2 : 2 * 1024 ** 2),
    series(target, 'node.psi.io.some', 'Node IO Pressure', 'ratio', 'pressure', pressure ? 0.41 : 0.08, 0.04, pressure ? 0.38 : 0.08),
  ];
}

export function eventsForSandbox(sandboxId: string): EventRecord[] {
  const target = sandboxes.find((item) => item.id === sandboxId) ?? sandboxes[0];
  const base = new Date(target.createdAt).getTime();
  const event = (offsetMs: number, severity: EventRecord['severity'], eventName: string, message: string, attributes: Record<string, unknown> = {}): EventRecord => ({
    id: `${target.id}-${eventName}-${offsetMs}`,
    timestamp: new Date(base + offsetMs).toISOString(),
    severity,
    eventType: eventName.split('.')[0] ?? 'runtime',
    eventName,
    sandboxId: target.id,
    nodeId: target.nodeId,
    runtimeType: target.runtimeType,
    reason: severity === 'error' ? 'RuntimeFailure' : severity === 'warning' ? 'SlowOperation' : undefined,
    message,
    source: `${target.runtimeType}-collector`,
    attributes,
  });

  const rows = [
    event(0, 'info', 'sandbox.created', 'Sandbox metadata observed'),
    event(120, 'info', 'image.pull.started', 'Image pull started'),
    event(Math.max(500, target.startupDurationMs * 0.16), 'info', 'image.pull.finished', 'Image pull finished'),
    event(Math.max(900, target.startupDurationMs * 0.18), 'info', 'image.unpack.started', 'Image unpack started'),
    event(Math.max(1300, target.startupDurationMs * 0.56), target.imageId === 'img-ml-heavy' ? 'warning' : 'info', 'image.unpack.finished', 'Image unpack finished', { layerCount: imageById(target.imageId).layerCount }),
    event(Math.max(1500, target.startupDurationMs * 0.62), 'info', 'runtime.create.started', 'Runtime create started'),
    event(Math.max(1800, target.startupDurationMs * 0.84), target.runtimeType === 'firecracker' && target.status === 'failed' ? 'error' : 'info', `${target.runtimeType}.ready`, `${target.runtimeType} runtime ready`),
  ];

  if (target.runtimeType === 'gvisor') {
    rows.push(event(target.startupDurationMs + 80_000, 'warning', 'gvisor.sentry.cpu.high', 'gVisor sentry CPU stayed above baseline', { sentryCpu: '38%' }));
  }
  if (target.nodeId === 'node-b') {
    rows.push(event(target.startupDurationMs + 25_000, 'warning', 'node.io.pressure.high', 'Node IO pressure is elevated', { psiSome: '0.78' }));
  }
  if (target.status === 'failed') {
    rows.push(event(target.startupDurationMs, 'error', 'microvm.guest_agent.timeout', 'Guest agent did not become ready within timeout', { timeoutMs: 30_000 }));
  } else {
    rows.push(event(target.startupDurationMs, 'info', 'container.started', 'Container process started'));
  }

  return rows.sort((left, right) => Date.parse(left.timestamp) - Date.parse(right.timestamp));
}

export function traceForSandbox(sandboxId: string): TraceSpan[] {
  const target = sandboxes.find((item) => item.id === sandboxId) ?? sandboxes[0];
  const start = new Date(target.createdAt).getTime();
  const total = target.startupDurationMs;
  const image = imageById(target.imageId);
  const pull = image.sizeBytes > 2 * 1024 ** 3 ? total * 0.18 : total * 0.14;
  const unpack = image.layerCount > 60 ? total * 0.52 : total * 0.22;
  const vmBoot = target.runtimeType === 'kata' || target.runtimeType === 'firecracker' ? total * 0.28 : total * 0.06;
  const spans: Array<[string, number, number, TraceSpan['status']]> = [
    ['image.pull', 0, pull, 'ok'],
    ['image.unpack', pull, pull + unpack, 'ok'],
    ['snapshot.prepare', pull + unpack, pull + unpack + total * 0.08, 'ok'],
    ['runtime.create', total * 0.54, total * 0.68, 'ok'],
    ['microvm.boot', total * 0.62, total * 0.62 + vmBoot, target.status === 'failed' ? 'error' : 'ok'],
    ['guest.agent.ready', total * 0.78, total * 0.9, target.status === 'failed' ? 'error' : 'ok'],
    ['container.start', total * 0.88, total, target.status === 'failed' ? 'error' : 'ok'],
  ];

  return spans.map(([name, spanStart, spanEnd, status], index) => ({
    traceId: `trace-${target.id}`,
    spanId: `span-${index + 1}`,
    parentSpanId: index === 0 ? undefined : 'span-1',
    sandboxId: target.id,
    spanName: name,
    startTime: new Date(start + spanStart).toISOString(),
    endTime: new Date(start + spanEnd).toISOString(),
    durationMs: Math.max(1, spanEnd - spanStart),
    status,
    attributes: { runtimeType: target.runtimeType, imageRef: target.imageRef },
  }));
}

export function profilesForSandbox(sandboxId: string): ProfileArtifact[] {
  const target = sandboxes.find((item) => item.id === sandboxId) ?? sandboxes[0];
  const role = target.runtimeType === 'gvisor' ? 'sentry' : target.runtimeType === 'kata' ? 'qemu' : target.runtimeType === 'firecracker' ? 'vmm' : 'shim';
  return [
    {
      id: `profile-${target.id}-cpu`,
      timestamp: minutesAgo(12),
      sandboxId: target.id,
      profileType: 'cpu',
      processRole: role,
      durationMs: 30_000,
      sampleCount: target.runtimeType === 'gvisor' ? 18_200 : 7_400,
      objectUri: `s3://runtimepulse/profiles/${target.id}/cpu.pprof`,
    },
    {
      id: `profile-${target.id}-io`,
      timestamp: minutesAgo(10),
      sandboxId: target.id,
      profileType: 'block_io',
      processRole: 'workload',
      durationMs: 30_000,
      sampleCount: target.nodeId === 'node-b' ? 12_300 : 3_900,
      objectUri: `s3://runtimepulse/profiles/${target.id}/block-io.pprof`,
    },
  ];
}

export const runtimeComparison: RuntimeCompareRow[] = [
  { runtimeType: 'runc', sampleCount: 1840, startupP50Ms: 940, startupP95Ms: 1480, cpuOverheadRatio: 0.03, memoryOverheadBytes: 90 * 1024 ** 2, failureRate: 0.002 },
  { runtimeType: 'gvisor', sampleCount: 920, startupP50Ms: 2840, startupP95Ms: 6120, cpuOverheadRatio: 0.24, memoryOverheadBytes: 340 * 1024 ** 2, failureRate: 0.006 },
  { runtimeType: 'kata', sampleCount: 760, startupP50Ms: 6480, startupP95Ms: 18_300, cpuOverheadRatio: 0.12, memoryOverheadBytes: 780 * 1024 ** 2, failureRate: 0.011 },
  { runtimeType: 'firecracker', sampleCount: 410, startupP50Ms: 4820, startupP95Ms: 31_600, cpuOverheadRatio: 0.08, memoryOverheadBytes: 510 * 1024 ** 2, failureRate: 0.026 },
];

function sandbox(
  id: string,
  runtimeType: RuntimeType,
  nodeId: string,
  imageId: string,
  workloadName: string,
  status: Sandbox['status'],
  startupDurationMs: number,
  cpuAvg: number,
  memoryPeakBytes: number,
  eventCount: number,
  attributes: Record<string, unknown> = {},
): Sandbox {
  const image = imageById(imageId);
  const offset = Number(id.match(/(\d+)$/)?.[1] ?? 1);
  return {
    id,
    clusterId: 'cluster-prod',
    nodeId,
    namespace: offset % 2 === 0 ? 'payments' : 'platform',
    workloadId: `pod-${workloadName}-${offset}`,
    workloadName,
    imageId,
    imageRef: image.ref,
    runtimeType,
    runtimeVersion: runtimeType === 'gvisor' ? 'runsc-20260415' : runtimeType === 'kata' ? '3.3.0' : runtimeType === 'firecracker' ? '1.8.2' : 'runc-1.2.1',
    status,
    createdAt: minutesAgo(70 - offset),
    startedAt: status === 'failed' ? undefined : new Date(Date.parse(minutesAgo(70 - offset)) + startupDurationMs).toISOString(),
    stoppedAt: status === 'stopped' ? minutesAgo(8) : undefined,
    startupDurationMs,
    cpuAvg,
    memoryPeakBytes,
    eventCount,
    labels: { app: workloadName, runtime: runtimeType },
    attributes,
  };
}

function imageById(id: string): Image {
  const image = images.find((item) => item.id === id);
  if (!image) throw new Error(`Missing mock image: ${id}`);
  return image;
}

function series(
  target: Sandbox,
  name: string,
  label: string,
  unit: string,
  group: MetricSeries['group'],
  base: number,
  wave: number,
  spike: number,
): MetricSeries {
  const points = Array.from({ length: 48 }, (_, index) => {
    const angle = index / 4;
    const spikeValue = index > 18 && index < 28 ? spike * Math.sin((index - 18) / 10 * Math.PI) : 0;
    const noise = Math.sin(angle) * wave + Math.cos(index / 7) * wave * 0.35;
    return {
      timestamp: minutesAgo(48 - index),
      value: Math.max(0, base + noise + spikeValue),
    };
  });

  return {
    id: `${target.id}-${name}`,
    name,
    label,
    unit,
    group,
    sandboxId: target.id,
    nodeId: target.nodeId,
    runtimeType: target.runtimeType,
    points,
  };
}
