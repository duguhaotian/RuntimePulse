import { useEffect, useMemo, useState } from 'react';
import type { ArtifactKind, RuntimePulseArtifact } from '../../domain/model';
import type { RuntimePulseApi } from '../../api/RuntimePulseApi';
import { formatDateTime } from '../../utils/time';
import { formatBytes, formatDuration } from '../../utils/units';

type ArtifactsProps = {
  api: RuntimePulseApi;
};

export function Artifacts({ api }: ArtifactsProps) {
  const [artifacts, setArtifacts] = useState<RuntimePulseArtifact[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string>();
  const [kind, setKind] = useState<ArtifactKind | 'all'>('all');
  const [text, setText] = useState('');
  const [activeArtifact, setActiveArtifact] = useState<RuntimePulseArtifact>();

  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    api.listArtifacts({ kind, text })
      .then((rows) => {
        if (cancelled) return;
        setArtifacts(rows);
        setError(undefined);
      })
      .catch((caught) => {
        if (cancelled) return;
        setError(caught instanceof Error ? caught.message : String(caught));
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    return () => { cancelled = true; };
  }, [api, kind, text]);

  const summary = useMemo(() => artifactSummary(artifacts), [artifacts]);

  return (
    <div className="page-stack artifacts-page">
      <section className="page-header split">
        <div>
          <p className="eyebrow">Artifact workspace</p>
          <h2>Profiles & diagnostics</h2>
          <p>Browse collector-backed pprof/flamegraph references and runtime diagnostic bundle indexes.</p>
        </div>
        <div className="collector-health">
          <span className={`status-dot ${error ? 'warning' : 'ready'}`} />
          <strong>{error ? 'API error' : loading ? 'Refreshing' : 'Live artifacts'}</strong>
          <em>{error ?? `${artifacts.length.toLocaleString()} rows`}</em>
        </div>
      </section>

      <section className="summary-grid artifacts-summary-grid">
        <SummaryCard label="Artifacts" value={summary.total.toLocaleString()} caption="profiles + diagnostics" />
        <SummaryCard label="Profiles" value={summary.profiles.toLocaleString()} caption="profile artifact indexes" />
        <SummaryCard label="Diagnostics" value={summary.diagnostics.toLocaleString()} caption="bundle / inspect / log indexes" />
        <SummaryCard label="Warnings" value={summary.warnings.toLocaleString()} caption="warning or error severity" hot={summary.warnings > 0} />
      </section>

      <section className="filter-bar artifacts-filter-bar">
        <input value={text} onChange={(event) => setText(event.target.value)} placeholder="Filter id, sandbox, node, URI, source" />
        <select value={kind} onChange={(event) => setKind(event.target.value as ArtifactKind | 'all')}>
          <option value="all">All artifact kinds</option>
          <option value="profile">Profiles</option>
          <option value="diagnostic">Diagnostics</option>
        </select>
      </section>

      <section className="table-card artifacts-table-card">
        <div className="table-titlebar">
          <div>
            <strong>Artifact index</strong>
            <span>Object URIs point to raw payloads outside the Query API.</span>
          </div>
        </div>
        {artifacts.length === 0 ? (
          <div className="empty-state">{loading ? 'Loading artifacts…' : 'No artifacts matched the current filters.'}</div>
        ) : (
          <table>
            <thead>
              <tr>
                <th>Artifact</th>
                <th>Kind</th>
                <th>Scope</th>
                <th>Runtime</th>
                <th>Size / Samples</th>
                <th>Duration</th>
                <th>Source</th>
                <th>Open</th>
              </tr>
            </thead>
            <tbody>
              {artifacts.map((artifact) => (
                <tr key={`${artifact.kind}-${artifact.id}`} onClick={() => setActiveArtifact(artifact)}>
                  <td><strong>{artifact.id}</strong><small>{formatDateTime(artifact.timestamp)} · {artifact.objectUri}</small></td>
                  <td><span className={`artifact-kind ${artifact.kind}`}>{artifact.kind}</span><small>{artifact.artifactType}</small></td>
                  <td>{artifact.sandboxId ?? artifact.nodeId ?? '-'}<small>{artifact.nodeId && artifact.sandboxId ? `node ${artifact.nodeId}` : ''}</small></td>
                  <td>{artifact.runtimeType ?? '-'}</td>
                  <td>{artifact.kind === 'profile' ? `${(artifact.sampleCount ?? 0).toLocaleString()} samples` : formatBytes(artifact.sizeBytes ?? 0)}</td>
                  <td>{artifact.durationMs !== undefined ? formatDuration(artifact.durationMs) : '-'}</td>
                  <td>{artifact.source ?? '-'}</td>
                  <td><button className="inline-action" onClick={(event) => { event.stopPropagation(); setActiveArtifact(artifact); }}>Details</button></td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </section>

      {activeArtifact && <ArtifactDrawer artifact={activeArtifact} onClose={() => setActiveArtifact(undefined)} />}
    </div>
  );
}

function SummaryCard({ label, value, caption, hot }: { label: string; value: string; caption: string; hot?: boolean }) {
  return <div className={`summary-card ${hot ? 'warning' : ''}`}><span>{label}</span><strong>{value}</strong><em>{caption}</em></div>;
}

function ArtifactDrawer({ artifact, onClose }: { artifact: RuntimePulseArtifact; onClose: () => void }) {
  return (
    <div className="profile-modal-backdrop" role="presentation" onClick={onClose}>
      <section className="profile-modal artifact-modal" role="dialog" aria-modal="true" aria-label="Artifact detail" onClick={(event) => event.stopPropagation()}>
        <div className="flamegraph-header">
          <div>
            <h3>{artifact.kind === 'profile' ? 'Profile artifact' : 'Diagnostic artifact'}</h3>
            <p>{artifact.id} · {artifact.artifactType} · {artifact.objectUri}</p>
          </div>
          <div className="profile-modal-actions">
            <span>{artifact.severity ?? 'info'}</span>
            <button onClick={onClose}>Close</button>
          </div>
        </div>
        <div className="artifact-detail-grid">
          <Fact label="Timestamp" value={formatDateTime(artifact.timestamp)} />
          <Fact label="Sandbox" value={artifact.sandboxId ?? '-'} />
          <Fact label="Node" value={artifact.nodeId ?? '-'} />
          <Fact label="Runtime" value={artifact.runtimeType ?? '-'} />
          <Fact label="Source" value={artifact.source ?? '-'} />
          <Fact label="Object URI" value={artifact.objectUri} wide />
        </div>
        {artifact.message && <p className="artifact-message">{artifact.message}</p>}
        <pre className="raw-json artifact-json">{JSON.stringify(artifact, null, 2)}</pre>
      </section>
    </div>
  );
}

function Fact({ label, value, wide }: { label: string; value: string; wide?: boolean }) {
  return <div className={`artifact-fact ${wide ? 'wide' : ''}`}><span>{label}</span><strong>{value}</strong></div>;
}

function artifactSummary(artifacts: RuntimePulseArtifact[]) {
  return artifacts.reduce((summary, artifact) => {
    summary.total += 1;
    if (artifact.kind === 'profile') summary.profiles += 1;
    if (artifact.kind === 'diagnostic') summary.diagnostics += 1;
    if (artifact.severity === 'warning' || artifact.severity === 'error') summary.warnings += 1;
    return summary;
  }, { total: 0, profiles: 0, diagnostics: 0, warnings: 0 });
}
