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
  const groups = buildWaterfallGroups(rows);

  return (
    <div className="waterfall">
      {groups.map((group, groupIndex) => (
        <div className="waterfall-group" key={`${group.label}-${group.start}-${groupIndex}`}>
          {groups.length > 1 && (
            <div className="waterfall-group-header">
              <strong>{group.label}</strong>
              <span>{formatDuration(group.total)} window</span>
            </div>
          )}
          <div className="waterfall-scale">
            <span>group start</span>
            <span>{formatDuration(group.total)}</span>
          </div>
          {group.rows.map(({ span, depth }) => {
            const left = ((toMs(span.startTime) - group.start) / group.total) * 100;
            const duration = Math.max(span.durationMs, toMs(span.endTime) - toMs(span.startTime), 1);
            const width = (duration / group.total) * 100;
            return (
              <div className={`span-row ${selectedSpanId === span.spanId ? 'selected' : ''}`} key={span.spanId}>
                <button
                  className="span-name"
                  onClick={() => onSelectSpan?.(span)}
                  style={{ '--span-depth': depth } as React.CSSProperties}
                  title={`${span.spanName} · ${formatDuration(span.durationMs)} · ${span.startTime} → ${span.endTime}`}
                >
                  <span className="span-tree-marker" aria-hidden="true">{depth > 0 ? '↳' : ''}</span>
                  <span className="span-name-text">{span.spanName}</span>
                </button>
                <div className="span-track">
                  <button className={`span-bar ${span.status}`} onClick={() => onSelectSpan?.(span)} style={{ left: `${left}%`, width: `${Math.max(width, 3)}%` }}>
                    <span>{formatDuration(span.durationMs)}</span>
                  </button>
                </div>
              </div>
            );
          })}
        </div>
      ))}
    </div>
  );
}


type WaterfallGroup = {
  label: string;
  start: number;
  end: number;
  total: number;
  rows: WaterfallRow[];
};

function buildWaterfallGroups(rows: WaterfallRow[]): WaterfallGroup[] {
  const sortedRows = [...rows].sort((left, right) => compareSpans(left.span, right.span));
  if (sortedRows.length === 0) return [];

  const groups: WaterfallRow[][] = [];
  for (const row of sortedRows) {
    const start = toMs(row.span.startTime);
    const end = Math.max(toMs(row.span.endTime), start + Math.max(row.span.durationMs, 1));
    const current = groups[groups.length - 1];
    if (!current) {
      groups.push([row]);
      continue;
    }
    const currentStart = Math.min(...current.map((item) => toMs(item.span.startTime)));
    const currentEnd = Math.max(...current.map((item) => Math.max(toMs(item.span.endTime), toMs(item.span.startTime) + Math.max(item.span.durationMs, 1))));
    const currentDuration = Math.max(currentEnd - currentStart, 1);
    const gap = start - currentEnd;
    const gapThreshold = Math.max(250, currentDuration * 4);
    if (gap > gapThreshold) {
      groups.push([row]);
    } else {
      current.push(row);
    }
  }

  return groups.map((groupRows, index) => {
    const start = Math.min(...groupRows.map((row) => toMs(row.span.startTime)));
    const end = Math.max(...groupRows.map((row) => Math.max(toMs(row.span.endTime), toMs(row.span.startTime) + Math.max(row.span.durationMs, 1))));
    return {
      label: groupLabel(groupRows, index),
      start,
      end,
      total: Math.max(end - start, 1),
      rows: groupRows,
    };
  });
}

function groupLabel(rows: WaterfallRow[], index: number) {
  const plugins = new Set(rows.map(({ span }) => String(span.attributes?.plugin ?? '')).filter(Boolean));
  if (plugins.has('containerd-startup-trace')) return 'Container startup';
  if (plugins.has('startup-callchain')) return 'eBPF startup callchain';
  return `Trace group ${index + 1}`;
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
