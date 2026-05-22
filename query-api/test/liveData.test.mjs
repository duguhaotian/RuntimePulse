import assert from 'node:assert/strict';
import test from 'node:test';
import { createLiveStore, liveMetricsForSandbox, liveStoreSnapshot, recordLiveBatch } from '../src/liveData.mjs';

test('attaches pod scoped network metrics to matching k8s container sandbox', () => {
  const store = createLiveStore();
  recordLiveBatch(store, {
    source: 'test',
    metadata: {
      clusters: [],
      nodes: [],
      images: [],
      sandboxes: [{
        id: 'k8s-default-runtimepulse-demo-app',
        nodeId: 'node-a',
        namespace: 'default',
        workloadId: 'runtimepulse-demo',
        workloadName: 'runtimepulse-demo/app',
        imageId: 'image-a',
        imageRef: 'image-a:latest',
        runtimeType: 'kubernetes',
        attributes: {
          'k8s.namespace': 'default',
          'k8s.pod': 'runtimepulse-demo',
          'k8s.container': 'app',
        },
      }],
    },
    metrics: [{
      timestamp: '2026-05-22T02:00:00.000Z',
      name: 'sandbox.network.rx_bytes',
      value: 42,
      unit: 'bytes/s',
      group: 'network',
      sandboxId: 'k8s-default-runtimepulse-demo-pod',
      nodeId: 'node-a',
      runtimeType: 'kubernetes',
      attributes: {
        'metrics.scope': 'pod',
        'k8s.namespace': 'default',
        'k8s.pod': 'runtimepulse-demo',
        'k8s.container': 'pod',
      },
    }],
    events: [],
    traces: [],
    profiles: [],
  });

  const series = liveMetricsForSandbox(store, 'k8s-default-runtimepulse-demo-app');
  assert.equal(series.length, 1);
  assert.equal(series[0].sandboxId, 'k8s-default-runtimepulse-demo-app');
  assert.equal(series[0].attributes['metrics.attachedFromSandboxId'], 'k8s-default-runtimepulse-demo-pod');
});

test('counts metric-only pod network series by ingest source', () => {
  const store = createLiveStore();
  recordLiveBatch(store, {
    source: 'test/kubernetes-metrics',
    metadata: {
      clusters: [],
      nodes: [],
      images: [],
      sandboxes: [],
    },
    metrics: [{
      timestamp: '2026-05-22T02:00:00.000Z',
      name: 'sandbox.network.rx_bytes',
      value: 42,
      unit: 'bytes/s',
      group: 'network',
      sandboxId: 'k8s-default-runtimepulse-demo-pod',
      nodeId: 'node-a',
      runtimeType: 'kubernetes',
      attributes: {
        'metrics.scope': 'pod',
        'k8s.namespace': 'default',
        'k8s.pod': 'runtimepulse-demo',
        'k8s.container': 'pod',
      },
    }],
    events: [],
    traces: [],
    profiles: [],
  });

  const snapshot = liveStoreSnapshot(store);
  const source = snapshot.sources.find((item) => item.source === 'test/kubernetes-metrics');
  assert.equal(source?.metricSeries, 1);
  assert.equal(source?.metricPoints, 1);
});

test('counts profile-only reports by ingest source', () => {
  const store = createLiveStore();
  recordLiveBatch(store, {
    source: 'test/profile-report',
    metadata: {
      clusters: [],
      nodes: [],
      images: [],
      sandboxes: [],
    },
    metrics: [],
    events: [],
    traces: [],
    profiles: [{
      id: 'profile-1',
      timestamp: '2026-05-22T02:00:00.000Z',
      sandboxId: 'sandbox-a',
      profileType: 'cpu',
      processRole: 'app',
      durationMs: 5000,
      sampleCount: 77,
      objectUri: 'file:///tmp/profile.perf',
    }],
  });

  const snapshot = liveStoreSnapshot(store);
  const source = snapshot.sources.find((item) => item.source === 'test/profile-report');
  assert.equal(snapshot.profiles, 1);
  assert.equal(source?.profiles, 1);
});

test('reconciles sandbox snapshots from metadata without sample events', () => {
  const store = createLiveStore();
  recordLiveBatch(store, {
    source: 'test/sandbox-cgroupfs',
    metadata: {
      clusters: [],
      nodes: [],
      images: [],
      sandboxes: [
        {
          id: 'docker-live',
          nodeId: 'node-a',
          imageRef: 'image-a:latest',
          runtimeType: 'runc',
          attributes: { 'snapshot.scope': 'docker-running' },
        },
        {
          id: 'docker-gone',
          nodeId: 'node-a',
          imageRef: 'image-a:latest',
          runtimeType: 'runc',
          attributes: { 'snapshot.scope': 'docker-running' },
        },
      ],
    },
    metrics: [],
    events: [],
    traces: [],
    profiles: [],
  });

  recordLiveBatch(store, {
    source: 'test/sandbox-cgroupfs',
    metadata: {
      clusters: [],
      nodes: [{
        id: 'node-a',
        attributes: {
          'snapshot.scope': 'docker-running',
          'snapshot.nodeId': 'node-a',
          'snapshot.sandboxIds': ['docker-live'],
        },
      }],
      images: [],
      sandboxes: [{
        id: 'docker-live',
        nodeId: 'node-a',
        imageRef: 'image-a:latest',
        runtimeType: 'runc',
        attributes: { 'snapshot.scope': 'docker-running' },
      }],
    },
    metrics: [],
    events: [],
    traces: [],
    profiles: [],
  });

  const snapshot = liveStoreSnapshot(store);
  assert.equal(snapshot.sandboxes, 1);
});

test('keeps event source attribution when node metadata is refreshed by another source', () => {
  const store = createLiveStore();
  recordLiveBatch(store, {
    source: 'docker-events',
    metadata: {
      clusters: [],
      nodes: [{ id: 'node-a' }],
      images: [],
      sandboxes: [],
    },
    metrics: [],
    events: [{
      id: 'event-a',
      timestamp: '2026-05-22T02:00:00.000Z',
      eventType: 'container',
      eventName: 'docker.start',
      severity: 'info',
      nodeId: 'node-a',
    }],
    traces: [],
    profiles: [],
  });

  recordLiveBatch(store, {
    source: 'host-cgroupfs',
    metadata: {
      clusters: [],
      nodes: [{ id: 'node-a' }],
      images: [],
      sandboxes: [],
    },
    metrics: [{
      timestamp: '2026-05-22T02:00:01.000Z',
      name: 'node.cpu.usage_ratio',
      value: 0.1,
      unit: 'ratio',
      group: 'cpu',
      nodeId: 'node-a',
    }],
    events: [],
    traces: [],
    profiles: [],
  });

  const snapshot = liveStoreSnapshot(store);
  const dockerEvents = snapshot.sources.find((item) => item.source === 'docker-events');
  const cgroupfs = snapshot.sources.find((item) => item.source === 'host-cgroupfs');
  assert.equal(dockerEvents?.events, 1);
  assert.equal(cgroupfs?.events ?? 0, 0);
});
