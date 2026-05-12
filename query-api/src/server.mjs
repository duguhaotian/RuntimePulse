import { createServer } from 'node:http';
import {
  clusters,
  eventsForSandbox,
  filterSandboxes,
  images,
  metricsForSandbox,
  nodes,
  profilesForSandbox,
  runtimeComparison,
  sandboxes,
  traceForSandbox,
} from './mockData.mjs';

const port = Number(process.env.PORT ?? 8081);

const server = createServer((request, response) => {
  const url = new URL(request.url ?? '/', `http://${request.headers.host ?? 'localhost'}`);

  if (request.method === 'OPTIONS') return sendOptions(response);
  if (request.method !== 'GET') return sendJson(response, 405, { error: 'method_not_allowed' });
  if (url.pathname === '/health') return sendJson(response, 200, { status: 'ok' });

  const path = stripApiPrefix(url.pathname);

  try {
    if (path === '/clusters') return sendData(response, clusters);
    if (path === '/nodes') return sendData(response, nodes);
    if (path === '/images') return sendData(response, images);
    if (path === '/sandboxes') return sendData(response, filterSandboxes(Object.fromEntries(url.searchParams)));
    if (path === '/runtimes/compare') return sendData(response, runtimeComparison);

    const match = path.match(/^\/(sandboxes|nodes|images)\/([^/]+)(?:\/([^/]+))?$/);
    if (!match) return sendJson(response, 404, { error: 'not_found' });

    const [, resource, rawId, child] = match;
    const id = decodeURIComponent(rawId);

    if (resource === 'nodes') return sendOptional(response, nodes.find((node) => node.id === id));
    if (resource === 'images') return sendOptional(response, images.find((image) => image.id === id));
    if (resource === 'sandboxes') return sendSandboxResource(response, id, child);

    return sendJson(response, 404, { error: 'not_found' });
  } catch (error) {
    return sendJson(response, 500, { error: 'internal_error', message: error instanceof Error ? error.message : String(error) });
  }
});

server.listen(port, () => {
  console.log(`RuntimePulse query API listening on ${port}`);
});

function sendSandboxResource(response, id, child) {
  const sandbox = sandboxes.find((item) => item.id === id);
  if (!sandbox) return sendJson(response, 404, { error: 'not_found' });

  if (!child) return sendData(response, sandbox);
  if (child === 'metrics') return sendData(response, metricsForSandbox(id));
  if (child === 'events') return sendData(response, eventsForSandbox(id));
  if (child === 'trace') return sendData(response, traceForSandbox(id));
  if (child === 'profiles') return sendData(response, profilesForSandbox(id));

  return sendJson(response, 404, { error: 'not_found' });
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
    'Access-Control-Allow-Methods': 'GET, OPTIONS',
    'Content-Type': 'application/json',
    'Content-Length': Buffer.byteLength(body),
  });
  response.end(body);
}

function sendOptions(response) {
  response.writeHead(204, {
    'Access-Control-Allow-Origin': '*',
    'Access-Control-Allow-Headers': 'Content-Type, Accept',
    'Access-Control-Allow-Methods': 'GET, OPTIONS',
  });
  response.end();
}

function stripApiPrefix(pathname) {
  return pathname.startsWith('/api/') ? pathname.slice(4) : pathname;
}
