import { createServer } from 'node:http';
import {
  createIngestStatus,
  ingestRecentSnapshot,
  ingestStatusSnapshot,
  recordAcceptedIngestBatch,
  recordRejectedIngestBatch,
  validateIngestBatch,
} from './ingest.mjs';
import {
  clusters,
  eventsForSandbox,
  images,
  metricsForSandbox,
  nodes,
  profilesForSandbox,
  runtimeComparison,
  sandboxes,
  traceForSandbox,
} from './mockData.mjs';
import {
  createLiveStore,
  liveClusters,
  liveEventsForSandbox,
  liveImages,
  liveMetricsForSandbox,
  liveNodes,
  liveProfilesForSandbox,
  liveSandboxes,
  liveTraceForSandbox,
  recordLiveBatch,
} from './liveData.mjs';

const port = Number(process.env.PORT ?? 8081);
const maxBodyBytes = 1024 * 1024;
const ingestStatus = createIngestStatus();
const liveStore = createLiveStore();

const server = createServer((request, response) => {
  handleRequest(request, response).catch((error) => {
    if (error instanceof HttpError) {
      sendJson(response, error.statusCode, { error: error.code, message: error.message });
      return;
    }

    sendJson(response, 500, { error: 'internal_error', message: error instanceof Error ? error.message : String(error) });
  });
});

server.listen(port, () => {
  console.log(`RuntimePulse query API listening on ${port}`);
});

async function handleRequest(request, response) {
  const url = new URL(request.url ?? '/', `http://${request.headers.host ?? 'localhost'}`);

  if (request.method === 'OPTIONS') return sendOptions(response);
  if (url.pathname === '/health') return sendJson(response, 200, { status: 'ok' });

  const path = stripApiPrefix(url.pathname);

  if (request.method === 'POST' && path === '/ingest/batch') return handleIngestBatch(request, response);
  if (request.method !== 'GET') return sendJson(response, 405, { error: 'method_not_allowed' });

  if (path === '/ingest/status') return sendData(response, ingestStatusSnapshot(ingestStatus));
  if (path === '/ingest/recent') return sendData(response, ingestRecentSnapshot(ingestStatus));
  if (path === '/clusters') return sendData(response, mergeById(clusters, liveClusters(liveStore)));
  if (path === '/nodes') return sendData(response, mergeById(nodes, liveNodes(liveStore)));
  if (path === '/images') return sendData(response, mergeById(images, liveImages(liveStore)));
  if (path === '/sandboxes') return sendData(response, filterSandboxRows(allSandboxes(), Object.fromEntries(url.searchParams)));
  if (path === '/runtimes/compare') return sendData(response, runtimeComparisonRows());

  const match = path.match(/^\/(sandboxes|nodes|images)\/([^/]+)(?:\/([^/]+))?$/);
  if (!match) return sendJson(response, 404, { error: 'not_found' });

  const [, resource, rawId, child] = match;
  const id = decodeURIComponent(rawId);

  if (resource === 'nodes') return sendOptional(response, mergeById(nodes, liveNodes(liveStore)).find((node) => node.id === id));
  if (resource === 'images') return sendOptional(response, mergeById(images, liveImages(liveStore)).find((image) => image.id === id));
  if (resource === 'sandboxes') return sendSandboxResource(response, id, child, timeRangeFromSearchParams(url.searchParams));

  return sendJson(response, 404, { error: 'not_found' });
}

async function handleIngestBatch(request, response) {
  const payload = await readJsonBody(request);
  const result = validateIngestBatch(payload);

  if (!result.ok) {
    recordRejectedIngestBatch(ingestStatus, payload, result);
    return sendJson(response, 400, {
      error: 'invalid_ingest_batch',
      message: 'Ingest batch did not match the collector payload contract.',
      details: result.errors,
    });
  }

  recordAcceptedIngestBatch(ingestStatus, payload, result);
  recordLiveBatch(liveStore, payload);

  return sendJson(response, 202, {
    data: {
      status: 'accepted',
      mode: 'validation_only',
      counts: result.counts,
    },
  });
}

function sendSandboxResource(response, id, child, range) {
  const sandbox = allSandboxes().find((item) => item.id === id);
  if (!sandbox) return sendJson(response, 404, { error: 'not_found' });

  if (!child) return sendData(response, sandbox);
  if (child === 'metrics') return sendData(response, filterMetricSeries(metricsForSandboxMerged(id), range));
  if (child === 'events') return sendData(response, filterTimestamped(eventsForSandboxMerged(id), range, 'timestamp'));
  if (child === 'trace') return sendData(response, filterTraceSpans(traceForSandboxMerged(id), range));
  if (child === 'profiles') return sendData(response, filterTimestamped(profilesForSandboxMerged(id), range, 'timestamp'));

  return sendJson(response, 404, { error: 'not_found' });
}

function allSandboxes() {
  return mergeById(sandboxes, liveSandboxes(liveStore));
}

function metricsForSandboxMerged(id) {
  const base = mockSandboxExists(id) ? metricsForSandbox(id) : [];
  const live = liveMetricsForSandbox(liveStore, id);
  if (live.length > 0) return live;
  return base;
}

function eventsForSandboxMerged(id) {
  const base = mockSandboxExists(id) ? eventsForSandbox(id) : [];
  return mergeById(base, liveEventsForSandbox(liveStore, id));
}

function traceForSandboxMerged(id) {
  const base = mockSandboxExists(id) ? traceForSandbox(id) : [];
  return mergeTraceSpans(base, liveTraceForSandbox(liveStore, id));
}

function profilesForSandboxMerged(id) {
  const base = mockSandboxExists(id) ? profilesForSandbox(id) : [];
  return mergeById(base, liveProfilesForSandbox(liveStore, id));
}

function mockSandboxExists(id) {
  return sandboxes.some((sandbox) => sandbox.id === id);
}

function mergeById(baseRows, liveRows) {
  const byId = new Map(baseRows.map((row) => [row.id, row]));
  for (const row of liveRows) byId.set(row.id, mergeRows(byId.get(row.id), row));
  return Array.from(byId.values());
}

function mergeRows(base, live) {
  if (!base) return live;

  return Object.fromEntries(
    Object.entries({ ...base, ...live }).map(([key, value]) => {
      if (value === 0 && typeof base[key] === 'number' && base[key] > 0) return [key, base[key]];
      if (value === 'collector-observed' && typeof base[key] === 'string') return [key, base[key]];
      if (value === undefined) return [key, base[key]];
      return [key, value];
    }),
  );
}

function mergeTraceSpans(baseRows, liveRows) {
  const byId = new Map(baseRows.map((row) => [`${row.traceId}:${row.spanId}`, row]));
  for (const row of liveRows) byId.set(`${row.traceId}:${row.spanId}`, row);
  return Array.from(byId.values());
}

function filterSandboxRows(rows, query) {
  return rows.filter((sandbox) => {
    const runtimeMatch = !query.runtimeType || sandbox.runtimeType === query.runtimeType;
    const statusMatch = !query.status || sandbox.status === query.status;
    const text = query.text?.trim().toLowerCase();
    const textMatch = !text || [sandbox.id, sandbox.nodeId, sandbox.workloadName, sandbox.imageRef, sandbox.namespace]
      .some((value) => String(value).toLowerCase().includes(text));
    return runtimeMatch && statusMatch && textMatch;
  });
}

function runtimeComparisonRows() {
  const rows = buildRuntimeComparisonRows(allSandboxes());
  return rows.length > 0 ? rows : runtimeComparison;
}

function buildRuntimeComparisonRows(rows) {
  const buckets = new Map();

  for (const sandbox of rows) {
    const current = buckets.get(sandbox.runtimeType) ?? [];
    current.push(sandbox);
    buckets.set(sandbox.runtimeType, current);
  }

  return Array.from(buckets.entries())
    .map(([runtimeType, group]) => {
      const startupValues = group.map((sandbox) => sandbox.startupDurationMs).sort((left, right) => left - right);
      const cpuValues = group.map((sandbox) => sandbox.cpuAvg);
      const memoryValues = group.map((sandbox) => sandbox.memoryPeakBytes);
      const failures = group.filter((sandbox) => sandbox.status === 'failed').length;

      return {
        runtimeType,
        sampleCount: group.length,
        startupP50Ms: percentile(startupValues, 0.5),
        startupP95Ms: percentile(startupValues, 0.95),
        cpuOverheadRatio: average(cpuValues),
        memoryOverheadBytes: Math.max(...memoryValues, 0),
        failureRate: failures / Math.max(group.length, 1),
      };
    })
    .sort((left, right) => right.startupP95Ms - left.startupP95Ms);
}

function percentile(values, ratio) {
  if (values.length === 0) return 0;
  const index = Math.min(values.length - 1, Math.max(0, Math.ceil(values.length * ratio) - 1));
  return values[index];
}

function average(values) {
  if (values.length === 0) return 0;
  return values.reduce((sum, value) => sum + value, 0) / values.length;
}

function sendOptional(response, payload) {
  if (!payload) return sendJson(response, 404, { error: 'not_found' });
  return sendData(response, payload);
}

function sendData(response, data) {
  return sendJson(response, 200, { data });
}

function sendJson(response, statusCode, payload) {
  const body = JSON.stringify(payload);
  response.writeHead(statusCode, {
    'Access-Control-Allow-Origin': '*',
    'Access-Control-Allow-Headers': 'Content-Type, Accept',
    'Access-Control-Allow-Methods': 'GET, POST, OPTIONS',
    'Content-Type': 'application/json',
    'Content-Length': Buffer.byteLength(body),
  });
  response.end(body);
}

function sendOptions(response) {
  response.writeHead(204, {
    'Access-Control-Allow-Origin': '*',
    'Access-Control-Allow-Headers': 'Content-Type, Accept',
    'Access-Control-Allow-Methods': 'GET, POST, OPTIONS',
  });
  response.end();
}

function readJsonBody(request) {
  return new Promise((resolve, reject) => {
    let body = '';

    request.setEncoding('utf8');
    request.on('data', (chunk) => {
      body += chunk;
      if (Buffer.byteLength(body) > maxBodyBytes) {
        reject(new HttpError(413, 'payload_too_large', 'Request body exceeds 1 MiB.'));
        request.destroy();
      }
    });
    request.on('end', () => {
      if (!body.trim()) {
        reject(new HttpError(400, 'invalid_json', 'Request body is required.'));
        return;
      }

      try {
        resolve(JSON.parse(body));
      } catch {
        reject(new HttpError(400, 'invalid_json', 'Request body must be valid JSON.'));
      }
    });
    request.on('error', reject);
  }).catch((error) => {
    if (error instanceof HttpError) throw error;
    throw new HttpError(400, 'invalid_request_body', error instanceof Error ? error.message : String(error));
  });
}

function stripApiPrefix(pathname) {
  return pathname.startsWith('/api/') ? pathname.slice(4) : pathname;
}

function timeRangeFromSearchParams(searchParams) {
  const from = searchParams.get('from');
  const to = searchParams.get('to');
  const fromMs = from ? Date.parse(from) : undefined;
  const toMs = to ? Date.parse(to) : undefined;

  return {
    fromMs: Number.isFinite(fromMs) ? fromMs : undefined,
    toMs: Number.isFinite(toMs) ? toMs : undefined,
  };
}

function filterMetricSeries(seriesList, range) {
  if (!hasRange(range)) return seriesList;

  return seriesList.map((series) => ({
    ...series,
    points: series.points.filter((point) => timestampInRange(point.timestamp, range)),
  }));
}

function filterTimestamped(rows, range, field) {
  if (!hasRange(range)) return rows;
  return rows.filter((row) => timestampInRange(row[field], range));
}

function filterTraceSpans(spans, range) {
  if (!hasRange(range)) return spans;
  return spans.filter((span) => intervalOverlapsRange(span.startTime, span.endTime, range));
}

function timestampInRange(timestamp, range) {
  const value = Date.parse(timestamp);
  if (range.fromMs !== undefined && value < range.fromMs) return false;
  if (range.toMs !== undefined && value > range.toMs) return false;
  return true;
}

function intervalOverlapsRange(startTime, endTime, range) {
  const start = Date.parse(startTime);
  const end = Date.parse(endTime);
  if (range.fromMs !== undefined && end < range.fromMs) return false;
  if (range.toMs !== undefined && start > range.toMs) return false;
  return true;
}

function hasRange(range) {
  return range.fromMs !== undefined || range.toMs !== undefined;
}

class HttpError extends Error {
  constructor(statusCode, code, message) {
    super(message);
    this.statusCode = statusCode;
    this.code = code;
  }
}
