import assert from 'node:assert/strict';
import test from 'node:test';
import { createLiveStore, liveEventsForNode, liveMetricsForSandbox, liveSandboxes, liveStoreSnapshot, recordLiveBatch } from '../src/liveData.mjs';

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

test('reconciles containerd running snapshots from inventory metadata', () => {
  const store = createLiveStore();
  recordLiveBatch(store, {
    source: 'host-containerd',
    metadata: {
      clusters: [],
      nodes: [],
      images: [],
      sandboxes: [
        {
          id: 'k8s-default-live-app',
          nodeId: 'node-a',
          imageRef: 'image-a:latest',
          runtimeType: 'runc',
          attributes: {
            'containerd.id': 'live',
            'snapshot.scope': 'containerd-running',
          },
        },
        {
          id: 'k8s-default-gone-app',
          nodeId: 'node-a',
          imageRef: 'image-a:latest',
          runtimeType: 'runc',
          attributes: {
            'containerd.id': 'gone',
            'snapshot.scope': 'containerd-running',
          },
        },
      ],
    },
    metrics: [],
    events: [],
    traces: [],
    profiles: [],
  });

  recordLiveBatch(store, {
    source: 'host-containerd',
    metadata: {
      clusters: [],
      nodes: [{
        id: 'node-a',
        attributes: {
          'snapshot.scope': 'containerd-running',
          'snapshot.nodeId': 'node-a',
          'snapshot.sandboxIds': ['k8s-default-live-app'],
        },
      }],
      images: [],
      sandboxes: [{
        id: 'k8s-default-live-app',
        nodeId: 'node-a',
        imageRef: 'image-a:latest',
        runtimeType: 'runc',
        attributes: {
          'containerd.id': 'live',
          'snapshot.scope': 'containerd-running',
        },
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

test('keeps aliased k8s sandbox when containerd snapshot uses raw runtime id', () => {
  const store = createLiveStore();
  recordLiveBatch(store, {
    source: 'cri-events',
    metadata: {
      clusters: [],
      nodes: [],
      images: [],
      sandboxes: [{
        id: 'k8s-default-runtimepulse-demo-pod',
        nodeId: 'node-a',
        imageRef: 'registry.k8s.io/pause:3.10',
        runtimeType: 'runc',
        attributes: {
          'cri.sandbox_id': 'abcdef0123456789',
          'containerd.container_id': 'containerd-k8s-io-abcdef0123456789',
          'k8s.namespace': 'default',
          'k8s.pod': 'runtimepulse-demo',
          'k8s.container': 'POD',
          'k8s.pod_attempt': '1',
        },
      }],
    },
    metrics: [],
    events: [],
    traces: [],
    profiles: [],
  });

  recordLiveBatch(store, {
    source: 'host-containerd',
    metadata: {
      clusters: [],
      nodes: [{
        id: 'node-a',
        attributes: {
          'snapshot.scope': 'containerd-running',
          'snapshot.nodeId': 'node-a',
          'snapshot.sandboxIds': ['containerd-k8s-io-abcdef0123456789'],
        },
      }],
      images: [],
      sandboxes: [{
        id: 'containerd-k8s-io-abcdef0123456789',
        nodeId: 'node-a',
        imageRef: 'registry.k8s.io/pause:3.10',
        runtimeType: 'runc',
        attributes: {
          'containerd.id': 'abcdef0123456789',
          'snapshot.scope': 'containerd-running',
        },
      }],
    },
    metrics: [],
    events: [],
    traces: [],
    profiles: [],
  });

  assert.deepEqual(liveSandboxes(store).map((sandbox) => sandbox.id), ['k8s-default-runtimepulse-demo-pod']);
});

test('empty containerd running snapshot removes stale live sandboxes', () => {
  const store = createLiveStore();
  recordLiveBatch(store, {
    source: 'host-containerd',
    metadata: {
      clusters: [],
      nodes: [],
      images: [],
      sandboxes: [{
        id: 'k8s-default-gone-app',
        nodeId: 'node-a',
        imageRef: 'image-a:latest',
        runtimeType: 'runc',
        attributes: {
          'containerd.id': 'gone',
          'snapshot.scope': 'containerd-running',
        },
      }],
    },
    metrics: [],
    events: [],
    traces: [],
    profiles: [],
  });

  recordLiveBatch(store, {
    source: 'host-containerd',
    metadata: {
      clusters: [],
      nodes: [{
        id: 'node-a',
        attributes: {
          'snapshot.scope': 'containerd-running',
          'snapshot.nodeId': 'node-a',
          'snapshot.sandboxIds': [],
        },
      }],
      images: [],
      sandboxes: [],
    },
    metrics: [],
    events: [],
    traces: [],
    profiles: [],
  });

  const snapshot = liveStoreSnapshot(store);
  assert.equal(snapshot.sandboxes, 0);
});


test('reconciles generic sandbox running snapshots by runtime type', () => {
  const store = createLiveStore();
  recordLiveBatch(store, {
    source: 'host-kata',
    metadata: {
      clusters: [],
      nodes: [],
      images: [],
      sandboxes: [
        {
          id: 'kata-live',
          nodeId: 'node-a',
          imageRef: 'image-a:latest',
          runtimeType: 'kata',
        },
        {
          id: 'kata-gone',
          nodeId: 'node-a',
          imageRef: 'image-a:latest',
          runtimeType: 'kata',
        },
        {
          id: 'firecracker-keep',
          nodeId: 'node-a',
          imageRef: 'image-b:latest',
          runtimeType: 'firecracker',
        },
      ],
    },
    metrics: [],
    events: [],
    traces: [],
    profiles: [],
  });

  recordLiveBatch(store, {
    source: 'host-sandbox-reconcile',
    metadata: {
      clusters: [],
      nodes: [{
        id: 'node-a',
        attributes: {
          'snapshot.scope': 'kata-running',
          'snapshot.nodeId': 'node-a',
          'snapshot.sandboxIds': ['kata-live'],
        },
      }],
      images: [],
      sandboxes: [],
    },
    metrics: [],
    events: [],
    traces: [],
    profiles: [],
  });

  const sandboxes = liveSandboxes(store).map((sandbox) => sandbox.id).sort();
  assert.deepEqual(sandboxes, ['firecracker-keep', 'kata-live']);
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

test('projects sandbox events without explicit node id onto the sandbox node', () => {
  const store = createLiveStore();
  recordLiveBatch(store, {
    source: 'containerd-events',
    metadata: {
      clusters: [],
      nodes: [{ id: 'node-a' }],
      images: [],
      sandboxes: [{
        id: 'k8s-default-demo-app',
        nodeId: 'node-a',
        imageRef: 'pause:latest',
        runtimeType: 'runc',
      }],
    },
    metrics: [],
    events: [{
      id: 'event-without-node',
      timestamp: '2026-05-22T02:00:00.000Z',
      eventType: 'container',
      eventName: 'containerd.container.start',
      severity: 'info',
      source: 'runtimepulse-rust-collector/node-a/containerd-events',
      message: 'containerd container demo emitted start.',
      sandboxId: 'k8s-default-demo-app',
      attributes: { 'containerd.action': 'start' },
    }],
    traces: [],
    profiles: [],
  });

  const events = liveEventsForNode(store, 'node-a');
  assert.equal(events.length, 1);
  assert.equal(events[0].id, 'event-without-node');
  assert.equal(events[0].nodeId, 'node-a');
});

test('derives node events from host-agent status metrics', () => {
  const store = createLiveStore();
  recordLiveBatch(store, {
    source: 'host-agent-self',
    metadata: {
      clusters: [],
      nodes: [{ id: 'node-a' }],
      images: [],
      sandboxes: [],
    },
    metrics: [
      {
        timestamp: '2026-05-22T02:00:00.000Z',
        name: 'host_agent.up',
        value: 1,
        unit: 'state',
        group: 'collector',
        nodeId: 'node-a',
      },
      {
        timestamp: '2026-05-22T02:00:00.000Z',
        name: 'host_agent.event_stream.enabled',
        value: 1,
        unit: 'state',
        group: 'collector',
        nodeId: 'node-a',
        attributes: { 'collector.event_stream': 'containerd-events' },
      },
      {
        timestamp: '2026-05-22T02:00:00.000Z',
        name: 'host_agent.event_stream.running',
        value: 1,
        unit: 'state',
        group: 'collector',
        nodeId: 'node-a',
        attributes: { 'collector.event_stream': 'containerd-events' },
      },
      {
        timestamp: '2026-05-22T02:00:01.000Z',
        name: 'host_agent.event_stream.errors_total',
        value: 1,
        unit: 'count',
        group: 'collector',
        nodeId: 'node-a',
        attributes: { 'collector.event_stream': 'containerd-events' },
      },
    ],
    events: [],
    traces: [],
    profiles: [],
  });

  recordLiveBatch(store, {
    source: 'host-agent-self',
    metadata: {
      clusters: [],
      nodes: [{ id: 'node-a' }],
      images: [],
      sandboxes: [],
    },
    metrics: [{
      timestamp: '2026-05-22T02:00:02.000Z',
      name: 'host_agent.event_stream.errors_total',
      value: 3,
      unit: 'count',
      group: 'collector',
      nodeId: 'node-a',
      attributes: { 'collector.event_stream': 'containerd-events' },
    }],
    events: [],
    traces: [],
    profiles: [],
  });

  const events = liveEventsForNode(store, 'node-a');
  assert.deepEqual(events.map((event) => event.eventName), [
    'host_agent.started',
    'host_agent.event_stream.connected',
    'host_agent.event_stream.error',
  ]);
});

test('derives sandbox startup duration from cri startup trace spans and callchain metrics', () => {
  const store = createLiveStore();
  recordLiveBatch(store, {
    source: 'startup-test',
    metadata: {
      clusters: [],
      nodes: [],
      images: [],
      sandboxes: [{
        id: 'cri-sandbox-a',
        nodeId: 'node-a',
        imageRef: 'pause:latest',
        runtimeType: 'runc',
        startupDurationMs: 0,
      }],
    },
    metrics: [],
    events: [],
    traces: [{
      traceId: 'cri-containerd-startup-cri-sandbox-a',
      spanId: 'cri-containerd-startup-cri-sandbox-a-e2e',
      spanName: 'sandbox.startup.e2e',
      startTime: '2026-05-22T02:00:00.000Z',
      endTime: '2026-05-22T02:00:01.200Z',
      durationMs: 1200,
      status: 'ok',
      attributes: { plugin: 'cri-startup-trace', 'runtime.type': 'kata' },
      sandboxId: 'cri-sandbox-a',
      runtimeType: 'kata',
    }],
    profiles: [],
  });

  assert.equal(liveSandboxes(store)[0].startupDurationMs, 1200);
  assert.equal(liveSandboxes(store)[0].runtimeType, 'kata');
  assert.equal(liveSandboxes(store)[0].attributes['runtime.type'], 'kata');
  assert.equal(liveSandboxes(store)[0].attributes['startup.duration.plugin'], 'cri-startup-trace');

  recordLiveBatch(store, {
    source: 'late-metadata',
    metadata: {
      clusters: [],
      nodes: [],
      images: [],
      sandboxes: [{ id: 'cri-sandbox-a', nodeId: 'node-a', imageRef: 'pause:latest', runtimeType: 'runc' }],
    },
    metrics: [],
    events: [],
    traces: [],
    profiles: [],
  });
  assert.equal(liveSandboxes(store)[0].runtimeType, 'kata');
  assert.equal(liveSandboxes(store)[0].attributes['startup.duration.source'], 'trace');
  assert.equal(liveSandboxes(store)[0].attributes['startup.duration.plugin'], 'cri-startup-trace');

  recordLiveBatch(store, {
    source: 'startup-callchain',
    metadata: { clusters: [], nodes: [], images: [], sandboxes: [] },
    metrics: [{
      timestamp: '2026-05-22T02:00:02.000Z',
      name: 'sandbox.startup.callchain_duration_ms',
      value: 1500,
      unit: 'ms',
      group: 'startup',
      sandboxId: 'cri-sandbox-a',
      nodeId: 'node-a',
      runtimeType: 'runc',
      attributes: { plugin: 'startup-callchain' },
    }],
    events: [],
    traces: [],
    profiles: [],
  });

  assert.equal(liveSandboxes(store)[0].startupDurationMs, 1200);
  assert.equal(liveSandboxes(store)[0].attributes['startup.duration.plugin'], 'cri-startup-trace');
});

test('keeps runpod e2e startup over short startup-callchain trace spans', () => {
  const store = createLiveStore();
  recordLiveBatch(store, {
    source: 'runpod-e2e',
    metadata: {
      clusters: [],
      nodes: [],
      images: [],
      sandboxes: [{ id: 'k8s-default-demo-pod', nodeId: 'node-a', imageRef: 'pause:latest', runtimeType: 'runc' }],
    },
    metrics: [],
    events: [],
    traces: [{
      traceId: 'cri-containerd-startup-k8s-default-demo-pod',
      spanId: 'cri-containerd-startup-k8s-default-demo-pod-e2e',
      spanName: 'sandbox.startup.e2e',
      startTime: '2026-05-22T02:00:00.000Z',
      endTime: '2026-05-22T02:00:00.036Z',
      durationMs: 36,
      status: 'ok',
      attributes: { plugin: 'cri-startup-trace', 'runtime.type': 'runc' },
      sandboxId: 'k8s-default-demo-pod',
      runtimeType: 'runc',
    }],
    profiles: [],
  });

  recordLiveBatch(store, {
    source: 'startup-callchain',
    metadata: { clusters: [], nodes: [], images: [], sandboxes: [] },
    metrics: [{
      timestamp: '2026-05-22T02:00:01.000Z',
      name: 'sandbox.startup.callchain_duration_ms',
      value: 2,
      unit: 'ms',
      group: 'startup',
      sandboxId: 'k8s-default-demo-pod',
      nodeId: 'node-a',
      runtimeType: 'runc',
      attributes: { plugin: 'startup-callchain' },
    }],
    events: [],
    traces: [{
      traceId: 'cri-containerd-startup-k8s-default-demo-pod',
      spanId: 'cri-containerd-startup-k8s-default-demo-pod-callchain',
      spanName: 'sandbox.startup.callchain',
      startTime: '2026-05-22T02:00:01.000Z',
      endTime: '2026-05-22T02:00:01.002Z',
      durationMs: 2,
      status: 'ok',
      attributes: { plugin: 'startup-callchain' },
      sandboxId: 'k8s-default-demo-pod',
      runtimeType: 'runc',
    }],
    profiles: [],
  });

  assert.equal(liveSandboxes(store)[0].startupDurationMs, 36);
  assert.equal(liveSandboxes(store)[0].startedAt, '2026-05-22T02:00:00.036Z');
  assert.equal(liveSandboxes(store)[0].attributes['startup.duration.plugin'], 'cri-startup-trace');
});

test('keeps containerd startup trace over startup-callchain trace spans', () => {
  const store = createLiveStore();
  recordLiveBatch(store, {
    source: 'containerd-startup-trace',
    metadata: {
      clusters: [],
      nodes: [],
      images: [],
      sandboxes: [{ id: 'k8s-default-containerd-demo-pod', nodeId: 'node-a', imageRef: 'pause:latest', runtimeType: 'runc' }],
    },
    metrics: [],
    events: [],
    traces: [{
      traceId: 'containerd-startup-demo',
      spanId: 'containerd-startup-demo-container-startup',
      spanName: 'container.startup',
      startTime: '2026-05-22T02:00:00.000Z',
      endTime: '2026-05-22T02:00:00.040Z',
      durationMs: 40,
      status: 'ok',
      attributes: { plugin: 'containerd-startup-trace', 'runtime.type': 'runc' },
      sandboxId: 'k8s-default-containerd-demo-pod',
      runtimeType: 'runc',
    }],
    profiles: [],
  });

  recordLiveBatch(store, {
    source: 'startup-callchain',
    metadata: { clusters: [], nodes: [], images: [], sandboxes: [] },
    metrics: [],
    events: [],
    traces: [{
      traceId: 'callchain-demo',
      spanId: 'callchain-demo-root',
      spanName: 'sandbox.startup.callchain',
      startTime: '2026-05-22T02:00:01.000Z',
      endTime: '2026-05-22T02:00:01.002Z',
      durationMs: 2,
      status: 'ok',
      attributes: { plugin: 'startup-callchain' },
      sandboxId: 'k8s-default-containerd-demo-pod',
      runtimeType: 'runc',
    }],
    profiles: [],
  });

  assert.equal(liveSandboxes(store)[0].startupDurationMs, 40);
  assert.equal(liveSandboxes(store)[0].startedAt, '2026-05-22T02:00:00.040Z');
  assert.equal(liveSandboxes(store)[0].attributes['startup.duration.plugin'], 'containerd-startup-trace');
});

test('applies stored cri startup trace when sandbox metadata arrives later', () => {
  const store = createLiveStore();
  recordLiveBatch(store, {
    source: 'cri-startup-trace',
    metadata: { clusters: [], nodes: [], images: [], sandboxes: [] },
    metrics: [{
      timestamp: '2026-05-22T02:00:01.500Z',
      name: 'sandbox.startup.e2e_duration_ms',
      value: 1500,
      unit: 'ms',
      group: 'startup',
      sandboxId: 'k8s-default-late-pod',
      nodeId: 'node-a',
      runtimeType: 'runc',
      attributes: { plugin: 'cri-startup-trace' },
    }],
    events: [],
    traces: [{
      traceId: 'cri-containerd-startup-k8s-default-late-pod',
      spanId: 'cri-containerd-startup-k8s-default-late-pod-e2e',
      spanName: 'sandbox.startup.e2e',
      startTime: '2026-05-22T02:00:00.000Z',
      endTime: '2026-05-22T02:00:01.500Z',
      durationMs: 1500,
      status: 'ok',
      attributes: { plugin: 'cri-startup-trace', 'runtime.type': 'runc' },
      sandboxId: 'k8s-default-late-pod',
      runtimeType: 'runc',
    }],
    profiles: [],
  });

  assert.equal(liveSandboxes(store).length, 0);

  recordLiveBatch(store, {
    source: 'containerd-events',
    metadata: {
      clusters: [],
      nodes: [],
      images: [],
      sandboxes: [{
        id: 'k8s-default-late-pod',
        nodeId: 'node-a',
        imageRef: 'pause:latest',
        runtimeType: 'runc',
        startupDurationMs: 0,
        attributes: {
          'startup.duration.source': 'event-required',
          'startup.duration.plugin': 'containerd-startup-trace',
        },
      }],
    },
    metrics: [],
    events: [],
    traces: [],
    profiles: [],
  });

  assert.equal(liveSandboxes(store)[0].startupDurationMs, 1500);
  assert.equal(liveSandboxes(store)[0].attributes['startup.duration.source'], 'trace');
  assert.equal(liveSandboxes(store)[0].attributes['startup.duration.plugin'], 'cri-startup-trace');
});

test('filters pseudo containerd image rows from live image list', async () => {
  const { liveImages } = await import('../src/liveData.mjs');
  const store = createLiveStore();
  recordLiveBatch(store, {
    source: 'test/containerd',
    metadata: {
      clusters: [],
      nodes: [],
      images: [
        {
          id: 'containerd-image-k8s-io-registry-k8s-io-pause-3-10',
          ref: 'registry.k8s.io/pause:3.10',
          digest: 'sha256:real',
          sizeBytes: 123,
          layerCount: 1,
        },
        {
          id: 'containerd-image-k8s-io-containerd-unknown-latest',
          ref: 'containerd/unknown:latest',
          digest: 'containerd:unknown',
        },
        {
          id: 'containerd-image-k8s-io-containerd-snapshot-k8s-io-sandboxabcdef',
          ref: 'containerd-snapshot:k8s.io:sandboxabcdef',
          digest: 'snapshot:sandboxabcdef',
        },
        {
          id: 'collector-collector-startup-callchain-unknown',
          ref: 'collector/startup-callchain:unknown',
          digest: 'collector:collector-collector-startup-callchain-unknown',
        },
      ],
      sandboxes: [],
    },
    metrics: [],
    events: [],
    traces: [],
    profiles: [],
  });

  assert.deepEqual(liveImages(store).map((image) => image.ref), ['registry.k8s.io/pause:3.10']);
});
