import { useRef, useState, type PointerEvent as ReactPointerEvent } from 'react';
import type { MetricSeries } from '../../domain/model';
import { chartPalette } from '../../utils/colors';
import { formatMetricValue } from '../../utils/units';
import { formatDateTime, formatTime, toMs } from '../../utils/time';

type MetricChartProps = {
  series: MetricSeries[];
  height?: number;
  markerTime?: string;
  markerLabel?: string;
  pinned?: boolean;
  onTogglePin?: () => void;
  selectable?: boolean;
  selectedRange?: { from: string; to: string };
  onSelectRange?: (range: { from: string; to: string }) => void;
  stacked?: boolean;
};

export function MetricChart({
  height = 220,
  markerLabel,
  markerTime,
  onSelectRange,
  onTogglePin,
  pinned = false,
  selectable = false,
  selectedRange,
  series,
  stacked = false,
}: MetricChartProps) {
  const chartRef = useRef<HTMLDivElement>(null);
  const [hoverIndex, setHoverIndex] = useState<number | null>(null);
  const [dragStartIndex, setDragStartIndex] = useState<number | null>(null);
  const [dragEndIndex, setDragEndIndex] = useState<number | null>(null);
  const width = 760;
  const padding = { top: 18, right: 22, bottom: 28, left: 58 };
  const stackedTotals = stacked ? buildStackedTotals(series) : [];
  const allPoints = stacked ? stackedTotals : series.flatMap((item) => item.points.map((point) => point.value));
  const maxValue = Math.max(...allPoints, 1);
  const minValue = stacked ? 0 : Math.min(...allPoints, 0);
  const firstSeries = series[0];
  const startTime = firstSeries?.points[0] ? toMs(firstSeries.points[0].timestamp) : 0;
  const endTime = firstSeries?.points.at(-1) ? toMs(firstSeries.points.at(-1)!.timestamp) : startTime;
  const markerX = markerTime && endTime > startTime ? padding.left + ((toMs(markerTime) - startTime) / (endTime - startTime)) * (width - padding.left - padding.right) : undefined;
  const markerVisible = markerX !== undefined && markerX >= padding.left && markerX <= width - padding.right;
  const markerPlotX = markerX ?? 0;

  const x = (index: number, count: number) => padding.left + (index / Math.max(count - 1, 1)) * (width - padding.left - padding.right);
  const y = (value: number) => padding.top + (1 - (value - minValue) / Math.max(maxValue - minValue, 1)) * (height - padding.top - padding.bottom);
  const pointCount = firstSeries?.points.length ?? 0;
  const hoverPoint = hoverIndex === null ? undefined : firstSeries?.points[hoverIndex];
  const hoverX = hoverIndex === null || pointCount === 0 ? undefined : x(hoverIndex, pointCount);
  const tooltipLeft = hoverX === undefined ? 0 : Math.min(Math.max((hoverX / width) * 100, 18), 82);
  const countAxis = firstSeries?.unit === 'count';
  const yTicks = buildTicks(minValue, maxValue, countAxis);
  const selectedRangeX = selectedRange && endTime > startTime
    ? {
      from: padding.left + ((toMs(selectedRange.from) - startTime) / (endTime - startTime)) * (width - padding.left - padding.right),
      to: padding.left + ((toMs(selectedRange.to) - startTime) / (endTime - startTime)) * (width - padding.left - padding.right),
    }
    : undefined;
  const dragRangeX = dragStartIndex !== null && dragEndIndex !== null && pointCount > 0
    ? {
      from: x(Math.min(dragStartIndex, dragEndIndex), pointCount),
      to: x(Math.max(dragStartIndex, dragEndIndex), pointCount),
    }
    : undefined;

  const indexFromPointer = (event: ReactPointerEvent<HTMLDivElement>) => {
    if (!chartRef.current || pointCount === 0) return undefined;
    const bounds = chartRef.current.getBoundingClientRect();
    const relativeX = ((event.clientX - bounds.left) / bounds.width) * width;
    const plotStart = padding.left;
    const plotEnd = width - padding.right;
    const clampedX = Math.min(Math.max(relativeX, plotStart), plotEnd);
    const ratio = (clampedX - plotStart) / Math.max(plotEnd - plotStart, 1);
    return Math.round(ratio * Math.max(pointCount - 1, 0));
  };

  const handlePointerMove = (event: ReactPointerEvent<HTMLDivElement>) => {
    const nextIndex = indexFromPointer(event);
    if (nextIndex === undefined) return;

    setHoverIndex(nextIndex);
    if (dragStartIndex !== null) setDragEndIndex(nextIndex);
  };

  const handlePointerDown = (event: ReactPointerEvent<HTMLDivElement>) => {
    if (!selectable) return;
    const nextIndex = indexFromPointer(event);
    if (nextIndex === undefined) return;
    event.currentTarget.setPointerCapture(event.pointerId);
    setDragStartIndex(nextIndex);
    setDragEndIndex(nextIndex);
  };

  const handlePointerUp = (event: ReactPointerEvent<HTMLDivElement>) => {
    if (!selectable || dragStartIndex === null || dragEndIndex === null || !firstSeries) return;
    event.currentTarget.releasePointerCapture(event.pointerId);
    const fromIndex = Math.min(dragStartIndex, dragEndIndex);
    const toIndex = Math.max(dragStartIndex, dragEndIndex);
    const from = firstSeries.points[fromIndex]?.timestamp;
    const to = firstSeries.points[toIndex]?.timestamp;
    setDragStartIndex(null);
    setDragEndIndex(null);
    if (from && to && from !== to) onSelectRange?.({ from, to });
  };

  return (
    <div
      ref={chartRef}
      className={`chart-card metric-chart-card ${pinned ? 'pinned' : ''} ${selectable ? 'selectable' : ''}`}
      onPointerDown={handlePointerDown}
      onPointerMove={handlePointerMove}
      onPointerUp={handlePointerUp}
      onPointerLeave={() => {
        setHoverIndex(null);
        if (dragStartIndex !== null) setDragEndIndex(dragStartIndex);
      }}
    >
      <div className="chart-header">
        <div>
          <h3>{firstSeries?.group.toUpperCase() ?? 'Metrics'}</h3>
          <p>{series.map((item) => item.label).join(' · ')}</p>
        </div>
        <div className="chart-header-actions">
          {onTogglePin && <button className={pinned ? 'active' : ''} onClick={onTogglePin}>{pinned ? 'Pinned' : 'Pin'}</button>}
          <div className="legend">
            {series.map((item, index) => (
              <span key={item.id}><i style={{ background: chartPalette[index % chartPalette.length] }} />{item.label}</span>
            ))}
          </div>
        </div>
      </div>
      <svg viewBox={`0 0 ${width} ${height}`} className="metric-svg" role="img">
        <line x1={padding.left} y1={padding.top} x2={padding.left} y2={height - padding.bottom} className="axis" />
        <line x1={padding.left} y1={height - padding.bottom} x2={width - padding.right} y2={height - padding.bottom} className="axis" />
        {yTicks.map((value) => {
          const yPos = y(value);
          return (
            <g key={value}>
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
        {[selectedRangeX, dragRangeX].filter((range): range is { from: number; to: number } => Boolean(range)).map((range, index) => {
          const left = Math.max(padding.left, Math.min(range.from, range.to));
          const right = Math.min(width - padding.right, Math.max(range.from, range.to));
          return (
            <rect
              key={index}
              x={left}
              y={padding.top}
              width={Math.max(right - left, 0)}
              height={height - padding.top - padding.bottom}
              className={index === 0 ? 'selected-range-fill' : 'drag-range-fill'}
            />
          );
        })}
        {hoverX !== undefined && (
          <line x1={hoverX} y1={padding.top} x2={hoverX} y2={height - padding.bottom} className="hover-marker-line" />
        )}
        {stacked ? renderStackedSeries(series, y, x) : series.map((item, index) => {
          const color = chartPalette[index % chartPalette.length];

          return (
            <g key={item.id}>
              <polyline
                points={item.points.map((point, pointIndex) => `${x(pointIndex, item.points.length)},${y(point.value)}`).join(' ')}
                fill="none"
                stroke={color}
                strokeWidth="2.5"
                strokeLinejoin="round"
                strokeLinecap="round"
              />
              {hoverIndex !== null && item.points[hoverIndex] && (
                <circle cx={x(hoverIndex, item.points.length)} cy={y(item.points[hoverIndex].value)} r="3.5" fill={color} className="hover-point" />
              )}
            </g>
          );
        })}
      </svg>
      {hoverPoint && (
        <div className="metric-tooltip" style={{ left: `${tooltipLeft}%` }}>
          <strong>{formatDateTime(hoverPoint.timestamp)}</strong>
          {stacked && (
            <span>
              <i style={{ background: '#f8fafc' }} />
              <b>Total</b>
              {formatMetricValue(series.reduce((sum, item) => sum + (item.points[hoverIndex ?? 0]?.value ?? 0), 0), firstSeries?.unit ?? '')}
            </span>
          )}
          {series.map((item, index) => {
            const point = item.points[hoverIndex ?? 0];
            if (!point) return null;

            return (
              <span key={item.id}>
                <i style={{ background: chartPalette[index % chartPalette.length] }} />
                <b>{item.label}</b>
                {formatMetricValue(point.value, item.unit)}
              </span>
            );
          })}
        </div>
      )}
    </div>
  );
}

function renderStackedSeries(
  series: MetricSeries[],
  y: (value: number) => number,
  x: (index: number, count: number) => number,
) {
  const pointCount = series[0]?.points.length ?? 0;
  const previous = Array.from({ length: pointCount }, () => 0);

  return series.map((item, index) => {
    const color = chartPalette[index % chartPalette.length];
    const bottom = previous.map((value) => value);
    const top = item.points.map((point, pointIndex) => {
      previous[pointIndex] += point.value;
      return previous[pointIndex];
    });
    const topPoints = top.map((value, pointIndex) => `${x(pointIndex, pointCount)},${y(value)}`).join(' ');
    const bottomPoints = bottom.map((value, pointIndex) => `${x(pointIndex, pointCount)},${y(value)}`).reverse().join(' ');

    return (
      <g key={item.id}>
        <polygon points={`${topPoints} ${bottomPoints}`} fill={color} opacity="0.28" />
        <polyline
          points={top.map((value, pointIndex) => `${x(pointIndex, pointCount)},${y(value)}`).join(' ')}
          fill="none"
          stroke={color}
          strokeWidth="2.5"
          strokeLinejoin="round"
          strokeLinecap="round"
        />
      </g>
    );
  });
}

function buildStackedTotals(series: MetricSeries[]) {
  const pointCount = series[0]?.points.length ?? 0;
  return Array.from({ length: pointCount }, (_, pointIndex) => series.reduce((sum, item) => sum + (item.points[pointIndex]?.value ?? 0), 0));
}

function buildTicks(minValue: number, maxValue: number, countAxis: boolean) {
  if (!countAxis) return [0, 0.5, 1].map((tick) => minValue + (maxValue - minValue) * tick);

  const min = Math.floor(Math.max(0, minValue));
  const max = Math.ceil(maxValue);
  const step = Math.max(1, Math.ceil((max - min) / 2));
  return Array.from(new Set([min, min + step, max])).filter((tick) => tick <= max);
}
