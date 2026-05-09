import { useEffect, useMemo, useState } from 'react';
import type { RuntimePulseApi } from '../../api/RuntimePulseApi';
import type { RuntimeType, Sandbox, SandboxStatus, TraceSpan } from '../../domain/model';
import { TraceWaterfall } from '../../components/trace/TraceWaterfall';
import { runtimeColors } from '../../utils/colors';
import { formatBytes, formatDuration, formatRatio } from '../../utils/units';
import { formatDateTime, toMs } from '../../utils/time';

type SavedExplorerView = {
  id: string;
  name: string;
  runtimeType: RuntimeType | 'all';
  status: SandboxStatus | 'all';
  text: string;
  selectedRunIds: string[];
};

const savedViewsStorageKey = 'runtimepulse.savedExplorerViews';

function loadSavedExplorerViews(): SavedExplorerView[] {
  if (typeof window === 'undefined') return [];

  try {
    const raw = window.localStorage.getItem(savedViewsStorageKey);
    if (!raw) return [];

    const parsed = JSON.parse(raw) as SavedExplorerView[];
    return Array.isArray(parsed) ? parsed.filter((item) => item && typeof item.id === 'string' && typeof item.name === 'string') : [];
  } catch {
    return [];
  }
}

type SandboxExplorerProps = {
  api: RuntimePulseApi;
  onSelectSandbox: (id: string) => void;
};

export function SandboxExplorer({ api, onSelectSandbox }: SandboxExplorerProps) {
  const [runtimeType, setRuntimeType] = useState<RuntimeType | 'all'>('all');
  const [status, setStatus] = useState<SandboxStatus | 'all'>('all');
  const [text, setText] = useState('');
  const [sandboxes, setSandboxes] = useState<Sandbox[]>([]);
  const [selectedRunIds, setSelectedRunIds] = useState<string[]>([]);
  const [selectedRunTraces, setSelectedRunTraces] = useState<Record<string, TraceSpan[]>>({});
  const [savedViews, setSavedViews] = useState<SavedExplorerView[]>(loadSavedExplorerViews);
  const [viewName, setViewName] = useState('');

  useEffect(() => {
    api.listSandboxes({ runtimeType, status, text }).then((nextSandboxes) => {
      setSandboxes(nextSandboxes);
      setSelectedRunIds((current) => current.filter((id) => nextSandboxes.some((sandbox) => sandbox.id === id)));
    });
  }, [api, runtimeType, status, text]);

  useEffect(() => {
    if (typeof window === 'undefined') return;
    window.localStorage.setItem(savedViewsStorageKey, JSON.stringify(savedViews));
  }, [savedViews]);

  const stats = useMemo(() => {
    const slow = sandboxes.filter((sandbox) => sandbox.startupDurationMs > 7000).length;
    const failed = sandboxes.filter((sandbox) => sandbox.status === 'failed').length;
    const avgStartup = sandboxes.reduce((sum, sandbox) => sum + sandbox.startupDurationMs, 0) / Math.max(sandboxes.length, 1);
    return { slow, failed, avgStartup };
  }, [sandboxes]);

  const selectedRuns = useMemo(() => sandboxes.filter((sandbox) => selectedRunIds.includes(sandbox.id)), [sandboxes, selectedRunIds]);
  const allVisibleSelected = sandboxes.length > 0 && selectedRunIds.length === sandboxes.length;

  useEffect(() => {
    let mounted = true;

    if (selectedRuns.length === 0) {
      setSelectedRunTraces({});
      return () => {
        mounted = false;
      };
    }

    Promise.all(selectedRuns.map(async (run) => [run.id, await api.getSandboxTrace(run.id)] as const)).then((entries) => {
      if (!mounted) return;
      setSelectedRunTraces(Object.fromEntries(entries));
    });

    return () => {
      mounted = false;
    };
  }, [api, selectedRuns]);

  function toggleRun(id: string) {
    setSelectedRunIds((current) => current.includes(id) ? current.filter((item) => item !== id) : [...current, id]);
  }

  function toggleAllVisible() {
    setSelectedRunIds(allVisibleSelected ? [] : sandboxes.map((sandbox) => sandbox.id));
  }

  function saveCurrentView() {
    const name = viewName.trim() || `${runtimeType === 'all' ? 'All runtimes' : runtimeType} · ${status === 'all' ? 'all status' : status}`;
    const nextView: SavedExplorerView = {
      id: `${Date.now()}`,
      name,
      runtimeType,
      status,
      text,
      selectedRunIds,
    };

    setSavedViews((current) => [nextView, ...current.filter((item) => item.name !== nextView.name)].slice(0, 6));
    setViewName('');
  }

  function applySavedView(view: SavedExplorerView) {
    setRuntimeType(view.runtimeType);
    setStatus(view.status);
    setText(view.text);
    setSelectedRunIds(view.selectedRunIds);
  }

  function deleteSavedView(id: string) {
    setSavedViews((current) => current.filter((item) => item.id !== id));
  }

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

      <div className="saved-views-panel">
        <div className="saved-views-header">
          <div>
            <strong>Saved views</strong>
            <span>Save common expert workflows with filter state and comparison set.</span>
          </div>
          <div className="saved-views-form">
            <input value={viewName} onChange={(event) => setViewName(event.target.value)} placeholder="Name this view" />
            <button className="primary" onClick={saveCurrentView}>Save current view</button>
          </div>
        </div>
        <div className="saved-views-list">
          {savedViews.length === 0 ? (
            <div className="saved-view-empty">No saved views yet. Save the current filter and selection state for quick recall.</div>
          ) : (
            savedViews.map((view) => (
              <article className="saved-view-card" key={view.id}>
                <div>
                  <strong>{view.name}</strong>
                  <span>{view.runtimeType === 'all' ? 'All runtimes' : view.runtimeType} · {view.status === 'all' ? 'all status' : view.status} · {view.selectedRunIds.length} selected</span>
                  {view.text && <small>query: {view.text}</small>}
                </div>
                <div className="saved-view-actions">
                  <button onClick={() => applySavedView(view)}>Apply</button>
                  <button onClick={() => deleteSavedView(view.id)}>Delete</button>
                </div>
              </article>
            ))
          )}
        </div>
      </div>

      {selectedRuns.length > 0 && (
        <div className="bulk-bar">
          <strong>{selectedRuns.length} runs selected</strong>
          <span>Compare startup, CPU, memory, and events across selected sandbox runs.</span>
          <button onClick={() => setSelectedRunIds([])}>Clear selection</button>
        </div>
      )}

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
              <th className="select-column"><input aria-label="Select all visible runs" checked={allVisibleSelected} type="checkbox" onChange={toggleAllVisible} /></th>
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
            {sandboxes.map((sandbox) => {
              const selected = selectedRunIds.includes(sandbox.id);
              return (
                <tr className={selected ? 'selected-row' : ''} key={sandbox.id} onClick={() => onSelectSandbox(sandbox.id)}>
                  <td className="select-column" onClick={(event) => event.stopPropagation()}>
                    <input aria-label={`Select ${sandbox.id}`} checked={selected} type="checkbox" onChange={() => toggleRun(sandbox.id)} />
                  </td>
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
              );
            })}
          </tbody>
        </table>
      </div>

      {selectedRuns.length > 0 && <RunComparePanel runs={selectedRuns} />}
      {selectedRuns.length > 0 && <RunTraceComparePanel runs={selectedRuns} traces={selectedRunTraces} />}
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

function RunComparePanel({ runs }: { runs: Sandbox[] }) {
  const maxStartup = Math.max(...runs.map((run) => run.startupDurationMs), 1);
  const maxMemory = Math.max(...runs.map((run) => run.memoryPeakBytes), 1);
  const avgCpu = runs.reduce((sum, run) => sum + run.cpuAvg, 0) / runs.length;
  const maxEvents = Math.max(...runs.map((run) => run.eventCount), 1);

  return (
    <div className="compare-panel">
      <div className="table-titlebar">
        <div>
          <strong>Compare selected runs</strong>
          <span>{runs.length} sandbox runs selected for side-by-side analysis</span>
        </div>
        <div className="column-pills">
          <span>startup</span>
          <span>resources</span>
          <span>events</span>
        </div>
      </div>
      <div className="compare-summary-grid">
        <SummaryCard label="Selected runs" value={String(runs.length)} caption="current comparison set" />
        <SummaryCard label="Max startup" value={formatDuration(maxStartup)} caption="slowest selected run" tone={maxStartup > 7000 ? 'warning' : undefined} />
        <SummaryCard label="Avg CPU" value={formatRatio(avgCpu)} caption="mean selected CPU" />
        <SummaryCard label="Max memory" value={formatBytes(maxMemory)} caption="peak selected memory" />
      </div>
      <div className="compare-bars">
        {runs.map((run) => (
          <article className="compare-run" key={run.id}>
            <div className="compare-run-header">
              <span className="run-color" style={{ background: runtimeColors[run.runtimeType] }} />
              <div>
                <strong>{run.id}</strong>
                <small>{run.runtimeType} · {run.nodeId} · {run.workloadName}</small>
              </div>
            </div>
            <CompareMetric label="Startup" value={formatDuration(run.startupDurationMs)} percent={run.startupDurationMs / maxStartup} hot={run.startupDurationMs > 7000} />
            <CompareMetric label="CPU" value={formatRatio(run.cpuAvg)} percent={run.cpuAvg} />
            <CompareMetric label="Memory" value={formatBytes(run.memoryPeakBytes)} percent={run.memoryPeakBytes / maxMemory} />
            <CompareMetric label="Events" value={String(run.eventCount)} percent={run.eventCount / maxEvents} hot={run.eventCount > 8} />
          </article>
        ))}
      </div>
    </div>
  );
}

function RunTraceComparePanel({ runs, traces }: { runs: Sandbox[]; traces: Record<string, TraceSpan[]> }) {
  const traceDurations = runs.map((run) => traceWindow(traces[run.id] ?? []));
  const maxTraceDuration = Math.max(...traceDurations, 1);
  const failedRuns = runs.filter((run) => run.status === 'failed').length;

  return (
    <div className="compare-panel trace-compare-panel">
      <div className="table-titlebar">
        <div>
          <strong>Compare startup waterfalls</strong>
          <span>Selected runs rendered with their startup trace spans for side-by-side inspection.</span>
        </div>
        <div className="column-pills">
          <span>trace</span>
          <span>startup</span>
          <span>outliers</span>
        </div>
      </div>
      <div className="compare-summary-grid">
        <SummaryCard label="Selected runs" value={String(runs.length)} caption="trace comparison set" />
        <SummaryCard label="Failed" value={String(failedRuns)} caption="runs with startup errors" tone={failedRuns > 0 ? 'danger' : undefined} />
        <SummaryCard label="Slowest trace" value={formatDuration(maxTraceDuration)} caption="widest startup trace window" />
        <SummaryCard label="Max startup" value={formatDuration(Math.max(...runs.map((run) => run.startupDurationMs), 1))} caption="slowest selected run" tone={Math.max(...runs.map((run) => run.startupDurationMs), 1) > 7000 ? 'warning' : undefined} />
      </div>
      <div className="trace-compare-grid">
        {runs.map((run) => {
          const runTraces = traces[run.id] ?? [];
          const traceDuration = traceWindow(runTraces);

          return (
            <article className="panel-card trace-compare-card" key={run.id}>
              <div className="trace-compare-header">
                <div className="compare-run-header">
                  <span className="run-color" style={{ background: runtimeColors[run.runtimeType] }} />
                  <div>
                    <strong>{run.id}</strong>
                    <small>{run.runtimeType} · {run.nodeId} · {run.workloadName}</small>
                  </div>
                </div>
                <div className="trace-compare-meta">
                  <span>{formatDuration(run.startupDurationMs)} startup</span>
                  <span>{formatDuration(traceDuration)} trace window</span>
                </div>
              </div>
              <TraceWaterfall spans={runTraces} />
            </article>
          );
        })}
      </div>
    </div>
  );
}

function traceWindow(spans: TraceSpan[]) {
  if (spans.length === 0) return 0;

  const start = Math.min(...spans.map((span) => toMs(span.startTime)));
  const end = Math.max(...spans.map((span) => toMs(span.endTime)));
  return Math.max(end - start, 1);
}

function CompareMetric({ label, value, percent, hot }: { label: string; value: string; percent: number; hot?: boolean }) {
  return (
    <div className="compare-metric">
      <div><span>{label}</span><strong className={hot ? 'hot-value' : ''}>{value}</strong></div>
      <div className="compare-track"><i style={{ width: `${Math.max(4, Math.min(percent, 1) * 100)}%` }} /></div>
    </div>
  );
}
