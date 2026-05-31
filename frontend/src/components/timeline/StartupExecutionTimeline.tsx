import type { EventRecord, TraceSpan } from '../../domain/model';
import { formatDateTime, toMs } from '../../utils/time';
import { formatDuration } from '../../utils/units';

type StartupExecutionTimelineProps = {
  events: EventRecord[];
  spans: TraceSpan[];
  selectedEventId?: string;
  selectedSpanId?: string;
  onSelectEvent?: (event: EventRecord) => void;
  onSelectSpan?: (span: TraceSpan) => void;
};

type ExecutionItem = {
  id: string;
  kind: 'event' | 'span';
  lane: string;
  label: string;
  timestamp: string;
  start: number;
  end?: number;
  durationMs?: number;
  source: string;
  status: string;
  pointOnly: boolean;
  event?: EventRecord;
  span?: TraceSpan;
};

type WaterfallRow = {
  id: string;
  lane: string;
  label: string;
  start: number;
  end: number;
  durationMs: number;
  rangeMs: number;
  count: number;
  source: string;
  status: string;
  items: ExecutionItem[];
};

type KeyMarker = {
  id: string;
  label: string;
  time: number;
};

export function StartupExecutionTimeline({
  events,
  spans,
  selectedEventId,
  selectedSpanId,
  onSelectEvent,
  onSelectSpan,
}: StartupExecutionTimelineProps) {
  const items = buildExecutionItems(events, spans);
  if (items.length === 0) return <div className="empty-state">No startup execution events available.</div>;

  const baseline = findRunPodBaseline(items) ?? Math.min(...items.map((item) => item.start));
  const end = Math.max(...items.map((item) => item.end ?? item.start), baseline + 1);
  const duration = Math.max(end - baseline, 1);
  const rows = buildWaterfallRows(items);
  const lanes = groupRowsByLane(rows);
  const markers = buildKeyMarkers(items, rows, baseline, end);
  const detailItems = items
    .slice()
    .sort((left, right) => left.start - right.start || left.label.localeCompare(right.label));

  function selectItem(item: ExecutionItem) {
    if (item.event) onSelectEvent?.(item.event);
    if (item.span) onSelectSpan?.(item.span);
  }

  function selectRow(row: WaterfallRow) {
    const selectable = row.items.find((item) => item.span || item.event);
    if (selectable) selectItem(selectable);
  }

  return (
    <div className="startup-execution-timeline">
      <div className="execution-axis">
        <span>RunPod +0ms</span>
        <span>{formatDuration(duration)} window</span>
        <span>{formatDateTime(new Date(end).toISOString())}</span>
      </div>
      <div className="execution-overview-track" role="img" aria-label="Startup milestone overview">
        <div className="execution-overview-line" />
        {markers.map((marker) => {
          const left = ((marker.time - baseline) / duration) * 100;
          return (
            <div className="execution-marker" key={marker.id} style={{ left: `${clamp(left, 0, 100)}%` }}>
              <i />
              <span>{marker.label}</span>
              <em>{formatRelative(marker.time - baseline)}</em>
            </div>
          );
        })}
      </div>
      <div className="execution-waterfall">
        {lanes.map(([lane, laneRows]) => (
          <div className="execution-waterfall-lane" key={lane}>
            <div className="execution-waterfall-lane-title">
              <strong>{lane}</strong>
              <span>{laneRows.length} item{laneRows.length === 1 ? '' : 's'}</span>
            </div>
            <div className="execution-waterfall-rows">
              {laneRows.map((row) => {
                const left = ((row.start - baseline) / duration) * 100;
                const width = Math.max((row.rangeMs / duration) * 100, row.rangeMs <= 2 ? 0.8 : 1.6);
                const selected = row.items.some((item) => item.event?.id === selectedEventId || item.span?.spanId === selectedSpanId);
                return (
                  <button
                    className={`execution-waterfall-row ${row.status} ${selected ? 'selected' : ''}`}
                    key={row.id}
                    onClick={() => selectRow(row)}
                    title={`${formatRelative(row.start - baseline)} → ${formatRelative(row.end - baseline)} · ${row.label} · ${row.source}`}
                  >
                    <span className="execution-row-time">{formatRelative(row.start - baseline)}</span>
                    <span className="execution-row-label">{row.label}</span>
                    <span className="execution-row-meta">
                      {row.count > 1 && <b>×{row.count}</b>}
                      <em>{formatDuration(row.durationMs)}</em>
                    </span>
                    <span className="execution-row-track">
                      <i style={{ left: `${clamp(left, 0, 100)}%`, width: `${Math.min(width, 100 - clamp(left, 0, 100))}%` }} />
                    </span>
                  </button>
                );
              })}
            </div>
          </div>
        ))}
      </div>
      <div className="execution-event-list">
        <div className="execution-event-list-header">
          <strong>Raw startup events</strong>
          <span>{detailItems.length} records · showing first {Math.min(detailItems.length, 36)}</span>
        </div>
        {detailItems.slice(0, 36).map((item) => (
          <button className="execution-event-row" key={`${item.id}-row`} onClick={() => selectItem(item)}>
            <strong>{formatRelative(item.start - baseline)}</strong>
            <span>{item.lane}</span>
            <em>{item.label}</em>
          </button>
        ))}
      </div>
    </div>
  );
}

function buildExecutionItems(events: EventRecord[], spans: TraceSpan[]): ExecutionItem[] {
  const eventItems = events
    .filter(isStartupExecutionEvent)
    .map((event) => ({
      id: `event-${event.id}`,
      kind: 'event' as const,
      lane: laneFromName(event.eventName, event.attributes),
      label: labelFromEvent(event),
      timestamp: event.timestamp,
      start: toMs(event.timestamp),
      source: event.source,
      status: event.severity,
      pointOnly: true,
      event,
    }));

  const spanItems = spans
    .filter(isStartupExecutionSpan)
    .map((span) => {
      const start = toMs(span.startTime);
      const end = Math.max(toMs(span.endTime), start + Math.max(span.durationMs, 0));
      const pointOnly = isExecLikeSpan(span);
      return {
        id: `span-${span.spanId}`,
        kind: 'span' as const,
        lane: laneFromName(span.spanName, span.attributes),
        label: span.spanName,
        timestamp: span.startTime,
        start,
        end,
        durationMs: span.durationMs,
        source: String(span.attributes?.plugin ?? 'trace'),
        status: span.status,
        pointOnly,
        span,
      };
    });

  return [...eventItems, ...spanItems]
    .filter((item) => Number.isFinite(item.start))
    .sort((left, right) => left.start - right.start || left.label.localeCompare(right.label));
}

function buildWaterfallRows(items: ExecutionItem[]): WaterfallRow[] {
  const spanItems = items.filter((item) => item.kind === 'span' && item.span);
  const fallbackItems = spanItems.length > 0 ? spanItems : items;
  const groups = new Map<string, ExecutionItem[]>();

  for (const item of fallbackItems) {
    const key = aggregateKey(item);
    groups.set(key, [...(groups.get(key) ?? []), item]);
  }

  return Array.from(groups.entries())
    .map(([id, groupItems]) => {
      const start = Math.min(...groupItems.map((item) => item.start));
      const end = Math.max(...groupItems.map((item) => item.end ?? item.start + Math.max(item.durationMs ?? 1, 1)));
      const first = groupItems.slice().sort((left, right) => left.start - right.start)[0];
      const durationMs = groupItems.reduce((sum, item) => sum + Math.max(item.durationMs ?? ((item.end ?? item.start) - item.start), 1), 0);
      return {
        id,
        lane: first.lane,
        label: first.label,
        start,
        end: Math.max(end, start + 1),
        durationMs,
        rangeMs: Math.max(end - start, 1),
        count: groupItems.length,
        source: first.source,
        status: groupItems.some((item) => item.status === 'error' || item.status === 'critical') ? 'error' : first.status,
        items: groupItems,
      };
    })
    .sort((left, right) => left.start - right.start || lanePriority(left.lane) - lanePriority(right.lane) || left.label.localeCompare(right.label));
}

function aggregateKey(item: ExecutionItem) {
  const lower = item.label.toLowerCase();
  if (lower.startsWith('process.exec.') || lower.startsWith('cni.plugin.')) return `${item.lane}:${item.label}`;
  return item.id;
}

function buildKeyMarkers(items: ExecutionItem[], rows: WaterfallRow[], baseline: number, end: number): KeyMarker[] {
  const markers: KeyMarker[] = [];
  const addMarker = (id: string, label: string, time?: number) => {
    if (time === undefined || !Number.isFinite(time)) return;
    if (markers.some((marker) => Math.abs(marker.time - time) < 0.5 && marker.label === label)) return;
    markers.push({ id, label, time });
  };

  addMarker('runpod', 'RunPod', baseline);
  addMarker('cni', 'CNI', firstTime(rows, (row) => row.lane === 'CNI'));
  addMarker('runtime', 'Runtime', firstTime(rows, (row) => row.lane === 'Runtime' || row.lane === 'Kata'));
  addMarker('ready', 'Ready', firstTime(items, (item) => item.label.toLowerCase().includes('ready')) ?? end);

  return markers.sort((left, right) => left.time - right.time);
}

function firstTime<T extends { start: number }>(items: T[], predicate: (item: T) => boolean) {
  return items
    .filter(predicate)
    .map((item) => item.start)
    .sort((left, right) => left - right)[0];
}

function isStartupExecutionEvent(event: EventRecord) {
  const name = event.eventName.toLowerCase();
  if (name === 'startup.callchain.observed' || name.endsWith('.observed')) return false;
  return name.startsWith('startup.')
    || name.startsWith('cri.')
    || name.includes('runpod')
    || name.includes('sandbox')
    || name.includes('cni')
    || name.includes('runtime')
    || name.includes('oci');
}

function isStartupExecutionSpan(span: TraceSpan) {
  const plugin = String(span.attributes?.plugin ?? '');
  const name = span.spanName.toLowerCase();
  return plugin === 'startup-callchain'
    || plugin === 'cri-startup-trace'
    || name.includes('run_pod')
    || name.includes('startup')
    || name.includes('cni')
    || name.includes('oci')
    || name.includes('kata')
    || name.includes('runc')
    || name.includes('exec');
}

function isExecLikeSpan(span: TraceSpan) {
  const name = span.spanName.toLowerCase();
  const kind = String(span.attributes?.['startup.event.kind'] ?? '');
  return kind === 'uprobe'
    || name.startsWith('process.exec.')
    || name.startsWith('cni.plugin.')
    || name.startsWith('oci.')
    || name.startsWith('kata.');
}

function findRunPodBaseline(items: ExecutionItem[]) {
  return items
    .filter((item) => item.label.toLowerCase().includes('runpod') || item.label.toLowerCase().includes('run_pod'))
    .map((item) => item.start)
    .sort((left, right) => left - right)[0];
}

function groupRowsByLane(rows: WaterfallRow[]): Array<[string, WaterfallRow[]]> {
  const groups = new Map<string, WaterfallRow[]>();
  for (const row of rows) groups.set(row.lane, [...(groups.get(row.lane) ?? []), row]);
  return Array.from(groups.entries()).sort(([left], [right]) => {
    return lanePriority(left) - lanePriority(right) || left.localeCompare(right);
  });
}

function lanePriority(lane: string) {
  const priority = ['RunPod', 'CRI', 'CNI', 'CNI helpers', 'Runtime', 'Kata', 'Process'];
  const index = priority.indexOf(lane);
  return index === -1 ? 99 : index;
}

function laneFromName(name: string, attributes?: Record<string, unknown>) {
  const lower = name.toLowerCase();
  const role = String(attributes?.['process.role'] ?? '').toLowerCase();
  if (lower.includes('runpod') || lower.includes('run_pod')) return 'RunPod';
  if (lower.startsWith('cri.') || lower.includes('sandbox')) return 'CRI';
  if (lower.includes('cni') || role === 'cni') return 'CNI';
  if (role === 'helper' || lower.startsWith('process.exec.iptables') || lower.startsWith('process.exec.nft') || lower.startsWith('process.exec.ip')) return 'CNI helpers';
  if (lower.includes('kata') || role === 'kata') return 'Kata';
  if (lower.includes('runtime') || lower.includes('oci') || lower.includes('runc') || role === 'oci') return 'Runtime';
  return 'Process';
}

function labelFromEvent(event: EventRecord) {
  const binary = stringAttr(event.attributes, 'process.binary.name') ?? stringAttr(event.attributes, 'process.binary');
  const phase = stringAttr(event.attributes, 'startup.event.phase');
  return [event.eventName, binary, phase].filter(Boolean).join(' · ');
}

function stringAttr(attributes: Record<string, unknown>, key: string) {
  const value = attributes?.[key];
  return typeof value === 'string' && value.trim() ? value : undefined;
}

function formatRelative(value: number) {
  const prefix = value < 0 ? '-' : '+';
  return `${prefix}${formatDuration(Math.abs(value))}`;
}

function clamp(value: number, min: number, max: number) {
  return Math.min(max, Math.max(min, value));
}
