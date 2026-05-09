import type { EventRecord } from '../../domain/model';
import { severityColors } from '../../utils/colors';
import { formatDateTime } from '../../utils/time';

type EventTimelineProps = {
  events: EventRecord[];
};

export function EventTimeline({ events }: EventTimelineProps) {
  return (
    <div className="event-timeline">
      {events.map((event) => (
        <article className={`event-item ${event.severity}`} key={event.id}>
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
