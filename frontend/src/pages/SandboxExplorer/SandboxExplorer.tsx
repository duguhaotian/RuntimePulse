import { useEffect, useMemo, useState } from 'react';
import type { RuntimePulseApi } from '../../api/RuntimePulseApi';
import type { RuntimeType, Sandbox, SandboxStatus } from '../../domain/model';
import { runtimeColors } from '../../utils/colors';
import { formatBytes, formatDuration, formatRatio } from '../../utils/units';
import { formatDateTime } from '../../utils/time';

type SandboxExplorerProps = {
  api: RuntimePulseApi;
  onSelectSandbox: (id: string) => void;
};

export function SandboxExplorer({ api, onSelectSandbox }: SandboxExplorerProps) {
  const [runtimeType, setRuntimeType] = useState<RuntimeType | 'all'>('all');
  const [status, setStatus] = useState<SandboxStatus | 'all'>('all');
  const [text, setText] = useState('');
  const [sandboxes, setSandboxes] = useState<Sandbox[]>([]);

  useEffect(() => {
    api.listSandboxes({ runtimeType, status, text }).then(setSandboxes);
  }, [api, runtimeType, status, text]);

  const stats = useMemo(() => {
    const slow = sandboxes.filter((sandbox) => sandbox.startupDurationMs > 7000).length;
    const failed = sandboxes.filter((sandbox) => sandbox.status === 'failed').length;
    const avgStartup = sandboxes.reduce((sum, sandbox) => sum + sandbox.startupDurationMs, 0) / Math.max(sandboxes.length, 1);
    return { slow, failed, avgStartup };
  }, [sandboxes]);

  return (
    <section className="page-stack">
      <div className="page-header">
        <div>
          <p className="eyebrow">Runs table</p>
          <h2>Sandbox runs</h2>
          <p>像实验 run 一样查看每个 sandbox：配置、指标、事件和启动链路都围绕 run 聚合。</p>
        </div>
        <div className="header-actions">
          <button>Export CSV</button>
          <button className="primary">Create report</button>
        </div>
      </div>

      <div className="run-tabs">
        <button className="active">All runs <strong>{sandboxes.length}</strong></button>
        <button>Slow starts <strong>{stats.slow}</strong></button>
        <button>Failed <strong>{stats.failed}</strong></button>
        <button>Pinned <strong>3</strong></button>
      </div>

      <div className="summary-grid">
        <SummaryCard label="Matched runs" value={String(sandboxes.length)} caption="filtered sandbox instances" />
        <SummaryCard label="Slow starts" value={String(stats.slow)} caption="> 7s startup duration" tone="warning" />
        <SummaryCard label="Failed" value={String(stats.failed)} caption="runtime or guest failures" tone="danger" />
        <SummaryCard label="Avg startup" value={formatDuration(stats.avgStartup)} caption="across selected runs" />
      </div>

      <div className="filter-bar">
        <input value={text} onChange={(event) => setText(event.target.value)} placeholder="Filter runs by id, node, image, workload..." />
        <select value={runtimeType} onChange={(event) => setRuntimeType(event.target.value as RuntimeType | 'all')}>
          <option value="all">All runtimes</option>
          <option value="runc">runc</option>
          <option value="gvisor">gVisor</option>
          <option value="kata">Kata</option>
          <option value="firecracker">Firecracker</option>
        </select>
        <select value={status} onChange={(event) => setStatus(event.target.value as SandboxStatus | 'all')}>
          <option value="all">All status</option>
          <option value="running">Running</option>
          <option value="stopped">Stopped</option>
          <option value="failed">Failed</option>
        </select>
      </div>

      <div className="table-card runs-table-card">
        <div className="table-titlebar">
          <div>
            <strong>Runs</strong>
            <span>{sandboxes.length} matching sandbox runs</span>
          </div>
          <div className="column-pills">
            <span>metrics</span>
            <span>config</span>
            <span>system</span>
          </div>
        </div>
        <table>
          <thead>
            <tr>
              <th>Run</th>
              <th>Runtime</th>
              <th>Node</th>
              <th>Workload</th>
              <th>Image</th>
              <th>Status</th>
              <th>Created</th>
              <th>Startup</th>
              <th>CPU Avg</th>
              <th>Memory Peak</th>
              <th>Trend</th>
            </tr>
          </thead>
          <tbody>
            {sandboxes.map((sandbox) => (
              <tr key={sandbox.id} onClick={() => onSelectSandbox(sandbox.id)}>
                <td>
                  <div className="run-name-cell">
                    <span className="run-color" style={{ background: runtimeColors[sandbox.runtimeType] }} />
                    <div><strong>{sandbox.id}</strong><small>{sandbox.namespace} / {sandbox.workloadName}</small></div>
                  </div>
                </td>
                <td><RuntimeBadge runtimeType={sandbox.runtimeType} /></td>
                <td>{sandbox.nodeId}</td>
                <td>{sandbox.workloadName}</td>
                <td className="truncate">{sandbox.imageRef}</td>
                <td><StatusBadge status={sandbox.status} /></td>
                <td>{formatDateTime(sandbox.createdAt)}</td>
                <td className={sandbox.startupDurationMs > 7000 ? 'hot-value' : ''}>{formatDuration(sandbox.startupDurationMs)}</td>
                <td>{formatRatio(sandbox.cpuAvg)}</td>
                <td>{formatBytes(sandbox.memoryPeakBytes)}</td>
                <td><MiniTrend value={sandbox.startupDurationMs} /></td>
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

export function RuntimeBadge({ runtimeType }: { runtimeType: RuntimeType }) {
  return <span className="runtime-badge"><i style={{ background: runtimeColors[runtimeType] }} />{runtimeType}</span>;
}

function StatusBadge({ status }: { status: SandboxStatus }) {
  return <span className={`status-badge ${status}`}>{status}</span>;
}

function MiniTrend({ value }: { value: number }) {
  const normalized = Math.min(value / 32_000, 1);
  const bars = [0.22, 0.38, 0.31, normalized, normalized * 0.8 + 0.08];
  return (
    <div className="mini-trend" aria-label="startup trend">
      {bars.map((bar, index) => <i key={index} style={{ height: `${Math.max(15, bar * 100)}%` }} />)}
    </div>
  );
}
