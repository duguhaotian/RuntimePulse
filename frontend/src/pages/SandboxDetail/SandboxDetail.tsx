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
      setSelectedSpan(undefined);
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

  const pressureSeries = useMemo(() => {
    const io = metrics.find((item) => item.name === 'sandbox.io.read_bytes');
    const network = metrics.find((item) => item.name === 'sandbox.network.rx_bytes');
    return { io, network };
  }, [metrics]);

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

      <DetailDataSections sandbox={sandbox} node={node} image={image} events={events} metrics={metrics} />

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
          {image && <ContainerImageAccessPanel sandbox={sandbox} image={image} ioSeries={pressureSeries.io} networkSeries={pressureSeries.network} />}
          <div className="overview-timeline-stack">
            <div className="panel-card">
              <h3>Lifecycle Timeline</h3>
              <EventTimeline events={events} selectedEventId={selectedEvent?.id} onSelectEvent={jumpToMetricsFromEvent} />
            </div>
            <div className="panel-card">
              <h3>Startup Trace</h3>
              <TraceWaterfall spans={spans} selectedSpanId={selectedSpan?.spanId} onSelectSpan={setSelectedSpan} />
            </div>
          </div>
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
        <div className="trace-full-width">
          <div className="panel-card"><TraceWaterfall spans={spans} selectedSpanId={selectedSpan?.spanId} onSelectSpan={setSelectedSpan} /></div>
        </div>
      )}
      {tab === 'profiles' && <ProfileTable profiles={profiles} />}
      {tab === 'raw' && <pre className="raw-json">{JSON.stringify({ sandbox, node, image, events, spans, profiles }, null, 2)}</pre>}
      {selectedSpan && <TraceSpanDetailModal span={selectedSpan} onClose={() => setSelectedSpan(undefined)} />}
    </section>
  );
}

function DetailDataSections({
  sandbox,
  node,
  image,
  events,
  metrics,
}: {
  sandbox: Sandbox;
  node?: Node;
  image?: Image;
  events: EventRecord[];
  metrics: MetricSeries[];
}) {
  const latestMetricAt = metrics.flatMap((series) => series.points).sort((left, right) => Date.parse(right.timestamp) - Date.parse(left.timestamp))[0]?.timestamp;
  const warningEvents = events.filter((event) => event.severity !== 'info').length;

  return (
    <div className="detail-data-sections">
      <section className="data-section dynamic">
        <div className="data-section-header">
          <span>Dynamic</span>
          <strong>运行时变化数据</strong>
        </div>
        <div className="data-section-grid">
          <Info label="Current status" value={sandbox.status} />
          <Info label="CPU Avg" value={formatRatio(sandbox.cpuAvg)} />
          <Info label="Memory Peak" value={formatBytes(sandbox.memoryPeakBytes)} />
          <Info label="Events" value={`${events.length} total / ${warningEvents} warning+`} hot={warningEvents > 0} />
          <Info label="Startup" value={formatDuration(sandbox.startupDurationMs)} hot={sandbox.startupDurationMs > 7000} />
          <Info label="Latest metric" value={latestMetricAt ? formatDateTime(latestMetricAt) : '-'} />
        </div>
      </section>

      <section className="data-section static">
        <div className="data-section-header">
          <span>Static</span>
          <strong>配置与不可变元数据</strong>
        </div>
        <div className="static-facts-grid">
          <Fact label="Cluster" value={sandbox.clusterId} />
          <Fact label="Runtime" value={`${sandbox.runtimeType} / ${sandbox.runtimeVersion}`} />
          <Fact label="Node" value={`${node?.name ?? sandbox.nodeId} · ${node?.kernelVersion ?? 'kernel unknown'}`} />
          <Fact label="Node capacity" value={node ? `${node.cpuCores} cores / ${formatBytes(node.memoryBytes)}` : '-'} />
          <Fact label="Image" value={sandbox.imageRef} />
          <Fact label="Image digest" value={image?.digest ?? '-'} />
          <Fact label="Image size" value={image ? `${formatBytes(image.sizeBytes)} / ${image.layerCount} layers` : '-'} />
          <Fact label="Workload" value={`${sandbox.namespace} / ${sandbox.workloadName}`} />
        </div>
      </section>
    </div>
  );
}

function Fact({ label, value }: { label: string; value: string }) {
  return (
    <div className="static-fact">
      <span>{label}</span>
      <strong>{value}</strong>
    </div>
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

function TraceSpanDetailModal({ onClose, span }: { onClose: () => void; span: TraceSpan }) {
  return (
    <div className="trace-span-modal-backdrop" role="presentation" onClick={onClose}>
      <section className="trace-span-modal" role="dialog" aria-modal="true" aria-label="Trace span detail" onClick={(event) => event.stopPropagation()}>
        <div className="trace-span-modal-header">
          <div>
            <h3>Trace span detail</h3>
            <p>{span.spanName} · {span.spanId}</p>
          </div>
          <button onClick={onClose}>Close</button>
        </div>
        <SpanDetailPanel span={span} />
      </section>
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

function ContainerImageAccessPanel({
  sandbox,
  image,
  ioSeries,
  networkSeries,
}: {
  sandbox: Sandbox;
  image: Image;
  ioSeries?: MetricSeries;
  networkSeries?: MetricSeries;
}) {
  const layers = image.layers ?? [];
  const avgIo = averageMetricValue(ioSeries);
  const peakIo = Math.max(...(ioSeries?.points.map((point) => point.value) ?? [0]), 0);
  const avgNetwork = averageMetricValue(networkSeries);
  const requestedBlocks = imageRequestedBlocks(image);
  const blockHitRatio = imageBlockHitRatio(image);
  const remoteReadBytes = imageRemoteReadBytes(image);
  const downloadDuration = imageDownloadDuration(image);

  return (
    <div className="panel-card container-image-panel">
      <div className="image-layer-header">
        <div>
          <h3>Container image and IO access</h3>
          <p>{sandbox.workloadName} uses {image.ref}</p>
        </div>
        <div className="image-cache-summary">
          <strong>{formatBytes(avgIo)}</strong>
          <span>avg read throughput</span>
        </div>
      </div>
      <div className="container-image-grid">
        <div className="container-image-facts">
          <Fact label="Image ref" value={image.ref} />
          <Fact label="Digest" value={image.digest} />
          <Fact label="Loading mode" value={image.loadingMode === 'lazy' ? 'lazy-load' : 'non-lazy'} />
          <Fact label="Image size" value={formatBytes(image.sizeBytes)} />
          <Fact label="Layers" value={String(image.layerCount)} />
          {image.loadingMode === 'lazy' ? (
            <>
              <Fact label="Requested blocks" value={String(requestedBlocks)} />
              <Fact label="Block hit ratio" value={formatRatio(blockHitRatio)} />
              <Fact label="Remote read" value={formatBytes(remoteReadBytes)} />
            </>
          ) : (
            <Fact label="Download time" value={formatDuration(downloadDuration)} />
          )}
        </div>
        <div className="container-io-summary">
          <Info label="Avg IO Read" value={formatBytes(avgIo)} hot={avgIo > 32 * 1024 ** 2} />
          <Info label="Peak IO Read" value={formatBytes(peakIo)} hot={peakIo > 64 * 1024 ** 2} />
          <Info label="Avg Network RX" value={formatBytes(avgNetwork)} />
        </div>
      </div>
      {image.loadingMode === 'lazy' ? (
        <LazyImageTemporalPanel image={image} />
      ) : (
        <ImageDownloadTimeline image={image} />
      )}
    </div>
  );
}

function LazyImageTemporalPanel({ image }: { image: Image }) {
  const layers = image.layers ?? [];
  const cacheSeries = lazyImageCacheSeries(image);

  return (
    <div className="container-layer-access">
      <div className="image-temporal-grid">
        <MetricChart height={180} series={[cacheSeries.hitRatio]} />
        <MetricChart height={180} series={[cacheSeries.remoteRead]} />
      </div>
      {layers.map((layer) => {
        return (
          <article className="image-layer-row" key={layer.id}>
            <div>
              <strong>{layer.command}</strong>
              <span>{layer.cacheHitBlockCount}/{layer.requestedBlockCount} blocks hit · remote {formatBytes(layer.remoteReadBytes)} · pull {formatDuration(layer.pullDurationMs)}</span>
            </div>
            <div className="image-layer-bars">
              <div className="image-layer-track"><i style={{ width: `${Math.max(6, layerBlockHitRatio(layer) * 100)}%` }} /></div>
            </div>
            <span className="cache-pill hit">{formatRatio(layerBlockHitRatio(layer))}</span>
          </article>
        );
      })}
    </div>
  );
}

function ImageDownloadTimeline({ image }: { image: Image }) {
  const steps = image.downloadTimeline ?? [];
  const totalDuration = imageDownloadDuration(image);
  const maxDuration = Math.max(...steps.map((step) => step.durationMs), 1);
  const downloadSeries = eagerImageDownloadSeries(image);

  return (
    <div className="image-download-panel">
      <div className="image-layer-header">
        <div>
          <h3>Image download timeline</h3>
          <p>{image.ref} · non-lazy pull before container start</p>
        </div>
        <div className="image-cache-summary">
          <strong>{formatDuration(totalDuration)}</strong>
          <span>download path</span>
        </div>
      </div>
      <div className="image-temporal-grid">
        <MetricChart height={180} series={[downloadSeries.duration]} />
        <MetricChart height={180} series={[downloadSeries.bytes]} />
      </div>
      <div className="download-timeline">
        {steps.map((step) => (
          <article className="download-step" key={step.id}>
            <div>
              <strong>{step.name}</strong>
              <span>{step.detail}</span>
            </div>
            <div className="download-step-track"><i className={step.phase} style={{ width: `${Math.max(6, step.durationMs / maxDuration * 100)}%` }} /></div>
            <em>{formatDuration(step.durationMs)}{step.bytes ? ` · ${formatBytes(step.bytes)}` : ''}</em>
          </article>
        ))}
      </div>
    </div>
  );
}

function averageMetricValue(series?: MetricSeries) {
  if (!series || series.points.length === 0) return 0;
  return series.points.reduce((sum, point) => sum + point.value, 0) / series.points.length;
}

function imageRequestedBlocks(image: Image) {
  return (image.layers ?? []).reduce((sum, layer) => sum + layer.requestedBlockCount, 0);
}

function imageHitBlocks(image: Image) {
  return (image.layers ?? []).reduce((sum, layer) => sum + layer.cacheHitBlockCount, 0);
}

function imageRemoteReadBytes(image: Image) {
  return (image.layers ?? []).reduce((sum, layer) => sum + layer.remoteReadBytes, 0);
}

function imageDownloadDuration(image: Image) {
  return (image.downloadTimeline ?? []).reduce((sum, step) => sum + step.durationMs, 0);
}

function imageBlockHitRatio(image: Image) {
  return ratio(imageHitBlocks(image), imageRequestedBlocks(image));
}

function layerBlockHitRatio(layer: NonNullable<Image['layers']>[number]) {
  return ratio(layer.cacheHitBlockCount, layer.requestedBlockCount);
}

function lazyImageCacheSeries(image: Image): { hitRatio: MetricSeries; remoteRead: MetricSeries } {
  const layers = image.layers ?? [];
  const start = Date.now() - 47 * 60_000;
  const points = Array.from({ length: 48 }, (_, index) => {
    const requested = layers.reduce((sum, layer, layerIndex) => {
      const activation = Math.max(0, Math.sin((index + layerIndex * 3) / 8));
      return sum + layer.requestedBlockCount * (0.18 + activation * 0.82);
    }, 0);
    const hit = layers.reduce((sum, layer, layerIndex) => {
      const drift = 0.88 + Math.sin((index + layerIndex) / 6) * 0.08;
      return sum + layer.cacheHitBlockCount * drift;
    }, 0);
    const remote = layers.reduce((sum, layer, layerIndex) => {
      const burst = 0.35 + Math.max(0, Math.sin((index - layerIndex * 4) / 5)) * 0.9;
      return sum + layer.remoteReadBytes * burst;
    }, 0);

    return {
      hitRatio: ratio(hit, Math.max(requested, 1)),
      remote,
      timestamp: new Date(start + index * 60_000).toISOString(),
    };
  });

  return {
    hitRatio: {
      id: `${image.id}-lazy-cache-hit-ratio`,
      name: 'image.lazy.cache_hit_ratio',
      label: 'Block cache hit ratio',
      unit: 'ratio',
      group: 'io',
      points: points.map((point) => ({ timestamp: point.timestamp, value: point.hitRatio })),
    },
    remoteRead: {
      id: `${image.id}-lazy-remote-read`,
      name: 'image.lazy.remote_read_bytes',
      label: 'Remote read bytes',
      unit: 'bytes',
      group: 'io',
      points: points.map((point) => ({ timestamp: point.timestamp, value: point.remote })),
    },
  };
}

function eagerImageDownloadSeries(image: Image): { duration: MetricSeries; bytes: MetricSeries } {
  const steps = image.downloadTimeline ?? [];
  const start = Date.now() - Math.max(imageDownloadDuration(image), 1);
  let cursor = start;

  const points = steps.flatMap((step) => {
    const stepStart = cursor;
    const stepEnd = cursor + step.durationMs;
    cursor = stepEnd;
    return [
      { timestamp: new Date(stepStart).toISOString(), duration: step.durationMs, bytes: 0 },
      { timestamp: new Date(stepEnd).toISOString(), duration: step.durationMs, bytes: step.bytes ?? 0 },
    ];
  });

  return {
    duration: {
      id: `${image.id}-eager-download-duration`,
      name: 'image.eager.download_stage_ms',
      label: 'Stage duration',
      unit: 'ms',
      group: 'startup',
      points: points.map((point) => ({ timestamp: point.timestamp, value: point.duration })),
    },
    bytes: {
      id: `${image.id}-eager-download-bytes`,
      name: 'image.eager.download_bytes',
      label: 'Downloaded bytes',
      unit: 'bytes',
      group: 'io',
      points: points.map((point) => ({ timestamp: point.timestamp, value: point.bytes })),
    },
  };
}

function ratio(numerator: number, denominator: number) {
  return denominator === 0 ? 0 : numerator / denominator;
}

function ProfileTable({ profiles }: { profiles: ProfileArtifact[] }) {
  const [activeProfile, setActiveProfile] = useState<ProfileArtifact>();

  return (
    <div className="profile-viewer-grid single">
      <div className="table-card profile-list-card">
        <div className="table-titlebar">
          <div>
            <strong>Profile artifacts</strong>
            <span>Click a profile to inspect the flame graph in a larger view</span>
          </div>
        </div>
        <table>
          <thead>
            <tr><th>Profile</th><th>Type</th><th>Process Role</th><th>Duration</th><th>Samples</th><th>View</th></tr>
          </thead>
          <tbody>
            {profiles.map((profile) => (
              <tr key={profile.id} onClick={() => setActiveProfile(profile)}>
                <td><strong>{profile.id}</strong><small>{formatDateTime(profile.timestamp)}</small></td>
                <td>{profile.profileType}</td>
                <td>{profile.processRole}</td>
                <td>{formatDuration(profile.durationMs)}</td>
                <td>{profile.sampleCount.toLocaleString()}</td>
                <td><button className="inline-action" onClick={(event) => { event.stopPropagation(); setActiveProfile(profile); }}>Open</button></td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>

      {activeProfile && (
        <div className="profile-modal-backdrop" role="presentation" onClick={() => setActiveProfile(undefined)}>
          <section className="profile-modal" role="dialog" aria-modal="true" aria-label="Flame graph detail" onClick={(event) => event.stopPropagation()}>
          <div className="flamegraph-header">
            <div>
              <h3>Flame graph detail</h3>
              <p>{activeProfile.id} · {activeProfile.profileType} · {activeProfile.objectUri}</p>
            </div>
            <div className="profile-modal-actions">
              <span>{activeProfile.sampleCount.toLocaleString()} samples</span>
              <button onClick={() => setActiveProfile(undefined)}>Close</button>
            </div>
          </div>
          {activeProfile.flamegraph ? <FlameGraph root={activeProfile.flamegraph} /> : <div className="empty-state">No flame graph data available.</div>}
          </section>
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
