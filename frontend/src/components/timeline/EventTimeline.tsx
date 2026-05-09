import type { EventRecord } from '../../domain/model';
import { severityColors } from '../../utils/colors';
import { formatDateTime } from '../../utils/time';

type EventTimelineProps = {
  events: EventRecord[];
  selectedEventId?: string;
  onSelectEvent?: (event: EventRecord) => void;
};

export function EventTimeline({ events, selectedEventId, onSelectEvent }: EventTimelineProps) {
  return (
    <div className="event-timeline">
      {events.map((event) => (
        <article className={`event-item ${event.severity} ${selectedEventId === event.id ? 'selected' : ''}`} key={event.id} onClick={() => onSelectEvent?.(event)}>
          <div className="event-dot" style={{ background: severityColors[event.severity] }} />
          <div className="event-body">
            <div className="event-title">
              <span>{event.eventName}</span>
              <time>{formatDateTime(event.timestamp)}</time>
            </div>
            <p>{event.message}</p>
            <div className="event-meta">
              <span>{event.severity}</span>
              <span>{event.source}</span>
              {event.reason && <span>{event.reason}</span>}
            </div>
          </div>
        </article>
      ))}
    </div>
  );
}
