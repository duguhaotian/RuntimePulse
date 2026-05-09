import type { RuntimeCompareRow } from '../../domain/model';
import { runtimeColors } from '../../utils/colors';
import { formatBytes, formatDuration, formatRatio } from '../../utils/units';

type RuntimeBarsProps = {
  rows: RuntimeCompareRow[];
};

export function RuntimeBars({ rows }: RuntimeBarsProps) {
  const maxP95 = Math.max(...rows.map((row) => row.startupP95Ms), 1);
  return (
    <div className="runtime-bars">
      {rows.map((row) => (
        <div className="runtime-row" key={row.runtimeType}>
          <div className="runtime-name"><i style={{ background: runtimeColors[row.runtimeType] }} />{row.runtimeType}</div>
          <div className="bar-track">
            <div className="bar-fill" style={{ width: `${(row.startupP95Ms / maxP95) * 100}%`, background: runtimeColors[row.runtimeType] }} />
          </div>
          <div className="runtime-stat">P95 {formatDuration(row.startupP95Ms)}</div>
          <div className="runtime-stat">CPU +{formatRatio(row.cpuOverheadRatio)}</div>
          <div className="runtime-stat">Mem +{formatBytes(row.memoryOverheadBytes)}</div>
        </div>
      ))}
    </div>
  );
}
