const maxMetricPointsPerSeries = 120;
const maxRowsPerKind = 300;

export function createLiveStore() {
  return {
    clusters: new Map(),
    nodes: new Map(),
    images: new Map(),
    sandboxes: new Map(),
    metricsBySandbox: new Map(),
    eventsBySandbox: new Map(),
    tracesBySandbox: new Map(),
    profilesBySandbox: new Map(),
  };
}

export function recordLiveBatch(store, payload) {
  rememberMetadata(store, payload.metadata);

  for (const metric of array(payload.metrics)) rememberMetric(store, metric);
  for (const event of array(payload.events)) rememberRow(store.eventsBySandbox, event.sandboxId, event, rowLimit('events'));
  for (const span of array(payload.traces)) rememberRow(store.tracesBySandbox, span.sandboxId, span, rowLimit('traces'));
  for (const profile of array(payload.profiles)) rememberRow(store.profilesBySandbox, profile.sandboxId, profile, rowLimit('profiles'));

  for (const sandbox of store.sandboxes.values()) refreshSandboxDerivedFields(store, sandbox);
}

export function liveClusters(store) {
  return Array.from(store.clusters.values());
}

export function liveNodes(store) {
  return Array.from(store.nodes.values());
}

export function liveImages(store) {
  return Array.from(store.images.values());
}

export function liveSandboxes(store) {
  return Array.from(store.sandboxes.values());
}

export function liveMetricsForSandbox(store, sandboxId) {
  return Array.from(store.metricsBySandbox.get(sandboxId)?.values() ?? []);
}

export function liveEventsForSandbox(store, sandboxId) {
  return store.eventsBySandbox.get(sandboxId) ?? [];
}

export function liveTraceForSandbox(store, sandboxId) {
  return store.tracesBySandbox.get(sandboxId) ?? [];
}

export function liveProfilesForSandbox(store, sandboxId) {
  return store.profilesBySandbox.get(sandboxId) ?? [];
}

function rememberMetadata(store, metadata) {
  for (const cluster of array(metadata?.clusters)) {
    if (!cluster.id) continue;
    store.clusters.set(cluster.id, {
      id: cluster.id,
      name: cluster.name ?? cluster.id,
      environment: cluster.environment ?? 'collector',
    });
  }

  for (const node of array(metadata?.nodes)) {
    if (!node.id) continue;
    store.nodes.set(node.id, normalizeNode(node));
  }

  for (const image of array(metadata?.images)) {
    const normalized = normalizeImage(image);
    if (normalized) store.images.set(normalized.id, normalized);
  }

  for (const sandbox of array(metadata?.sandboxes)) {
    const normalized = normalizeSandbox(sandbox, store);
    if (normalized) store.sandboxes.set(normalized.id, normalized);
  }
}

function normalizeNode(node) {
  return {
    id: node.id,
    name: node.name ?? node.id,
    clusterId: node.clusterId ?? 'cluster-prod',
    kernelVersion: node.kernelVersion ?? 'collector-observed',
    cpuCores: numberOr(node.cpuCores, 0),
    memoryBytes: numberOr(node.memoryBytes, 0),
    status: node.status ?? 'unknown',
  };
}

function normalizeImage(image) {
  if (!image?.id && !image?.ref) return undefined;
  const ref = image.ref ?? image.id;
  return {
    id: image.id ?? imageIdFromRef(ref),
    ref,
    digest: image.digest ?? `collector:${imageIdFromRef(ref)}`,
    loadingMode: image.loadingMode ?? 'eager',
    sizeBytes: numberOr(image.sizeBytes, 0),
    layerCount: numberOr(image.layerCount, 0),
    layers: Array.isArray(image.layers) ? image.layers : undefined,
    downloadTimeline: Array.isArray(image.downloadTimeline) ? image.downloadTimeline : undefined,
  };
}

function normalizeSandbox(sandbox, store) {
  if (!sandbox?.id) return undefined;
  const imageRef = sandbox.imageRef ?? sandbox.imageId ?? 'collector/unknown:latest';
  const imageId = sandbox.imageId ?? imageIdFromRef(imageRef);
  const createdAt = sandbox.createdAt ?? sandbox.observedAt ?? new Date().toISOString();

  if (!store.images.has(imageId)) {
    store.images.set(imageId, normalizeImage({
      id: imageId,
      ref: imageRef,
      digest: `collector:${imageId}`,
      loadingMode: 'eager',
    }));
  }

  return {
    id: sandbox.id,
    clusterId: sandbox.clusterId ?? store.nodes.get(sandbox.nodeId)?.clusterId ?? 'cluster-prod',
    nodeId: sandbox.nodeId,
    namespace: sandbox.namespace ?? 'collector',
    workloadId: sandbox.workloadId ?? sandbox.workloadName ?? sandbox.id,
    workloadName: sandbox.workloadName ?? sandbox.id,
    imageId,
    imageRef,
    runtimeType: sandbox.runtimeType,
    runtimeVersion: sandbox.runtimeVersion ?? sandbox.runtimeType,
    status: sandbox.status ?? 'running',
    createdAt,
    startedAt: sandbox.startedAt,
    stoppedAt: sandbox.stoppedAt,
    startupDurationMs: numberOr(sandbox.startupDurationMs, 0),
    cpuAvg: numberOr(sandbox.cpuAvg, 0),
    memoryPeakBytes: numberOr(sandbox.memoryPeakBytes, 0),
    eventCount: numberOr(sandbox.eventCount, 0),
    labels: isObject(sandbox.labels) ? sandbox.labels : { source: 'collector' },
    attributes: isObject(sandbox.attributes) ? sandbox.attributes : {},
  };
}

function rememberMetric(store, metric) {
  const sandboxId = metric.sandboxId;
  if (!sandboxId) return;

  const seriesMap = ensureSeriesMap(store.metricsBySandbox, sandboxId);
  const id = `${sandboxId}-${metric.name}`;
  const existing = seriesMap.get(id);
  const point = {
    timestamp: metric.timestamp,
    value: metric.value,
  };

  if (!existing) {
    seriesMap.set(id, {
      id,
      name: metric.name,
      label: metricLabel(metric.name),
      unit: metric.unit ?? '',
      group: metric.group ?? 'runtime',
      sandboxId,
      nodeId: metric.nodeId,
      runtimeType: metric.runtimeType,
      points: [point],
    });
    refreshSandboxFromMetric(store, metric);
    return;
  }

  const points = [...existing.points.filter((item) => item.timestamp !== point.timestamp), point]
    .sort((left, right) => Date.parse(left.timestamp) - Date.parse(right.timestamp))
    .slice(-maxMetricPointsPerSeries);

  seriesMap.set(id, {
    ...existing,
    unit: metric.unit ?? existing.unit,
    group: metric.group ?? existing.group,
    nodeId: metric.nodeId ?? existing.nodeId,
    runtimeType: metric.runtimeType ?? existing.runtimeType,
    points,
  });
  refreshSandboxFromMetric(store, metric);
}

function refreshSandboxFromMetric(store, metric) {
  if (!metric.sandboxId) return;
  const sandbox = store.sandboxes.get(metric.sandboxId);
  if (!sandbox) return;

  if (metric.name === 'sandbox.startup.duration_ms') {
    sandbox.startupDurationMs = metric.value;
    sandbox.startedAt = new Date(Date.parse(sandbox.createdAt) + metric.value).toISOString();
  }
  if (metric.name === 'sandbox.cpu.usage_ratio') sandbox.cpuAvg = metric.value;
  if (metric.name === 'sandbox.memory.working_set_bytes') {
    sandbox.memoryPeakBytes = Math.max(sandbox.memoryPeakBytes, metric.value);
  }
}

function refreshSandboxDerivedFields(store, sandbox) {
  sandbox.eventCount = (store.eventsBySandbox.get(sandbox.id) ?? []).length;
}

function rememberRow(collection, key, row, limit) {
  if (!key) return;
  const rows = collection.get(key) ?? [];
  rows.push(row);
  rows.sort((left, right) => Date.parse(rowTime(left)) - Date.parse(rowTime(right)));
  collection.set(key, dedupeRows(rows).slice(-limit));
}

function ensureSeriesMap(collection, sandboxId) {
  const existing = collection.get(sandboxId);
  if (existing) return existing;
  const next = new Map();
  collection.set(sandboxId, next);
  return next;
}

function dedupeRows(rows) {
  const seen = new Set();
  return rows.filter((row) => {
    const key = row.id ?? `${row.traceId ?? ''}:${row.spanId ?? ''}:${row.timestamp ?? row.startTime ?? ''}`;
    if (seen.has(key)) return false;
    seen.add(key);
    return true;
  });
}

function rowTime(row) {
  return row.timestamp ?? row.startTime ?? row.acceptedAt ?? new Date(0).toISOString();
}

function rowLimit(kind) {
  return kind === 'traces' ? maxRowsPerKind * 2 : maxRowsPerKind;
}

function metricLabel(name) {
  return name
    .split('.')
    .map((part) => part.replace(/_/g, ' '))
    .map((part) => part.charAt(0).toUpperCase() + part.slice(1))
    .join(' ');
}

function imageIdFromRef(ref) {
  return `collector-${String(ref).replace(/[^a-z0-9]+/gi, '-').replace(/^-|-$/g, '').toLowerCase() || 'image'}`;
}

function numberOr(value, fallback) {
  return typeof value === 'number' && Number.isFinite(value) ? value : fallback;
}

function array(value) {
  return Array.isArray(value) ? value : [];
}

function isObject(value) {
  return Boolean(value) && typeof value === 'object' && !Array.isArray(value);
}
