import { useEffect, useMemo, useState } from 'react';
import type { RuntimePulseApi } from '../../api/RuntimePulseApi';
import type { EventRecord, FlamegraphFrame, Image, MetricSeries, Node, ProfileArtifact, Sandbox, TraceSpan } from '../../domain/model';
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
type MetricPanelSize = 'compact' | 'standard' | 'expanded';

const metricPanelHeights: Record<MetricPanelSize, number> = {
  compact: 170,
  standard: 220,
  expanded: 310,
};

const metricPanelLabels: Record<MetricPanelSize, string> = {
  compact: 'Compact',
  standard: 'Standard',
  expanded: 'Expanded',
};

export function SandboxDetail({ api, sandboxId, onBack }: SandboxDetailProps) {
  const [tab, setTab] = useState<Tab>('overview');
  const [sandbox, setSandbox] = useState<Sandbox>();
  const [node, setNode] = useState<Node>();
  const [image, setImage] = useState<Image>();
  const [metrics, setMetrics] = useState<MetricSeries[]>([]);
  const [events, setEvents] = useState<EventRecord[]>([]);
  const [spans, setSpans] = useState<TraceSpan[]>([]);
  const [profiles, setProfiles] = useState<ProfileArtifact[]>([]);
  const [selectedSpan, setSelectedSpan] = useState<TraceSpan>();
  const [selectedEvent, setSelectedEvent] = useState<EventRecord>();
  const [pinnedMetricGroups, setPinnedMetricGroups] = useState<string[]>([]);
  const [metricPanelSize, setMetricPanelSize] = useState<MetricPanelSize>('standard');

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
      setSelectedEvent(undefined);
      setSpans(nextSpans);
      setSelectedSpan(nextSpans[0]);
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

  const metricGroups = useMemo(() => {
    return Object.entries(metricsByGroup).sort(([leftGroup], [rightGroup]) => {
      const leftPinned = pinnedMetricGroups.includes(leftGroup);
      const rightPinned = pinnedMetricGroups.includes(rightGroup);

      if (leftPinned !== rightPinned) return leftPinned ? -1 : 1;
      if (leftPinned && rightPinned) return pinnedMetricGroups.indexOf(leftGroup) - pinnedMetricGroups.indexOf(rightGroup);

      return leftGroup.localeCompare(rightGroup);
    });
  }, [metricsByGroup, pinnedMetricGroups]);

  function toggleMetricPin(group: string) {
    setPinnedMetricGroups((current) => current.includes(group) ? current.filter((item) => item !== group) : [...current, group]);
  }

  function jumpToMetricsFromEvent(event: EventRecord) {
    setSelectedEvent(event);
    setTab('metrics');
  }

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
        <div className="overview-stack">
          <div className="two-column">
            <div className="panel-card">
              <h3>Lifecycle Timeline</h3>
              <EventTimeline events={events} selectedEventId={selectedEvent?.id} onSelectEvent={jumpToMetricsFromEvent} />
            </div>
            <div className="panel-card">
              <h3>Startup Trace</h3>
              <TraceWaterfall spans={spans} selectedSpanId={selectedSpan?.spanId} onSelectSpan={setSelectedSpan} />
            </div>
          </div>
          {image && <ImageLayerPanel image={image} />}
        </div>
      )}

      {tab === 'metrics' && (
        <div className={`chart-grid metric-panel-grid ${metricPanelSize}`}>
          <MetricPanelToolbar size={metricPanelSize} onSizeChange={setMetricPanelSize} />
          {selectedEvent && <SelectedEventContext event={selectedEvent} onClear={() => setSelectedEvent(undefined)} />}
          {pinnedMetricGroups.length > 0 && <PinnedMetricSummary pinnedGroups={pinnedMetricGroups} onClear={() => setPinnedMetricGroups([])} />}
          {metricGroups.map(([group, groupSeries]) => (
            <MetricChart
              height={metricPanelHeights[metricPanelSize]}
              key={group}
              markerLabel={selectedEvent?.eventName}
              markerTime={selectedEvent?.timestamp}
              onTogglePin={() => toggleMetricPin(group)}
              pinned={pinnedMetricGroups.includes(group)}
              series={groupSeries}
            />
          ))}
        </div>
      )}

      {tab === 'timeline' && <div className="panel-card"><EventTimeline events={events} selectedEventId={selectedEvent?.id} onSelectEvent={jumpToMetricsFromEvent} /></div>}
      {tab === 'trace' && (
        <div className="trace-detail-grid">
          <div className="panel-card"><TraceWaterfall spans={spans} selectedSpanId={selectedSpan?.spanId} onSelectSpan={setSelectedSpan} /></div>
          <SpanDetailPanel span={selectedSpan} />
        </div>
      )}
      {tab === 'profiles' && <ProfileTable profiles={profiles} />}
      {tab === 'raw' && <pre className="raw-json">{JSON.stringify({ sandbox, node, image, events, spans, profiles }, null, 2)}</pre>}
    </section>
  );
}

function MetricPanelToolbar({ size, onSizeChange }: { size: MetricPanelSize; onSizeChange: (size: MetricPanelSize) => void }) {
  return (
    <div className="metric-panel-toolbar">
      <div>
        <strong>Metric panel size</strong>
        <span>Resize charts for dense scanning or deep inspection.</span>
      </div>
      <div className="segmented-control" role="group" aria-label="Metric panel size">
        {(['compact', 'standard', 'expanded'] as MetricPanelSize[]).map((item) => (
          <button className={size === item ? 'active' : ''} key={item} onClick={() => onSizeChange(item)}>{metricPanelLabels[item]}</button>
        ))}
      </div>
    </div>
  );
}

function PinnedMetricSummary({ pinnedGroups, onClear }: { pinnedGroups: string[]; onClear: () => void }) {
  return (
    <div className="pinned-metrics-bar">
      <div>
        <strong>Pinned metric groups</strong>
        <span>{pinnedGroups.map((group) => group.toUpperCase()).join(' · ')}</span>
      </div>
      <button onClick={onClear}>Clear pins</button>
    </div>
  );
}

function SelectedEventContext({ event, onClear }: { event: EventRecord; onClear: () => void }) {
  return (
    <div className="selected-event-context">
      <div>
        <strong>{event.eventName}</strong>
        <span>{formatDateTime(event.timestamp)} · {event.severity} · {event.source}</span>
      </div>
      <p>{event.message}</p>
      <button onClick={onClear}>Clear marker</button>
    </div>
  );
}

function SpanDetailPanel({ span }: { span?: TraceSpan }) {
  if (!span) {
    return <div className="panel-card span-detail-panel"><div className="empty-state">Select a trace span to inspect details.</div></div>;
  }

  return (
    <div className="panel-card span-detail-panel">
      <div className="span-detail-header">
        <div>
          <h3>{span.spanName}</h3>
          <p>{span.spanId} · {span.status}</p>
        </div>
        <span className={`status-badge ${span.status === 'error' ? 'failed' : 'running'}`}>{span.status}</span>
      </div>
      <div className="span-detail-stats">
        <Info label="Duration" value={formatDuration(span.durationMs)} hot={span.durationMs > 7000} />
        <Info label="Start" value={formatDateTime(span.startTime)} />
        <Info label="End" value={formatDateTime(span.endTime)} />
      </div>
      <div className="span-attributes">
        <h3>Attributes</h3>
        <dl>
          {Object.entries(span.attributes).map(([key, value]) => (
            <div key={key}>
              <dt>{key}</dt>
              <dd>{String(value)}</dd>
            </div>
          ))}
        </dl>
      </div>
      <div className="span-next-actions">
        <h3>Next analysis actions</h3>
        <button>Focus metrics ±30s</button>
        <button>Show nearby events</button>
        <button>Create report note</button>
      </div>
    </div>
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

function ImageLayerPanel({ image }: { image: Image }) {
  const layers = image.layers ?? [];
  const totalDuration = layers.reduce((sum, layer) => sum + layer.pullDurationMs + layer.unpackDurationMs, 0);
  const cacheHits = layers.filter((layer) => layer.cacheHit).length;
  const cacheHitRatio = layers.length === 0 ? 0 : cacheHits / layers.length;
  const largestLayer = layers.reduce((largest, layer) => (layer.sizeBytes > largest.sizeBytes ? layer : largest), layers[0]);

  return (
    <div className="panel-card image-layer-panel">
      <div className="image-layer-header">
        <div>
          <h3>Image layer breakdown</h3>
          <p>{image.ref} · {formatBytes(image.sizeBytes)} · {image.layerCount} layers</p>
        </div>
        <div className="image-cache-summary">
          <strong>{formatRatio(cacheHitRatio)}</strong>
          <span>cache hit estimate</span>
        </div>
      </div>
      <div className="image-layer-summary">
        <Info label="Layer Samples" value={String(layers.length)} />
        <Info label="Pull+Unpack" value={formatDuration(totalDuration)} hot={totalDuration > 7000} />
        <Info label="Largest Layer" value={largestLayer ? formatBytes(largestLayer.sizeBytes) : '-'} hot={(largestLayer?.sizeBytes ?? 0) > 1024 ** 3} />
      </div>
      <div className="image-layer-list">
        {layers.map((layer) => {
          const layerDuration = layer.pullDurationMs + layer.unpackDurationMs;
          const sizeWidth = Math.max(6, (layer.sizeBytes / image.sizeBytes) * 100);
          const durationWidth = totalDuration > 0 ? Math.max(6, (layerDuration / totalDuration) * 100) : 6;

          return (
            <article className="image-layer-row" key={layer.id}>
              <div>
                <strong>{layer.command}</strong>
                <span>{formatBytes(layer.sizeBytes)} · pull {formatDuration(layer.pullDurationMs)} · unpack {formatDuration(layer.unpackDurationMs)}</span>
              </div>
              <div className="image-layer-bars">
                <div className="image-layer-track"><i style={{ width: `${sizeWidth}%` }} /></div>
                <div className="image-layer-track duration"><i style={{ width: `${durationWidth}%` }} /></div>
              </div>
              <span className={`cache-pill ${layer.cacheHit ? 'hit' : 'miss'}`}>{layer.cacheHit ? 'cache hit' : 'cache miss'}</span>
            </article>
          );
        })}
      </div>
    </div>
  );
}

function ProfileTable({ profiles }: { profiles: ProfileArtifact[] }) {
  const [expandedProfileId, setExpandedProfileId] = useState(profiles[0]?.id ?? '');
  const selectedProfile = profiles.find((profile) => profile.id === expandedProfileId) ?? profiles[0];

  return (
    <div className="profile-viewer-grid">
      <div className="table-card profile-list-card">
        <div className="table-titlebar">
          <div>
            <strong>Profile artifacts</strong>
            <span>Click a profile to open the flame graph preview</span>
          </div>
        </div>
        <table>
          <thead>
            <tr><th>Profile</th><th>Type</th><th>Process Role</th><th>Duration</th><th>Samples</th><th>View</th></tr>
          </thead>
          <tbody>
            {profiles.map((profile) => {
              const selected = selectedProfile?.id === profile.id;
              return (
                <tr className={selected ? 'selected-row' : ''} key={profile.id} onClick={() => setExpandedProfileId(profile.id)}>
                  <td><strong>{profile.id}</strong><small>{formatDateTime(profile.timestamp)}</small></td>
                  <td>{profile.profileType}</td>
                  <td>{profile.processRole}</td>
                  <td>{formatDuration(profile.durationMs)}</td>
                  <td>{profile.sampleCount.toLocaleString()}</td>
                  <td><button className="inline-action">{selected ? 'Open' : 'Preview'}</button></td>
                </tr>
              );
            })}
          </tbody>
        </table>
      </div>

      {selectedProfile && (
        <div className="panel-card flamegraph-card">
          <div className="flamegraph-header">
            <div>
              <h3>Flame graph preview</h3>
              <p>{selectedProfile.id} · {selectedProfile.profileType} · {selectedProfile.objectUri}</p>
            </div>
            <span>{selectedProfile.sampleCount.toLocaleString()} samples</span>
          </div>
          {selectedProfile.flamegraph ? <FlameGraph root={selectedProfile.flamegraph} /> : <div className="empty-state">No flame graph data available.</div>}
        </div>
      )}
    </div>
  );
}

function FlameGraph({ root }: { root: FlamegraphFrame }) {
  const rows = useMemo(() => layoutFlamegraph(root), [root]);
  const maxDepth = Math.max(...rows.map((row) => row.depth), 0);
  const height = (maxDepth + 1) * 30 + 18;

  return (
    <div className="flamegraph-scroll">
      <svg className="flamegraph-svg" viewBox={`0 0 1000 ${height}`} preserveAspectRatio="none" role="img" aria-label="Flame graph preview">
        {rows.map((row) => (
          <g key={`${row.depth}-${row.x}-${row.frame.name}`}>
            <rect
              className="flame-frame"
              x={row.x}
              y={height - (row.depth + 1) * 30}
              width={Math.max(row.width, 2)}
              height="24"
              rx="4"
            />
            {row.width > 70 && (
              <text x={row.x + 7} y={height - (row.depth + 1) * 30 + 16} className="flame-label">
                {row.frame.name} ({row.frame.value})
              </text>
            )}
            <title>{row.frame.name}: {row.frame.value} samples</title>
          </g>
        ))}
      </svg>
      <div className="flamegraph-footer">
        <span>Root on bottom, callees stack upward.</span>
        <span>Mock preview; real pprof/flamegraph artifacts will use the same panel.</span>
      </div>
    </div>
  );
}

type FlameRow = {
  frame: FlamegraphFrame;
  depth: number;
  x: number;
  width: number;
};

function layoutFlamegraph(root: FlamegraphFrame): FlameRow[] {
  const rows: FlameRow[] = [];
  const visit = (frame: FlamegraphFrame, depth: number, x: number, width: number) => {
    rows.push({ frame, depth, x, width });
    const children = frame.children ?? [];
    const total = children.reduce((sum, child) => sum + child.value, 0) || frame.value;
    let cursor = x;
    children.forEach((child) => {
      const childWidth = width * (child.value / total);
      visit(child, depth + 1, cursor, childWidth);
      cursor += childWidth;
    });
  };

  visit(root, 0, 0, 1000);
  return rows;
}
