import { useEffect, useMemo, useState } from 'react';
import type { RuntimePulseApi } from '../../api/RuntimePulseApi';
import type { RuntimeCompareRow, Sandbox, RuntimeType } from '../../domain/model';
import { RuntimeBars } from '../../components/charts/RuntimeBars';
import { RuntimeBadge } from '../SandboxExplorer/SandboxExplorer';
import { formatBytes, formatDuration, formatRatio } from '../../utils/units';
import { chartPalette, runtimeColors } from '../../utils/colors';

type RuntimeComparisonProps = {
  api: RuntimePulseApi;
};

type ComparisonScope = 'runtime' | 'node' | 'image';

type AggregateComparisonRow = {
  key: string;
  label: string;
  detail: string;
  sampleCount: number;
  startupP50Ms: number;
  startupP95Ms: number;
  cpuValue: number;
  memoryValueBytes: number;
  failureRate: number;
  color: string;
};

export function RuntimeComparison({ api }: RuntimeComparisonProps) {
  const [rows, setRows] = useState<RuntimeCompareRow[]>([]);
  const [sandboxes, setSandboxes] = useState<Sandbox[]>([]);
  const [nodeLabels, setNodeLabels] = useState<Record<string, string>>({});
  const [scope, setScope] = useState<ComparisonScope>('runtime');

  useEffect(() => {
    api.compareRuntimes().then(setRows);
    api.listSandboxes().then(setSandboxes);
  }, [api]);

  useEffect(() => {
    let mounted = true;
    const nodeIds = [...new Set(sandboxes.map((sandbox) => sandbox.nodeId))];

    if (nodeIds.length === 0) {
      setNodeLabels({});
      return () => {
        mounted = false;
      };
    }

    Promise.all(nodeIds.map(async (nodeId) => {
      const node = await api.getNode(nodeId);
      return [nodeId, node?.name ?? nodeId] as const;
    })).then((entries) => {
      if (!mounted) return;
      setNodeLabels(Object.fromEntries(entries));
    });

    return () => {
      mounted = false;
    };
  }, [api, sandboxes]);

  const aggregateRows = useMemo(() => {
    if (scope === 'runtime') return [];
    return buildAggregateRows(sandboxes, scope, nodeLabels);
  }, [sandboxes, scope, nodeLabels]);

  const currentRows = scope === 'runtime'
    ? rows.map((row) => ({
        key: row.runtimeType,
        label: row.runtimeType,
        detail: `${row.sampleCount.toLocaleString()} samples`,
        sampleCount: row.sampleCount,
        startupP50Ms: row.startupP50Ms,
        startupP95Ms: row.startupP95Ms,
        cpuValue: row.cpuOverheadRatio,
        memoryValueBytes: row.memoryOverheadBytes,
        failureRate: row.failureRate,
        color: runtimeColor(row.runtimeType),
      }))
    : aggregateRows;

  const currentSlowest = currentRows.reduce((slowest, row) => (row.startupP95Ms > slowest.startupP95Ms ? row : slowest), currentRows[0]);

  const currentTitle = scope === 'runtime'
    ? 'Runtime overhead comparison'
    : scope === 'node'
      ? 'Node overhead comparison'
      : 'Image overhead comparison';

  const currentDescription = scope === 'runtime'
    ? '对比 runc、gVisor、Kata、Firecracker 的启动耗时、开销和失败率。'
    : scope === 'node'
      ? '按节点聚合 sandbox 采样，观察不同节点上的启动窗口和资源压力。'
      : '按镜像聚合 sandbox 采样，观察镜像大小、层数和启动开销的相关性。';

  return (
    <section className="page-stack">
      <div className="page-header">
        <div>
          <p className="eyebrow">Runtime analysis</p>
          <h2>Runtime Comparison</h2>
          <p>{currentDescription}</p>
        </div>
      </div>
      <div className="comparison-mode-bar">
        <button className={scope === 'runtime' ? 'active' : ''} onClick={() => setScope('runtime')}>Runtimes</button>
        <button className={scope === 'node' ? 'active' : ''} onClick={() => setScope('node')}>Nodes</button>
        <button className={scope === 'image' ? 'active' : ''} onClick={() => setScope('image')}>Images</button>
      </div>
      <div className="panel-card comparison-overview-card">
        <div className="table-titlebar">
          <div>
            <strong>{currentTitle}</strong>
            <span>{scope === 'runtime' ? 'Starting point for runtime-wide overhead comparison.' : 'Aggregate sandbox records to highlight environment-specific overhead.'}</span>
          </div>
          <div className="column-pills">
            <span>{scope}</span>
            <span>{currentRows.length} groups</span>
          </div>
        </div>
        <div className="compare-summary-grid">
          <SummaryCard label="Groups" value={String(currentRows.length)} caption="current comparison rows" />
          <SummaryCard label="Slowest" value={currentSlowest ? formatDuration(currentSlowest.startupP95Ms) : '-'} caption={currentSlowest ? currentSlowest.label : 'no rows'} tone={currentSlowest?.startupP95Ms > 10_000 ? 'warning' : undefined} />
          <SummaryCard label="Max CPU" value={currentRows.length > 0 ? formatRatio(currentRows.reduce((max, row) => Math.max(max, row.cpuValue), 0)) : '-'} caption="highest CPU metric" />
          <SummaryCard label="Max failure" value={currentRows.length > 0 ? formatRatio(currentRows.reduce((max, row) => Math.max(max, row.failureRate), 0)) : '-'} caption="highest failure rate" tone={currentRows.some((row) => row.failureRate > 0.02) ? 'danger' : undefined} />
        </div>
        {scope === 'runtime' ? <RuntimeBars rows={rows} /> : <ComparisonBars rows={currentRows} />}
      </div>
      <div className="table-card comparison-table-card">
        <table>
          <thead>
            <tr>
              <th>{scope === 'runtime' ? 'Runtime' : scope === 'node' ? 'Node' : 'Image'}</th>
              <th>Samples</th>
              <th>Startup P50</th>
              <th>Startup P95</th>
              <th>CPU</th>
              <th>Memory</th>
              <th>Failure Rate</th>
            </tr>
          </thead>
          <tbody>
            {currentRows.map((row) => (
              <tr key={row.key}>
                <td>
                  {scope === 'runtime' ? (
                    <RuntimeBadge runtimeType={row.label as RuntimeType} />
                  ) : (
                    <div>
                      <strong>{row.label}</strong>
                      <small>{row.detail}</small>
                    </div>
                  )}
                </td>
                <td>{row.sampleCount.toLocaleString()}</td>
                <td>{formatDuration(row.startupP50Ms)}</td>
                <td className={row.startupP95Ms > 10_000 ? 'hot-value' : ''}>{formatDuration(row.startupP95Ms)}</td>
                <td>{formatRatio(row.cpuValue)}</td>
                <td>{formatBytes(row.memoryValueBytes)}</td>
                <td className={row.failureRate > 0.02 ? 'hot-value' : ''}>{formatRatio(row.failureRate)}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </section>
  );
}

function SummaryCard({ label, value, caption, tone }: { label: string; value: string; caption: string; tone?: 'warning' | 'danger' }) {
  return (
    <div className={`summary-card ${tone ?? ''}`}>
      <span>{label}</span>
      <strong>{value}</strong>
      <em>{caption}</em>
    </div>
  );
}

function ComparisonBars({ rows }: { rows: AggregateComparisonRow[] }) {
  const maxStartup = Math.max(...rows.map((row) => row.startupP95Ms), 1);

  return (
    <div className="runtime-bars comparison-bars-view">
      {rows.map((row, index) => (
        <div className="runtime-row" key={row.key}>
          <div className="runtime-name"><i style={{ background: row.color ?? chartPalette[index % chartPalette.length] }} />{row.label}</div>
          <div className="bar-track">
            <div className="bar-fill" style={{ width: `${(row.startupP95Ms / maxStartup) * 100}%`, background: row.color ?? chartPalette[index % chartPalette.length] }} />
          </div>
          <div className="runtime-stat">P95 {formatDuration(row.startupP95Ms)}</div>
          <div className="runtime-stat">CPU {formatRatio(row.cpuValue)}</div>
          <div className="runtime-stat">Mem {formatBytes(row.memoryValueBytes)}</div>
        </div>
      ))}
    </div>
  );
}

function buildAggregateRows(sandboxes: Sandbox[], scope: Exclude<ComparisonScope, 'runtime'>, nodeLabels: Record<string, string>): AggregateComparisonRow[] {
  const buckets = new Map<string, Sandbox[]>();

  sandboxes.forEach((sandbox) => {
    const key = scope === 'node' ? sandbox.nodeId : sandbox.imageRef;
    const current = buckets.get(key) ?? [];
    current.push(sandbox);
    buckets.set(key, current);
  });

  return [...buckets.entries()].map(([key, group], index) => {
    const samples = group.length;
    const startupValues = group.map((sandbox) => sandbox.startupDurationMs).sort((left, right) => left - right);
    const cpuValues = group.map((sandbox) => sandbox.cpuAvg);
    const memoryValues = group.map((sandbox) => sandbox.memoryPeakBytes);
    const failures = group.filter((sandbox) => sandbox.status === 'failed').length;
    const label = scope === 'node' ? nodeLabels[key] ?? key : key;
    const detail = scope === 'node' ? key : group[0]?.imageId ?? key;

    return {
      key,
      label,
      detail,
      sampleCount: samples,
      startupP50Ms: percentile(startupValues, 0.5),
      startupP95Ms: percentile(startupValues, 0.95),
      cpuValue: average(cpuValues),
      memoryValueBytes: Math.max(...memoryValues, 1),
      failureRate: failures / Math.max(samples, 1),
      color: chartPalette[index % chartPalette.length],
    };
  }).sort((left, right) => right.startupP95Ms - left.startupP95Ms);
}

function percentile(values: number[], ratio: number) {
  if (values.length === 0) return 0;
  const index = Math.min(values.length - 1, Math.max(0, Math.ceil(values.length * ratio) - 1));
  return values[index];
}

function average(values: number[]) {
  if (values.length === 0) return 0;
  return values.reduce((sum, value) => sum + value, 0) / values.length;
}

function runtimeColor(runtimeType: RuntimeType) {
  return runtimeColors[runtimeType];
}
