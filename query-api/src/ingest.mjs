const maxPreviewErrors = 20;

const metadataCollections = ['clusters', 'nodes', 'images', 'sandboxes'];

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
