import { useEffect, useMemo, useState } from 'react';
import type { RuntimePulseApi } from '../../api/RuntimePulseApi';
import type { IngestCounts, IngestStatus } from '../../domain/model';
import { formatDateTime } from '../../utils/time';

type CollectorStatusProps = {
  api: RuntimePulseApi;
};

export function CollectorStatus({ api }: CollectorStatusProps) {
  const [status, setStatus] = useState<IngestStatus>();
  const [error, setError] = useState<string>();

  useEffect(() => {
    let active = true;

    api.getIngestStatus()
      .then((nextStatus) => {
        if (!active) return;
        setStatus(nextStatus);
        setError(undefined);
      })
      .catch((nextError) => {
        if (!active) return;
        setError(nextError instanceof Error ? nextError.message : String(nextError));
      });

    return () => {
      active = false;
    };
  }, [api]);

  const totalRecords = useMemo(() => status ? totalCount(status.totals) : 0, [status]);
  const health = !status ? 'unknown' : status.rejectedBatches > 0 ? 'warning' : status.acceptedBatches > 0 ? 'ready' : 'unknown';

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
        </div>
      </header>

      <section className="summary-grid collector-summary-grid">
        <div className="summary-card">
          <span>Accepted batches</span>
          <strong>{status.acceptedBatches.toLocaleString()}</strong>
        </div>
        <div className={status.rejectedBatches > 0 ? 'summary-card warning' : 'summary-card'}>
          <span>Rejected batches</span>
          <strong>{status.rejectedBatches.toLocaleString()}</strong>
        </div>
        <div className="summary-card">
          <span>Records accepted</span>
          <strong>{totalRecords.toLocaleString()}</strong>
        </div>
        <div className="summary-card">
          <span>Sources</span>
          <strong>{status.sources.length.toLocaleString()}</strong>
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
              </div>
            </div>
          ) : (
            <div className="empty-inline">No accepted batch yet.</div>
          )}
        </div>
      </section>

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

function healthLabel(health: 'ready' | 'warning' | 'unknown') {
  if (health === 'ready') return 'Receiving';
  if (health === 'warning') return 'Warnings';
  return 'Waiting';
}
