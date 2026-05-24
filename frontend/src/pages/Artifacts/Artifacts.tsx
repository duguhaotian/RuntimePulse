import { useEffect, useMemo, useState } from 'react';
import type { ArtifactKind, RuntimePulseArtifact } from '../../domain/model';
import type { RuntimePulseApi } from '../../api/RuntimePulseApi';
import { formatDateTime } from '../../utils/time';
import { formatBytes, formatDuration } from '../../utils/units';

type ArtifactsProps = {
  api: RuntimePulseApi;
};

type SortKey = 'timestamp' | 'severity' | 'duration' | 'size';

export function Artifacts({ api }: ArtifactsProps) {
  const [artifacts, setArtifacts] = useState<RuntimePulseArtifact[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string>();
  const [kind, setKind] = useState<ArtifactKind | 'all'>('all');
  const [text, setText] = useState('');
  const [severity, setSeverity] = useState<'all' | 'warning' | 'error'>('all');
  const [sortKey, setSortKey] = useState<SortKey>('timestamp');
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

  const visibleArtifacts = useMemo(() => sortArtifacts(filterArtifactsBySeverity(artifacts, severity), sortKey), [artifacts, severity, sortKey]);
  const summary = useMemo(() => artifactSummary(artifacts), [artifacts]);
  const diagnosticsByType = useMemo(() => topDiagnosticTypes(visibleArtifacts), [visibleArtifacts]);

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
          <em>{error ?? `${visibleArtifacts.length.toLocaleString()} / ${artifacts.length.toLocaleString()} rows`}</em>
        </div>
      </section>

      <section className="summary-grid artifacts-summary-grid">
        <SummaryCard label="Artifacts" value={summary.total.toLocaleString()} caption="profiles + diagnostics" />
        <SummaryCard label="Profiles" value={summary.profiles.toLocaleString()} caption="profile artifact indexes" />
        <SummaryCard label="Diagnostics" value={summary.diagnostics.toLocaleString()} caption="bundle / inspect / log indexes" />
        <SummaryCard label="Warnings" value={summary.warnings.toLocaleString()} caption="warning or error severity" hot={summary.warnings > 0} />
      </section>

      <section className="artifact-workflow-grid">
        <div className="artifact-workflow-card">
          <span>Most common diagnostic types</span>
          {diagnosticsByType.length === 0 ? (
            <strong>No diagnostics in current view</strong>
          ) : diagnosticsByType.map((item) => (
            <button key={item.type} onClick={() => { setKind('diagnostic'); setText(item.type); }}>
              <strong>{item.type}</strong>
              <em>{item.count.toLocaleString()} rows</em>
            </button>
          ))}
        </div>
        <div className="artifact-workflow-card artifact-workflow-card-muted">
          <span>Suggested workflow</span>
          <ol>
            <li>Filter warnings/errors first, then open Details.</li>
            <li>Copy the object URI and inspect raw artifacts on the host/object store.</li>
            <li>Correlate timestamp with sandbox metrics, lifecycle events, and profile captures.</li>
          </ol>
        </div>
      </section>

      <section className="filter-bar artifacts-filter-bar">
        <input value={text} onChange={(event) => setText(event.target.value)} placeholder="Filter id, sandbox, node, URI, source" />
        <select value={kind} onChange={(event) => setKind(event.target.value as ArtifactKind | 'all')}>
          <option value="all">All artifact kinds</option>
          <option value="profile">Profiles</option>
          <option value="diagnostic">Diagnostics</option>
        </select>
        <select value={severity} onChange={(event) => setSeverity(event.target.value as 'all' | 'warning' | 'error')}>
          <option value="all">All severities</option>
          <option value="warning">Warnings + errors</option>
          <option value="error">Errors only</option>
        </select>
        <select value={sortKey} onChange={(event) => setSortKey(event.target.value as SortKey)}>
          <option value="timestamp">Newest first</option>
          <option value="severity">Severity first</option>
          <option value="duration">Longest capture</option>
          <option value="size">Largest artifact</option>
        </select>
      </section>

      <section className="table-card artifacts-table-card">
        <div className="table-titlebar">
          <div>
            <strong>Artifact index</strong>
            <span>Object URIs point to raw payloads outside the Query API.</span>
          </div>
        </div>
        {visibleArtifacts.length === 0 ? (
          <div className="empty-state">{loading ? 'Loading artifacts…' : 'No artifacts matched the current filters.'}</div>
        ) : (
          <>
            <div className="artifact-card-list">
              {visibleArtifacts.map((artifact) => (
                <ArtifactListCard key={`${artifact.kind}-${artifact.id}`} artifact={artifact} onOpen={setActiveArtifact} />
              ))}
            </div>
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
                {visibleArtifacts.map((artifact) => (
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
          </>
        )}
      </section>

      {activeArtifact && <ArtifactDrawer artifact={activeArtifact} onClose={() => setActiveArtifact(undefined)} />}
    </div>
  );
}

function ArtifactListCard({ artifact, onOpen }: { artifact: RuntimePulseArtifact; onOpen: (artifact: RuntimePulseArtifact) => void }) {
  return (
    <button className="artifact-list-card" onClick={() => onOpen(artifact)}>
      <span className={`artifact-kind ${artifact.kind}`}>{artifact.kind}</span>
      <strong>{artifact.id}</strong>
      <small>{formatDateTime(artifact.timestamp)}</small>
      <em>{artifact.sandboxId ?? artifact.nodeId ?? 'node scope'} · {artifact.artifactType}</em>
      <code>{artifact.objectUri}</code>
      <span className={`artifact-severity severity-${artifact.severity ?? 'info'}`}>{artifact.severity ?? 'info'}</span>
    </button>
  );
}

function SummaryCard({ label, value, caption, hot }: { label: string; value: string; caption: string; hot?: boolean }) {
  return <div className={`summary-card ${hot ? 'warning' : ''}`}><span>{label}</span><strong>{value}</strong><em>{caption}</em></div>;
}

function ArtifactDrawer({ artifact, onClose }: { artifact: RuntimePulseArtifact; onClose: () => void }) {
  const [copied, setCopied] = useState(false);
  const copyObjectUri = async () => {
    try {
      await navigator.clipboard.writeText(artifact.objectUri);
      setCopied(true);
      window.setTimeout(() => setCopied(false), 1400);
    } catch {
      setCopied(false);
    }
  };

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
            <button onClick={copyObjectUri}>{copied ? 'Copied' : 'Copy URI'}</button>
            <button onClick={onClose}>Close</button>
          </div>
        </div>
        <div className="artifact-detail-grid">
          <Fact label="Timestamp" value={formatDateTime(artifact.timestamp)} />
          <Fact label="Sandbox" value={artifact.sandboxId ?? '-'} />
          <Fact label="Node" value={artifact.nodeId ?? '-'} />
          <Fact label="Runtime" value={artifact.runtimeType ?? '-'} />
          <Fact label="Source" value={artifact.source ?? '-'} />
          <Fact label="Status" value={artifact.status ?? artifact.severity ?? 'info'} />
          <Fact label="Object URI" value={artifact.objectUri} wide />
        </div>
        {artifact.message && <p className="artifact-message">{artifact.message}</p>}
        {artifact.kind === 'diagnostic' && <DiagnosticPayload artifact={artifact} />}
        <pre className="raw-json artifact-json">{JSON.stringify(artifact, null, 2)}</pre>
      </section>
    </div>
  );
}

function DiagnosticPayload({ artifact }: { artifact: RuntimePulseArtifact }) {
  const files = Array.isArray(artifact.artifacts) ? artifact.artifacts : [];
  const attributes = artifact.attributes ?? {};
  const importantAttributes = Object.entries(attributes)
    .filter(([key]) => key.startsWith('diagnostic.') || key.startsWith('containerd.') || key.startsWith('docker.') || key.startsWith('cri.') || key.startsWith('k8s.') || key.startsWith('image.'))
    .slice(0, 18);

  return (
    <div className="artifact-payload-grid">
      <div className="artifact-payload-card">
        <strong>Referenced files</strong>
        {files.length === 0 ? <span>No nested artifact files reported.</span> : files.map((file, index) => (
          <code key={index}>{artifactFileLabel(file)}</code>
        ))}
      </div>
      <div className="artifact-payload-card">
        <strong>Key attributes</strong>
        {importantAttributes.length === 0 ? <span>No diagnostic attributes reported.</span> : importantAttributes.map(([key, value]) => (
          <code key={key}>{key}={String(value)}</code>
        ))}
      </div>
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

function filterArtifactsBySeverity(artifacts: RuntimePulseArtifact[], severity: 'all' | 'warning' | 'error') {
  if (severity === 'all') return artifacts;
  if (severity === 'error') return artifacts.filter((artifact) => artifact.severity === 'error');
  return artifacts.filter((artifact) => artifact.severity === 'warning' || artifact.severity === 'error');
}

function sortArtifacts(artifacts: RuntimePulseArtifact[], sortKey: SortKey) {
  return [...artifacts].sort((left, right) => {
    if (sortKey === 'severity') return severityRank(right) - severityRank(left) || newestFirst(left, right);
    if (sortKey === 'duration') return (right.durationMs ?? 0) - (left.durationMs ?? 0) || newestFirst(left, right);
    if (sortKey === 'size') return artifactSize(right) - artifactSize(left) || newestFirst(left, right);
    return newestFirst(left, right);
  });
}

function newestFirst(left: RuntimePulseArtifact, right: RuntimePulseArtifact) {
  return String(right.timestamp).localeCompare(String(left.timestamp));
}

function severityRank(artifact: RuntimePulseArtifact) {
  if (artifact.severity === 'error') return 3;
  if (artifact.severity === 'warning') return 2;
  return 1;
}

function artifactSize(artifact: RuntimePulseArtifact) {
  return artifact.kind === 'profile' ? artifact.sampleCount ?? 0 : artifact.sizeBytes ?? 0;
}

function topDiagnosticTypes(artifacts: RuntimePulseArtifact[]) {
  const counts = new Map<string, number>();
  for (const artifact of artifacts) {
    if (artifact.kind !== 'diagnostic') continue;
    counts.set(artifact.artifactType, (counts.get(artifact.artifactType) ?? 0) + 1);
  }
  return [...counts.entries()]
    .map(([type, count]) => ({ type, count }))
    .sort((left, right) => right.count - left.count)
    .slice(0, 3);
}

function artifactFileLabel(file: unknown) {
  if (!file || typeof file !== 'object') return String(file);
  const row = file as Record<string, unknown>;
  return [row.name, row.artifactType, row.objectUri, row.sizeBytes ? formatBytes(Number(row.sizeBytes)) : undefined]
    .filter(Boolean)
    .join(' · ');
}
