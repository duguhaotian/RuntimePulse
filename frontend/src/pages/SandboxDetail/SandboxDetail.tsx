import { useEffect, useMemo, useState } from 'react';
import type { RuntimePulseApi } from '../../api/RuntimePulseApi';
import type { EventRecord, Image, MetricSeries, Node, ProfileArtifact, Sandbox, TraceSpan } from '../../domain/model';
import { MetricChart } from '../../components/charts/MetricChart';
import { EventTimeline } from '../../components/timeline/EventTimeline';
import { TraceWaterfall } from '../../components/trace/TraceWaterfall';
import { RuntimeBadge } from '../SandboxExplorer/SandboxExplorer';
import { formatBytes, formatDuration, formatRatio } from '../../utils/units';
import { formatDateTime } from '../../utils/time';

type SandboxDetailProps = {
  api: RuntimePulseApi;
  sandboxId: string;
  onBack: () => void;
};

type Tab = 'overview' | 'metrics' | 'timeline' | 'trace' | 'profiles' | 'raw';

export function SandboxDetail({ api, sandboxId, onBack }: SandboxDetailProps) {
  const [tab, setTab] = useState<Tab>('overview');
  const [sandbox, setSandbox] = useState<Sandbox>();
  const [node, setNode] = useState<Node>();
  const [image, setImage] = useState<Image>();
  const [metrics, setMetrics] = useState<MetricSeries[]>([]);
  const [events, setEvents] = useState<EventRecord[]>([]);
  const [spans, setSpans] = useState<TraceSpan[]>([]);
  const [profiles, setProfiles] = useState<ProfileArtifact[]>([]);

  useEffect(() => {
    let mounted = true;
    api.getSandbox(sandboxId).then(async (nextSandbox) => {
      if (!mounted || !nextSandbox) return;
      const [nextNode, nextImage, nextMetrics, nextEvents, nextSpans, nextProfiles] = await Promise.all([
        api.getNode(nextSandbox.nodeId),
        api.getImage(nextSandbox.imageId),
        api.getSandboxMetrics(sandboxId),
        api.getSandboxEvents(sandboxId),
        api.getSandboxTrace(sandboxId),
        api.getSandboxProfiles(sandboxId),
      ]);
      if (!mounted) return;
      setSandbox(nextSandbox);
      setNode(nextNode);
      setImage(nextImage);
      setMetrics(nextMetrics);
      setEvents(nextEvents);
      setSpans(nextSpans);
      setProfiles(nextProfiles);
    });
    return () => {
      mounted = false;
    };
  }, [api, sandboxId]);

  const metricsByGroup = useMemo(() => {
    return metrics.reduce<Record<string, MetricSeries[]>>((groups, item) => {
      groups[item.group] = [...(groups[item.group] ?? []), item];
      return groups;
    }, {});
  }, [metrics]);

  if (!sandbox) return <div className="empty-state">Loading sandbox...</div>;

  return (
    <section className="page-stack run-detail-page">
      <button className="back-button" onClick={onBack}>← Back to runs</button>

      <div className="run-hero">
        <div className="run-identity">
          <div className="run-avatar">{sandbox.runtimeType.slice(0, 2).toUpperCase()}</div>
          <div>
            <p className="breadcrumb">runtimepulse / sandbox-lab / runs / {sandbox.id}</p>
            <h2>{sandbox.id}</h2>
            <div className="tag-row">
              <RuntimeBadge runtimeType={sandbox.runtimeType} />
              <span className={`status-badge ${sandbox.status}`}>{sandbox.status}</span>
              <span className="tag-pill">{sandbox.namespace}</span>
              <span className="tag-pill">{sandbox.workloadName}</span>
              {sandbox.startupDurationMs > 7000 && <span className="tag-pill warning">slow-start</span>}
            </div>
          </div>
        </div>
        <div className="run-actions">
          <button>Pin</button>
          <button>Compare</button>
          <button className="primary">Create report</button>
        </div>
      </div>

      <div className="summary-grid detail-summary">
        <Info label="Status" value={sandbox.status} />
        <Info label="Startup" value={formatDuration(sandbox.startupDurationMs)} hot={sandbox.startupDurationMs > 7000} />
        <Info label="CPU Avg" value={formatRatio(sandbox.cpuAvg)} />
        <Info label="Memory Peak" value={formatBytes(sandbox.memoryPeakBytes)} />
        <Info label="Node" value={node?.name ?? sandbox.nodeId} />
        <Info label="Image Layers" value={String(image?.layerCount ?? '-')} hot={(image?.layerCount ?? 0) > 60} />
      </div>

      <div className="run-layout-grid">
        <div className="panel-card run-notes">
          <h3>Run notes</h3>
          <p>Mock run for expert analysis. Review lifecycle events, startup spans, runtime process metrics, and profile artifacts before collector integration.</p>
        </div>
        <div className="panel-card config-card">
          <h3>Config</h3>
          <dl>
            <dt>runtime.version</dt><dd>{sandbox.runtimeVersion}</dd>
            <dt>node.id</dt><dd>{sandbox.nodeId}</dd>
            <dt>image.ref</dt><dd>{sandbox.imageRef}</dd>
            <dt>created.at</dt><dd>{formatDateTime(sandbox.createdAt)}</dd>
          </dl>
        </div>
      </div>

      <div className="tabs">
        {(['overview', 'metrics', 'timeline', 'trace', 'profiles', 'raw'] as Tab[]).map((item) => (
          <button className={tab === item ? 'active' : ''} key={item} onClick={() => setTab(item)}>{item}</button>
        ))}
      </div>

      {tab === 'overview' && (
        <div className="two-column">
          <div className="panel-card">
            <h3>Lifecycle Timeline</h3>
            <EventTimeline events={events} />
          </div>
          <div className="panel-card">
            <h3>Startup Trace</h3>
            <TraceWaterfall spans={spans} />
          </div>
        </div>
      )}

      {tab === 'metrics' && (
        <div className="chart-grid">
          {Object.entries(metricsByGroup).map(([group, groupSeries]) => <MetricChart key={group} series={groupSeries} />)}
        </div>
      )}

      {tab === 'timeline' && <div className="panel-card"><EventTimeline events={events} /></div>}
      {tab === 'trace' && <div className="panel-card"><TraceWaterfall spans={spans} /></div>}
      {tab === 'profiles' && <ProfileTable profiles={profiles} />}
      {tab === 'raw' && <pre className="raw-json">{JSON.stringify({ sandbox, node, image, events, spans, profiles }, null, 2)}</pre>}
    </section>
  );
}

function Info({ label, value, hot }: { label: string; value: string; hot?: boolean }) {
  return (
    <div className={`summary-card ${hot ? 'warning' : ''}`}>
      <span>{label}</span>
      <strong>{value}</strong>
      <em>summary</em>
    </div>
  );
}

function ProfileTable({ profiles }: { profiles: ProfileArtifact[] }) {
  return (
    <div className="table-card">
      <div className="table-titlebar">
        <div>
          <strong>Profile artifacts</strong>
          <span>pprof-compatible profiling outputs</span>
        </div>
      </div>
      <table>
        <thead>
          <tr><th>Profile</th><th>Type</th><th>Process Role</th><th>Timestamp</th><th>Duration</th><th>Samples</th><th>Object</th></tr>
        </thead>
        <tbody>
          {profiles.map((profile) => (
            <tr key={profile.id}>
              <td><strong>{profile.id}</strong></td>
              <td>{profile.profileType}</td>
              <td>{profile.processRole}</td>
              <td>{formatDateTime(profile.timestamp)}</td>
              <td>{formatDuration(profile.durationMs)}</td>
              <td>{profile.sampleCount.toLocaleString()}</td>
              <td className="truncate">{profile.objectUri}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}
