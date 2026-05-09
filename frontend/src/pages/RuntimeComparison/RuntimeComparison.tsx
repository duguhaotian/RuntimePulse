import { useEffect, useState } from 'react';
import type { RuntimePulseApi } from '../../api/RuntimePulseApi';
import type { RuntimeCompareRow } from '../../domain/model';
import { RuntimeBars } from '../../components/charts/RuntimeBars';
import { RuntimeBadge } from '../SandboxExplorer/SandboxExplorer';
import { formatBytes, formatDuration, formatRatio } from '../../utils/units';

type RuntimeComparisonProps = {
  api: RuntimePulseApi;
};

export function RuntimeComparison({ api }: RuntimeComparisonProps) {
  const [rows, setRows] = useState<RuntimeCompareRow[]>([]);

  useEffect(() => {
    api.compareRuntimes().then(setRows);
  }, [api]);

  return (
    <section className="page-stack">
      <div className="page-header">
        <div>
          <p className="eyebrow">Runtime analysis</p>
          <h2>Runtime Comparison</h2>
          <p>对比 runc、gVisor、Kata、Firecracker 的启动耗时、开销和失败率。</p>
        </div>
      </div>
      <div className="panel-card">
        <h3>Startup P95 and Runtime Overhead</h3>
        <RuntimeBars rows={rows} />
      </div>
      <div className="table-card">
        <table>
          <thead>
            <tr><th>Runtime</th><th>Samples</th><th>Startup P50</th><th>Startup P95</th><th>CPU Overhead</th><th>Memory Overhead</th><th>Failure Rate</th></tr>
          </thead>
          <tbody>
            {rows.map((row) => (
              <tr key={row.runtimeType}>
                <td><RuntimeBadge runtimeType={row.runtimeType} /></td>
                <td>{row.sampleCount.toLocaleString()}</td>
                <td>{formatDuration(row.startupP50Ms)}</td>
                <td className={row.startupP95Ms > 10_000 ? 'hot-value' : ''}>{formatDuration(row.startupP95Ms)}</td>
                <td>{formatRatio(row.cpuOverheadRatio)}</td>
                <td>{formatBytes(row.memoryOverheadBytes)}</td>
                <td className={row.failureRate > 0.02 ? 'hot-value' : ''}>{formatRatio(row.failureRate)}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </section>
  );
}
