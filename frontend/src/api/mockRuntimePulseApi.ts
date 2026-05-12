import type { RuntimePulseApi } from './RuntimePulseApi';
import type { IngestRecent, IngestStatus, SandboxQuery } from '../domain/model';
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
  mode: 'validation_only',
  startedAt: '2026-05-09T03:52:00.000Z',
  acceptedBatches: 28,
  rejectedBatches: 1,
  totals: {
    metadata: { clusters: 0, nodes: 28, images: 0, sandboxes: 28 },
    metrics: 224,
    events: 28,
    traces: 140,
    profiles: 0,
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
        profiles: 0,
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
      profiles: 0,
    },
  },
  lastRejectedBatch: {
    source: 'mock-node-collector/node-a',
    rejectedAt: '2026-05-09T03:48:40.000Z',
    errors: ['metrics[0].timestamp must be an ISO date-time string'],
  },
};

const mockIngestRecent: IngestRecent = {
  mode: 'validation_only',
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
};
