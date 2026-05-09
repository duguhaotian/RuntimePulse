import type { MetricSeries } from '../../domain/model';
import { chartPalette } from '../../utils/colors';
import { formatMetricValue } from '../../utils/units';
import { formatDateTime, formatTime, toMs } from '../../utils/time';

type MetricChartProps = {
  series: MetricSeries[];
  height?: number;
  markerTime?: string;
  markerLabel?: string;
};

export function MetricChart({ series, height = 220, markerTime, markerLabel }: MetricChartProps) {
  const width = 760;
  const padding = { top: 18, right: 22, bottom: 28, left: 58 };
  const allPoints = series.flatMap((item) => item.points.map((point) => point.value));
  const maxValue = Math.max(...allPoints, 1);
  const minValue = Math.min(...allPoints, 0);
  const firstSeries = series[0];
  const startTime = firstSeries?.points[0] ? toMs(firstSeries.points[0].timestamp) : 0;
  const endTime = firstSeries?.points.at(-1) ? toMs(firstSeries.points.at(-1)!.timestamp) : startTime;
  const markerX = markerTime && endTime > startTime ? padding.left + ((toMs(markerTime) - startTime) / (endTime - startTime)) * (width - padding.left - padding.right) : undefined;
  const markerVisible = markerX !== undefined && markerX >= padding.left && markerX <= width - padding.right;
  const markerPlotX = markerX ?? 0;

  const x = (index: number, count: number) => padding.left + (index / Math.max(count - 1, 1)) * (width - padding.left - padding.right);
  const y = (value: number) => padding.top + (1 - (value - minValue) / Math.max(maxValue - minValue, 1)) * (height - padding.top - padding.bottom);

  return (
    <div className="chart-card">
      <div className="chart-header">
        <div>
          <h3>{firstSeries?.group.toUpperCase() ?? 'Metrics'}</h3>
          <p>{series.map((item) => item.label).join(' · ')}</p>
        </div>
        <div className="legend">
        {series.map((item, index) => (
            <span key={item.id}><i style={{ background: chartPalette[index % chartPalette.length] }} />{item.label}</span>
          ))}
        </div>
      </div>
      <svg viewBox={`0 0 ${width} ${height}`} className="metric-svg" role="img">
        <line x1={padding.left} y1={padding.top} x2={padding.left} y2={height - padding.bottom} className="axis" />
        <line x1={padding.left} y1={height - padding.bottom} x2={width - padding.right} y2={height - padding.bottom} className="axis" />
        {[0, 0.5, 1].map((tick) => {
          const value = minValue + (maxValue - minValue) * tick;
          const yPos = y(value);
          return (
            <g key={tick}>
              <line x1={padding.left} y1={yPos} x2={width - padding.right} y2={yPos} className="grid-line" />
              <text x={padding.left - 10} y={yPos + 4} textAnchor="end" className="axis-label">{formatMetricValue(value, firstSeries?.unit ?? '')}</text>
            </g>
          );
        })}
        {firstSeries?.points.filter((_, index) => index % 12 === 0).map((point, index) => (
          <text key={point.timestamp} x={x(index * 12, firstSeries.points.length)} y={height - 8} textAnchor="middle" className="axis-label">{formatTime(point.timestamp)}</text>
        ))}
        {markerVisible && (
          <g>
            <line x1={markerPlotX} y1={padding.top} x2={markerPlotX} y2={height - padding.bottom} className="event-marker-line" />
            <circle cx={markerPlotX} cy={padding.top + 8} r="4" className="event-marker-dot" />
            <text x={Math.min(markerPlotX + 8, width - 145)} y={padding.top + 12} className="event-marker-label">
              {markerLabel ?? `event ${formatDateTime(markerTime!)}`}
            </text>
          </g>
        )}
        {series.map((item, index) => (
          <polyline
            key={item.id}
            points={item.points.map((point, pointIndex) => `${x(pointIndex, item.points.length)},${y(point.value)}`).join(' ')}
            fill="none"
            stroke={chartPalette[index % chartPalette.length]}
            strokeWidth="2.5"
            strokeLinejoin="round"
            strokeLinecap="round"
          />
        ))}
      </svg>
    </div>
  );
}
