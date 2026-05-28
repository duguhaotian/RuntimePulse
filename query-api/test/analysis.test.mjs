import assert from 'node:assert/strict';
import test from 'node:test';
import { buildSandboxAnalysis } from '../src/analysis.mjs';

const baseSandbox = {
  id: 'cri-sandbox-a',
  clusterId: 'local',
  nodeId: 'node-a',
  namespace: 'default',
  workloadId: 'pod-a',
  workloadName: 'pod-a',
  imageId: 'pause',
  imageRef: 'pause:latest',
  runtimeType: 'kata',
  runtimeVersion: 'kata-3',
  status: 'running',
  createdAt: '2026-05-22T02:00:00.000Z',
  startupDurationMs: 4000,
  cpuAvg: 0,
  memoryPeakBytes: 0,
  eventCount: 0,
  labels: {},
  attributes: {},
};

function metric(name, value) {
  return {
    id: `${name}-cri-sandbox-a`,
    name,
    label: name,
    unit: name.endsWith('_count') ? 'count' : 'ms',
    group: 'startup',
    sandboxId: 'cri-sandbox-a',
    runtimeType: 'kata',
    points: [{ timestamp: '2026-05-22T02:00:01.000Z', value }],
  };
}

test('analysis flags CNI-dominated startup callchain metrics', () => {
  const analysis = buildSandboxAnalysis({
    sandbox: baseSandbox,
    image: undefined,
    metrics: [
      metric('sandbox.startup.callchain_duration_ms', 4000),
      metric('sandbox.startup.cni_duration_ms', 2600),
      metric('sandbox.startup.cni_plugin_count', 3),
      metric('sandbox.startup.cni.plugin.bridge_duration_ms', 1800),
      metric('sandbox.startup.cni.plugin.bridge_count', 1),
      metric('sandbox.startup.cni.plugin.loopback_duration_ms', 200),
      metric('sandbox.startup.cni.plugin.loopback_count', 1),
    ],
    events: [],
    spans: [{
      traceId: 'cri-containerd-startup-cri-sandbox-a',
      spanId: 'span-cni-add',
      parentSpanId: 'root',
      sandboxId: 'cri-sandbox-a',
      spanName: 'cni.ADD',
      startTime: '2026-05-22T02:00:00.100Z',
      endTime: '2026-05-22T02:00:02.700Z',
      durationMs: 2600,
      status: 'ok',
      attributes: { 'process.binary': '/opt/cni/bin/bridge' },
    }],
    profiles: [],
  });

  const finding = analysis.findings.find((item) => item.id === 'cri-sandbox-a-startup-callchain-cni');
  assert.ok(finding);
  assert.equal(finding.title, 'bridge is the hottest CNI plugin binary');
  assert.equal(finding.severity, 'warning');
  assert.deepEqual(finding.relatedMetricNames, ['sandbox.startup.cni.plugin.bridge_duration_ms', 'sandbox.startup.cni.plugin.bridge_count']);
  assert.deepEqual(finding.relatedSpanIds, ['span-cni-add']);
});

test('analysis flags high helper-binary count even with small aggregate duration', () => {
  const analysis = buildSandboxAnalysis({
    sandbox: { ...baseSandbox, runtimeType: 'runc', startupDurationMs: 1800 },
    image: undefined,
    metrics: [
      metric('sandbox.startup.callchain_duration_ms', 1800),
      metric('sandbox.startup.helper_binary_duration_ms', 120),
      metric('sandbox.startup.helper_binary_count', 18),
    ],
    events: [],
    spans: [],
    profiles: [],
  });

  const finding = analysis.findings.find((item) => item.id === 'cri-sandbox-a-startup-callchain-helper-binaries');
  assert.ok(finding);
  assert.equal(finding.title, 'Helper binaries are high during startup');
  assert.equal(finding.severity, 'warning');
  assert.match(finding.summary, /helper binaries accounts|helper binaries executed/);
});

test('analysis names hottest startup process binary from breakdown metrics', () => {
  const bridgeDuration = metric('sandbox.startup.process.binary.bridge_duration_ms', 1600);
  bridgeDuration.attributes = { 'process.binary.name': 'bridge', 'process.roles': ['cni'] };
  const bridgeCount = metric('sandbox.startup.process.binary.bridge_count', 2);
  bridgeCount.attributes = { 'process.binary.name': 'bridge', 'process.roles': ['cni'] };
  const iptablesDuration = metric('sandbox.startup.process.binary.iptables_duration_ms', 300);
  iptablesDuration.attributes = { 'process.binary.name': 'iptables', 'process.roles': ['helper'] };

  const analysis = buildSandboxAnalysis({
    sandbox: { ...baseSandbox, runtimeType: 'runc', startupDurationMs: 3200 },
    image: undefined,
    metrics: [
      metric('sandbox.startup.callchain_duration_ms', 3200),
      metric('sandbox.startup.binary_exec_duration_ms', 2100),
      metric('sandbox.startup.binary_exec_count', 8),
      bridgeDuration,
      bridgeCount,
      iptablesDuration,
    ],
    events: [],
    spans: [],
    profiles: [],
  });

  const finding = analysis.findings.find((item) => item.id === 'cri-sandbox-a-startup-callchain-binary-exec');
  assert.ok(finding);
  assert.equal(finding.title, 'bridge is the hottest startup process binary');
  assert.match(finding.summary, /bridge accounts/);
  assert.deepEqual(finding.relatedMetricNames, [
    'sandbox.startup.process.binary.bridge_duration_ms',
    'sandbox.startup.process.binary.bridge_count',
  ]);
});
