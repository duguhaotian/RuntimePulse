import { useMemo, useState } from 'react';
import type { EventRecord, Severity } from '../../domain/model';
import { severityColors } from '../../utils/colors';
import { formatDateTime, toMs } from '../../utils/time';
import { formatDuration } from '../../utils/units';

type EventTimelineProps = {
  events: EventRecord[];
  bounds?: {
    startTime: string;
    endTime: string;
  };
  selectedEventId?: string;
  onSelectEvent?: (event: EventRecord) => void;
};

type TimelineStage = {
  id: string;
  label: string;
  start: number;
  end: number;
  severity: Severity;
  events: EventRecord[];
};

export function EventTimeline({ bounds, events, selectedEventId, onSelectEvent }: EventTimelineProps) {
  const stages = useMemo(() => buildStages(events, bounds), [bounds, events]);
  const selectedStageFromEvent = selectedEventId ? stages.find((stage) => stage.events.some((event) => event.id === selectedEventId)) : undefined;
  const [selectedStageId, setSelectedStageId] = useState<string>();
  const selectedStage = stages.find((stage) => stage.id === (selectedStageId ?? selectedStageFromEvent?.id)) ?? stages[0];
  if (stages.length === 0) return <div className="empty-state">No lifecycle events available.</div>;

  const start = bounds ? toMs(bounds.startTime) : Math.min(...stages.map((stage) => stage.start));
  const end = Math.max(...stages.map((stage) => stage.end), start + 1);
  const duration = Math.max(end - start, 1);

  function selectStage(stage: TimelineStage) {
    setSelectedStageId(stage.id);
    onSelectEvent?.(stage.events[0]);
  }

  return (
    <div className="event-stage-timeline">
      <div className="stage-chart" role="img" aria-label="Lifecycle stage timeline">
        <div className="stage-axis">
          <span>{formatDateTime(new Date(start).toISOString())}</span>
          <span>{formatDuration(duration)}</span>
          <span>{formatDateTime(new Date(end).toISOString())}</span>
        </div>
        <div className="stage-waterfall">
          {stages.map((stage) => {
            const left = ((stage.start - start) / duration) * 100;
            const width = Math.max(5, ((stage.end - stage.start) / duration) * 100);
            const selected = selectedStage?.id === stage.id;

            return (
              <div className="stage-row" key={stage.id}>
                <div className="stage-row-label">
                  <i style={{ background: severityColors[stage.severity] }} />
                  <span>{stage.label}</span>
                </div>
                <div className="stage-row-track">
                  <button
                    className={`stage-segment ${selected ? 'selected' : ''}`}
                    onClick={() => selectStage(stage)}
                    style={{ left: `${left}%`, width: `${Math.min(width, 100 - left)}%`, borderColor: severityColors[stage.severity] }}
                    title={`${stage.label} · ${formatDuration(stage.end - stage.start)}`}
                  >
                    <span>{formatDuration(stage.end - stage.start)}</span>
                  </button>
                </div>
              </div>
            );
          })}
        </div>
      </div>

      {selectedStage && (
        <div className="stage-detail-card">
          <div className="stage-detail-header">
            <div>
              <strong>{selectedStage.label}</strong>
              <span>{formatDuration(selectedStage.end - selectedStage.start)} · {selectedStage.events.length} events</span>
            </div>
            <span className={`status-badge ${selectedStage.severity === 'error' ? 'failed' : selectedStage.severity === 'warning' ? 'stopped' : 'running'}`}>{selectedStage.severity}</span>
          </div>
          <div className="stage-event-list">
            {selectedStage.events.map((event) => (
              <button className={`stage-event ${selectedEventId === event.id ? 'selected' : ''}`} key={event.id} onClick={() => onSelectEvent?.(event)}>
                <span style={{ background: severityColors[event.severity] }} />
                <div>
                  <strong>{event.eventName}</strong>
                  <small>{formatDateTime(event.timestamp)} · {event.source}{event.reason ? ` · ${event.reason}` : ''}</small>
                  <p>{event.message}</p>
                </div>
              </button>
            ))}
          </div>
        </div>
      )}
    </div>
  );
}

function buildStages(events: EventRecord[], bounds?: EventTimelineProps['bounds']): TimelineStage[] {
  const boundsStart = bounds ? toMs(bounds.startTime) : undefined;
  const boundsEnd = bounds ? toMs(bounds.endTime) : undefined;
  const sorted = [...events]
    .filter((event) => {
      const timestamp = toMs(event.timestamp);
      return (boundsStart === undefined || timestamp >= boundsStart) && (boundsEnd === undefined || timestamp <= boundsEnd);
    })
    .sort((left, right) => toMs(left.timestamp) - toMs(right.timestamp));
  const stageMap = new Map<string, EventRecord[]>();

  sorted.forEach((event) => {
    const phase = phaseLabel(event.eventName);
    stageMap.set(phase, [...(stageMap.get(phase) ?? []), event]);
  });

  return Array.from(stageMap.entries()).map(([phase, phaseEvents], index, allStages) => {
    const start = Math.min(...phaseEvents.map((event) => toMs(event.timestamp)));
    const nextStageEvents = allStages[index + 1]?.[1] ?? [];
    const nextStageStart = nextStageEvents.length > 0 ? Math.min(...nextStageEvents.map((event) => toMs(event.timestamp))) : undefined;
    const eventEnd = Math.max(...phaseEvents.map((event) => toMs(event.timestamp)));
    const inferredEnd = Math.max(nextStageStart ?? eventEnd + 250, eventEnd + 250);
    const end = Math.max(start + 1, Math.min(inferredEnd, boundsEnd ?? inferredEnd));
    const severity = phaseEvents.some((event) => event.severity === 'error') ? 'error' : phaseEvents.some((event) => event.severity === 'warning') ? 'warning' : 'info';

    return {
      id: `${phase}-stage`,
      label: phase,
      start,
      end,
      severity,
      events: phaseEvents,
    };
  });
}

function phaseLabel(eventName: string) {
  if (eventName.includes('image.pull')) return 'Image pull';
  if (eventName.includes('image.unpack')) return 'Image unpack';
  if (eventName.includes('runtime.create')) return 'Runtime create';
  if (eventName.includes('microvm') || eventName.includes('guest.agent') || eventName.includes('ready')) return 'Guest ready';
  if (eventName.includes('container')) return 'Container start';
  if (eventName.includes('node')) return 'Node signal';
  return 'Sandbox create';
}
