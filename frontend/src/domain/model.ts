export type RuntimeType = 'runc' | 'gvisor' | 'kata' | 'firecracker';

export type SandboxStatus = 'running' | 'stopped' | 'failed';

export type Severity = 'info' | 'warning' | 'error';

export type TimeRange = {
  from: string;
  to: string;
};

export type Cluster = {
  id: string;
  name: string;
  environment: string;
};

export type Node = {
  id: string;
  name: string;
  clusterId: string;
  kernelVersion: string;
  cpuCores: number;
  memoryBytes: number;
  status: 'ready' | 'degraded' | 'unknown';
};

export type Image = {
  id: string;
  ref: string;
  digest: string;
  sizeBytes: number;
  layerCount: number;
};

export type Sandbox = {
  id: string;
  clusterId: string;
  nodeId: string;
  namespace: string;
  workloadId: string;
  workloadName: string;
  imageId: string;
  imageRef: string;
  runtimeType: RuntimeType;
  runtimeVersion: string;
  status: SandboxStatus;
  createdAt: string;
  startedAt?: string;
  stoppedAt?: string;
  startupDurationMs: number;
  cpuAvg: number;
  memoryPeakBytes: number;
  eventCount: number;
  labels: Record<string, string>;
  attributes: Record<string, unknown>;
};

export type MetricPoint = {
  timestamp: string;
  value: number;
};

export type MetricSeries = {
  id: string;
  name: string;
  label: string;
  unit: string;
  group: 'cpu' | 'memory' | 'io' | 'network' | 'runtime' | 'startup' | 'pressure';
  sandboxId?: string;
  nodeId?: string;
  runtimeType?: RuntimeType;
  points: MetricPoint[];
};

export type EventRecord = {
  id: string;
  timestamp: string;
  severity: Severity;
  eventType: string;
  eventName: string;
  sandboxId?: string;
  nodeId?: string;
  runtimeType?: RuntimeType;
  reason?: string;
  message: string;
  source: string;
  attributes: Record<string, unknown>;
};

export type TraceSpan = {
  traceId: string;
  spanId: string;
  parentSpanId?: string;
  sandboxId: string;
  spanName: string;
  startTime: string;
  endTime: string;
  durationMs: number;
  status: 'ok' | 'error';
  attributes: Record<string, unknown>;
};

export type FlamegraphFrame = {
  name: string;
  value: number;
  children?: FlamegraphFrame[];
};

export type ProfileArtifact = {
  id: string;
  timestamp: string;
  sandboxId: string;
  profileType: 'cpu' | 'off_cpu' | 'memory' | 'block_io' | 'syscall';
  processRole: string;
  durationMs: number;
  sampleCount: number;
  objectUri: string;
  flamegraph?: FlamegraphFrame;
};

export type SandboxQuery = {
  runtimeType?: RuntimeType | 'all';
  status?: SandboxStatus | 'all';
  text?: string;
};

export type RuntimeCompareRow = {
  runtimeType: RuntimeType;
  sampleCount: number;
  startupP50Ms: number;
  startupP95Ms: number;
  cpuOverheadRatio: number;
  memoryOverheadBytes: number;
  failureRate: number;
};
