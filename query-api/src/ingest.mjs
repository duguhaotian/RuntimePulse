const maxPreviewErrors = 20;
const maxRecentBatches = 25;
const maxRecentRows = 80;
const maxRowsPerBatchKind = 8;

const metadataCollections = ['clusters', 'nodes', 'images', 'sandboxes'];

export function createIngestStatus() {
  return {
    mode: 'validation_only',
    startedAt: new Date().toISOString(),
    acceptedBatches: 0,
    rejectedBatches: 0,
    totals: emptyCounts(),
    sources: new Map(),
    recentBatches: [],
    recentMetrics: [],
    recentEvents: [],
    recentTraces: [],
    lastAcceptedBatch: undefined,
    lastRejectedBatch: undefined,
  };
}

export function validateIngestBatch(payload) {
  const errors = [];

  if (!isPlainObject(payload)) {
    return {
      ok: false,
      errors: ['body must be a JSON object'],
      counts: emptyCounts(),
    };
  }

  requireString(payload, 'source', errors);
  optionalDateTime(payload, 'observedAt', errors);

  const counts = emptyCounts();

  if (payload.metadata !== undefined) {
    if (!isPlainObject(payload.metadata)) {
      errors.push('metadata must be an object');
    } else {
      for (const name of metadataCollections) {
        const rows = payload.metadata[name];
        if (rows === undefined) continue;
        if (!Array.isArray(rows)) {
          errors.push(`metadata.${name} must be an array`);
          continue;
        }
        counts.metadata[name] = rows.length;
        rows.forEach((row, index) => validateMetadataRow(name, row, index, errors));
      }
    }
  }

  validateArray(payload, 'metrics', errors, counts, validateMetricSample);
  validateArray(payload, 'events', errors, counts, validateEventRecord);
  validateArray(payload, 'traces', errors, counts, validateTraceSpan);
  validateArray(payload, 'profiles', errors, counts, validateProfileArtifact);

  return {
    ok: errors.length === 0,
    errors: errors.slice(0, maxPreviewErrors),
    counts,
  };
}

export function recordAcceptedIngestBatch(status, payload, result) {
  const acceptedAt = new Date().toISOString();
  const source = payload.source;
  const observedAt = typeof payload.observedAt === 'string' ? payload.observedAt : undefined;
  const batch = {
    source,
    observedAt,
    acceptedAt,
    counts: result.counts,
  };

  status.acceptedBatches += 1;
  mergeCounts(status.totals, result.counts);
  status.lastAcceptedBatch = batch;
  rememberRecentPayload(status, payload, batch);

  const sourceStatus = status.sources.get(source) ?? {
    source,
    acceptedBatches: 0,
    totals: emptyCounts(),
    firstAcceptedAt: acceptedAt,
    lastAcceptedAt: acceptedAt,
    lastObservedAt: observedAt,
  };

  sourceStatus.acceptedBatches += 1;
  sourceStatus.lastAcceptedAt = acceptedAt;
  sourceStatus.lastObservedAt = observedAt;
  mergeCounts(sourceStatus.totals, result.counts);
  status.sources.set(source, sourceStatus);

  return batch;
}

export function ingestRecentSnapshot(status) {
  return {
    mode: status.mode,
    recentBatches: [...status.recentBatches],
    recentMetrics: [...status.recentMetrics],
    recentEvents: [...status.recentEvents],
    recentTraces: [...status.recentTraces],
  };
}

export function recordRejectedIngestBatch(status, payload, result) {
  const rejectedAt = new Date().toISOString();
  const source = isPlainObject(payload) && typeof payload.source === 'string' ? payload.source : undefined;

  status.rejectedBatches += 1;
  status.lastRejectedBatch = {
    source,
    rejectedAt,
    errors: result.errors,
  };
}

export function ingestStatusSnapshot(status) {
  return {
    mode: status.mode,
    startedAt: status.startedAt,
    acceptedBatches: status.acceptedBatches,
    rejectedBatches: status.rejectedBatches,
    totals: cloneCounts(status.totals),
    sources: Array.from(status.sources.values())
      .sort((left, right) => right.lastAcceptedAt.localeCompare(left.lastAcceptedAt))
      .map((source) => ({
        ...source,
        totals: cloneCounts(source.totals),
      })),
    lastAcceptedBatch: status.lastAcceptedBatch
      ? {
          ...status.lastAcceptedBatch,
          counts: cloneCounts(status.lastAcceptedBatch.counts),
        }
      : undefined,
    lastRejectedBatch: status.lastRejectedBatch,
  };
}

function emptyCounts() {
  return {
    metadata: {
      clusters: 0,
      nodes: 0,
      images: 0,
      sandboxes: 0,
    },
    metrics: 0,
    events: 0,
    traces: 0,
    profiles: 0,
  };
}

function cloneCounts(counts) {
  return {
    metadata: { ...counts.metadata },
    metrics: counts.metrics,
    events: counts.events,
    traces: counts.traces,
    profiles: counts.profiles,
  };
}

function mergeCounts(target, increment) {
  for (const name of metadataCollections) {
    target.metadata[name] += increment.metadata[name] ?? 0;
  }

  target.metrics += increment.metrics;
  target.events += increment.events;
  target.traces += increment.traces;
  target.profiles += increment.profiles;
}

function rememberRecentPayload(status, payload, batch) {
  prependBounded(status.recentBatches, batch, maxRecentBatches);

  for (const row of (Array.isArray(payload.metrics) ? payload.metrics : []).slice(0, maxRowsPerBatchKind)) {
    prependBounded(status.recentMetrics, {
      source: batch.source,
      acceptedAt: batch.acceptedAt,
      timestamp: row.timestamp,
      name: row.name,
      value: row.value,
      unit: row.unit,
      group: row.group,
      sandboxId: row.sandboxId,
      nodeId: row.nodeId,
      imageId: row.imageId,
      runtimeType: row.runtimeType,
      attributes: row.attributes,
    }, maxRecentRows);
  }

  for (const row of (Array.isArray(payload.events) ? payload.events : []).slice(0, maxRowsPerBatchKind)) {
    prependBounded(status.recentEvents, {
      source: batch.source,
      acceptedAt: batch.acceptedAt,
      id: row.id,
      timestamp: row.timestamp,
      severity: row.severity,
      eventType: row.eventType,
      eventName: row.eventName,
      sandboxId: row.sandboxId,
      nodeId: row.nodeId,
      runtimeType: row.runtimeType,
      message: row.message,
    }, maxRecentRows);
  }

  for (const row of (Array.isArray(payload.traces) ? payload.traces : []).slice(0, maxRowsPerBatchKind)) {
    prependBounded(status.recentTraces, {
      source: batch.source,
      acceptedAt: batch.acceptedAt,
      traceId: row.traceId,
      spanId: row.spanId,
      parentSpanId: row.parentSpanId,
      sandboxId: row.sandboxId,
      spanName: row.spanName,
      startTime: row.startTime,
      endTime: row.endTime,
      durationMs: row.durationMs,
      status: row.status,
    }, maxRecentRows);
  }
}

function prependBounded(target, row, limit) {
  target.unshift(row);
  if (target.length > limit) target.length = limit;
}

function validateArray(payload, field, errors, counts, validator) {
  const rows = payload[field];
  if (rows === undefined) return;

  if (!Array.isArray(rows)) {
    errors.push(`${field} must be an array`);
    return;
  }

  counts[field] = rows.length;
  rows.forEach((row, index) => validator(row, index, errors));
}

function validateMetadataRow(collection, row, index, errors) {
  if (!isPlainObject(row)) {
    errors.push(`metadata.${collection}[${index}] must be an object`);
    return;
  }

  requireString(row, 'id', errors, `metadata.${collection}[${index}]`);

  if (collection === 'nodes') {
    requireString(row, 'clusterId', errors, `metadata.${collection}[${index}]`);
    requireString(row, 'name', errors, `metadata.${collection}[${index}]`);
  }

  if (collection === 'images') {
    requireString(row, 'ref', errors, `metadata.${collection}[${index}]`);
    requireString(row, 'digest', errors, `metadata.${collection}[${index}]`);
  }

  if (collection === 'sandboxes') {
    requireString(row, 'nodeId', errors, `metadata.${collection}[${index}]`);
    requireString(row, 'runtimeType', errors, `metadata.${collection}[${index}]`);
  }
}

function validateMetricSample(row, index, errors) {
  const prefix = `metrics[${index}]`;
  if (!isPlainObject(row)) {
    errors.push(`${prefix} must be an object`);
    return;
  }

  requireDateTime(row, 'timestamp', errors, prefix);
  requireString(row, 'name', errors, prefix);
  requireNumber(row, 'value', errors, prefix);
  optionalString(row, 'unit', errors, prefix);
  optionalString(row, 'group', errors, prefix);
  optionalString(row, 'sandboxId', errors, prefix);
  optionalString(row, 'nodeId', errors, prefix);
  optionalString(row, 'imageId', errors, prefix);
  optionalString(row, 'runtimeType', errors, prefix);
}

function validateEventRecord(row, index, errors) {
  const prefix = `events[${index}]`;
  if (!isPlainObject(row)) {
    errors.push(`${prefix} must be an object`);
    return;
  }

  requireString(row, 'id', errors, prefix);
  requireDateTime(row, 'timestamp', errors, prefix);
  requireString(row, 'severity', errors, prefix);
  requireString(row, 'eventType', errors, prefix);
  requireString(row, 'eventName', errors, prefix);
  requireString(row, 'message', errors, prefix);
  requireString(row, 'source', errors, prefix);
  requireObject(row, 'attributes', errors, prefix);
  optionalString(row, 'sandboxId', errors, prefix);
  optionalString(row, 'nodeId', errors, prefix);
  optionalString(row, 'runtimeType', errors, prefix);
}

function validateTraceSpan(row, index, errors) {
  const prefix = `traces[${index}]`;
  if (!isPlainObject(row)) {
    errors.push(`${prefix} must be an object`);
    return;
  }

  requireString(row, 'traceId', errors, prefix);
  requireString(row, 'spanId', errors, prefix);
  requireString(row, 'spanName', errors, prefix);
  requireDateTime(row, 'startTime', errors, prefix);
  requireDateTime(row, 'endTime', errors, prefix);
  requireNumber(row, 'durationMs', errors, prefix);
  requireString(row, 'status', errors, prefix);
  requireObject(row, 'attributes', errors, prefix);
  optionalString(row, 'sandboxId', errors, prefix);
  optionalString(row, 'parentSpanId', errors, prefix);
}

function validateProfileArtifact(row, index, errors) {
  const prefix = `profiles[${index}]`;
  if (!isPlainObject(row)) {
    errors.push(`${prefix} must be an object`);
    return;
  }

  requireString(row, 'id', errors, prefix);
  requireString(row, 'sandboxId', errors, prefix);
  requireDateTime(row, 'timestamp', errors, prefix);
  requireString(row, 'profileType', errors, prefix);
  requireString(row, 'processRole', errors, prefix);
  requireNumber(row, 'durationMs', errors, prefix);
  requireNumber(row, 'sampleCount', errors, prefix);
  requireString(row, 'objectUri', errors, prefix);
  optionalString(row, 'flamegraphUri', errors, prefix);
}

function requireString(row, field, errors, prefix = 'body') {
  if (typeof row[field] !== 'string' || row[field].trim() === '') {
    errors.push(`${prefix}.${field} must be a non-empty string`);
  }
}

function optionalString(row, field, errors, prefix) {
  if (row[field] !== undefined && typeof row[field] !== 'string') {
    errors.push(`${prefix}.${field} must be a string`);
  }
}

function requireNumber(row, field, errors, prefix) {
  if (typeof row[field] !== 'number' || !Number.isFinite(row[field])) {
    errors.push(`${prefix}.${field} must be a finite number`);
  }
}

function requireObject(row, field, errors, prefix) {
  if (!isPlainObject(row[field])) {
    errors.push(`${prefix}.${field} must be an object`);
  }
}

function requireDateTime(row, field, errors, prefix) {
  if (!isValidDateTime(row[field])) {
    errors.push(`${prefix}.${field} must be an ISO date-time string`);
  }
}

function optionalDateTime(row, field, errors) {
  if (row[field] !== undefined && !isValidDateTime(row[field])) {
    errors.push(`body.${field} must be an ISO date-time string`);
  }
}

function isValidDateTime(value) {
  return typeof value === 'string' && Number.isFinite(Date.parse(value));
}

function isPlainObject(value) {
  return Boolean(value) && typeof value === 'object' && !Array.isArray(value);
}
