import { useEffect, useMemo, useState } from 'react';
import type { ReactNode } from 'react';
import type { RuntimePulseApi } from '../../api/RuntimePulseApi';
import type { Cluster, Image, MetricSeries, Node, Sandbox, TraceSpan } from '../../domain/model';
import { MetricChart } from '../../components/charts/MetricChart';
import { formatBytes, formatDuration, formatRatio } from '../../utils/units';
import { formatDateTime, toMs } from '../../utils/time';
import { RuntimeBadge, StatusBadge, SummaryCard } from '../SandboxExplorer/SandboxExplorer';

type NodeDetailProps = {
  api: RuntimePulseApi;
  nodeId: string;
  onBack: () => void;
  onSelectSandbox: (id: string) => void;
};

export function NodeDetail({ api, nodeId, onBack, onSelectSandbox }: NodeDetailProps) {
  const [node, setNode] = useState<Node>();
  const [cluster, setCluster] = useState<Cluster>();
  const [sandboxes, setSandboxes] = useState<Sandbox[]>([]);
  const [images, setImages] = useState<Image[]>([]);
  const [metrics, setMetrics] = useState<Record<string, MetricSeries[]>>({});
  const [traces, setTraces] = useState<Record<string, TraceSpan[]>>({});
  const [selectedLifecycleRange, setSelectedLifecycleRange] = useState<ContainerRange>();
  const [modal, setModal] = useState<{ type: 'image' | 'sandbox'; id: string }>();

  useEffect(() => {
    let mounted = true;
    api.getNode(nodeId).then(async (nextNode) => {
      if (!mounted || !nextNode) return;

      const [clusters, allImages, allSandboxes] = await Promise.all([
        api.listClusters(),
        api.listImages(),
        api.listSandboxes(),
      ]);
      const nodeSandboxes = allSandboxes.filter((sandbox) => sandbox.nodeId === nextNode.id);
      const nodeImages = allImages.filter((image) => nodeSandboxes.some((sandbox) => sandbox.imageId === image.id));
      const [metricEntries, traceEntries] = await Promise.all([
        Promise.all(nodeSandboxes.map(async (sandbox) => [sandbox.id, await api.getSandboxMetrics(sandbox.id)] as const)),
        Promise.all(nodeSandboxes.map(async (sandbox) => [sandbox.id, await api.getSandboxTrace(sandbox.id)] as const)),
      ]);

      if (!mounted) return;
      setNode(nextNode);
      setCluster(clusters.find((item) => item.id === nextNode.clusterId));
      setSandboxes(nodeSandboxes);
      setImages(nodeImages);
      setMetrics(Object.fromEntries(metricEntries));
      setTraces(Object.fromEntries(traceEntries));
    });

    return () => {
      mounted = false;
    };
  }, [api, nodeId]);

  const nodeStats = useMemo(() => {
    const running = sandboxes.filter((sandbox) => sandbox.status === 'running').length;
    const failed = sandboxes.filter((sandbox) => sandbox.status === 'failed').length;
    const slow = sandboxes.filter((sandbox) => sandbox.startupDurationMs > 7000).length;
    const avgStartup = average(sandboxes.map((sandbox) => sandbox.startupDurationMs));
    const avgCpu = average(sandboxes.map((sandbox) => sandbox.cpuAvg));
    const memoryPeak = Math.max(...sandboxes.map((sandbox) => sandbox.memoryPeakBytes), 0);
    return { running, failed, slow, avgStartup, avgCpu, memoryPeak };
  }, [sandboxes]);

  const imageStats = useMemo(() => {
    const lazyImages = images.filter((image) => image.loadingMode === 'lazy');
    const eagerImages = images.filter((image) => image.loadingMode === 'eager');
    const eagerDownloadDuration = eagerImages.reduce((sum, image) => sum + imageDownloadDuration(image), 0);
    const eagerDownloadBytes = eagerImages.reduce((sum, image) => sum + image.sizeBytes, 0);
    const totalSize = images.reduce((sum, image) => sum + image.sizeBytes, 0);
    return { eagerDownloadBytes, eagerDownloadDuration, eagerImages, lazyImages, totalSize };
  }, [images, sandboxes]);

  const pressureSeries = useMemo(() => {
    const allSeries = Object.values(metrics).flat();
    return {
      cpu: aggregateSeries(allSeries.filter((series) => series.name === 'sandbox.cpu.usage_ratio'), 'avg'),
      io: aggregateSeries(allSeries.filter((series) => series.name === 'sandbox.io.read_bytes'), 'sum'),
      memory: aggregateSeries(allSeries.filter((series) => series.name === 'sandbox.memory.working_set_bytes'), 'sum'),
      psi: aggregateSeries(allSeries.filter((series) => series.name === 'node.psi.io.some'), 'avg'),
    };
  }, [metrics]);

  const temporalSeries = useMemo(() => {
    const reference = pressureSeries.cpu ?? pressureSeries.io ?? pressureSeries.memory ?? pressureSeries.psi;
    return {
      container: reference ? [
        timelineSeries(reference, 'node.containers.running', 'Running containers', 'count', 'startup', nodeStats.running, 1),
        timelineSeries(reference, 'node.containers.failed', 'Failed containers', 'count', 'startup', nodeStats.failed, 0.35),
        timelineSeries(reference, 'node.containers.slow_start', 'Slow starts', 'count', 'startup', nodeStats.slow, 0.6),
      ] : [],
      image: reference ? [
        timelineSeries(reference, 'node.images.lazy_remote_read', 'Lazy remote read', 'bytes', 'io', images.filter((image) => image.loadingMode === 'lazy').reduce((sum, image) => sum + imageRemoteReadBytes(image), 0), 0.7),
        timelineSeries(reference, 'node.images.eager_download', 'Non-lazy download', 'bytes', 'io', imageStats.eagerDownloadBytes, 0.8),
      ] : [],
    };
  }, [images, imageStats.eagerDownloadBytes, nodeStats.failed, nodeStats.running, nodeStats.slow, pressureSeries.cpu, pressureSeries.io, pressureSeries.memory, pressureSeries.psi]);

  const selectedImage = modal?.type === 'image' ? images.find((image) => image.id === modal.id) : undefined;
  const selectedSandbox = modal?.type === 'sandbox' ? sandboxes.find((sandbox) => sandbox.id === modal.id) : undefined;
  const selectedSandboxImage = selectedSandbox ? images.find((image) => image.id === selectedSandbox.imageId) : undefined;
  const selectedSandboxMetrics = selectedSandbox ? metrics[selectedSandbox.id] ?? [] : [];

  if (!node) return <div className="empty-state">Loading node...</div>;

  return (
    <section className="page-stack node-detail-page">
      <button className="back-button" onClick={onBack}>← Back to clusters</button>

      <div className="page-header">
        <div>
          <p className="eyebrow">Node level</p>
          <h2>{node.name}</h2>
          <p>{cluster?.name ?? node.clusterId} / {node.id}。在此查看节点上的沙箱和镜像，并继续下钻到沙箱详情。</p>
        </div>
        <div className="header-actions">
          <button>Export node</button>
          <button className="primary">Create report</button>
        </div>
      </div>

      <section className="node-section-card">
        <div className="section-titlebar">
          <div>
            <strong>Static data</strong>
            <span>节点拓扑、系统配置和容量信息。</span>
          </div>
        </div>
        <div className="node-static-grid">
          <NodeFact label="Cluster" value={cluster?.name ?? node.clusterId} />
          <NodeFact label="Node ID" value={node.id} />
          <NodeFact label="Node name" value={node.name} />
          <NodeFact label="Kernel" value={node.kernelVersion} />
          <NodeFact label="CPU capacity" value={`${node.cpuCores} cores`} />
          <NodeFact label="Memory capacity" value={formatBytes(node.memoryBytes)} />
        </div>
      </section>

      <section className="node-section-card">
        <div className="section-titlebar">
          <div>
            <strong>Direct dynamic data</strong>
            <span>节点 CPU/IO、容器总体信息、镜像总体信息。</span>
          </div>
        </div>
        <div className="node-temporal-grid">
          <MetricChart height={190} series={[pressureSeries.cpu, pressureSeries.psi].filter((series): series is MetricSeries => Boolean(series))} />
          <MetricChart height={190} series={[pressureSeries.io].filter((series): series is MetricSeries => Boolean(series))} />
          <MetricChart height={190} series={[pressureSeries.memory].filter((series): series is MetricSeries => Boolean(series))} />
          <MetricChart height={190} series={temporalSeries.container} />
          <MetricChart height={190} series={temporalSeries.image} />
        </div>
        <NodePressureOverlay node={node} ioSeries={pressureSeries.io} psiSeries={pressureSeries.psi} />
      </section>

      <section className="node-section-card">
        <div className="section-titlebar">
          <div>
            <strong>Container change overview</strong>
            <span>感知容器数量变化，并按时间段合并查看生命周期耗时。</span>
          </div>
        </div>
        <ContainerChangeOverview
          onSelectRange={setSelectedLifecycleRange}
          sandboxes={sandboxes}
          selectedRange={selectedLifecycleRange}
          traces={traces}
        />
      </section>

      <section className="node-lists-grid">
        <div className="table-card">
          <div className="table-titlebar">
            <div>
              <strong>Image list</strong>
              <span>点击镜像查看层和使用详情</span>
            </div>
          </div>
          <table>
            <thead>
              <tr>
                <th>Image</th>
                <th>Type</th>
                <th>Runtime Activity</th>
                <th>Size</th>
                <th>Layers</th>
              </tr>
            </thead>
            <tbody>
              {images.map((image) => {
                const imageSandboxes = sandboxes.filter((sandbox) => sandbox.imageId === image.id);
                return (
                  <tr className="clickable-row" key={image.id} onClick={() => setModal({ type: 'image', id: image.id })}>
                    <td><strong>{image.ref}</strong><small>{image.digest}</small></td>
                    <td><ImageModeBadge mode={image.loadingMode} /></td>
                    <td><ImageActivityCell image={image} sandboxes={imageSandboxes} /></td>
                    <td>{formatBytes(image.sizeBytes)}</td>
                    <td>{image.layerCount}</td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        </div>

        <div className="table-card">
          <div className="table-titlebar">
            <div>
              <strong>Container list</strong>
              <span>点击容器查看镜像和 IO 明细</span>
            </div>
            <div className="column-pills">
              <span>dynamic</span>
              <span>static</span>
            </div>
          </div>
          <table>
            <thead>
              <tr>
                <th>Container</th>
                <th>Runtime</th>
                <th>Status</th>
                <th>CPU / IO trend</th>
                <th>Memory trend</th>
                <th>Workload</th>
                <th>Image</th>
              </tr>
            </thead>
            <tbody>
              {sandboxes.map((sandbox) => (
                <tr className="clickable-row" key={sandbox.id} onClick={() => setModal({ type: 'sandbox', id: sandbox.id })}>
                  <td><strong>{sandbox.id}</strong><small>{sandbox.namespace}</small></td>
                  <td><RuntimeBadge runtimeType={sandbox.runtimeType} /></td>
                  <td><StatusBadge status={sandbox.status} /></td>
                  <td><SandboxMetricSparklines metrics={metrics[sandbox.id] ?? []} names={['sandbox.cpu.usage_ratio', 'sandbox.io.read_bytes']} /></td>
                  <td><SandboxMetricSparklines metrics={metrics[sandbox.id] ?? []} names={['sandbox.memory.working_set_bytes']} /></td>
                  <td>{sandbox.workloadName}</td>
                  <td className="truncate">{sandbox.imageRef}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      </section>

      {selectedImage && (
        <DetailModal title="Image detail" onClose={() => setModal(undefined)}>
          <ImageDetail image={selectedImage} sandboxes={sandboxes.filter((sandbox) => sandbox.imageId === selectedImage.id)} />
        </DetailModal>
      )}
      {selectedSandbox && (
        <DetailModal title="Container detail" onClose={() => setModal(undefined)}>
          <ContainerDetail
            image={selectedSandboxImage}
            metrics={selectedSandboxMetrics}
            onOpenSandbox={() => onSelectSandbox(selectedSandbox.id)}
            sandbox={selectedSandbox}
          />
        </DetailModal>
      )}
    </section>
  );
}

function NodeFact({ label, value }: { label: string; value: string }) {
  return (
    <div className="node-fact">
      <span>{label}</span>
      <strong>{value}</strong>
    </div>
  );
}

type ContainerRange = {
  from: string;
  to: string;
};

function ContainerChangeOverview({
  onSelectRange,
  sandboxes,
  selectedRange,
  traces,
}: {
  onSelectRange: (range: ContainerRange) => void;
  sandboxes: Sandbox[];
  selectedRange?: ContainerRange;
  traces: Record<string, TraceSpan[]>;
}) {
  const overviewSeries = useMemo(() => buildContainerChangeSeries(sandboxes), [sandboxes]);

  return (
    <div className="container-change-panel">
      <MetricChart height={190} onSelectRange={onSelectRange} selectable selectedRange={selectedRange} series={overviewSeries} />
      <div className="range-selection-bar">
        {selectedRange ? (
          <span>{formatDateTime(selectedRange.from)} - {formatDateTime(selectedRange.to)}</span>
        ) : (
          <span>Select a time range on the chart to merge lifecycle stages.</span>
        )}
      </div>
      <MergedLifecycleView sandboxes={sandboxes} traces={traces} range={selectedRange} />
    </div>
  );
}

function MergedLifecycleView({ range, sandboxes, traces }: { range?: ContainerRange; sandboxes: Sandbox[]; traces: Record<string, TraceSpan[]> }) {
  const rangeStart = range ? toMs(range.from) : 0;
  const rangeEnd = range ? toMs(range.to) : 0;
  const selectedSandboxes = range ? sandboxes.filter((sandbox) => lifecycleOverlapsRange(sandbox, range)) : [];
  const lifecycleRows = selectedSandboxes.map((sandbox) => {
    const spans = (traces[sandbox.id] ?? []).filter((span) => spanOverlapsRange(span, rangeStart, rangeEnd));
    return { sandbox, spans };
  }).filter((row) => row.spans.length > 0);
  const stageBottlenecks = range ? buildLifecycleStageBottlenecks(lifecycleRows.flatMap((row) => row.spans), rangeStart, rangeEnd) : [];
  const dominantStage = stageBottlenecks[0];
  const maxStageCost = Math.max(...stageBottlenecks.map((stage) => stage.totalDurationMs), 1);
  const rangeDuration = Math.max(rangeEnd - rangeStart, 1);
  const totalStageCost = stageBottlenecks.reduce((sum, stage) => sum + stage.totalDurationMs, 0);

  if (!range) return <div className="empty-state">Select a time range on the startup chart to inspect merged lifecycle phases.</div>;
  if (stageBottlenecks.length === 0) return <div className="empty-state">No lifecycle stages in the selected time range.</div>;

  return (
    <div className="merged-lifecycle">
      <div className="merged-lifecycle-header">
        <div>
          <strong>Merged lifecycle view</strong>
          <span>{formatDateTime(range.from)} - {formatDateTime(range.to)}</span>
        </div>
        <span>{lifecycleRows.length} containers · {stageBottlenecks.length} stages</span>
      </div>
      <div className="merged-lifecycle-axis">
        <span>{formatDateTime(range.from)}</span>
        <span>{formatDuration(rangeEnd - rangeStart)}</span>
        <span>{formatDateTime(range.to)}</span>
      </div>
      {dominantStage && (
        <div className="bottleneck-callout">
          <span>Bottleneck stage</span>
          <strong>{dominantStage.name}</strong>
          <p>
            {formatDuration(dominantStage.totalDurationMs)} aggregated stage time · p95 {formatDuration(dominantStage.p95DurationMs)} · {dominantStage.containerCount}/{lifecycleRows.length} containers
          </p>
        </div>
      )}
      <div className="bottleneck-ranking">
        {stageBottlenecks.map((stage, index) => (
          <div className={`bottleneck-row ${index === 0 ? 'dominant' : ''}`} key={stage.name}>
            <div className="bottleneck-label">
              <strong>{stage.name}</strong>
              <span>{stage.containerCount} containers · {formatRatio(ratio(stage.totalDurationMs, Math.max(totalStageCost, 1)))} of stage cost</span>
            </div>
            <div className="bottleneck-bar-track">
              <i style={{ width: `${Math.max(4, ratio(stage.totalDurationMs, maxStageCost) * 100)}%` }} />
            </div>
            <div className="bottleneck-metrics">
              <span>Total <b>{formatDuration(stage.totalDurationMs)}</b></span>
              <span>P95 <b>{formatDuration(stage.p95DurationMs)}</b></span>
              <span>Max <b>{formatDuration(stage.maxDurationMs)}</b></span>
              {stage.errorCount > 0 && <span className="hot-value">{stage.errorCount} errors</span>}
            </div>
          </div>
        ))}
      </div>
      <div className="lifecycle-container-summary">
        {lifecycleRows.sort((left, right) => right.sandbox.startupDurationMs - left.sandbox.startupDurationMs).map(({ sandbox, spans }) => (
          <div className="lifecycle-container-row" key={sandbox.id}>
            <div className="merged-container-label">
              <strong>{sandbox.id}</strong>
              <span>{sandbox.workloadName}</span>
            </div>
            <div className="parallel-stage-track">
              {spans.map((span) => {
                const clippedStart = Math.max(toMs(span.startTime), rangeStart);
                const clippedEnd = Math.min(toMs(span.endTime), rangeEnd);
                const left = ratio(clippedStart - rangeStart, rangeDuration) * 100;
                const width = ratio(Math.max(clippedEnd - clippedStart, 1), rangeDuration) * 100;
                const stageIndex = Math.max(stageBottlenecks.findIndex((stage) => stage.name === span.spanName), 0);
                return (
                  <span
                    className={`parallel-stage-segment stage-color-${stageIndex % 7} ${span.status === 'error' ? 'failed' : ''} ${span.spanName === dominantStage?.name ? 'dominant' : ''}`}
                    key={span.spanId}
                    style={{ left: `${left}%`, width: `${Math.max(2, Math.min(width, 100 - left))}%` }}
                    title={`${span.spanName} · ${formatDuration(span.durationMs)}`}
                  >
                    {span.spanName}
                  </span>
                );
              })}
            </div>
            <strong className={sandbox.startupDurationMs > 7000 ? 'hot-value' : ''}>{formatDuration(sandbox.startupDurationMs)}</strong>
          </div>
        ))}
      </div>
    </div>
  );
}

function buildContainerChangeSeries(sandboxes: Sandbox[]): MetricSeries[] {
  if (sandboxes.length === 0) return [];

  const sorted = [...sandboxes].sort((left, right) => toMs(left.createdAt) - toMs(right.createdAt));
  const first = toMs(sorted[0].createdAt);
  const last = Math.max(...sorted.map((sandbox) => toMs(sandbox.createdAt) + Math.max(sandbox.startupDurationMs, 1)));
  const points = Array.from({ length: 48 }, (_, index) => {
    const timestamp = first + (index / 47) * Math.max(last - first, 1);
    const running = sorted.filter((sandbox) => toMs(sandbox.createdAt) <= timestamp && (!sandbox.stoppedAt || toMs(sandbox.stoppedAt) >= timestamp) && sandbox.status !== 'failed').length;
    const starting = sorted.filter((sandbox) => {
      const created = toMs(sandbox.createdAt);
      return timestamp >= created && timestamp <= created + sandbox.startupDurationMs;
    }).length;
    const failed = sorted.filter((sandbox) => sandbox.status === 'failed' && toMs(sandbox.createdAt) <= timestamp).length;
    return { failed, running, starting, timestamp: new Date(timestamp).toISOString() };
  });

  const base = {
    group: 'startup' as const,
    nodeId: sorted[0].nodeId,
    unit: 'count',
  };

  return [
    { ...base, id: `${base.nodeId}-containers-running`, name: 'node.containers.running.timeline', label: 'Running containers', points: points.map((point) => ({ timestamp: point.timestamp, value: point.running })) },
    { ...base, id: `${base.nodeId}-containers-starting`, name: 'node.containers.starting.timeline', label: 'Starting containers', points: points.map((point) => ({ timestamp: point.timestamp, value: point.starting })) },
    { ...base, id: `${base.nodeId}-containers-failed`, name: 'node.containers.failed.timeline', label: 'Failed containers', points: points.map((point) => ({ timestamp: point.timestamp, value: point.failed })) },
  ];
}

function lifecycleOverlapsRange(sandbox: Sandbox, range: ContainerRange) {
  const created = toMs(sandbox.createdAt);
  const ended = sandbox.stoppedAt ? toMs(sandbox.stoppedAt) : created + Math.max(sandbox.startupDurationMs, 1);
  return created <= toMs(range.to) && ended >= toMs(range.from);
}

function spanOverlapsRange(span: TraceSpan, rangeStart: number, rangeEnd: number) {
  return toMs(span.startTime) <= rangeEnd && toMs(span.endTime) >= rangeStart;
}

function buildLifecycleStageBottlenecks(spans: TraceSpan[], rangeStart: number, rangeEnd: number) {
  const stageNames = Array.from(new Set(spans.map((span) => span.spanName)));

  return stageNames.map((name) => {
    const stageSpans = spans.filter((span) => span.spanName === name);
    const durations = stageSpans.map((span) => clippedSpanDuration(span, rangeStart, rangeEnd));
    const totalDurationMs = durations.reduce((sum, duration) => sum + duration, 0);
    const avgDurationMs = average(durations);
    const maxDurationMs = Math.max(...durations, 0);
    const p95DurationMs = percentile(durations, 0.95);
    const containerCount = new Set(stageSpans.map((span) => span.sandboxId)).size;
    const errorCount = stageSpans.filter((span) => span.status === 'error').length;
    return { avgDurationMs, containerCount, errorCount, maxDurationMs, name, p95DurationMs, spans: stageSpans, totalDurationMs };
  }).sort((left, right) => right.totalDurationMs - left.totalDurationMs);
}

function clippedSpanDuration(span: TraceSpan, rangeStart: number, rangeEnd: number) {
  const start = Math.max(toMs(span.startTime), rangeStart);
  const end = Math.min(toMs(span.endTime), rangeEnd);
  return Math.max(end - start, 0);
}

function ImageActivityCell({ image, sandboxes }: { image: Image; sandboxes: Sandbox[] }) {
  const activity = image.loadingMode === 'lazy'
    ? [
      imageBlockHitRatio(image),
      ratio(imageRemoteReadBytes(image), Math.max(image.sizeBytes, 1)),
      ratio(sandboxes.filter((sandbox) => sandbox.startupDurationMs > 7000).length, Math.max(sandboxes.length, 1)),
    ]
    : (image.downloadTimeline ?? []).map((step) => ratio(step.durationMs, Math.max(imageDownloadDuration(image), 1)));

  return (
    <div className="inline-activity">
      <SparkBars values={activity} />
      <span>{image.loadingMode === 'lazy' ? 'block cache / remote read trend' : 'download phase timeline'}</span>
    </div>
  );
}

function SandboxMetricSparklines({ metrics, names }: { metrics: MetricSeries[]; names: string[] }) {
  const selectedSeries = names.map((name) => metrics.find((series) => series.name === name)).filter((series): series is MetricSeries => Boolean(series));

  if (selectedSeries.length === 0) return <span className="inline-empty">No samples</span>;

  return (
    <div className="sparkline-stack">
      {selectedSeries.map((series) => (
        <div className="sparkline-row" key={series.id}>
          <Sparkline points={series.points.map((point) => point.value)} />
          <span>{series.label}</span>
        </div>
      ))}
    </div>
  );
}

function Sparkline({ points }: { points: number[] }) {
  const width = 112;
  const height = 28;
  const max = Math.max(...points, 1);
  const min = Math.min(...points, 0);
  const coords = points.map((point, index) => {
    const x = (index / Math.max(points.length - 1, 1)) * width;
    const y = height - ((point - min) / Math.max(max - min, 1)) * (height - 4) - 2;
    return `${x},${y}`;
  }).join(' ');

  return (
    <svg className="inline-sparkline" viewBox={`0 0 ${width} ${height}`} role="img" aria-label="metric trend">
      <polyline points={coords} />
    </svg>
  );
}

function SparkBars({ values }: { values: number[] }) {
  return (
    <div className="spark-bars" aria-label="activity timeline">
      {values.map((value, index) => <i key={index} style={{ height: `${Math.max(12, Math.min(value, 1) * 100)}%` }} />)}
    </div>
  );
}

function ImageModeBadge({ mode }: { mode: Image['loadingMode'] }) {
  return <span className={`image-mode-badge ${mode}`}>{mode === 'lazy' ? 'lazy-load' : 'non-lazy'}</span>;
}

function DetailModal({ title, children, onClose }: { title: string; children: ReactNode; onClose: () => void }) {
  return (
    <div className="detail-modal-backdrop" role="presentation" onClick={onClose}>
      <section className="detail-modal" role="dialog" aria-modal="true" aria-label={title} onClick={(event) => event.stopPropagation()}>
        <div className="detail-modal-header">
          <h3>{title}</h3>
          <button onClick={onClose}>Close</button>
        </div>
        {children}
      </section>
    </div>
  );
}

function ImageDetail({ image, sandboxes }: { image: Image; sandboxes: Sandbox[] }) {
  return (
    <div className="modal-stack">
      <div className="image-modal-summary">
        <NodeFact label="Image ref" value={image.ref} />
        <NodeFact label="Digest" value={image.digest} />
        <NodeFact label="Loading mode" value={image.loadingMode === 'lazy' ? 'lazy-load' : 'non-lazy'} />
        <NodeFact label="Size" value={formatBytes(image.sizeBytes)} />
        <NodeFact label="Layers" value={String(image.layerCount)} />
        <NodeFact label="Containers" value={String(sandboxes.length)} />
      </div>
      <ImageLayerPanel image={image} />
    </div>
  );
}

function ContainerDetail({
  image,
  metrics,
  onOpenSandbox,
  sandbox,
}: {
  image?: Image;
  metrics: MetricSeries[];
  onOpenSandbox: () => void;
  sandbox: Sandbox;
}) {
  const ioSeries = metrics.find((series) => series.name === 'sandbox.io.read_bytes');
  const cpuSeries = metrics.find((series) => series.name === 'sandbox.cpu.usage_ratio');
  const memorySeries = metrics.find((series) => series.name === 'sandbox.memory.working_set_bytes');
  const networkSeries = metrics.find((series) => series.name === 'sandbox.network.rx_bytes');
  const peakIo = Math.max(...(ioSeries?.points.map((point) => point.value) ?? [0]), 0);

  return (
    <div className="modal-stack">
      <div className="container-modal-header">
        <NodeFact label="Workload" value={sandbox.workloadName} />
        <NodeFact label="Namespace" value={sandbox.namespace} />
        <NodeFact label="Runtime" value={`${sandbox.runtimeType} / ${sandbox.runtimeVersion}`} />
        <NodeFact label="Image" value={image?.ref ?? sandbox.imageRef} />
      </div>
      <div className="modal-metric-grid">
        {cpuSeries && <MetricChart height={160} series={[cpuSeries]} />}
        {ioSeries && <MetricChart height={160} series={[ioSeries]} />}
        {memorySeries && <MetricChart height={160} series={[memorySeries]} />}
      </div>
      <div className="container-detail-grid">
        <SummaryCard label="Peak IO Read" value={formatBytes(peakIo)} caption="sandbox.io.read_bytes" tone={peakIo > 64 * 1024 ** 2 ? 'warning' : undefined} />
        <SummaryCard label="Avg Network RX" value={formatBytes(averageMetricValue(networkSeries))} caption="sandbox.network.rx_bytes" />
        <SummaryCard label="Memory Peak" value={formatBytes(sandbox.memoryPeakBytes)} caption="container working set" />
        <SummaryCard label="Created" value={formatDateTime(sandbox.createdAt)} caption="container metadata" />
      </div>
      {image && <ImageLayerPanel image={image} />}
      <div className="modal-actions">
        <button className="primary" onClick={onOpenSandbox}>Open sandbox detail</button>
      </div>
    </div>
  );
}

function NodePressureOverlay({ node, ioSeries, psiSeries }: { node: Node; ioSeries?: MetricSeries; psiSeries?: MetricSeries }) {
  const maxIo = Math.max(...(ioSeries?.points.map((point) => point.value) ?? [0]), 1);
  const maxPsi = Math.max(...(psiSeries?.points.map((point) => point.value) ?? [0]), 1);
  const avgPsi = averageMetricValue(psiSeries);
  const avgIo = averageMetricValue(ioSeries);
  const hotPressure = avgPsi > 0.25 || node.status === 'degraded';
  const points = psiSeries?.points ?? ioSeries?.points ?? [];

  return (
    <div className="panel-card node-pressure-panel">
      <div className="node-pressure-header">
        <div>
          <h3>Node IO pressure overlay</h3>
          <p>{node.name} · {node.cpuCores} cores · {formatBytes(node.memoryBytes)} memory · {node.kernelVersion}</p>
        </div>
        <span className={`pressure-badge ${hotPressure ? 'hot' : 'normal'}`}>{hotPressure ? 'pressure elevated' : 'pressure normal'}</span>
      </div>
      <div className="node-pressure-summary">
        <SummaryCard label="Node Status" value={node.status} caption="node condition" tone={node.status === 'degraded' ? 'warning' : undefined} />
        <SummaryCard label="Avg PSI IO" value={formatRatio(avgPsi)} caption="node.psi.io.some" tone={avgPsi > 0.25 ? 'warning' : undefined} />
        <SummaryCard label="Avg IO Read" value={formatBytes(avgIo)} caption="sandbox read throughput" tone={avgIo > 32 * 1024 ** 2 ? 'warning' : undefined} />
      </div>
      <div className="pressure-overlay-chart">
        {points.map((point, index) => {
          const ioPoint = ioSeries?.points[index];
          const psiPoint = psiSeries?.points[index];
          const psiHeight = Math.max(4, ((psiPoint?.value ?? 0) / maxPsi) * 100);
          const ioHeight = Math.max(4, ((ioPoint?.value ?? 0) / maxIo) * 100);

          return (
            <div className="pressure-sample" key={point.timestamp} title={`${point.timestamp} · PSI ${formatRatio(psiPoint?.value ?? 0)} · IO ${formatBytes(ioPoint?.value ?? 0)}`}>
              <i className="io" style={{ height: `${ioHeight}%` }} />
              <i className="psi" style={{ height: `${psiHeight}%` }} />
            </div>
          );
        })}
      </div>
      <div className="pressure-legend">
        <span><i className="io" />Aggregated sandbox IO read</span>
        <span><i className="psi" />node.psi.io.some</span>
      </div>
    </div>
  );
}

function ImageLayerPanel({ image }: { image: Image }) {
  if (image.loadingMode === 'eager') return <ImageDownloadTimeline image={image} />;

  const layers = image.layers ?? [];
  const totalDuration = layers.reduce((sum, layer) => sum + layer.pullDurationMs + layer.unpackDurationMs, 0);
  const blockHitRatio = imageBlockHitRatio(image);
  const remoteReadBytes = imageRemoteReadBytes(image);
  const largestLayer = layers.reduce((largest, layer) => (layer.sizeBytes > largest.sizeBytes ? layer : largest), layers[0]);
  const cacheSeries = lazyImageCacheSeries(image);

  return (
    <div className="panel-card image-layer-panel">
      <div className="image-layer-header">
        <div>
          <h3>Image layer breakdown</h3>
          <p>{image.ref} · {formatBytes(image.sizeBytes)} · {image.layerCount} layers</p>
        </div>
        <div className="image-cache-summary">
          <strong>{formatRatio(blockHitRatio)}</strong>
          <span>block cache hit</span>
        </div>
      </div>
      <div className="image-layer-summary">
        <SummaryCard label="Requested Blocks" value={String(imageRequestedBlocks(image))} caption="lazy-load block reads" />
        <SummaryCard label="Remote Read" value={formatBytes(remoteReadBytes)} caption="missed block bytes" tone={remoteReadBytes > 512 * 1024 ** 2 ? 'warning' : undefined} />
        <SummaryCard label="Pull+Unpack" value={formatDuration(totalDuration)} caption="estimated startup cost" tone={totalDuration > 7000 ? 'warning' : undefined} />
        <SummaryCard label="Largest Layer" value={largestLayer ? formatBytes(largestLayer.sizeBytes) : '-'} caption="static image layer" tone={(largestLayer?.sizeBytes ?? 0) > 1024 ** 3 ? 'warning' : undefined} />
      </div>
      <div className="image-temporal-grid">
        <MetricChart height={180} series={[cacheSeries.hitRatio]} />
        <MetricChart height={180} series={[cacheSeries.remoteRead]} />
      </div>
      <div className="image-layer-list">
        {layers.map((layer) => {
          return (
            <article className="image-layer-row" key={layer.id}>
              <div>
                <strong>{layer.command}</strong>
                <span>{layer.cacheHitBlockCount}/{layer.requestedBlockCount} blocks hit · remote {formatBytes(layer.remoteReadBytes)} · pull+unpack {formatDuration(layer.pullDurationMs + layer.unpackDurationMs)}</span>
              </div>
              <div className="image-layer-bars">
                <div className="image-layer-track"><i style={{ width: `${Math.max(6, layerBlockHitRatio(layer) * 100)}%` }} /></div>
              </div>
              <span className="cache-pill hit">{formatRatio(layerBlockHitRatio(layer))}</span>
            </article>
          );
        })}
      </div>
    </div>
  );
}

function ImageDownloadTimeline({ image }: { image: Image }) {
  const steps = image.downloadTimeline ?? [];
  const totalDuration = imageDownloadDuration(image);
  const maxDuration = Math.max(...steps.map((step) => step.durationMs), 1);
  const downloadSeries = eagerImageDownloadSeries(image);

  return (
    <div className="panel-card image-download-panel">
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
      <div className="image-layer-summary">
        <SummaryCard label="Total Download" value={formatDuration(totalDuration)} caption="resolve to snapshot ready" tone={totalDuration > 7000 ? 'warning' : undefined} />
        <SummaryCard label="Downloaded Bytes" value={formatBytes(image.sizeBytes)} caption="full image materialized" />
        <SummaryCard label="Layers" value={String(image.layerCount)} caption="full layer pull" />
      </div>
      <div className="image-temporal-grid single">
        <MetricChart height={180} series={[downloadSeries.duration]} />
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

function aggregateSeries(seriesList: MetricSeries[], mode: 'avg' | 'sum'): MetricSeries | undefined {
  const first = seriesList[0];
  if (!first) return undefined;

  return {
    ...first,
    id: `${first.nodeId ?? 'node'}-${first.name}-${mode}`,
    label: mode === 'sum' ? `Total ${first.label}` : `Average ${first.label}`,
    points: first.points.map((point, index) => {
      const values = seriesList.map((series) => series.points[index]?.value ?? 0);
      const value = mode === 'sum' ? values.reduce((sum, item) => sum + item, 0) : average(values);
      return { timestamp: point.timestamp, value };
    }),
  };
}

function timelineSeries(
  reference: MetricSeries,
  name: string,
  label: string,
  unit: string,
  group: MetricSeries['group'],
  targetValue: number,
  variation: number,
): MetricSeries {
  return {
    id: `${reference.nodeId ?? 'node'}-${name}`,
    name,
    label,
    unit,
    group,
    nodeId: reference.nodeId,
    points: reference.points.map((point, index) => {
      const ratio = index / Math.max(reference.points.length - 1, 1);
      const wave = Math.sin(ratio * Math.PI * 2) * targetValue * 0.08 * variation;
      const ramp = targetValue * Math.min(ratio * 1.4, 1);
      return {
        timestamp: point.timestamp,
        value: Math.max(0, ramp + wave),
      };
    }),
  };
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

function eagerImageDownloadSeries(image: Image): { duration: MetricSeries } {
  const steps = image.downloadTimeline ?? [];
  const start = Date.now() - Math.max(imageDownloadDuration(image), 1);
  let cursor = start;

  const points = steps.flatMap((step) => {
    const stepStart = cursor;
    const stepEnd = cursor + step.durationMs;
    cursor = stepEnd;
    return [
      { timestamp: new Date(stepStart).toISOString(), duration: step.durationMs },
      { timestamp: new Date(stepEnd).toISOString(), duration: step.durationMs },
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
  };
}

function averageMetricValue(series?: MetricSeries) {
  if (!series || series.points.length === 0) return 0;
  return average(series.points.map((point) => point.value));
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

function ratio(numerator: number, denominator: number) {
  return denominator === 0 ? 0 : numerator / denominator;
}

function average(values: number[]) {
  return values.reduce((sum, value) => sum + value, 0) / Math.max(values.length, 1);
}

function percentile(values: number[], percentileValue: number) {
  if (values.length === 0) return 0;
  const sorted = [...values].sort((left, right) => left - right);
  const index = Math.min(sorted.length - 1, Math.ceil(sorted.length * percentileValue) - 1);
  return sorted[index];
}
