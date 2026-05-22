import assert from 'node:assert/strict';
import test from 'node:test';
import { createLiveStore, liveMetricsForSandbox, recordLiveBatch } from '../src/liveData.mjs';

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
