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
  const lanes = groupItemsByLane(items);

  function selectItem(item: ExecutionItem) {
    if (item.event) onSelectEvent?.(item.event);
    if (item.span) onSelectSpan?.(item.span);
  }

  return (
    <div className="startup-execution-timeline">
      <div className="execution-axis">
        <span>RunPod +0ms</span>
        <span>{formatDuration(duration)} window</span>
        <span>{formatDateTime(new Date(end).toISOString())}</span>
      </div>
      <div className="execution-lanes">
        {lanes.map(([lane, laneItems]) => (
          <div className="execution-lane" key={lane}>
            <div className="execution-lane-label">{lane}</div>
            <div className="execution-lane-track">
              {laneItems.map((item) => {
                const left = ((item.start - baseline) / duration) * 100;
                const width = item.pointOnly ? 0 : Math.max((((item.end ?? item.start) - item.start) / duration) * 100, 1.5);
                const selected = item.event?.id === selectedEventId || item.span?.spanId === selectedSpanId;
                return (
                  <button
                    className={`execution-item ${item.kind} ${item.status} ${item.pointOnly ? 'point' : 'span'} ${selected ? 'selected' : ''}`}
                    key={item.id}
                    onClick={() => selectItem(item)}
                    style={{ left: `${clamp(left, 0, 100)}%`, width: item.pointOnly ? undefined : `${Math.min(width, 100 - clamp(left, 0, 100))}%` }}
                    title={`${formatRelative(item.start - baseline)} · ${item.label} · ${item.source}`}
                  >
                    <span>{item.pointOnly ? formatRelative(item.start - baseline) : item.label}</span>
                  </button>
                );
              })}
            </div>
          </div>
        ))}
      </div>
      <div className="execution-event-list">
        {items.slice().sort((left, right) => left.start - right.start).slice(0, 24).map((item) => (
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

function isStartupExecutionEvent(event: EventRecord) {
  const name = event.eventName.toLowerCase();
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

function groupItemsByLane(items: ExecutionItem[]): Array<[string, ExecutionItem[]]> {
  const priority = ['RunPod', 'CRI', 'CNI', 'Runtime', 'Kata', 'Process'];
  const groups = new Map<string, ExecutionItem[]>();
  for (const item of items) groups.set(item.lane, [...(groups.get(item.lane) ?? []), item]);
  return Array.from(groups.entries()).sort(([left], [right]) => {
    const leftIndex = priority.indexOf(left);
    const rightIndex = priority.indexOf(right);
    return (leftIndex === -1 ? 99 : leftIndex) - (rightIndex === -1 ? 99 : rightIndex) || left.localeCompare(right);
  });
}

function laneFromName(name: string, attributes?: Record<string, unknown>) {
  const lower = name.toLowerCase();
  const role = String(attributes?.['process.role'] ?? '').toLowerCase();
  if (lower.includes('runpod') || lower.includes('run_pod')) return 'RunPod';
  if (lower.startsWith('cri.') || lower.includes('sandbox')) return 'CRI';
  if (lower.includes('cni') || role === 'cni') return 'CNI';
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
