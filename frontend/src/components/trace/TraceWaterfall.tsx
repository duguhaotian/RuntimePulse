import type { TraceSpan } from '../../domain/model';
import { formatDuration } from '../../utils/units';
import { toMs } from '../../utils/time';

type TraceWaterfallProps = {
  spans: TraceSpan[];
  selectedSpanId?: string;
  onSelectSpan?: (span: TraceSpan) => void;
};

type WaterfallRow = {
  span: TraceSpan;
  depth: number;
};

export function TraceWaterfall({ spans, selectedSpanId, onSelectSpan }: TraceWaterfallProps) {
  if (spans.length === 0) return <div className="empty-state">No trace spans.</div>;

  const rows = buildWaterfallRows(spans);
  const start = Math.min(...spans.map((span) => toMs(span.startTime)));
  const end = Math.max(...spans.map((span) => toMs(span.endTime)));
  const total = Math.max(end - start, 1);

  return (
    <div className="waterfall">
      <div className="waterfall-scale">
        <span>0 ms</span>
        <span>{formatDuration(total)}</span>
      </div>
      {rows.map(({ span, depth }) => {
        const left = ((toMs(span.startTime) - start) / total) * 100;
        const width = (span.durationMs / total) * 100;
        return (
          <div className={`span-row ${selectedSpanId === span.spanId ? 'selected' : ''}`} key={span.spanId}>
            <button
              className="span-name"
              onClick={() => onSelectSpan?.(span)}
              style={{ '--span-depth': depth } as React.CSSProperties}
              title={span.spanName}
            >
              <span className="span-tree-marker" aria-hidden="true">{depth > 0 ? '↳' : ''}</span>
              <span className="span-name-text">{span.spanName}</span>
            </button>
            <div className="span-track">
              <button className={`span-bar ${span.status}`} onClick={() => onSelectSpan?.(span)} style={{ left: `${left}%`, width: `${Math.max(width, 1.5)}%` }}>
                {formatDuration(span.durationMs)}
              </button>
            </div>
          </div>
        );
      })}
    </div>
  );
}

function buildWaterfallRows(spans: TraceSpan[]): WaterfallRow[] {
  const byParent = new Map<string, TraceSpan[]>();
  const byId = new Map(spans.map((span) => [span.spanId, span]));
  const sorted = [...spans].sort(compareSpans);

  for (const span of sorted) {
    const parentId = span.parentSpanId && byId.has(span.parentSpanId) ? span.parentSpanId : '__root__';
    const siblings = byParent.get(parentId) ?? [];
    siblings.push(span);
    byParent.set(parentId, siblings);
  }

  const rows: WaterfallRow[] = [];
  const visited = new Set<string>();
  const visit = (span: TraceSpan, depth: number) => {
    if (visited.has(span.spanId)) return;
    visited.add(span.spanId);
    rows.push({ span, depth });
    for (const child of byParent.get(span.spanId) ?? []) {
      visit(child, depth + 1);
    }
  };

  for (const root of byParent.get('__root__') ?? []) {
    visit(root, 0);
  }
  for (const span of sorted) {
    visit(span, 0);
  }
  return rows;
}

function compareSpans(left: TraceSpan, right: TraceSpan): number {
  return toMs(left.startTime) - toMs(right.startTime)
    || left.durationMs - right.durationMs
    || left.spanName.localeCompare(right.spanName)
    || left.spanId.localeCompare(right.spanId);
}
