import { useEffect, useMemo, useState } from 'react';
import type { RuntimePulseApi } from '../../api/RuntimePulseApi';
import type { AnalysisFinding, EventRecord, FlamegraphFrame, Image, MetricSeries, Node, ProfileArtifact, Sandbox, SandboxAnalysis, TraceSpan } from '../../domain/model';
import { MetricChart } from '../../components/charts/MetricChart';
import { EventList } from '../../components/timeline/EventList';
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

const refreshIntervalMs = 5000;

export function SandboxDetail({ api, sandboxId, onBack }: SandboxDetailProps) {
  const [tab, setTab] = useState<Tab>('overview');
  const [sandbox, setSandbox] = useState<Sandbox>();
  const [node, setNode] = useState<Node>();
  const [image, setImage] = useState<Image>();
  const [metrics, setMetrics] = useState<MetricSeries[]>([]);
  const [imageMetrics, setImageMetrics] = useState<MetricSeries[]>([]);
  const [imageEvents, setImageEvents] = useState<EventRecord[]>([]);
  const [events, setEvents] = useState<EventRecord[]>([]);
  const [spans, setSpans] = useState<TraceSpan[]>([]);
  const [profiles, setProfiles] = useState<ProfileArtifact[]>([]);
  const [analysis, setAnalysis] = useState<SandboxAnalysis>();
  const [selectedSpan, setSelectedSpan] = useState<TraceSpan>();
  const [selectedEvent, setSelectedEvent] = useState<EventRecord>();
  const [lastRefreshAt, setLastRefreshAt] = useState<string>();
  const [pinnedMetricGroups, setPinnedMetricGroups] = useState<string[]>([]);
  const [metricPanelSize, setMetricPanelSize] = useState<MetricPanelSize>('standard');

  useEffect(() => {
    let mounted = true;
    let timer: number | undefined;

    const refresh = () => {
      api.getSandbox(sandboxId).then(async (nextSandbox) => {
        if (!mounted || !nextSandbox) return;
        const [nextNode, nextImage, nextImageMetrics, nextImageEvents, nextMetrics, nextEvents, nextSpans, nextProfiles, nextAnalysis] = await Promise.all([
          api.getNode(nextSandbox.nodeId),
          api.getImage(nextSandbox.imageId),
          api.getImageMetrics(nextSandbox.imageId),
          api.getImageEvents(nextSandbox.imageId),
          api.getSandboxMetrics(sandboxId),
          api.getSandboxEvents(sandboxId),
          api.getSandboxTrace(sandboxId),
          api.getSandboxProfiles(sandboxId),
          api.getSandboxAnalysis(sandboxId),
        ]);
        if (!mounted) return;
        setSandbox(nextSandbox);
        setNode(nextNode);
        setImage(nextImage);
        setImageMetrics(nextImageMetrics);
        setImageEvents(nextImageEvents);
        setMetrics(nextMetrics);
        setEvents(nextEvents);
        setSpans(nextSpans);
        setProfiles(nextProfiles);
        setAnalysis(nextAnalysis);
        setLastRefreshAt(new Date().toISOString());
      });
    };

    refresh();
    timer = window.setInterval(refresh, refreshIntervalMs);

    return () => {
      mounted = false;
      if (timer) window.clearInterval(timer);
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
    const networkRx = metrics.find((item) => item.name === 'sandbox.network.rx_bytes');
    const networkTx = metrics.find((item) => item.name === 'sandbox.network.tx_bytes');
    const networkRxTotal = metrics.find((item) => item.name === 'sandbox.network.rx_total_bytes');
    const networkTxTotal = metrics.find((item) => item.name === 'sandbox.network.tx_total_bytes');
    return { io, networkRx, networkTx, networkRxTotal, networkTxTotal };
  }, [metrics]);

  function toggleMetricPin(group: string) {
    setPinnedMetricGroups((current) => current.includes(group) ? current.filter((item) => item !== group) : [...current, group]);
  }

  function jumpToMetricsFromEvent(event: EventRecord) {
    setSelectedEvent(event);
    setTab('metrics');
  }

  if (!sandbox) return <div className="empty-state">Loading sandbox...</div>;
  const startupBounds = sandboxStartupBounds(sandbox);

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
          {lastRefreshAt && <span className="refresh-pill">Updated {formatDateTime(lastRefreshAt)}</span>}
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
          <p>Collector-backed run preview for expert analysis. Review lifecycle events, startup spans, runtime process metrics, and profile artifacts from the live Query API view.</p>
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
          <StartupCallchainPanel sandbox={sandbox} metrics={metrics} spans={spans} onSelectSpan={setSelectedSpan} />
          {analysis && <AnalysisPanel analysis={analysis} spans={spans} metrics={metrics} onSelectSpan={setSelectedSpan} />}
          {image && (
            <ContainerImageAccessPanel
              sandbox={sandbox}
              image={image}
              imageEvents={imageEvents}
              imageMetrics={imageMetrics}
              ioSeries={pressureSeries.io}
              networkRxSeries={pressureSeries.networkRx}
              networkRxTotalSeries={pressureSeries.networkRxTotal}
              networkTxSeries={pressureSeries.networkTx}
              networkTxTotalSeries={pressureSeries.networkTxTotal}
            />
          )}
          <div className="overview-timeline-stack">
            <div className="panel-card">
              <h3>Lifecycle Timeline</h3>
              <EventTimeline bounds={startupBounds} events={events} selectedEventId={selectedEvent?.id} onSelectEvent={jumpToMetricsFromEvent} />
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

      {tab === 'timeline' && <div className="panel-card"><EventTimeline bounds={startupBounds} events={events} selectedEventId={selectedEvent?.id} onSelectEvent={jumpToMetricsFromEvent} /></div>}
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

function AnalysisPanel({
  analysis,
  spans,
  metrics,
  onSelectSpan,
}: {
  analysis: SandboxAnalysis;
  spans: TraceSpan[];
  metrics: MetricSeries[];
  onSelectSpan: (span: TraceSpan) => void;
}) {
  const topFinding = analysis.findings[0];

  return (
    <div className="panel-card analysis-panel">
      <div className="analysis-header">
        <div>
          <h3>Rule analysis</h3>
          <p>{analysis.summary}</p>
        </div>
        <div className="analysis-generated">
          <strong>{analysis.findings.length}</strong>
          <span>{analysis.bottleneckStage ?? 'no dominant stage'}</span>
        </div>
      </div>

      {topFinding ? (
        <div className={`analysis-lead ${topFinding.severity}`}>
          <span>{topFinding.category}</span>
          <strong>{topFinding.title}</strong>
          <p>{topFinding.summary}</p>
        </div>
      ) : (
        <div className="empty-state">No findings generated for this sandbox.</div>
      )}

      <div className="analysis-finding-grid">
        {analysis.findings.map((finding) => (
          <AnalysisFindingCard finding={finding} key={finding.id} metrics={metrics} spans={spans} onSelectSpan={onSelectSpan} />
        ))}
      </div>
    </div>
  );
}

function AnalysisFindingCard({
  finding,
  spans,
  metrics,
  onSelectSpan,
}: {
  finding: AnalysisFinding;
  spans: TraceSpan[];
  metrics: MetricSeries[];
  onSelectSpan: (span: TraceSpan) => void;
}) {
  const relatedSpans = (finding.relatedSpanIds ?? [])
    .map((spanId) => spans.find((span) => span.spanId === spanId))
    .filter((span): span is TraceSpan => Boolean(span));
  const relatedMetrics = (finding.relatedMetricNames ?? [])
    .map((name) => metrics.find((series) => series.name === name))
    .filter((series): series is MetricSeries => Boolean(series));

  return (
    <article className={`analysis-finding-card ${finding.severity}`}>
      <div className="analysis-finding-title">
        <span>{finding.severity}</span>
        <strong>{finding.title}</strong>
      </div>
      <p>{finding.summary}</p>
      <div className="analysis-list">
        <span>Evidence</span>
        {finding.evidence.slice(0, 3).map((item) => <em key={item}>{item}</em>)}
      </div>
      <div className="analysis-list">
        <span>Next actions</span>
        {finding.recommendedActions.slice(0, 2).map((item) => <em key={item}>{item}</em>)}
      </div>
      {relatedMetrics.length > 0 && (
        <div className="analysis-related-metrics">
          {relatedMetrics.slice(0, 4).map((series) => (
            <span key={series.name}>{metricShortLabel(series.name)} <strong>{formatMetricValue(latestMetricValue(series), series.unit)}</strong></span>
          ))}
        </div>
      )}
      {relatedSpans.length > 0 && (
        <div className="analysis-related-actions">
          {relatedSpans.map((span) => (
            <button key={span.spanId} onClick={() => onSelectSpan(span)}>Open {span.spanName}</button>
          ))}
        </div>
      )}
    </article>
  );
}


function StartupCallchainPanel({
  sandbox,
  metrics,
  spans,
  onSelectSpan,
}: {
  sandbox: Sandbox;
  metrics: MetricSeries[];
  spans: TraceSpan[];
  onSelectSpan: (span: TraceSpan) => void;
}) {
  const phases = startupCallchainPhases(metrics, spans);
  const totalDuration = startupCallchainTotal(sandbox, metrics, spans);
  const maxDuration = Math.max(...phases.items.map((phase) => phase.durationMs), 1);
  const hasCallchainData = phases.items.some((phase) => phase.durationMs > 0 || phase.count > 0);

  if (!hasCallchainData) return null;

  return (
    <div className="panel-card startup-callchain-panel">
      <div className="startup-callchain-header">
        <div>
          <h3>RunPod startup call chain</h3>
          <p>Collector-derived CNI plugin, OCI, Kata, and helper attribution for this sandbox startup.</p>
        </div>
        <div className="startup-callchain-total">
          <strong>{formatDuration(totalDuration)}</strong>
          <span>measured startup</span>
        </div>
      </div>
      {phases.cniPlugins.length > 0 && (
        <div className="cni-plugin-breakdown">
          <div className="cni-plugin-breakdown-header">
            <strong>CNI plugin binaries</strong>
            <span>{phases.cniPlugins.length} observed</span>
          </div>
          <div className="cni-plugin-list">
            {phases.cniPlugins.map((plugin) => (
              <article className="cni-plugin-row" key={plugin.binary}>
                <div>
                  <strong>{plugin.binary}</strong>
                  <span>{plugin.count} calls · {formatRatio(ratio(plugin.durationMs, Math.max(totalDuration, 1)))} of startup</span>
                </div>
                <div className="cni-plugin-track"><i style={{ width: `${Math.max(plugin.durationMs > 0 ? 7 : 0, plugin.durationMs / Math.max(phases.cniMaxDurationMs, 1) * 100)}%` }} /></div>
                <em>{formatDuration(plugin.durationMs)}</em>
              </article>
            ))}
          </div>
        </div>
      )}
      <div className="startup-callchain-grid">
        {phases.items.map((phase) => (
          <article className={`startup-callchain-card ${phase.tone}`} key={phase.key}>
            <div className="startup-callchain-card-title">
              <span>{phase.label}{phase.binary ? ` · ${phase.binary}` : ''}</span>
              <strong>{formatDuration(phase.durationMs)}</strong>
            </div>
            <div className="startup-callchain-track"><i style={{ width: `${Math.max(phase.durationMs > 0 ? 7 : 0, phase.durationMs / maxDuration * 100)}%` }} /></div>
            <div className="startup-callchain-meta">
              <span>{formatRatio(ratio(phase.durationMs, Math.max(totalDuration, 1)))} of startup</span>
              {phase.count > 0 && <span>{phase.count} calls</span>}
            </div>
            {phase.relatedSpans.length > 0 && (
              <div className="startup-callchain-spans">
                {phase.relatedSpans.slice(0, 3).map((span) => (
                  <button key={span.spanId} onClick={() => onSelectSpan(span)}>{span.spanName}</button>
                ))}
              </div>
            )}
          </article>
        ))}
      </div>
    </div>
  );
}

function startupCallchainPhases(metrics: MetricSeries[], spans: TraceSpan[]) {
  const definitions = [
    { key: 'cni', label: 'CNI plugin', durationMetric: 'sandbox.startup.cni_duration_ms', countMetric: 'sandbox.startup.cni_plugin_count', tone: 'network', patterns: [/^cni\./i] },
    { key: 'oci', label: 'OCI runtime', durationMetric: 'sandbox.startup.oci_duration_ms', countMetric: 'sandbox.startup.oci_call_count', tone: 'runtime', patterns: [/\boci\b/i, /runc/i, /runtime\.(create|start)/i] },
    { key: 'kata', label: 'Kata VM', durationMetric: 'sandbox.startup.kata_duration_ms', countMetric: undefined, tone: 'secure', patterns: [/kata/i, /vm\.(boot|start|ready)/i, /hypervisor/i] },
    { key: 'helpers', label: 'Helper drill-down', durationMetric: 'sandbox.startup.helper_binary_duration_ms', countMetric: 'sandbox.startup.helper_binary_count', tone: 'helper', patterns: [/exec/i, /process/i] },
    { key: 'exec', label: 'Exec total', durationMetric: 'sandbox.startup.binary_exec_duration_ms', countMetric: 'sandbox.startup.binary_exec_count', tone: 'helper', patterns: [/exec/i, /process/i] },
  ];

  const cniPlugins = cniPluginMetrics(metrics);
  const items = definitions.map((definition) => {
    const relatedSpans = definition.key === 'cni'
      ? spans.filter(isCniStartupSpan)
      : spans.filter((span) => definition.patterns.some((pattern) => pattern.test(span.spanName)));
    const spanDuration = relatedSpans.reduce((sum, span) => sum + span.durationMs, 0);
    const pluginBreakdown = definition.key === 'cni' ? cniPlugins[0] : undefined;
    return {
      ...definition,
      binary: pluginBreakdown?.binary ?? dominantStartupBinaryName(relatedSpans),
      durationMs: pluginBreakdown?.durationMs ?? latestMetricByName(metrics, definition.durationMetric) ?? spanDuration,
      count: pluginBreakdown?.count ?? (definition.countMetric ? latestMetricByName(metrics, definition.countMetric) ?? 0 : 0),
      relatedSpans: pluginBreakdown?.binary ? relatedSpans.filter((span) => spanMatchesBinary(span, pluginBreakdown.binary)) : relatedSpans,
    };
  });

  return {
    items,
    cniPlugins,
    cniMaxDurationMs: Math.max(...cniPlugins.map((plugin) => plugin.durationMs), 1),
  };
}

function cniPluginMetrics(metrics: MetricSeries[]) {
  const plugins = new Map<string, { binary: string; count: number; durationMs: number }>();

  for (const series of metrics) {
    const match = series.name.match(/^sandbox\.startup\.cni\.plugin\.(.+)_(count|duration_ms)$/);
    if (!match) continue;

    const [, binary, kind] = match;
    const candidate = plugins.get(binary) ?? { binary, count: 0, durationMs: 0 };
    const value = latestMetricValue(series);
    if (kind === 'count') candidate.count = Math.max(candidate.count, value);
    if (kind === 'duration_ms') candidate.durationMs = Math.max(candidate.durationMs, value);
    plugins.set(binary, candidate);
  }

  return Array.from(plugins.values()).sort((left, right) => right.durationMs - left.durationMs || right.count - left.count);
}

function isCniStartupSpan(span: TraceSpan) {
  return span.spanName.startsWith('cni.')
    || Boolean(span.attributes?.['cni.plugin'])
    || Boolean(span.attributes?.['cni.command'])
    || Boolean(span.attributes?.['cni.container_id'])
    || Boolean(span.attributes?.['cni.netns']);
}

function dominantStartupBinaryName(spans: TraceSpan[]) {
  const counts = new Map<string, number>();

  for (const span of spans) {
    const binary = processBinaryName(span) ?? cniPluginName(span) ?? binaryFromSpanName(span);
    if (!binary) continue;
    counts.set(binary, (counts.get(binary) ?? 0) + 1);
  }

  return Array.from(counts.entries()).sort((left, right) => right[1] - left[1])[0]?.[0];
}

function processBinaryName(span: TraceSpan) {
  const raw = span.attributes?.['process.binary'] ?? span.attributes?.['process.command'];
  if (typeof raw !== 'string' || !raw.trim()) return undefined;
  return raw.trim().split(/\s+/)[0].split('/').filter(Boolean).pop();
}

function cniPluginName(span: TraceSpan) {
  const value = span.attributes?.['cni.plugin'];
  return typeof value === 'string' && value.trim() ? value.trim() : undefined;
}

function binaryFromSpanName(span: TraceSpan) {
  if (span.spanName.startsWith('cni.plugin.')) return span.spanName.slice('cni.plugin.'.length).split('.').filter(Boolean).pop();
  if (span.spanName.startsWith('oci.')) return span.spanName.slice('oci.'.length).split('.').filter(Boolean).pop();
  return undefined;
}

function spanMatchesBinary(span: TraceSpan, binary: string) {
  return [processBinaryName(span), cniPluginName(span), binaryFromSpanName(span)]
    .filter(Boolean)
    .some((value) => value === binary);
}

function startupCallchainTotal(sandbox: Sandbox, metrics: MetricSeries[], spans: TraceSpan[]) {
  return sandbox.startupDurationMs
    || latestMetricByName(metrics, 'sandbox.startup.callchain_duration_ms')
    || latestMetricByName(metrics, 'sandbox.startup.e2e_duration_ms')
    || latestMetricByName(metrics, 'sandbox.startup.duration_ms')
    || Math.max(0, ...spans
      .filter((span) => ['sandbox.startup.callchain', 'sandbox.startup.e2e', 'sandbox.startup'].includes(span.spanName))
      .map((span) => span.durationMs));
}

function latestMetricByName(metrics: MetricSeries[], name?: string) {
  if (!name) return undefined;
  const values = metrics
    .filter((series) => series.name === name)
    .map((series) => latestMetricValue(series))
    .filter((value) => Number.isFinite(value));
  return values.length > 0 ? Math.max(...values) : undefined;
}

function metricShortLabel(name: string) {
  return name.replace(/^sandbox\.startup\./, '').replace(/_/g, ' ');
}

function formatMetricValue(value: number, unit: string) {
  if (unit === 'ms') return formatDuration(value);
  if (unit === 'bytes') return formatBytes(value);
  if (unit === 'ratio') return formatRatio(value);
  return Number.isInteger(value) ? String(value) : value.toFixed(2);
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
  const sourceSummary = sandboxSourceSummary(sandbox, metrics, events);

  return (
    <div className="detail-data-sections">
      <section className="data-section sources">
        <div className="data-section-header">
          <span>Sources</span>
          <strong>数据来源</strong>
        </div>
        <div className="source-facts-grid">
          {sourceSummary.map((item) => (
            <SourceFact item={item} key={item.label} />
          ))}
        </div>
      </section>

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

type SourceSummaryItem = {
  label: string;
  primary: string;
  secondary: string;
  tone?: 'runtime' | 'metrics' | 'image';
};

function SourceFact({ item }: { item: SourceSummaryItem }) {
  return (
    <div className={`source-fact ${item.tone ?? ''}`}>
      <span>{item.label}</span>
      <strong>{item.primary}</strong>
      <em>{item.secondary}</em>
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

function sandboxSourceSummary(sandbox: Sandbox, metrics: MetricSeries[], events: EventRecord[]): SourceSummaryItem[] {
  const runtimeSource = stringAttribute(sandbox.attributes, 'runtime.source') ?? sourceFromPlugin(sandbox.labels.plugin) ?? 'collector';
  const runtimePlugin = stringAttribute(sandbox.attributes, 'plugin') ?? sandbox.labels.plugin ?? runtimeSource;
  const runtimeId = stringAttribute(sandbox.attributes, 'containerd.id')
    ?? stringAttribute(sandbox.attributes, 'docker.id')
    ?? stringAttribute(sandbox.attributes, 'cri.container_id')
    ?? sandbox.id;
  const metricsSources = uniqueStrings(metrics.map((series) => stringAttribute(series.attributes, 'metrics.source') ?? stringAttribute(series.attributes, 'plugin')).filter(Boolean));
  const prometheusMetrics = metrics.filter((series) => stringAttribute(series.attributes, 'metrics.source') === 'prometheus');
  const latestPrometheusMetric = prometheusMetrics.flatMap((series) => series.points).sort((left, right) => Date.parse(right.timestamp) - Date.parse(left.timestamp))[0]?.timestamp;
  const runtimeEvents = events.filter((event) => stringAttribute(event.attributes, 'runtime.source') || stringAttribute(event.attributes, 'containerd.id') || stringAttribute(event.attributes, 'docker.id') || stringAttribute(event.attributes, 'cri.container_id'));
  const latestRuntimeEvent = runtimeEvents.sort((left, right) => Date.parse(right.timestamp) - Date.parse(left.timestamp))[0]?.timestamp;

  return [
    {
      label: 'Runtime',
      primary: runtimeSource,
      secondary: `${runtimePlugin} · ${runtimeId}`,
      tone: 'runtime',
    },
    {
      label: 'Metrics',
      primary: metricsSources.length > 0 ? metricsSources.join(', ') : 'none',
      secondary: latestPrometheusMetric ? `latest ${formatDateTime(latestPrometheusMetric)}` : `${metrics.length} series`,
      tone: 'metrics',
    },
    {
      label: 'Lifecycle',
      primary: runtimeEvents.length > 0 ? `${runtimeEvents.length} runtime events` : `${events.length} events`,
      secondary: latestRuntimeEvent ? `latest ${formatDateTime(latestRuntimeEvent)}` : 'no runtime event yet',
      tone: 'runtime',
    },
    {
      label: 'Image',
      primary: stringAttribute(sandbox.attributes, 'containerd.snapshotter') ?? stringAttribute(sandbox.attributes, 'image.snapshotter') ?? 'image metadata',
      secondary: sandbox.imageRef,
      tone: 'image',
    },
  ];
}

function sourceFromPlugin(plugin?: string) {
  if (!plugin) return undefined;
  if (plugin.includes('containerd')) return 'containerd';
  if (plugin.includes('docker')) return 'docker';
  if (plugin.includes('kubelet') || plugin.includes('cri')) return 'cri';
  return plugin;
}

function uniqueStrings(values: Array<string | undefined>): string[] {
  return Array.from(new Set(values.filter((value): value is string => Boolean(value))));
}

function stringAttribute(attributes: Record<string, unknown> | undefined, name: string) {
  const value = attributes?.[name];
  return typeof value === 'string' && value.trim() ? value : undefined;
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
  imageEvents,
  imageMetrics,
  ioSeries,
  networkRxSeries,
  networkRxTotalSeries,
  networkTxSeries,
  networkTxTotalSeries,
}: {
  sandbox: Sandbox;
  image: Image;
  imageEvents: EventRecord[];
  imageMetrics: MetricSeries[];
  ioSeries?: MetricSeries;
  networkRxSeries?: MetricSeries;
  networkRxTotalSeries?: MetricSeries;
  networkTxSeries?: MetricSeries;
  networkTxTotalSeries?: MetricSeries;
}) {
  const startupBounds = sandboxStartupBounds(sandbox);
  const layers = image.layers ?? [];
  const avgIo = averageMetricValue(ioSeries);
  const peakIo = Math.max(...(ioSeries?.points.map((point) => point.value) ?? [0]), 0);
  const avgNetworkRx = averageMetricValue(networkRxSeries);
  const avgNetworkTx = averageMetricValue(networkTxSeries);
  const latestNetworkRxTotal = latestMetricValue(networkRxTotalSeries);
  const latestNetworkTxTotal = latestMetricValue(networkTxTotalSeries);
  const requestedBlocks = imageRequestedBlocks(image);
  const blockHitRatio = latestMetricValue(imageMetrics.find((series) => series.name === 'image.lazy.cache_hit_ratio')) ?? imageBlockHitRatio(image);
  const remoteReadBytes = latestMetricValue(imageMetrics.find((series) => series.name === 'image.lazy.remote_read_bytes')) ?? imageRemoteReadBytes(image);
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
          <Info label="Avg Network RX" value={formatBytes(avgNetworkRx)} />
          <Info label="Avg Network TX" value={formatBytes(avgNetworkTx)} />
          <Info label="Network Total" value={`${formatBytes(latestNetworkRxTotal)} / ${formatBytes(latestNetworkTxTotal)}`} />
        </div>
      </div>
      {image.loadingMode === 'lazy' ? (
        <LazyImageTemporalPanel image={image} metrics={imageMetrics} />
      ) : (
        <ImageDownloadTimeline bounds={startupBounds} image={image} metrics={imageMetrics} />
      )}
      <div className="container-image-events">
        <h3>Image events</h3>
        <EventList emptyLabel="No image events available." events={imageEvents} />
      </div>
    </div>
  );
}

function LazyImageTemporalPanel({ image, metrics = [] }: { image: Image; metrics?: MetricSeries[] }) {
  const layers = image.layers ?? [];
  const cacheSeries = lazyImageCacheSeries(image, metrics);

  return (
    <div className="container-layer-access">
      <div className="image-temporal-grid">
        <MetricChart height={180} series={[cacheSeries.hitRatio]} subtitle="Block-level cache hit ratio over time" title="Lazy cache hit ratio" />
        <MetricChart height={180} series={[cacheSeries.remoteRead]} subtitle="Remote bytes read because blocks missed cache" title="Lazy remote reads" />
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

function ImageDownloadTimeline({ bounds, image, metrics = [] }: { bounds?: { startTime: string; endTime: string }; image: Image; metrics?: MetricSeries[] }) {
  const steps = image.downloadTimeline ?? [];
  const totalDuration = imageDownloadDuration(image);
  const maxDuration = Math.max(...steps.map((step) => step.durationMs), 1);
  const downloadSeries = eagerImageDownloadSeries(image, metrics, bounds);
  const observedOnly = totalDuration === 0 && steps.length > 0;

  return (
    <div className="image-download-panel">
      <div className="image-layer-header">
        <div>
          <h3>Image download timeline</h3>
          <p>{image.ref} · {observedOnly ? 'observed image events; precise pull timing needs lower-level data' : 'non-lazy pull before container start'}</p>
        </div>
        <div className="image-cache-summary">
          <strong>{observedOnly ? String(steps.length) : formatDuration(totalDuration)}</strong>
          <span>{observedOnly ? 'observed stages' : 'download path'}</span>
        </div>
      </div>
      {!observedOnly && (
        <div className="image-temporal-grid single">
          <MetricChart height={210} series={downloadSeries} stacked subtitle="Stacked duration of image download stages" title="Image download stages" />
        </div>
      )}
      <div className="download-timeline">
        {steps.map((step) => (
          <article className="download-step" key={step.id}>
            <div>
              <strong>{step.name}</strong>
              <span>{step.detail}</span>
            </div>
            <div className={`download-step-track ${step.durationMs === 0 ? 'observed' : ''}`}><i className={step.phase} style={{ width: `${step.durationMs === 0 ? 100 : Math.max(6, step.durationMs / maxDuration * 100)}%` }} /></div>
            <em>{step.durationMs === 0 ? 'observed' : formatDuration(step.durationMs)}{step.bytes ? ` · ${formatBytes(step.bytes)}` : ''}</em>
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

function latestMetricValue(series?: MetricSeries) {
  return series?.points.at(-1)?.value ?? 0;
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

function lazyImageCacheSeries(image: Image, metrics: MetricSeries[] = []): { hitRatio: MetricSeries; remoteRead: MetricSeries } {
  const realHitRatio = metrics.find((series) => series.name === 'image.lazy.cache_hit_ratio');
  const realRemoteRead = metrics.find((series) => series.name === 'image.lazy.remote_read_bytes');
  if (realHitRatio && realRemoteRead) {
    return {
      hitRatio: realHitRatio,
      remoteRead: realRemoteRead,
    };
  }

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

function eagerImageDownloadSeries(image: Image, metrics: MetricSeries[] = [], bounds?: { startTime: string; endTime: string }): MetricSeries[] {
  const realSeries = metrics.filter((series) => series.name.startsWith('image.eager.') && series.name.endsWith('_ms'));
  if (realSeries.length > 0) return realSeries;

  const steps = image.downloadTimeline ?? [];
  const totalDuration = Math.max(imageDownloadDuration(image), 1);
  const boundStart = bounds ? new Date(bounds.startTime).getTime() : undefined;
  const boundEnd = bounds ? new Date(bounds.endTime).getTime() : undefined;
  const start = boundStart !== undefined && boundEnd !== undefined
    ? Math.max(boundStart, boundEnd - totalDuration)
    : Date.now() - totalDuration;
  let cursor = start;
  const timeline = [{ timestamp: new Date(start).toISOString(), completedStepIndex: -1 }];

  steps.forEach((step, index) => {
    cursor += step.durationMs;
    timeline.push({ timestamp: new Date(cursor).toISOString(), completedStepIndex: index });
  });

  return steps.map((step, stepIndex) => ({
    id: `${image.id}-eager-${step.phase}`,
    name: `image.eager.${step.phase}_ms`,
    label: step.name,
    unit: 'ms',
    group: 'startup',
    points: timeline.map((point) => ({
      timestamp: point.timestamp,
      value: point.completedStepIndex >= stepIndex ? step.durationMs : 0,
    })),
  }));
}

function sandboxStartupBounds(sandbox: Sandbox) {
  const start = new Date(sandbox.createdAt).getTime();
  const end = sandbox.startedAt
    ? new Date(sandbox.startedAt).getTime()
    : start + Math.max(sandbox.startupDurationMs, 1);

  return {
    startTime: new Date(start).toISOString(),
    endTime: new Date(Math.max(start + 1, end)).toISOString(),
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
        <span>Preview renderer for profile artifacts exposed by the Query API.</span>
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
