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
    sourceByMetricSeries: new Map(),
    sourceByEvent: new Map(),
    sourceByProfile: new Map(),
    diagnosticArtifactsByScope: new Map(),
    sourceByDiagnosticArtifact: new Map(),
    snapshotScopeBySandbox: new Map(),
    derivedEventState: new Map(),
    lastUpdatedAt: undefined,
  };
}

export function recordLiveBatch(store, payload) {
  rememberMetadata(store, payload.metadata, payload.source);
  reconcileSnapshotEvents(store, payload.events);

  for (const metric of array(payload.metrics)) rememberMetric(store, metric, payload.source);
  for (const event of array(payload.events)) rememberEvent(store, event, payload.source);
  for (const span of array(payload.traces)) rememberTraceSpan(store, span);
  for (const profile of array(payload.profiles)) rememberProfile(store, profile, payload.source);

  rememberDiagnosticArtifacts(store, payload.events, payload.source);

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
  return Array.from(store.images.values()).filter((image) => !pseudoImage(image));
}

export function liveSandboxes(store) {
  return Array.from(store.sandboxes.values());
}

export function liveSandboxHistory(store) {
  return Array.from(store.sandboxHistory.values());
}

export function liveMetricsForSandbox(store, sandboxId) {
  const ownSeries = Array.from(store.metricsBySandbox.get(sandboxId)?.values() ?? []);
  const sandbox = store.sandboxes.get(sandboxId) ?? store.sandboxHistory.get(sandboxId);
  const podNetworkSeries = podNetworkMetricsForSandbox(store, sandbox, sandboxId);
  if (podNetworkSeries.length === 0) return ownSeries;

  const byId = new Map(ownSeries.map((series) => [series.id, series]));
  for (const series of podNetworkSeries) {
    byId.set(`${series.id}-attached-to-${sanitizeMetricIdPart(sandboxId)}`, {
      ...series,
      id: `${series.id}-attached-to-${sanitizeMetricIdPart(sandboxId)}`,
      sandboxId,
      attributes: {
        ...(series.attributes ?? {}),
        'metrics.attachedFromSandboxId': series.sandboxId,
        'metrics.attachedReason': 'same-k8s-pod-network',
      },
    });
  }
  return Array.from(byId.values());
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
  const byId = new Map();
  for (const event of store.eventsByNode.get(nodeId) ?? []) {
    byId.set(rowKey(event), event);
  }

  for (const [sandboxId, events] of store.eventsBySandbox.entries()) {
    const sandboxNodeId = nodeIdForSandbox(store, sandboxId);
    if (sandboxNodeId !== nodeId) continue;
    for (const event of events) {
      byId.set(rowKey(event), {
        ...event,
        nodeId: stringValue(event.nodeId) ?? sandboxNodeId,
      });
    }
  }

  return Array.from(byId.values())
    .sort((left, right) => Date.parse(rowTime(left)) - Date.parse(rowTime(right)))
    .slice(-rowLimit('events'));
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

export function liveDiagnosticArtifacts(store) {
  return Array.from(store.diagnosticArtifactsByScope.values())
    .flat()
    .sort((left, right) => String(right.timestamp).localeCompare(String(left.timestamp)));
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
    if (normalized && !pseudoImage(normalized)) {
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

  reconcileSnapshotMetadata(store, metadata);
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
    current.traces += store.tracesBySandbox.get(sandboxId)?.length ?? 0;
    bySource.set(source, current);
  }

  addMetricCountsBySource(bySource, store);
  addProfileCountsBySource(bySource, store);

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
    current.traces += store.tracesByImage.get(imageId)?.length ?? 0;
    bySource.set(source, current);
  }

  addEventCountsBySource(bySource, store.eventsBySandbox, store.sourceBySandbox, store.sourceByEvent);
  addEventCountsBySource(bySource, store.eventsByNode, store.sourceByNode, store.sourceByEvent);
  addEventCountsBySource(bySource, store.eventsByImage, store.sourceByImage, store.sourceByEvent);

  return Array.from(bySource.values())
    .map(({ eventIds: _eventIds, ...snapshot }) => snapshot)
    .sort((left, right) => right.metricPoints - left.metricPoints);
}

function addEventCountsBySource(bySource, eventsByKey, sourceByKey, sourceByEvent) {
  for (const [key, rows] of eventsByKey.entries()) {
    for (const row of rows) {
      const source = sourceByEvent.get(rowKey(row)) ?? sourceByKey.get(key);
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
      current.eventIds.add(rowKey(row));
      current.events = current.eventIds.size;
      bySource.set(source, current);
    }
  }
}

function addMetricCountsBySource(bySource, store) {
  for (const [seriesKey, source] of store.sourceByMetricSeries.entries()) {
    const [scope, scopeId, seriesId] = seriesKey.split('\u0000');
    const collection = scope === 'sandbox'
      ? store.metricsBySandbox
      : scope === 'image'
        ? store.metricsByImage
        : store.metricsByNode;
    const series = collection.get(scopeId)?.get(seriesId);
    if (!series) continue;

    const current = bySource.get(source) ?? {
      source,
      sandboxes: 0,
      metricSeries: 0,
      metricPoints: 0,
      events: 0,
      traces: 0,
      profiles: 0,
    };
    current.metricSeries += 1;
    current.metricPoints += series.points.length;
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

function pseudoImage(image) {
  return pseudoImageRef(image?.ref)
    || pseudoImageRef(image?.id)
    || String(image?.digest ?? '').startsWith('snapshot:');
}

function pseudoImageRef(value) {
  const text = String(value ?? '');
  return text === 'collector/unknown:latest'
    || text === 'containerd/unknown:latest'
    || text === 'docker/unknown:latest'
    || text === 'collector/startup-callchain:unknown'
    || text.includes('collector-startup-callchain-unknown')
    || text.includes('containerd-unknown-latest')
    || text.startsWith('containerd-snapshot:')
    || text.includes('containerd-snapshot-');
}

function meaningfulImageRef(value) {
  return Boolean(value) && !pseudoImageRef(value);
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
  const traceRuntimeType = stringValue(existing.attributes?.['runtime.type']);
  const incomingCpu = numberOr(incoming.cpuAvg, 0);
  const existingCpu = numberOr(existing.cpuAvg, 0);
  const metadataCpu = incomingCpu > 0 ? incomingCpu : existingCpu;
  const attributes = { ...existing.attributes, ...incoming.attributes };

  if (keepTraceStartup) {
    preserveAttribute(attributes, existing.attributes, 'startup.duration.source');
    preserveAttribute(attributes, existing.attributes, 'startup.duration.plugin');
  }

  return {
    ...existing,
    ...incoming,
    runtimeType: traceRuntimeType ?? incoming.runtimeType ?? existing.runtimeType,
    createdAt: keepTraceStartup ? existing.createdAt : lifecycleAction ? existing.createdAt : incoming.createdAt ?? existing.createdAt,
    startedAt: keepTraceStartup ? existing.startedAt : incoming.startedAt ?? existing.startedAt,
    stoppedAt: incoming.stoppedAt ?? existing.stoppedAt,
    removedAt: incoming.removedAt ?? existing.removedAt,
    startupDurationMs: keepTraceStartup ? existing.startupDurationMs : incoming.startupDurationMs || existing.startupDurationMs,
    cpuAvg: metadataCpu,
    memoryPeakBytes: Math.max(incoming.memoryPeakBytes, existing.memoryPeakBytes),
    eventCount: Math.max(incoming.eventCount, existing.eventCount),
    labels: { ...existing.labels, ...incoming.labels },
    attributes,
  };
}

function preserveAttribute(target, source, key) {
  if (source?.[key] !== undefined) target[key] = source[key];
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

function reconcileSnapshotMetadata(store, metadata) {
  for (const node of array(metadata?.nodes)) {
    const attributes = isObject(node.attributes) ? node.attributes : {};
    const scope = stringValue(attributes['snapshot.scope']);
    const nodeId = stringValue(attributes['snapshot.nodeId']) ?? stringValue(node.id);
    const sandboxIds = stringSet(attributes['snapshot.sandboxIds']);

    if (!scope || !nodeId || !sandboxIds) continue;
    reconcileSandboxSnapshot(store, scope, nodeId, sandboxIds);
  }
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

    reconcileSandboxSnapshot(store, scope, nodeId, sandboxIds);
  }
}

function reconcileSandboxSnapshot(store, scope, nodeId, sandboxIds) {
  for (const sandbox of Array.from(store.sandboxes.values())) {
    if (sandbox.nodeId !== nodeId) continue;
    if (!sandboxMatchesSnapshotScope(sandbox, scope)) continue;
    if (!sandboxIds.has(sandbox.id)) removeLiveSandbox(store, sandbox.id);
  }
}

function sandboxMatchesSnapshotScope(sandbox, scope) {
  if (stringValue(sandbox.attributes?.['snapshot.scope']) === scope) return true;
  if (scope === 'docker-running') {
    return sandbox.id.startsWith('docker-') || Boolean(stringValue(sandbox.attributes?.['docker.id']));
  }
  if (scope === 'containerd-running') {
    return sandbox.id.startsWith('containerd-')
      || sandbox.id.startsWith('k8s-')
      || Boolean(stringValue(sandbox.attributes?.['containerd.id']));
  }

  const runtimePrefix = scope.endsWith('-running') ? scope.slice(0, -'-running'.length) : undefined;
  if (runtimePrefix && stringValue(sandbox.runtimeType) === runtimePrefix) return true;

  return false;
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


function rememberDiagnosticArtifacts(store, events, source) {
  for (const event of array(events)) {
    const attributes = isObject(event.attributes) ? event.attributes : {};
    const artifactId = stringValue(attributes['diagnostic.id']);
    const objectUri = stringValue(attributes['diagnostic.objectUri']);
    if (event.eventType !== 'diagnostic' || !artifactId || !objectUri) continue;

    const scopeId = stringValue(event.sandboxId) ?? stringValue(event.nodeId) ?? 'node';
    const row = {
      id: artifactId,
      timestamp: event.timestamp,
      scope: event.sandboxId ? 'sandbox' : 'node',
      sandboxId: event.sandboxId,
      nodeId: event.nodeId,
      runtimeType: event.runtimeType,
      artifactType: stringValue(attributes['diagnostic.artifactType']) ?? 'diagnostic_bundle',
      objectUri,
      sizeBytes: numberOr(attributes['diagnostic.sizeBytes'], 0),
      durationMs: numberOr(attributes['diagnostic.durationMs'], 0),
      severity: event.severity,
      status: stringValue(attributes['diagnostic.status']) ?? 'captured',
      reason: event.reason ?? stringValue(attributes['diagnostic.reason']),
      message: event.message,
      source: source ?? event.source,
      artifacts: Array.isArray(attributes['diagnostic.artifacts']) ? attributes['diagnostic.artifacts'] : [],
      attributes,
    };

    rememberRow(store.diagnosticArtifactsByScope, scopeId, row, rowLimit('events'));
    if (source) store.sourceByDiagnosticArtifact.set(row.id, source);
  }
}

function rememberProfile(store, profile, source) {
  rememberRow(store.profilesBySandbox, profile.sandboxId, profile, rowLimit('profiles'));
  if (source && profile?.id) store.sourceByProfile.set(profileKey(profile), source);
}

function rememberEvent(store, event, source) {
  if (source) store.sourceByEvent.set(rowKey(event), source);
  rememberRow(store.eventsBySandbox, event.sandboxId, event, rowLimit('events'));

  const nodeId = nodeIdForEvent(store, event);
  rememberRow(store.eventsByNode, nodeId, { ...event, nodeId }, rowLimit('events'));

  const imageId = stringValue(event.imageId)
    ?? stringValue(event.attributes?.['image.id'])
    ?? stringValue(event.attributes?.imageId);
  if (imageId && imageRemovalEvent(event)) removeLiveImage(store, imageId);
  rememberRow(store.eventsByImage, imageId, { ...event, imageId }, rowLimit('events'));
}

function nodeIdForEvent(store, event) {
  return stringValue(event.nodeId)
    ?? stringValue(event.attributes?.['snapshot.nodeId'])
    ?? stringValue(event.attributes?.nodeId)
    ?? nodeIdForSandbox(store, stringValue(event.sandboxId));
}

function nodeIdForSandbox(store, sandboxId) {
  if (!sandboxId) return undefined;
  return stringValue(store.sandboxes.get(sandboxId)?.nodeId)
    ?? stringValue(store.sandboxHistory.get(sandboxId)?.nodeId);
}


function addProfileCountsBySource(bySource, store) {
  for (const profiles of store.profilesBySandbox.values()) {
    for (const profile of profiles) {
      const source = store.sourceByProfile.get(profileKey(profile)) ?? store.sourceBySandbox.get(profile.sandboxId);
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
      current.profiles += 1;
      bySource.set(source, current);
    }
  }
}

function profileKey(profile) {
  return `${profile.sandboxId ?? 'unknown'}:${profile.id ?? 'unknown'}`;
}

function imageRemovalEvent(event) {
  const action = stringValue(event.attributes?.dockerAction)
    ?? stringValue(event.attributes?.['docker.action'])
    ?? String(event.eventName ?? '').split('.').at(-1);
  return event.eventType === 'image' && ['delete', 'untag', 'remove', 'content_delete', 'snapshot_remove'].includes(action);
}

function rememberMetric(store, metric, source) {
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
  if (source) store.sourceByMetricSeries.set(metricSeriesSourceKey(scope, scopeId, id), source);
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
    rememberDerivedEventFromMetric(store, metric, source);
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
  rememberDerivedEventFromMetric(store, metric, source);
  refreshSandboxFromMetric(store, metric);
}

function rememberDerivedEventFromMetric(store, metric, source) {
  const nodeId = stringValue(metric.nodeId);
  const name = stringValue(metric.name);
  if (!nodeId || !name?.startsWith('host_agent.')) return;

  const attributes = isObject(metric.attributes) ? metric.attributes : {};

  if (name === 'host_agent.up') {
    if (numberOr(metric.value, 0) <= 0) return;
    const key = `host-agent-up:${nodeId}`;
    if (store.derivedEventState.has(key)) return;
    store.derivedEventState.set(key, true);
    rememberDerivedNodeEvent(store, nodeId, metric, source, {
      eventName: 'host_agent.started',
      severity: 'info',
      message: `RuntimePulse host-agent reported up on ${nodeId}.`,
      attributes: {
        'collector.component': 'host-agent',
      },
    });
    return;
  }

  if (name === 'host_agent.source.collect.success') {
    const collectorSource = stringValue(attributes['collector.source']);
    if (!collectorSource) return;
    const stateKey = `source-success:${nodeId}:${collectorSource}`;
    const previous = store.derivedEventState.get(stateKey);
    const current = numberOr(metric.value, 0) > 0;
    store.derivedEventState.set(stateKey, current);
    if (previous === undefined || previous === current) return;
    rememberDerivedNodeEvent(store, nodeId, metric, source, {
      eventName: current ? 'host_agent.collector.recovered' : 'host_agent.collector.failed',
      severity: current ? 'info' : 'error',
      message: current
        ? `Collector source ${collectorSource} recovered on ${nodeId}.`
        : `Collector source ${collectorSource} failed on ${nodeId}.`,
      attributes: {
        'collector.component': 'host-agent',
        'collector.source': collectorSource,
      },
    });
    return;
  }

  const stream = stringValue(attributes['collector.event_stream']);
  if (!stream) return;

  if (name === 'host_agent.event_stream.enabled') {
    store.derivedEventState.set(`event-stream-enabled:${nodeId}:${stream}`, numberOr(metric.value, 0) > 0);
    return;
  }

  if (name === 'host_agent.event_stream.running') {
    const enabled = store.derivedEventState.get(`event-stream-enabled:${nodeId}:${stream}`);
    if (enabled === false) return;
    const stateKey = `event-stream-running:${nodeId}:${stream}`;
    const previous = store.derivedEventState.get(stateKey);
    const current = numberOr(metric.value, 0) > 0;
    store.derivedEventState.set(stateKey, current);
    if (previous !== undefined && previous === current) return;
    rememberDerivedNodeEvent(store, nodeId, metric, source, {
      eventName: current ? 'host_agent.event_stream.connected' : 'host_agent.event_stream.disconnected',
      severity: current ? 'info' : 'warning',
      message: current
        ? `${stream} event stream is running on ${nodeId}.`
        : `${stream} event stream is not running on ${nodeId}.`,
      attributes: {
        'collector.component': 'host-agent',
        'collector.event_stream': stream,
      },
    });
    return;
  }

  if (name === 'host_agent.event_stream.restarts_total' || name === 'host_agent.event_stream.errors_total') {
    const stateKey = `${name}:${nodeId}:${stream}`;
    const previous = store.derivedEventState.get(stateKey);
    const current = numberOr(metric.value, 0);
    store.derivedEventState.set(stateKey, current);
    if (previous === undefined || current <= previous) return;
    const isError = name.endsWith('.errors_total');
    rememberDerivedNodeEvent(store, nodeId, metric, source, {
      eventName: isError ? 'host_agent.event_stream.error' : 'host_agent.event_stream.restarted',
      severity: isError ? 'error' : 'warning',
      message: isError
        ? `${stream} event stream reported ${current - previous} new error(s) on ${nodeId}.`
        : `${stream} event stream restarted on ${nodeId}.`,
      attributes: {
        'collector.component': 'host-agent',
        'collector.event_stream': stream,
        'collector.counter.previous': previous,
        'collector.counter.current': current,
      },
    });
  }
}

function rememberDerivedNodeEvent(store, nodeId, metric, source, event) {
  const attributes = {
    ...(isObject(metric.attributes) ? metric.attributes : {}),
    ...(event.attributes ?? {}),
    'derived.fromMetric': metric.name,
    'derived.source': 'query-api',
  };
  const row = {
    id: [
      'node',
      sanitizeMetricIdPart(nodeId),
      sanitizeMetricIdPart(event.eventName),
      sanitizeMetricIdPart(stringValue(attributes['collector.source']) ?? stringValue(attributes['collector.event_stream']) ?? 'host-agent'),
      sanitizeMetricIdPart(metric.timestamp),
    ].join('-'),
    timestamp: metric.timestamp,
    severity: event.severity,
    eventType: 'node',
    eventName: event.eventName,
    nodeId,
    message: event.message,
    source: source ? `${source}/derived` : 'runtimepulse-query-api/derived',
    attributes,
  };

  if (source) store.sourceByEvent.set(rowKey(row), source);
  rememberRow(store.eventsByNode, nodeId, row, rowLimit('events'));
}

function metricSeriesSourceKey(scope, scopeId, seriesId) {
  return [scope, scopeId, seriesId].join('\u0000');
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

function podNetworkMetricsForSandbox(store, sandbox, sandboxId) {
  if (!sandbox || !kubernetesSandboxId(sandboxId)) return [];
  const attributes = isObject(sandbox.attributes) ? sandbox.attributes : {};
  const namespace = stringValue(attributes['k8s.namespace']) ?? stringValue(sandbox.namespace);
  const pod = stringValue(attributes['k8s.pod']) ?? stringValue(sandbox.workloadId);
  const container = stringValue(attributes['k8s.container']);
  if (!namespace || !pod || container === 'pod') return [];

  const podSandboxId = `k8s-${sanitizeKubernetesIdPart(namespace)}-${sanitizeKubernetesIdPart(pod)}-pod`;
  return Array.from(store.metricsBySandbox.get(podSandboxId)?.values() ?? [])
    .filter((series) => series.group === 'network' && stringValue(series.attributes?.['metrics.scope']) === 'pod');
}

function kubernetesSandboxId(value) {
  return String(value ?? '').startsWith('k8s-');
}

function sanitizeKubernetesIdPart(value) {
  return String(value)
    .trim()
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, '-')
    .replace(/^-+|-+$/g, '') || 'unknown';
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

  if (startupDurationMetric(metric)) {
    applySandboxStartupDuration(sandbox, metric.value, metric, 'metric');
    if (historicalSandbox) applySandboxStartupDuration(historicalSandbox, metric.value, metric, 'metric');
  }
  if (metric.name === 'sandbox.cpu.usage_ratio') {
    const cpuAverage = averageMetricSeriesValue(store.metricsBySandbox.get(metric.sandboxId)?.get(metricSeriesId('sandbox', metric.sandboxId, metric)))
      ?? numberOr(metric.value, 0);
    sandbox.cpuAvg = cpuAverage;
    if (historicalSandbox) historicalSandbox.cpuAvg = cpuAverage;
  }
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
  if (sandbox) {
    applySandboxStartupDuration(sandbox, durationMs, span, 'trace');
    applySandboxRuntimeTypeFromTrace(sandbox, span);
  }
  if (historicalSandbox) {
    applySandboxStartupDuration(historicalSandbox, durationMs, span, 'trace');
    applySandboxRuntimeTypeFromTrace(historicalSandbox, span);
  }
}

function startupTraceSpan(span) {
  return ['container.startup', 'sandbox.startup', 'sandbox.startup.e2e'].includes(String(span?.spanName ?? ''));
}

function applySandboxRuntimeTypeFromTrace(sandbox, span) {
  const runtimeType = stringValue(span?.runtimeType) ?? stringValue(span?.attributes?.['runtime.type']);
  if (!runtimeType) return;
  sandbox.runtimeType = runtimeType;
  sandbox.attributes = {
    ...(sandbox.attributes ?? {}),
    'runtime.type': runtimeType,
  };
}

function startupDurationMetric(metric) {
  return [
    'sandbox.startup.duration_ms',
    'sandbox.startup.e2e_duration_ms',
  ].includes(String(metric?.name ?? ''));
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
  if (source === 'trace' && plugin === 'cri-startup-trace') return 5;
  if (source === 'metric' && plugin === 'cri-startup-trace') return 4.9;
  if (source === 'trace' && runtimeStartupTracePlugin(plugin)) return 4;
  if (source === 'metric' && runtimeStartupTracePlugin(plugin)) return 3.9;
  if (source === 'trace' && plugin === 'startup-callchain') return 3;
  if (source === 'trace') return 2;
  if (source === 'metric') return 1;
  return 0;
}

function runtimeStartupTracePlugin(plugin) {
  return ['docker-startup-trace', 'containerd-startup-trace', 'cri-startup-trace'].includes(plugin);
}

function refreshSandboxDerivedFields(store, sandbox) {
  sandbox.eventCount = (store.eventsBySandbox.get(sandbox.id) ?? []).length;
  refreshSandboxStartupFromStoredData(store, sandbox);
}

function refreshSandboxStartupFromStoredData(store, sandbox) {
  for (const span of store.tracesBySandbox.get(sandbox.id) ?? []) {
    if (!startupTraceSpan(span)) continue;
    const durationMs = numberOr(span.durationMs, 0);
    if (durationMs <= 0) continue;
    applySandboxStartupDuration(sandbox, durationMs, span, 'trace');
    applySandboxRuntimeTypeFromTrace(sandbox, span);
  }

  for (const series of store.metricsBySandbox.get(sandbox.id)?.values() ?? []) {
    if (!startupDurationMetric(series)) continue;
    const point = latestMetricPoint(series);
    if (!point) continue;
    applySandboxStartupDuration(sandbox, numberOr(point.value, 0), {
      ...series,
      timestamp: point.timestamp,
    }, 'metric');
  }
}

function latestMetricPoint(series) {
  return array(series?.points)
    .slice()
    .sort((left, right) => Date.parse(left.timestamp) - Date.parse(right.timestamp))
    .at(-1);
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

function averageMetricSeriesValue(series) {
  const points = array(series?.points);
  if (points.length === 0) return undefined;
  const values = points.map((point) => numberOr(point.value, 0));
  return values.reduce((sum, value) => sum + value, 0) / values.length;
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
  return new Set(value.filter((item) => typeof item === 'string').map((item) => item.trim()).filter(Boolean));
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
