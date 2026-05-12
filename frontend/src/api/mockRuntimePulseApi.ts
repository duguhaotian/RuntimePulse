import type { RuntimePulseApi } from './RuntimePulseApi';
import type { IngestStatus, SandboxQuery } from '../domain/model';
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
