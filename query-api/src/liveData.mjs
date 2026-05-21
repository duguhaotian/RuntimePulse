const maxMetricPointsPerSeries = 120;
const maxRowsPerKind = 300;

export function createLiveStore() {
  return {
    clusters: new Map(),
    nodes: new Map(),
    images: new Map(),
    sandboxes: new Map(),
    sandboxHistory: new Map(),
    metricsBySandbox: new Map(),
    metricsByNode: new Map(),
    metricsByImage: new Map(),
    eventsBySandbox: new Map(),
    eventsByNode: new Map(),
    eventsByImage: new Map(),
    tracesBySandbox: new Map(),
    tracesByImage: new Map(),
    profilesBySandbox: new Map(),
    sourceByNode: new Map(),
    sourceByImage: new Map(),
    sourceBySandbox: new Map(),
    snapshotScopeBySandbox: new Map(),
    lastUpdatedAt: undefined,
  };
}

export function recordLiveBatch(store, payload) {
  rememberMetadata(store, payload.metadata, payload.source);
  reconcileSnapshotEvents(store, payload.events);

  for (const metric of array(payload.metrics)) rememberMetric(store, metric);
  for (const event of array(payload.events)) rememberEvent(store, event);
  for (const span of array(payload.traces)) rememberTraceSpan(store, span);
  for (const profile of array(payload.profiles)) rememberRow(store.profilesBySandbox, profile.sandboxId, profile, rowLimit('profiles'));

  reconcileSnapshotEvents(store, payload.events);
  for (const sandbox of store.sandboxes.values()) refreshSandboxDerivedFields(store, sandbox);
  store.lastUpdatedAt = new Date().toISOString();
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

export function liveSandboxHistory(store) {
  return Array.from(store.sandboxHistory.values());
}

export function liveMetricsForSandbox(store, sandboxId) {
  return Array.from(store.metricsBySandbox.get(sandboxId)?.values() ?? []);
}

export function liveMetricsForNode(store, nodeId) {
  return Array.from(store.metricsByNode.get(nodeId)?.values() ?? []);
}

export function liveMetricsForImage(store, imageId) {
  return Array.from(store.metricsByImage.get(imageId)?.values() ?? []);
}

export function liveEventsForSandbox(store, sandboxId) {
  return store.eventsBySandbox.get(sandboxId) ?? [];
}

export function liveEventsForNode(store, nodeId) {
  return store.eventsByNode.get(nodeId) ?? [];
}

export function liveEventsForImage(store, imageId) {
  return store.eventsByImage.get(imageId) ?? [];
}

export function liveTraceForSandbox(store, sandboxId) {
  return store.tracesBySandbox.get(sandboxId) ?? [];
}

export function liveTraceForImage(store, imageId) {
  return store.tracesByImage.get(imageId) ?? [];
}

export function liveProfilesForSandbox(store, sandboxId) {
  return store.profilesBySandbox.get(sandboxId) ?? [];
}

export function liveStoreSnapshot(store) {
  const metricCollections = [...store.metricsBySandbox.values(), ...store.metricsByNode.values(), ...store.metricsByImage.values()];
  const metricSeries = metricCollections.reduce((sum, seriesMap) => sum + seriesMap.size, 0);
  const metricPoints = metricCollections.reduce((sum, seriesMap) => {
    return sum + Array.from(seriesMap.values()).reduce((seriesSum, series) => seriesSum + series.points.length, 0);
  }, 0);

  return {
    clusters: store.clusters.size,
    nodes: store.nodes.size,
    images: store.images.size,
    sandboxes: store.sandboxes.size,
    metricSeries,
    metricPoints,
    events: uniqueRowCount([store.eventsBySandbox, store.eventsByNode, store.eventsByImage]),
    traces: uniqueRowCount([store.tracesBySandbox, store.tracesByImage]),
    profiles: rowCount(store.profilesBySandbox),
    lastUpdatedAt: store.lastUpdatedAt,
    sources: liveSourceSnapshots(store),
    limits: {
      metricPointsPerSeries: maxMetricPointsPerSeries,
      rowsPerKind: maxRowsPerKind,
    },
  };
}

function rememberMetadata(store, metadata, source) {
  for (const cluster of array(metadata?.clusters)) {
    if (!cluster.id) continue;
    store.clusters.set(cluster.id, normalizeCluster(cluster));
  }

  for (const node of array(metadata?.nodes)) {
    if (!node.id) continue;
    const normalized = normalizeNode(node);
    store.nodes.set(normalized.id, mergeNode(store.nodes.get(normalized.id), normalized));
    if (source) store.sourceByNode.set(node.id, source);
  }

  for (const image of array(metadata?.images)) {
    const normalized = normalizeImage(image);
    if (normalized) {
      store.images.set(normalized.id, mergeImage(store.images.get(normalized.id), normalized));
      if (source) store.sourceByImage.set(normalized.id, source);
    }
  }

  for (const sandbox of array(metadata?.sandboxes)) {
    const normalized = normalizeSandbox(sandbox, store);
    if (normalized) {
      if (isRemovedSandbox(normalized)) {
        rememberSandboxHistory(store, normalized, source);
        removeLiveSandbox(store, normalized.id);
      } else {
        const merged = mergeSandbox(store.sandboxes.get(normalized.id), normalized);
        store.sandboxes.set(normalized.id, merged);
        rememberSandboxHistory(store, merged, source);
      }
      if (source) store.sourceBySandbox.set(normalized.id, source);
      if (stringValue(normalized.attributes?.['snapshot.scope'])) {
        store.snapshotScopeBySandbox.set(normalized.id, normalized.attributes['snapshot.scope']);
      }
    }
  }
}

function normalizeCluster(cluster) {
  const id = cluster.id;
  const name = cluster.name ?? id;
  const localClusterNames = new Set(['cluster-prod', 'runtimepulse-prod', 'runtimepulse-local']);

  if (localClusterNames.has(id) || localClusterNames.has(name)) {
    return {
      id,
      name: 'Local observed cluster',
      environment: 'single-node collector group',
    };
  }

  return {
    id,
    name,
    environment: cluster.environment ?? 'collector',
  };
}

function liveSourceSnapshots(store) {
  const bySource = new Map();

  for (const [sandboxId, source] of store.sourceBySandbox.entries()) {
    const current = bySource.get(source) ?? {
      source,
      sandboxes: 0,
      metricSeries: 0,
      metricPoints: 0,
      events: 0,
      traces: 0,
      profiles: 0,
    };
    const series = store.metricsBySandbox.get(sandboxId);

    current.sandboxes += store.sandboxes.has(sandboxId) ? 1 : 0;
    current.metricSeries += series?.size ?? 0;
    current.metricPoints += series ? Array.from(series.values()).reduce((sum, item) => sum + item.points.length, 0) : 0;
    current.traces += store.tracesBySandbox.get(sandboxId)?.length ?? 0;
    current.profiles += store.profilesBySandbox.get(sandboxId)?.length ?? 0;
    bySource.set(source, current);
  }

  for (const [nodeId, source] of store.sourceByNode.entries()) {
    const current = bySource.get(source) ?? {
      source,
      sandboxes: 0,
      metricSeries: 0,
      metricPoints: 0,
      events: 0,
      traces: 0,
      profiles: 0,
    };
    const series = store.metricsByNode.get(nodeId);
    current.metricSeries += series?.size ?? 0;
    current.metricPoints += series ? Array.from(series.values()).reduce((sum, item) => sum + item.points.length, 0) : 0;
    bySource.set(source, current);
  }

  for (const [imageId, source] of store.sourceByImage.entries()) {
    const current = bySource.get(source) ?? {
      source,
      sandboxes: 0,
      metricSeries: 0,
      metricPoints: 0,
      events: 0,
      traces: 0,
      profiles: 0,
    };
    const series = store.metricsByImage.get(imageId);
    current.metricSeries += series?.size ?? 0;
    current.metricPoints += series ? Array.from(series.values()).reduce((sum, item) => sum + item.points.length, 0) : 0;
    current.traces += store.tracesByImage.get(imageId)?.length ?? 0;
    bySource.set(source, current);
  }

  addEventCountsBySource(bySource, store.eventsBySandbox, store.sourceBySandbox);
  addEventCountsBySource(bySource, store.eventsByNode, store.sourceByNode);
  addEventCountsBySource(bySource, store.eventsByImage, store.sourceByImage);

  return Array.from(bySource.values())
    .map(({ eventIds: _eventIds, ...snapshot }) => snapshot)
    .sort((left, right) => right.metricPoints - left.metricPoints);
}

function addEventCountsBySource(bySource, eventsByKey, sourceByKey) {
  for (const [key, rows] of eventsByKey.entries()) {
    const source = sourceByKey.get(key);
    if (!source) continue;
    const current = bySource.get(source) ?? {
      source,
      sandboxes: 0,
      metricSeries: 0,
      metricPoints: 0,
      events: 0,
      traces: 0,
      profiles: 0,
    };
    current.eventIds ??= new Set();
    for (const row of rows) current.eventIds.add(rowKey(row));
    current.events = current.eventIds.size;
    bySource.set(source, current);
  }
}

function normalizeNode(node) {
  return {
    id: node.id,
    name: node.name ?? node.id,
    clusterId: node.clusterId ?? 'runtimepulse-local',
    kernelVersion: node.kernelVersion ?? 'collector-observed',
    cpuCores: numberOr(node.cpuCores, 0),
    memoryBytes: numberOr(node.memoryBytes, 0),
    status: node.status ?? 'unknown',
  };
}

function mergeNode(existing, incoming) {
  if (!existing) return incoming;

  return {
    ...existing,
    ...incoming,
    name: incoming.name || existing.name,
    clusterId: incoming.clusterId || existing.clusterId,
    kernelVersion: meaningfulNodeString(incoming.kernelVersion) ? incoming.kernelVersion : existing.kernelVersion,
    cpuCores: incoming.cpuCores > 0 ? incoming.cpuCores : existing.cpuCores,
    memoryBytes: incoming.memoryBytes > 0 ? incoming.memoryBytes : existing.memoryBytes,
    status: incoming.status ?? existing.status,
  };
}

function meaningfulNodeString(value) {
  return Boolean(value) && !String(value).endsWith('-observed') && value !== 'collector-observed';
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
    attributes: isObject(image.attributes) ? image.attributes : {},
  };
}

function mergeImage(existing, incoming) {
  if (!existing) return incoming;
  return {
    ...existing,
    ...incoming,
    ref: meaningfulImageRef(incoming.ref) ? incoming.ref : existing.ref,
    digest: meaningfulImageDigest(incoming.digest) ? incoming.digest : existing.digest,
    loadingMode: incoming.loadingMode ?? existing.loadingMode,
    sizeBytes: incoming.sizeBytes || existing.sizeBytes,
    layerCount: incoming.layerCount || existing.layerCount,
    layers: incoming.layers && incoming.layers.length > 0 ? incoming.layers : existing.layers,
    downloadTimeline: mergeTimeline(existing.downloadTimeline, incoming.downloadTimeline),
    attributes: { ...(existing.attributes ?? {}), ...(incoming.attributes ?? {}) },
  };
}

function meaningfulImageRef(value) {
  return Boolean(value) && value !== 'collector/unknown:latest' && value !== 'containerd/unknown:latest' && value !== 'docker/unknown:latest';
}

function meaningfulImageDigest(value) {
  return Boolean(value) && !String(value).startsWith('collector:') && value !== 'containerd:unknown';
}

function mergeTimeline(existing, incoming) {
  if (!Array.isArray(existing)) return incoming;
  if (!Array.isArray(incoming)) return existing;

  const byId = new Map();
  for (const step of existing) byId.set(step.id ?? `${step.phase}:${step.name}`, step);
  for (const step of incoming) byId.set(step.id ?? `${step.phase}:${step.name}`, { ...byId.get(step.id), ...step });

  return Array.from(byId.values()).sort((left, right) => {
    const leftTime = Date.parse(left.timestamp ?? '') || 0;
    const rightTime = Date.parse(right.timestamp ?? '') || 0;
    return leftTime - rightTime;
  });
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
    clusterId: sandbox.clusterId ?? store.nodes.get(sandbox.nodeId)?.clusterId ?? 'runtimepulse-local',
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
    removedAt: sandbox.removedAt,
    startupDurationMs: numberOr(sandbox.startupDurationMs, 0),
    cpuAvg: numberOr(sandbox.cpuAvg, 0),
    memoryPeakBytes: numberOr(sandbox.memoryPeakBytes, 0),
    eventCount: numberOr(sandbox.eventCount, 0),
    labels: isObject(sandbox.labels) ? sandbox.labels : { source: 'collector' },
    attributes: isObject(sandbox.attributes) ? sandbox.attributes : {},
  };
}

function mergeSandbox(existing, incoming) {
  if (!existing) return incoming;
  const lifecycleAction = incoming.attributes?.['lifecycle.action'];
  const keepTraceStartup = startupDurationSource(existing) === 'trace' && startupDurationSource(incoming) !== 'trace';

  return {
    ...existing,
    ...incoming,
    createdAt: keepTraceStartup ? existing.createdAt : lifecycleAction ? existing.createdAt : incoming.createdAt ?? existing.createdAt,
    startedAt: keepTraceStartup ? existing.startedAt : incoming.startedAt ?? existing.startedAt,
    stoppedAt: incoming.stoppedAt ?? existing.stoppedAt,
    removedAt: incoming.removedAt ?? existing.removedAt,
    startupDurationMs: keepTraceStartup ? existing.startupDurationMs : incoming.startupDurationMs || existing.startupDurationMs,
    cpuAvg: incoming.cpuAvg || existing.cpuAvg,
    memoryPeakBytes: Math.max(incoming.memoryPeakBytes, existing.memoryPeakBytes),
    eventCount: Math.max(incoming.eventCount, existing.eventCount),
    labels: { ...existing.labels, ...incoming.labels },
    attributes: { ...existing.attributes, ...incoming.attributes },
  };
}

function startupDurationSource(sandbox) {
  return stringValue(sandbox?.attributes?.['startup.duration.source']);
}

function isRemovedSandbox(sandbox) {
  return sandbox.attributes?.['lifecycle.removed'] === true
    || sandbox.attributes?.['lifecycle.action'] === 'destroy'
    || sandbox.removedAt !== undefined;
}

function rememberSandboxHistory(store, sandbox, source) {
  const existing = store.sandboxHistory.get(sandbox.id);
  const merged = mergeSandbox(existing, sandbox);
  store.sandboxHistory.set(sandbox.id, merged);
  if (source) store.sourceBySandbox.set(sandbox.id, source);
}

function removeLiveSandbox(store, sandboxId) {
  store.sandboxes.delete(sandboxId);
}

function reconcileSnapshotEvents(store, events) {
  for (const event of array(events)) {
    const attributes = isObject(event.attributes) ? event.attributes : {};
    const scope = stringValue(attributes['snapshot.scope']);
    const nodeId = stringValue(attributes['snapshot.nodeId']) ?? stringValue(event.nodeId);
    const sandboxIds = stringSet(attributes['snapshot.sandboxIds']);

    if (!scope || !nodeId) continue;

    const imageIds = stringSet(attributes['snapshot.imageIds']);
    if (imageIds) {
      reconcileImageSnapshot(store, scope, nodeId, imageIds);
      markImageSnapshotMembership(store, scope, nodeId, imageIds);
      continue;
    }

    if (!sandboxIds) continue;

    for (const sandbox of Array.from(store.sandboxes.values())) {
      if (sandbox.nodeId !== nodeId) continue;
      if (!sandboxMatchesSnapshotScope(sandbox, scope)) continue;
      if (!sandboxIds.has(sandbox.id)) removeLiveSandbox(store, sandbox.id);
    }
  }
}

function sandboxMatchesSnapshotScope(sandbox, scope) {
  if (stringValue(sandbox.attributes?.['snapshot.scope']) === scope) return true;
  if (scope !== 'docker-running') return false;

  return sandbox.id.startsWith('docker-') || Boolean(stringValue(sandbox.attributes?.['docker.id']));
}

function reconcileImageSnapshot(store, scope, nodeId, imageIds) {
  for (const image of Array.from(store.images.values())) {
    if (image.attributes?.['snapshot.scope'] !== scope) continue;
    if (image.attributes?.['snapshot.nodeId'] !== nodeId) continue;
    if (!imageIds.has(image.id)) removeLiveImage(store, image.id);
  }
}

function markImageSnapshotMembership(store, scope, nodeId, imageIds) {
  for (const imageId of imageIds) {
    const image = store.images.get(imageId);
    if (!image) continue;
    store.images.set(imageId, {
      ...image,
      attributes: {
        ...(image.attributes ?? {}),
        'snapshot.scope': scope,
        'snapshot.nodeId': nodeId,
      },
    });
  }
}

function removeLiveImage(store, imageId) {
  store.images.delete(imageId);
}

function rememberTraceSpan(store, span) {
  rememberRow(store.tracesBySandbox, span.sandboxId, span, rowLimit('traces'));
  const imageId = stringValue(span.imageId)
    ?? stringValue(span.attributes?.['image.id'])
    ?? stringValue(span.attributes?.imageId);
  rememberRow(store.tracesByImage, imageId, { ...span, imageId }, rowLimit('traces'));
  refreshSandboxFromTraceSpan(store, span);
}

function rememberEvent(store, event) {
  rememberRow(store.eventsBySandbox, event.sandboxId, event, rowLimit('events'));

  const nodeId = stringValue(event.nodeId)
    ?? stringValue(event.attributes?.['snapshot.nodeId'])
    ?? stringValue(event.attributes?.nodeId);
  rememberRow(store.eventsByNode, nodeId, { ...event, nodeId }, rowLimit('events'));

  const imageId = stringValue(event.imageId)
    ?? stringValue(event.attributes?.['image.id'])
    ?? stringValue(event.attributes?.imageId);
  if (imageId && imageRemovalEvent(event)) removeLiveImage(store, imageId);
  rememberRow(store.eventsByImage, imageId, { ...event, imageId }, rowLimit('events'));
}

function imageRemovalEvent(event) {
  const action = stringValue(event.attributes?.dockerAction)
    ?? stringValue(event.attributes?.['docker.action'])
    ?? String(event.eventName ?? '').split('.').at(-1);
  return event.eventType === 'image' && ['delete', 'untag', 'remove', 'content_delete', 'snapshot_remove'].includes(action);
}

function rememberMetric(store, metric) {
  const sandboxId = metric.sandboxId;
  const nodeId = metric.nodeId;
  const imageId = metric.imageId;
  if (!sandboxId && !nodeId && !imageId) return;

  const scope = metricScope(metric);
  const scopeId = scope === 'sandbox' ? sandboxId : scope === 'image' ? imageId : nodeId;
  if (!scopeId) return;
  const collection = scope === 'sandbox'
    ? store.metricsBySandbox
    : scope === 'image'
      ? store.metricsByImage
      : store.metricsByNode;
  const seriesMap = ensureSeriesMap(collection, scopeId);
  const id = metricSeriesId(scope, scopeId, metric);
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
      imageId,
      nodeId,
      runtimeType: metric.runtimeType,
      attributes: isObject(metric.attributes) ? metric.attributes : {},
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
    nodeId: nodeId ?? existing.nodeId,
    imageId: imageId ?? existing.imageId,
    runtimeType: metric.runtimeType ?? existing.runtimeType,
    attributes: isObject(metric.attributes) ? { ...(existing.attributes ?? {}), ...metric.attributes } : existing.attributes,
    points,
  });
  refreshSandboxFromMetric(store, metric);
}

function metricScope(metric) {
  if (metric.sandboxId) return 'sandbox';
  if (metric.imageId && String(metric.name ?? '').startsWith('image.')) return 'image';
  if (metric.nodeId) return 'node';
  return 'image';
}

function metricSeriesId(scope, scopeId, metric) {
  const parts = [scope, scopeId, metric.name];
  if (scope === 'image') {
    const nodeId = stringValue(metric.nodeId);
    if (nodeId) parts.push('node', sanitizeMetricIdPart(nodeId));
  }
  const source = stringValue(metric.attributes?.['collector.source']);
  if (source) parts.push('source', sanitizeMetricIdPart(source));
  const eventStream = stringValue(metric.attributes?.['collector.event_stream']);
  if (eventStream) parts.push('event-stream', sanitizeMetricIdPart(eventStream));
  return parts.join('-');
}

function sanitizeMetricIdPart(value) {
  return String(value)
    .trim()
    .replace(/[^a-zA-Z0-9_.:-]+/g, '-')
    .replace(/^-+|-+$/g, '') || 'unknown';
}

function refreshSandboxFromMetric(store, metric) {
  if (!metric.sandboxId) return;
  const sandbox = store.sandboxes.get(metric.sandboxId);
  const historicalSandbox = store.sandboxHistory.get(metric.sandboxId);
  if (!sandbox) return;

  if (metric.name === 'sandbox.startup.duration_ms') {
    applySandboxStartupDuration(sandbox, metric.value, undefined, 'metric');
    if (historicalSandbox) applySandboxStartupDuration(historicalSandbox, metric.value, undefined, 'metric');
  }
  if (metric.name === 'sandbox.cpu.usage_ratio') sandbox.cpuAvg = metric.value;
  if (metric.name === 'sandbox.memory.working_set_bytes') {
    sandbox.memoryPeakBytes = Math.max(sandbox.memoryPeakBytes, metric.value);
  }
}

function refreshSandboxFromTraceSpan(store, span) {
  const sandboxId = stringValue(span?.sandboxId);
  if (!sandboxId || !startupTraceSpan(span)) return;

  const durationMs = numberOr(span.durationMs, 0);
  if (durationMs <= 0) return;

  const sandbox = store.sandboxes.get(sandboxId);
  const historicalSandbox = store.sandboxHistory.get(sandboxId);
  if (sandbox) applySandboxStartupDuration(sandbox, durationMs, span, 'trace');
  if (historicalSandbox) applySandboxStartupDuration(historicalSandbox, durationMs, span, 'trace');
}

function startupTraceSpan(span) {
  return ['container.startup', 'sandbox.startup'].includes(String(span?.spanName ?? ''));
}

function applySandboxStartupDuration(sandbox, durationMs, span, source = 'metric') {
  const plugin = stringValue(span?.attributes?.plugin);
  const existingPriority = startupDurationPriority(
    startupDurationSource(sandbox),
    stringValue(sandbox?.attributes?.['startup.duration.plugin']),
  );
  const incomingPriority = startupDurationPriority(source, plugin);
  if (existingPriority > incomingPriority) return;

  sandbox.startupDurationMs = durationMs;
  sandbox.attributes = {
    ...(sandbox.attributes ?? {}),
    'startup.duration.source': source,
    ...(plugin ? { 'startup.duration.plugin': plugin } : {}),
  };
  if (span?.startTime) sandbox.createdAt = span.startTime;
  if (span?.endTime) {
    sandbox.startedAt = span.endTime;
    return;
  }

  const createdAt = Date.parse(sandbox.createdAt ?? '');
  if (Number.isFinite(createdAt)) sandbox.startedAt = new Date(createdAt + durationMs).toISOString();
}

function startupDurationPriority(source, plugin) {
  if (source === 'trace' && runtimeStartupTracePlugin(plugin)) return 3;
  if (source === 'trace') return 2;
  if (source === 'metric') return 1;
  return 0;
}

function runtimeStartupTracePlugin(plugin) {
  return ['docker-startup-trace', 'containerd-startup-trace'].includes(plugin);
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

function stringValue(value) {
  return typeof value === 'string' && value.trim() !== '' ? value : undefined;
}

function stringSet(value) {
  if (!Array.isArray(value)) return undefined;
  return new Set(value.filter((item) => typeof item === 'string' && item.trim() !== ''));
}

function rowCount(collection) {
  return Array.from(collection.values()).reduce((sum, rows) => sum + rows.length, 0);
}

function uniqueRowCount(collections) {
  const keys = new Set();
  for (const collection of collections) {
    for (const rows of collection.values()) {
      for (const row of rows) keys.add(rowKey(row));
    }
  }
  return keys.size;
}

function rowKey(row) {
  return row.id ?? `${row.timestamp ?? row.startTime ?? ''}:${row.eventName ?? row.name ?? ''}`;
}
