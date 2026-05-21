import type {
  Cluster,
  EventRecord,
  FlamegraphFrame,
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

export const clusters: Cluster[] = [
  { id: 'runtimepulse-local', name: 'Local observed cluster', environment: 'single-node collector group' },
];

export const nodes: Node[] = [
  { id: 'node-a', name: 'rp-node-a', clusterId: 'runtimepulse-local', kernelVersion: '6.8.0', cpuCores: 64, memoryBytes: 256 * 1024 ** 3, status: 'ready' },
  { id: 'node-b', name: 'rp-node-b', clusterId: 'runtimepulse-local', kernelVersion: '6.8.0', cpuCores: 48, memoryBytes: 192 * 1024 ** 3, status: 'degraded' },
  { id: 'node-c', name: 'rp-node-c', clusterId: 'runtimepulse-local', kernelVersion: '5.15.0', cpuCores: 32, memoryBytes: 128 * 1024 ** 3, status: 'ready' },
];

export const images: Image[] = [
  image('img-api', 'registry.local/api:v42', 'sha256:api42', 'eager', 810 * 1024 ** 2, 18, 0.78, 2_800),
  image('img-ml-heavy', 'registry.local/ml-heavy:v8', 'sha256:mlheavy8', 'lazy', 4.6 * 1024 ** 3, 91, 0.34, 13_400),
  image('img-worker', 'registry.local/worker:v17', 'sha256:worker17', 'lazy', 1.4 * 1024 ** 3, 34, 0.62, 5_500),
  image('img-edge', 'registry.local/edge-proxy:v5', 'sha256:edge5', 'eager', 380 * 1024 ** 2, 12, 0.86, 1_600),
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
      flamegraph: cpuFlamegraph(target.runtimeType, role),
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
      flamegraph: ioFlamegraph(target.nodeId === 'node-b'),
    },
  ];
}

function cpuFlamegraph(runtimeType: RuntimeType, role: string): FlamegraphFrame {
  if (runtimeType === 'gvisor') {
    return frame(`${role}:root`, 100, [
      frame('sentry/syscalls.handle', 42, [
        frame('fs/gofer.walk', 17),
        frame('netstack/tcp.dispatch', 14),
        frame('security/seccomp.check', 11),
      ]),
      frame('runtime.schedule', 24, [frame('goroutine.scan', 13), frame('timer.run', 6), frame('gc.assist', 5)]),
      frame('application.proxy', 21, [frame('http.parse', 9), frame('json.encode', 7), frame('tls.write', 5)]),
      frame('kernel.copy_user', 13),
    ]);
  }

  if (runtimeType === 'kata') {
    return frame(`${role}:root`, 100, [
      frame('qemu/vcpu_loop', 34, [frame('kvm.run', 19), frame('virtio.queue_notify', 9), frame('irq.inject', 6)]),
      frame('virtiofsd.fuse_read', 27, [frame('metadata.lookup', 12), frame('page_cache.fill', 10), frame('copy_to_guest', 5)]),
      frame('guest.agent.rpc', 18, [frame('grpc.recv', 8), frame('sandbox.status', 6), frame('json.decode', 4)]),
      frame('runtime.schedule', 21),
    ]);
  }

  if (runtimeType === 'firecracker') {
    return frame(`${role}:root`, 100, [
      frame('vmm.run_vcpu', 39, [frame('kvm.vmexit', 18), frame('virtio-mmio.handle', 13), frame('irqfd.signal', 8)]),
      frame('jailer.io_proxy', 22, [frame('read_pipe', 11), frame('write_pipe', 7), frame('epoll.wait', 4)]),
      frame('guest.boot.wait', 24, [frame('agent.handshake', 15), frame('block.init', 9)]),
      frame('metrics.flush', 15),
    ]);
  }

  return frame(`${role}:root`, 100, [
    frame('container.init', 28, [frame('namespace.setup', 11), frame('cgroup.apply', 9), frame('seccomp.load', 8)]),
    frame('application.work', 36, [frame('http.serve', 16), frame('db.query', 12), frame('json.encode', 8)]),
    frame('runtime.schedule', 21),
    frame('kernel.syscall', 15),
  ]);
}

function ioFlamegraph(highPressure: boolean): FlamegraphFrame {
  return frame('block_io:root', 100, [
    frame('overlayfs.read_iter', highPressure ? 34 : 20, [frame('lookup_fast', 9), frame('copy_page_to_iter', 8), frame('xattr.read', highPressure ? 17 : 3)]),
    frame('snapshotter.fetch', highPressure ? 28 : 17, [frame('remote.block.get', 13), frame('decompress.chunk', 9), frame('verify.digest', 6)]),
    frame('page_cache.readahead', highPressure ? 21 : 34, [frame('bio.submit', 18), frame('cache.hit', highPressure ? 3 : 16)]),
    frame('blk_mq.dispatch', highPressure ? 17 : 29),
  ]);
}

function frame(name: string, value: number, children?: FlamegraphFrame[]): FlamegraphFrame {
  return { name, value, children };
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
    clusterId: 'runtimepulse-local',
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

function image(
  id: string,
  ref: string,
  digest: string,
  loadingMode: Image['loadingMode'],
  sizeBytes: number,
  layerCount: number,
  warmBlockRatio: number,
  startupCostMs: number,
): Image {
  return {
    id,
    ref,
    digest,
    loadingMode,
    sizeBytes,
    layerCount,
    layers: imageLayers(id, sizeBytes, layerCount, warmBlockRatio, startupCostMs),
    downloadTimeline: loadingMode === 'eager' ? imageDownloadTimeline(id, sizeBytes, layerCount, startupCostMs) : undefined,
  };
}

function imageDownloadTimeline(imageId: string, imageSizeBytes: number, layerCount: number, startupCostMs: number): Image['downloadTimeline'] {
  const manifestMs = Math.max(90, startupCostMs * 0.08);
  const pullMs = Math.max(600, startupCostMs * 0.52);
  const verifyMs = Math.max(120, startupCostMs * 0.1);
  const unpackMs = Math.max(500, startupCostMs * (layerCount > 30 ? 0.42 : 0.28));
  const snapshotMs = Math.max(80, startupCostMs * 0.06);

  return [
    { id: `${imageId}-resolve`, name: 'Resolve manifest', phase: 'resolve', durationMs: manifestMs, detail: 'Resolve tag and image manifest from registry.' },
    { id: `${imageId}-pull`, name: 'Download layers', phase: 'pull', durationMs: pullMs, bytes: imageSizeBytes, detail: `${layerCount} layers downloaded before container start.` },
    { id: `${imageId}-verify`, name: 'Verify digests', phase: 'verify', durationMs: verifyMs, bytes: imageSizeBytes, detail: 'Verify layer digests and image config.' },
    { id: `${imageId}-unpack`, name: 'Unpack layers', phase: 'unpack', durationMs: unpackMs, bytes: imageSizeBytes, detail: 'Unpack layer tar streams into snapshotter storage.' },
    { id: `${imageId}-snapshot`, name: 'Prepare snapshot', phase: 'snapshot', durationMs: snapshotMs, detail: 'Prepare rootfs snapshot for container create.' },
  ];
}

function imageLayers(imageId: string, imageSizeBytes: number, layerCount: number, warmBlockRatio: number, startupCostMs: number): Image['layers'] {
  const commands = ['FROM base runtime', 'RUN install packages', 'COPY application bundle', 'RUN dependency restore', 'COPY model/assets', 'RUN user permissions'];

  return commands.map((command, index) => {
    const weight = index === 4 ? 0.32 : index === 2 ? 0.2 : index === 3 ? 0.18 : 0.075;
    const sizeBytes = Math.max(8 * 1024 ** 2, imageSizeBytes * weight);
    const blockSizeBytes = 256 * 1024;
    const blockCount = Math.max(1, Math.ceil(sizeBytes / blockSizeBytes));
    const requestRatio = index === 4 ? 0.46 : index === 2 ? 0.38 : index === 3 ? 0.3 : 0.18;
    const requestedBlockCount = Math.max(1, Math.ceil(blockCount * requestRatio));
    const layerWarmth = Math.max(0.05, Math.min(0.96, warmBlockRatio - index * 0.07 + (index % 2) * 0.05));
    const cacheHitBlockCount = Math.floor(requestedBlockCount * layerWarmth);
    const localReadBytes = cacheHitBlockCount * blockSizeBytes;
    const remoteReadBytes = Math.max(0, requestedBlockCount - cacheHitBlockCount) * blockSizeBytes;
    const layerShare = sizeBytes / imageSizeBytes;
    const remotePenalty = 0.25 + (requestedBlockCount === 0 ? 0 : (requestedBlockCount - cacheHitBlockCount) / requestedBlockCount);

    return {
      id: `${imageId}-layer-${index + 1}`,
      command,
      sizeBytes,
      blockSizeBytes,
      blockCount,
      requestedBlockCount,
      cacheHitBlockCount,
      localReadBytes,
      remoteReadBytes,
      pullDurationMs: startupCostMs * layerShare * remotePenalty * 0.55,
      unpackDurationMs: startupCostMs * layerShare * remotePenalty * 0.45 * (layerCount > 60 ? 1.5 : 1),
    };
  });
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
