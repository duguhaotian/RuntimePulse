import type { RuntimePulseApi } from './RuntimePulseApi';
import type { IngestRecent, IngestStatus, SandboxAnalysis, SandboxQuery } from '../domain/model';
import {
  clusters,
  eventsForSandbox,
  images,
  metricsForSandbox,
  nodes,
  profilesForSandbox,
  runtimeComparison,
  sandboxes,
  traceForSandbox,
} from '../mock/data';

function filterSandboxes(query?: SandboxQuery) {
  return sandboxes.filter((sandbox) => {
    const runtimeMatch = !query?.runtimeType || query.runtimeType === 'all' || sandbox.runtimeType === query.runtimeType;
    const statusMatch = !query?.status || query.status === 'all' || sandbox.status === query.status;
    const text = query?.text?.trim().toLowerCase();
    const textMatch = !text || [sandbox.id, sandbox.nodeId, sandbox.workloadName, sandbox.imageRef, sandbox.namespace].some((value) => value.toLowerCase().includes(text));
    return runtimeMatch && statusMatch && textMatch;
  });
}

export const mockRuntimePulseApi: RuntimePulseApi = {
  async listClusters() {
    return clusters;
  },
  async listNodes() {
    return nodes;
  },
  async listImages() {
    return images;
  },
  async listSandboxes(query) {
    return filterSandboxes(query);
  },
  async getSandbox(id) {
    return sandboxes.find((sandbox) => sandbox.id === id);
  },
  async getNode(id) {
    return nodes.find((node) => node.id === id);
  },
  async getImage(id) {
    return images.find((image) => image.id === id);
  },
  async getSandboxMetrics(id) {
    return metricsForSandbox(id);
  },
  async getSandboxEvents(id) {
    return eventsForSandbox(id);
  },
  async getSandboxTrace(id) {
    return traceForSandbox(id);
  },
  async getSandboxProfiles(id) {
    return profilesForSandbox(id);
  },
  async getSandboxAnalysis(id) {
    const sandbox = sandboxes.find((item) => item.id === id) ?? sandboxes[0];
    const spans = traceForSandbox(id);
    const largestSpan = [...spans].sort((left, right) => right.durationMs - left.durationMs)[0];
    const slow = sandbox.startupDurationMs > 7000;

    return {
      sandboxId: sandbox.id,
      generatedAt: new Date().toISOString(),
      summary: slow
        ? `${largestSpan.spanName} is the largest observed startup stage. Review trace, image access, and profile artifacts before changing runtime settings.`
        : 'No obvious bottleneck detected from the current mock analysis context.',
      bottleneckStage: largestSpan.spanName,
      findings: [
        {
          id: `${sandbox.id}-mock-startup`,
          severity: slow ? 'warning' : 'info',
          category: largestSpan.spanName.startsWith('image.') ? 'image' : 'startup',
          title: slow ? 'Startup path needs review' : 'Startup path looks normal',
          summary: `${sandbox.runtimeType} startup is ${Math.round(sandbox.startupDurationMs)}ms and ${largestSpan.spanName} is the largest span.`,
          evidence: [
            `startup.duration_ms=${sandbox.startupDurationMs}`,
            `${largestSpan.spanName}=${Math.round(largestSpan.durationMs)}ms`,
          ],
          recommendedActions: [
            'Open the startup trace and inspect the largest span.',
            'Compare with another run that uses the same image.',
          ],
          relatedSpanIds: [largestSpan.spanId],
        },
      ],
    } satisfies SandboxAnalysis;
  },
  async compareRuntimes() {
    return runtimeComparison;
  },
  async getIngestStatus() {
    return mockIngestStatus;
  },
  async getIngestRecent() {
    return mockIngestRecent;
  },
};

const mockIngestStatus: IngestStatus = {
  mode: 'in_memory_live_store',
  startedAt: '2026-05-09T03:52:00.000Z',
  acceptedBatches: 28,
  rejectedBatches: 1,
  totals: {
    metadata: { clusters: 0, nodes: 28, images: 0, sandboxes: 28 },
    metrics: 224,
    events: 28,
    traces: 140,
    profiles: 28,
  },
  sources: [
    {
      source: 'mock-node-collector/node-a',
      acceptedBatches: 28,
      totals: {
        metadata: { clusters: 0, nodes: 28, images: 0, sandboxes: 28 },
        metrics: 224,
        events: 28,
        traces: 140,
        profiles: 28,
      },
      firstAcceptedAt: '2026-05-09T03:52:00.000Z',
      lastAcceptedAt: '2026-05-09T04:00:00.000Z',
      lastObservedAt: '2026-05-09T03:59:59.000Z',
    },
  ],
  lastAcceptedBatch: {
    source: 'mock-node-collector/node-a',
    observedAt: '2026-05-09T03:59:59.000Z',
    acceptedAt: '2026-05-09T04:00:00.000Z',
    counts: {
      metadata: { clusters: 0, nodes: 1, images: 0, sandboxes: 1 },
      metrics: 8,
      events: 1,
      traces: 5,
      profiles: 1,
    },
  },
  lastRejectedBatch: {
    source: 'mock-node-collector/node-a',
    rejectedAt: '2026-05-09T03:48:40.000Z',
    errors: ['metrics[0].timestamp must be an ISO date-time string'],
  },
};

const mockIngestRecent: IngestRecent = {
  mode: 'in_memory_live_store',
  recentBatches: [
    mockIngestStatus.lastAcceptedBatch!,
  ],
  recentMetrics: [
    {
      source: 'mock-node-collector/node-a',
      acceptedAt: '2026-05-09T04:00:00.000Z',
      timestamp: '2026-05-09T03:59:59.000Z',
      name: 'node.container.count',
      value: 23,
      unit: 'count',
      group: 'lifecycle',
      nodeId: 'node-a',
    },
    {
      source: 'mock-node-collector/node-a',
      acceptedAt: '2026-05-09T04:00:00.000Z',
      timestamp: '2026-05-09T03:59:59.000Z',
      name: 'sandbox.lifecycle.runtime.create.duration_ms',
      value: 820,
      unit: 'ms',
      group: 'lifecycle',
      sandboxId: 'collector-node-a-001',
      nodeId: 'node-a',
      runtimeType: 'runc',
    },
  ],
  recentEvents: [
    {
      source: 'mock-node-collector/node-a',
      acceptedAt: '2026-05-09T04:00:00.000Z',
      id: 'collector-node-a-001-created',
      timestamp: '2026-05-09T03:59:59.000Z',
      severity: 'info',
      eventType: 'lifecycle',
      eventName: 'sandbox.created',
      sandboxId: 'collector-node-a-001',
      nodeId: 'node-a',
      runtimeType: 'runc',
      message: 'Mock collector observed runc sandbox startup',
    },
  ],
  recentTraces: [
    {
      source: 'mock-node-collector/node-a',
      acceptedAt: '2026-05-09T04:00:00.000Z',
      traceId: 'trace-collector-node-a-001',
      spanId: 'span-3',
      parentSpanId: 'span-1',
      sandboxId: 'collector-node-a-001',
      spanName: 'runtime.create',
      startTime: '2026-05-09T03:59:59.900Z',
      endTime: '2026-05-09T04:00:00.720Z',
      durationMs: 820,
      status: 'ok',
    },
  ],
  recentProfiles: [
    {
      source: 'mock-node-collector/node-a',
      acceptedAt: '2026-05-09T04:00:00.000Z',
      id: 'collector-node-a-001-cpu-1',
      timestamp: '2026-05-09T03:59:59.000Z',
      sandboxId: 'collector-node-a-001',
      profileType: 'cpu',
      processRole: 'container-init',
      durationMs: 720,
      sampleCount: 451,
      objectUri: 's3://runtimepulse/mock-profiles/node-a/collector-node-a-001/cpu.pprof',
    },
  ],
};
