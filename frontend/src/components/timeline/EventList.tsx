import type { EventRecord } from '../../domain/model';
import { formatDateTime, toMs } from '../../utils/time';

type EventListProps = {
  emptyLabel?: string;
  events: EventRecord[];
  limit?: number;
};

export function EventList({ emptyLabel = 'No events available.', events, limit = 30 }: EventListProps) {
  const recentEvents = [...events]
    .sort((left, right) => toMs(right.timestamp) - toMs(left.timestamp))
    .slice(0, limit);

  if (recentEvents.length === 0) return <div className="empty-state">{emptyLabel}</div>;

  return (
    <div className="event-list">
      {recentEvents.map((event) => (
        <article className={`event-list-row ${event.severity}`} key={event.id}>
          <div className="event-list-marker" />
          <div>
            <div className="event-list-title">
              <strong>{event.eventName}</strong>
              <time>{formatDateTime(event.timestamp)}</time>
            </div>
            <p>{event.message}</p>
            <div className="event-list-meta">
              <span>{event.source}</span>
              {event.sandboxId && <span>{event.sandboxId}</span>}
              {event.imageId && <span>{event.imageId}</span>}
              {event.reason && <span>{event.reason}</span>}
            </div>
          </div>
        </article>
      ))}
    </div>
  );
}
