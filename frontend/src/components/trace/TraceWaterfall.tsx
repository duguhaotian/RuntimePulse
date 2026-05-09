import type { TraceSpan } from '../../domain/model';
import { formatDuration } from '../../utils/units';
import { toMs } from '../../utils/time';

type TraceWaterfallProps = {
  spans: TraceSpan[];
};

export function TraceWaterfall({ spans }: TraceWaterfallProps) {
  if (spans.length === 0) return <div className="empty-state">No trace spans.</div>;

  const start = Math.min(...spans.map((span) => toMs(span.startTime)));
  const end = Math.max(...spans.map((span) => toMs(span.endTime)));
  const total = Math.max(end - start, 1);

  return (
    <div className="waterfall">
      <div className="waterfall-scale">
        <span>0 ms</span>
        <span>{formatDuration(total)}</span>
      </div>
      {spans.map((span) => {
        const left = ((toMs(span.startTime) - start) / total) * 100;
        const width = (span.durationMs / total) * 100;
        return (
          <div className="span-row" key={span.spanId}>
            <div className="span-name">{span.spanName}</div>
            <div className="span-track">
              <div className={`span-bar ${span.status}`} style={{ left: `${left}%`, width: `${Math.max(width, 1.5)}%` }}>
                {formatDuration(span.durationMs)}
              </div>
            </div>
          </div>
        );
      })}
    </div>
  );
}
