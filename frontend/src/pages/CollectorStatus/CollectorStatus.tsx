import { useEffect, useMemo, useState } from 'react';
import type { RuntimePulseApi } from '../../api/RuntimePulseApi';
import type { IngestCounts, IngestRecent, IngestStatus } from '../../domain/model';
import { formatDateTime } from '../../utils/time';
import { formatDuration, formatMetricValue } from '../../utils/units';

type CollectorStatusProps = {
  api: RuntimePulseApi;
};

type Snapshot = {
  timestamp: string;
  acceptedBatches: number;
  totalRecords: number;
};

type RecentTab = 'metrics' | 'events' | 'traces' | 'profiles';

const refreshIntervalMs = 5000;
const maxSnapshots = 12;

export function CollectorStatus({ api }: CollectorStatusProps) {
  const [status, setStatus] = useState<IngestStatus>();
  const [recent, setRecent] = useState<IngestRecent>();
  const [error, setError] = useState<string>();
  const [lastRefreshAt, setLastRefreshAt] = useState<string>();
  const [snapshots, setSnapshots] = useState<Snapshot[]>([]);
  const [refreshTick, setRefreshTick] = useState(0);
  const [recentTab, setRecentTab] = useState<RecentTab>('metrics');
  const [selectedSource, setSelectedSource] = useState('all');

  useEffect(() => {
    let active = true;
    let timer: number | undefined;

    const refresh = () => {
      Promise.all([api.getIngestStatus(), api.getIngestRecent()])
        .then(([nextStatus, nextRecent]) => {
          if (!active) return;
          const refreshedAt = new Date().toISOString();
          const nextTotalRecords = totalCount(nextStatus.totals);

          setStatus(nextStatus);
          setRecent(nextRecent);
          setError(undefined);
          setLastRefreshAt(refreshedAt);
          setRefreshTick((current) => current + 1);
          setSnapshots((current) => [
            ...current,
            {
              timestamp: refreshedAt,
              acceptedBatches: nextStatus.acceptedBatches,
              totalRecords: nextTotalRecords,
            },
          ].slice(-maxSnapshots));
        })
        .catch((nextError) => {
          if (!active) return;
          setError(nextError instanceof Error ? nextError.message : String(nextError));
        });
    };

    refresh();
    timer = window.setInterval(refresh, refreshIntervalMs);

    return () => {
      active = false;
      if (timer) window.clearInterval(timer);
    };
  }, [api]);

  const totalRecords = useMemo(() => status ? totalCount(status.totals) : 0, [status]);
  const batchDelta = useMemo(() => deltaFor(snapshots, 'acceptedBatches'), [snapshots]);
  const recordDelta = useMemo(() => deltaFor(snapshots, 'totalRecords'), [snapshots]);
  const latestBatchDelta = useMemo(() => latestDeltaFor(snapshots, 'acceptedBatches'), [snapshots]);
  const latestRecordDelta = useMemo(() => latestDeltaFor(snapshots, 'totalRecords'), [snapshots]);
  const lastBatchAge = status?.lastAcceptedBatch ? ageText(status.lastAcceptedBatch.acceptedAt, refreshTick) : 'no batch';
  const health = !status ? 'unknown' : status.rejectedBatches > 0 ? 'warning' : status.acceptedBatches > 0 ? 'ready' : 'unknown';
  const visibleRecent = useMemo(() => recent ? filterRecentBySource(recent, selectedSource) : undefined, [recent, selectedSource]);

  if (error) {
    return (
      <div className="page-stack">
        <header className="page-header">
          <div>
            <p className="eyebrow">Collectors</p>
            <h2>Ingest status</h2>
            <p>Collector pipeline status and validation counters.</p>
          </div>
        </header>
        <section className="panel-card collector-empty-state">
          <strong>Unable to load ingest status</strong>
          <span>{error}</span>
        </section>
      </div>
    );
  }

  if (!status) {
    return (
      <div className="page-stack">
        <header className="page-header">
          <div>
            <p className="eyebrow">Collectors</p>
            <h2>Ingest status</h2>
            <p>Loading collector pipeline counters.</p>
          </div>
        </header>
        <section className="panel-card collector-empty-state">
          <strong>Loading ingest status</strong>
          <span>Waiting for the Query API status snapshot.</span>
        </section>
      </div>
    );
  }

  return (
    <div className="page-stack">
      <header className="page-header">
        <div>
          <p className="eyebrow">Collectors</p>
          <h2>Ingest status</h2>
          <p>Validation-only collector flow from mock node collector to Query API.</p>
        </div>
        <div className={`collector-health ${health}`}>
          <span className={`status-dot ${health}`} />
          <strong>{healthLabel(health)}</strong>
          {lastRefreshAt && <em>{formatDateTime(lastRefreshAt)}</em>}
        </div>
      </header>

      <section className="summary-grid collector-summary-grid">
        <div className="summary-card">
          <span>Accepted batches</span>
          <strong>{status.acceptedBatches.toLocaleString()}</strong>
          <em>{formatSigned(latestBatchDelta)} last refresh</em>
        </div>
        <div className={status.rejectedBatches > 0 ? 'summary-card warning' : 'summary-card'}>
          <span>Rejected batches</span>
          <strong>{status.rejectedBatches.toLocaleString()}</strong>
        </div>
        <div className="summary-card">
          <span>Records accepted</span>
          <strong>{totalRecords.toLocaleString()}</strong>
          <em>{formatSigned(latestRecordDelta)} last refresh</em>
        </div>
        <div className="summary-card">
          <span>Last batch age</span>
          <strong>{lastBatchAge}</strong>
          <em>{status.sources.length.toLocaleString()} source</em>
        </div>
      </section>

      <section className="panel-card collector-panel">
        <div className="section-heading">
          <div>
            <h3>Ingest activity</h3>
            <p>Auto-refreshes every 5 seconds from the Query API status endpoint.</p>
          </div>
          <span>Last {snapshots.length} samples</span>
        </div>
        <div className="collector-trend-grid">
          <TrendPreview label="Accepted batches" snapshots={snapshots} field="acceptedBatches" delta={batchDelta} latestDelta={latestBatchDelta} />
          <TrendPreview label="Accepted records" snapshots={snapshots} field="totalRecords" delta={recordDelta} latestDelta={latestRecordDelta} />
        </div>
      </section>

      <section className="collector-grid">
        <div className="panel-card collector-panel">
          <div className="section-heading">
            <div>
              <h3>Accepted records</h3>
              <p>Current in-memory totals since Query API start.</p>
            </div>
            <span>{formatDateTime(status.startedAt)}</span>
          </div>
          <div className="collector-count-grid">
            <CountTile label="Nodes" value={status.totals.metadata.nodes} />
            <CountTile label="Sandboxes" value={status.totals.metadata.sandboxes} />
            <CountTile label="Metrics" value={status.totals.metrics} />
            <CountTile label="Events" value={status.totals.events} />
            <CountTile label="Traces" value={status.totals.traces} />
            <CountTile label="Profiles" value={status.totals.profiles} />
          </div>
        </div>

        <div className="panel-card collector-panel">
          <div className="section-heading">
            <div>
              <h3>Last accepted batch</h3>
              <p>Most recent batch accepted by the validation boundary.</p>
            </div>
          </div>
          {status.lastAcceptedBatch ? (
            <div className="batch-card">
              <strong>{status.lastAcceptedBatch.source}</strong>
              <span>Accepted {formatDateTime(status.lastAcceptedBatch.acceptedAt)}</span>
              {status.lastAcceptedBatch.observedAt && <span>Observed {formatDateTime(status.lastAcceptedBatch.observedAt)}</span>}
              <div className="batch-count-row">
                <CountPill label="metrics" value={status.lastAcceptedBatch.counts.metrics} />
                <CountPill label="events" value={status.lastAcceptedBatch.counts.events} />
                <CountPill label="traces" value={status.lastAcceptedBatch.counts.traces} />
                <CountPill label="profiles" value={status.lastAcceptedBatch.counts.profiles} />
              </div>
            </div>
          ) : (
            <div className="empty-inline">No accepted batch yet.</div>
          )}
        </div>
      </section>

      {status.liveStore && (
        <section className="panel-card collector-panel">
          <div className="section-heading">
            <div>
              <h3>Live query store</h3>
              <p>In-memory query objects currently merged into cluster, node, sandbox, runtime, and detail API responses.</p>
            </div>
            <span>{status.liveStore.lastUpdatedAt ? `Updated ${ageText(status.liveStore.lastUpdatedAt, refreshTick)} ago` : `${status.liveStore.metricPoints.toLocaleString()} points`}</span>
          </div>
          <div className="collector-count-grid live-store-grid">
            <CountTile label="Clusters" value={status.liveStore.clusters} />
            <CountTile label="Nodes" value={status.liveStore.nodes} />
            <CountTile label="Images" value={status.liveStore.images} />
            <CountTile label="Sandboxes" value={status.liveStore.sandboxes} />
            <CountTile label="Metric series" value={status.liveStore.metricSeries} />
            <CountTile label="Metric points" value={status.liveStore.metricPoints} />
            <CountTile label="Events" value={status.liveStore.events} />
            <CountTile label="Traces" value={status.liveStore.traces} />
            <CountTile label="Profiles" value={status.liveStore.profiles} />
          </div>
          <div className="store-limit-row">
            <span>Metric point retention: {status.liveStore.limits.metricPointsPerSeries.toLocaleString()} per series</span>
            <span>Event/profile retention: {status.liveStore.limits.rowsPerKind.toLocaleString()} rows per kind</span>
            {status.liveStore.lastUpdatedAt && <span>Last update: {formatDateTime(status.liveStore.lastUpdatedAt)}</span>}
          </div>
        </section>
      )}

      <section className="table-card">
        <div className="table-header">
          <h3>Collector sources</h3>
          <span>{status.mode}</span>
        </div>
        <table>
          <thead>
            <tr>
              <th>Source</th>
              <th>Batches</th>
              <th>Metrics</th>
              <th>Events</th>
              <th>Traces</th>
              <th>Profiles</th>
              <th>Last accepted</th>
            </tr>
          </thead>
          <tbody>
            {status.sources.map((source) => (
              <tr key={source.source}>
                <td><strong>{source.source}</strong></td>
                <td>{source.acceptedBatches.toLocaleString()}</td>
                <td>{source.totals.metrics.toLocaleString()}</td>
                <td>{source.totals.events.toLocaleString()}</td>
                <td>{source.totals.traces.toLocaleString()}</td>
                <td>{source.totals.profiles.toLocaleString()}</td>
                <td>{formatDateTime(source.lastAcceptedAt)}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </section>

      {status.lastRejectedBatch && (
        <section className="panel-card collector-panel warning-panel">
          <div className="section-heading">
            <div>
              <h3>Last rejected batch</h3>
              <p>Latest validation error retained by the Query API process.</p>
            </div>
            <span>{formatDateTime(status.lastRejectedBatch.rejectedAt)}</span>
          </div>
          <div className="error-list">
            {status.lastRejectedBatch.source && <strong>{status.lastRejectedBatch.source}</strong>}
            {status.lastRejectedBatch.errors.map((item) => <span key={item}>{item}</span>)}
          </div>
        </section>
      )}

      {recent && (
        <section className="panel-card collector-panel">
          <div className="section-heading">
            <div>
              <h3>Recent ingest samples</h3>
              <p>Bounded in-memory preview of the latest accepted collector payloads.</p>
            </div>
            <span>{visibleRecent?.recentBatches.length ?? 0} batches</span>
          </div>
          <div className="recent-toolbar">
            <label>
              Source
              <select value={selectedSource} onChange={(event) => setSelectedSource(event.target.value)}>
                <option value="all">All sources</option>
                {status.sources.map((source) => (
                  <option key={source.source} value={source.source}>{source.source}</option>
                ))}
              </select>
            </label>
          </div>
          <div className="recent-tab-bar" role="tablist" aria-label="Recent ingest sample kind">
            <button className={recentTab === 'metrics' ? 'active' : ''} onClick={() => setRecentTab('metrics')}>
              Metrics <span>{visibleRecent?.recentMetrics.length ?? 0}</span>
            </button>
            <button className={recentTab === 'events' ? 'active' : ''} onClick={() => setRecentTab('events')}>
              Events <span>{visibleRecent?.recentEvents.length ?? 0}</span>
            </button>
            <button className={recentTab === 'traces' ? 'active' : ''} onClick={() => setRecentTab('traces')}>
              Trace spans <span>{visibleRecent?.recentTraces.length ?? 0}</span>
            </button>
            <button className={recentTab === 'profiles' ? 'active' : ''} onClick={() => setRecentTab('profiles')}>
              Profiles <span>{visibleRecent?.recentProfiles.length ?? 0}</span>
            </button>
          </div>
          {visibleRecent && recentTab === 'metrics' && <RecentMetricList recent={visibleRecent} />}
          {visibleRecent && recentTab === 'events' && <RecentEventList recent={visibleRecent} />}
          {visibleRecent && recentTab === 'traces' && <RecentTraceList recent={visibleRecent} />}
          {visibleRecent && recentTab === 'profiles' && <RecentProfileList recent={visibleRecent} />}
        </section>
      )}
    </div>
  );
}

function CountTile({ label, value }: { label: string; value: number }) {
  return (
    <div className="count-tile">
      <span>{label}</span>
      <strong>{value.toLocaleString()}</strong>
    </div>
  );
}

function CountPill({ label, value }: { label: string; value: number }) {
  return (
    <span className="count-pill">
      {label}
      <b>{value.toLocaleString()}</b>
    </span>
  );
}

function RecentMetricList({ recent }: { recent: IngestRecent }) {
  return (
    <div className="recent-sample-table">
      <table>
        <thead>
          <tr>
            <th>Metric</th>
            <th>Value</th>
            <th>Scope</th>
            <th>Group</th>
            <th>Accepted</th>
          </tr>
        </thead>
        <tbody>
          {recent.recentMetrics.slice(0, 18).map((metric, index) => (
            <tr key={`${metric.acceptedAt}-${metric.name}-${index}`}>
              <td><strong>{metric.name}</strong></td>
              <td>{formatMetricValue(metric.value, metric.unit ?? '')}</td>
              <td>{metric.sandboxId ?? metric.nodeId ?? metric.source}</td>
              <td>{metric.group ?? '-'}</td>
              <td>{formatDateTime(metric.acceptedAt)}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

function RecentEventList({ recent }: { recent: IngestRecent }) {
  return (
    <div className="recent-sample-table">
      <table>
        <thead>
          <tr>
            <th>Event</th>
            <th>Severity</th>
            <th>Scope</th>
            <th>Message</th>
            <th>Accepted</th>
          </tr>
        </thead>
        <tbody>
          {recent.recentEvents.slice(0, 18).map((event) => (
            <tr key={event.id}>
              <td><strong>{event.eventName}</strong></td>
              <td><span className={`severity-pill severity-${event.severity}`}>{event.severity}</span></td>
              <td>{event.sandboxId ?? event.nodeId ?? event.source}</td>
              <td>{event.message}</td>
              <td>{formatDateTime(event.acceptedAt)}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

function RecentTraceList({ recent }: { recent: IngestRecent }) {
  return (
    <div className="recent-sample-table">
      <table>
        <thead>
          <tr>
            <th>Span</th>
            <th>Duration</th>
            <th>Sandbox</th>
            <th>Status</th>
            <th>Accepted</th>
          </tr>
        </thead>
        <tbody>
          {recent.recentTraces.slice(0, 18).map((span) => (
            <tr key={`${span.traceId}-${span.spanId}-${span.acceptedAt}`}>
              <td><strong>{span.spanName}</strong></td>
              <td>{formatMetricValue(span.durationMs, 'ms')}</td>
              <td>{span.sandboxId ?? span.traceId}</td>
              <td>{span.status}</td>
              <td>{formatDateTime(span.acceptedAt)}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

function RecentProfileList({ recent }: { recent: IngestRecent }) {
  return (
    <div className="recent-sample-table">
      <table>
        <thead>
          <tr>
            <th>Profile</th>
            <th>Type</th>
            <th>Duration</th>
            <th>Samples</th>
            <th>Sandbox</th>
            <th>Accepted</th>
          </tr>
        </thead>
        <tbody>
          {recent.recentProfiles.slice(0, 18).map((profile) => (
            <tr key={`${profile.id}-${profile.acceptedAt}`}>
              <td><strong>{profile.processRole}</strong><small>{profile.objectUri}</small></td>
              <td>{profile.profileType}</td>
              <td>{formatDuration(profile.durationMs)}</td>
              <td>{profile.sampleCount.toLocaleString()}</td>
              <td>{profile.sandboxId}</td>
              <td>{formatDateTime(profile.acceptedAt)}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

function filterRecentBySource(recent: IngestRecent, source: string): IngestRecent {
  if (source === 'all') return recent;

  return {
    ...recent,
    recentBatches: recent.recentBatches.filter((batch) => batch.source === source),
    recentMetrics: recent.recentMetrics.filter((metric) => metric.source === source),
    recentEvents: recent.recentEvents.filter((event) => event.source === source),
    recentTraces: recent.recentTraces.filter((span) => span.source === source),
    recentProfiles: recent.recentProfiles.filter((profile) => profile.source === source),
  };
}

function totalCount(counts: IngestCounts) {
  return counts.metadata.clusters
    + counts.metadata.nodes
    + counts.metadata.images
    + counts.metadata.sandboxes
    + counts.metrics
    + counts.events
    + counts.traces
    + counts.profiles;
}

function deltaFor(snapshots: Snapshot[], field: 'acceptedBatches' | 'totalRecords') {
  if (snapshots.length < 2) return 0;
  return snapshots[snapshots.length - 1][field] - snapshots[0][field];
}

function latestDeltaFor(snapshots: Snapshot[], field: 'acceptedBatches' | 'totalRecords') {
  if (snapshots.length < 2) return 0;
  return snapshots[snapshots.length - 1][field] - snapshots[snapshots.length - 2][field];
}

function TrendPreview({
  label,
  snapshots,
  field,
  delta,
  latestDelta,
}: {
  label: string;
  snapshots: Snapshot[];
  field: 'acceptedBatches' | 'totalRecords';
  delta: number;
  latestDelta: number;
}) {
  const values = snapshots.map((snapshot) => snapshot[field]);
  const min = Math.min(...values, 0);
  const max = Math.max(...values, 1);
  const span = Math.max(1, max - min);

  return (
    <div className="collector-trend-card">
      <div>
        <span>{label}</span>
        <strong>{delta > 0 ? `+${delta.toLocaleString()}` : delta.toLocaleString()}</strong>
        <em>{formatSigned(latestDelta)} last refresh</em>
      </div>
      <div className="collector-spark-bars" aria-label={`${label} trend`}>
        {snapshots.length === 0 ? (
          <i style={{ height: '12%' }} />
        ) : snapshots.map((snapshot, index) => (
          <i
            key={`${snapshot.timestamp}-${field}-${index}`}
            style={{ height: `${Math.max(12, ((snapshot[field] - min) / span) * 88 + 12)}%` }}
          />
        ))}
      </div>
    </div>
  );
}

function formatSigned(value: number) {
  return value > 0 ? `+${value.toLocaleString()}` : value.toLocaleString();
}

function ageText(timestamp: string, tick: number) {
  void tick;
  const ageMs = Date.now() - Date.parse(timestamp);
  if (!Number.isFinite(ageMs) || ageMs < 0) return 'now';
  if (ageMs < 1000) return 'now';
  if (ageMs < 60_000) return `${Math.round(ageMs / 1000)}s`;
  return `${Math.round(ageMs / 60_000)}m`;
}

function healthLabel(health: 'ready' | 'warning' | 'unknown') {
  if (health === 'ready') return 'Receiving';
  if (health === 'warning') return 'Warnings';
  return 'Waiting';
}
