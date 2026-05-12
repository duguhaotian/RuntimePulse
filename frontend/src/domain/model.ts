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
  loadingMode: 'lazy' | 'eager';
  sizeBytes: number;
  layerCount: number;
  layers?: ImageLayer[];
  downloadTimeline?: ImageDownloadStep[];
};

export type ImageLayer = {
  id: string;
  command: string;
  sizeBytes: number;
  blockSizeBytes: number;
  blockCount: number;
  requestedBlockCount: number;
  cacheHitBlockCount: number;
  localReadBytes: number;
  remoteReadBytes: number;
  pullDurationMs: number;
  unpackDurationMs: number;
};

export type ImageDownloadStep = {
  id: string;
  name: string;
  phase: 'resolve' | 'pull' | 'verify' | 'unpack' | 'snapshot';
  durationMs: number;
  bytes?: number;
  detail: string;
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

export type IngestCounts = {
  metadata: {
    clusters: number;
    nodes: number;
    images: number;
    sandboxes: number;
  };
  metrics: number;
  events: number;
  traces: number;
  profiles: number;
};

export type IngestBatchSummary = {
  source: string;
  observedAt?: string;
  acceptedAt: string;
  counts: IngestCounts;
};

export type IngestRejectedSummary = {
  source?: string;
  rejectedAt: string;
  errors: string[];
};

export type IngestSourceStatus = {
  source: string;
  acceptedBatches: number;
  totals: IngestCounts;
  firstAcceptedAt: string;
  lastAcceptedAt: string;
  lastObservedAt?: string;
};

export type LiveStoreStatus = {
  clusters: number;
  nodes: number;
  images: number;
  sandboxes: number;
  metricSeries: number;
  metricPoints: number;
  events: number;
  traces: number;
  profiles: number;
  limits: {
    metricPointsPerSeries: number;
    rowsPerKind: number;
  };
};

export type IngestStatus = {
  mode: 'validation_only';
  startedAt: string;
  acceptedBatches: number;
  rejectedBatches: number;
  totals: IngestCounts;
  sources: IngestSourceStatus[];
  liveStore?: LiveStoreStatus;
  lastAcceptedBatch?: IngestBatchSummary;
  lastRejectedBatch?: IngestRejectedSummary;
};

export type RecentMetricSample = {
  source: string;
  acceptedAt: string;
  timestamp: string;
  name: string;
  value: number;
  unit?: string;
  group?: string;
  sandboxId?: string;
  nodeId?: string;
  imageId?: string;
  runtimeType?: RuntimeType;
  attributes?: Record<string, unknown>;
};

export type RecentEventSample = {
  source: string;
  acceptedAt: string;
  id: string;
  timestamp: string;
  severity: Severity;
  eventType: string;
  eventName: string;
  sandboxId?: string;
  nodeId?: string;
  runtimeType?: RuntimeType;
  message: string;
};

export type RecentTraceSample = {
  source: string;
  acceptedAt: string;
  traceId: string;
  spanId: string;
  parentSpanId?: string;
  sandboxId?: string;
  spanName: string;
  startTime: string;
  endTime: string;
  durationMs: number;
  status: 'ok' | 'error';
};

export type RecentProfileSample = {
  source: string;
  acceptedAt: string;
  id: string;
  timestamp: string;
  sandboxId: string;
  profileType: ProfileArtifact['profileType'];
  processRole: string;
  durationMs: number;
  sampleCount: number;
  objectUri: string;
};

export type IngestRecent = {
  mode: 'validation_only';
  recentBatches: IngestBatchSummary[];
  recentMetrics: RecentMetricSample[];
  recentEvents: RecentEventSample[];
  recentTraces: RecentTraceSample[];
  recentProfiles: RecentProfileSample[];
};
