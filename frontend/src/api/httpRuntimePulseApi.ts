import type {
  EventRecord,
  Cluster,
  Image,
  IngestRecent,
  IngestStatus,
  MetricSeries,
  Node,
  ProfileArtifact,
  RuntimeCompareRow,
  RuntimePulseArtifact,
  Sandbox,
  SandboxAnalysis,
  SandboxQuery,
  TimeRange,
  TraceSpan,
} from '../domain/model';
import type { ArtifactQuery, RuntimePulseApi } from './RuntimePulseApi';

type QueryValue = string | number | boolean | undefined;

export function createHttpRuntimePulseApi(baseUrl: string): RuntimePulseApi {
  const request = createRequester(baseUrl);

  return {
    listClusters() {
      return request<Cluster[]>('/clusters');
    },
    listNodes() {
      return request<Node[]>('/nodes');
    },
    listImages() {
      return request<Image[]>('/images');
    },
    listSandboxes(query) {
      return request<Sandbox[]>('/sandboxes', sandboxQueryParams(query));
    },
    listSandboxHistory(query) {
      return request<Sandbox[]>('/sandboxes/history', sandboxQueryParams(query));
    },
    getSandbox(id) {
      return requestOptional<Sandbox>(request, `/sandboxes/${encodeURIComponent(id)}`);
    },
    getNode(id) {
      return requestOptional<Node>(request, `/nodes/${encodeURIComponent(id)}`);
    },
    getImage(id) {
      return requestOptional<Image>(request, `/images/${encodeURIComponent(id)}`);
    },
    getNodeMetrics(id, range) {
      return request<MetricSeries[]>(`/nodes/${encodeURIComponent(id)}/metrics`, timeRangeParams(range));
    },
    getNodeEvents(id, range) {
      return request<EventRecord[]>(`/nodes/${encodeURIComponent(id)}/events`, timeRangeParams(range));
    },
    getImageMetrics(id, range) {
      return request<MetricSeries[]>(`/images/${encodeURIComponent(id)}/metrics`, timeRangeParams(range));
    },
    getImageEvents(id, range) {
      return request<EventRecord[]>(`/images/${encodeURIComponent(id)}/events`, timeRangeParams(range));
    },
    getImageTrace(id, range) {
      return request<TraceSpan[]>(`/images/${encodeURIComponent(id)}/trace`, timeRangeParams(range));
    },
    getSandboxMetrics(id, range) {
      return request<MetricSeries[]>(`/sandboxes/${encodeURIComponent(id)}/metrics`, timeRangeParams(range));
    },
    getSandboxEvents(id, range) {
      return request<EventRecord[]>(`/sandboxes/${encodeURIComponent(id)}/events`, timeRangeParams(range));
    },
    getSandboxTrace(id) {
      return request<TraceSpan[]>(`/sandboxes/${encodeURIComponent(id)}/trace`);
    },
    getSandboxProfiles(id) {
      return request<ProfileArtifact[]>(`/sandboxes/${encodeURIComponent(id)}/profiles`);
    },
    getSandboxAnalysis(id) {
      return request<SandboxAnalysis>(`/sandboxes/${encodeURIComponent(id)}/analysis`);
    },
    compareRuntimes(range) {
      return request<RuntimeCompareRow[]>('/runtimes/compare', timeRangeParams(range));
    },
    listArtifacts(query) {
      return request<RuntimePulseArtifact[]>('/artifacts', artifactQueryParams(query));
    },
    getIngestStatus() {
      return request<IngestStatus>('/ingest/status');
    },
    getIngestRecent() {
      return request<IngestRecent>('/ingest/recent');
    },
  };
}

function createRequester(baseUrl: string) {
  const normalizedBaseUrl = baseUrl.replace(/\/+$/, '');

  return async function request<T>(path: string, params?: Record<string, QueryValue>): Promise<T> {
    const url = new URL(`${normalizedBaseUrl}${path.startsWith('/') ? path : `/${path}`}`, window.location.origin);
    Object.entries(params ?? {}).forEach(([key, value]) => {
      if (value !== undefined && value !== '') url.searchParams.set(key, String(value));
    });

    const response = await fetch(url, { headers: { Accept: 'application/json' } });
    if (!response.ok) throw new RuntimePulseHttpError(response.status, response.statusText, url.toString());

    const payload = await response.json();
    return unwrapResponse<T>(payload);
  };
}

async function requestOptional<T>(
  request: ReturnType<typeof createRequester>,
  path: string,
): Promise<T | undefined> {
  try {
    return await request<T>(path);
  } catch (error) {
    if (error instanceof RuntimePulseHttpError && error.status === 404) return undefined;
    throw error;
  }
}

function unwrapResponse<T>(payload: unknown): T {
  if (payload && typeof payload === 'object' && 'data' in payload) {
    return (payload as { data: T }).data;
  }

  return payload as T;
}


function artifactQueryParams(query?: ArtifactQuery): Record<string, QueryValue> {
  return {
    kind: query?.kind === 'all' ? undefined : query?.kind,
    sandboxId: query?.sandboxId,
    nodeId: query?.nodeId,
    text: query?.text,
  };
}

function sandboxQueryParams(query?: SandboxQuery): Record<string, QueryValue> {
  return {
    runtimeType: query?.runtimeType === 'all' ? undefined : query?.runtimeType,
    status: query?.status === 'all' ? undefined : query?.status,
    text: query?.text,
  };
}

function timeRangeParams(range?: TimeRange): Record<string, QueryValue> {
  return {
    from: range?.from,
    to: range?.to,
  };
}

class RuntimePulseHttpError extends Error {
  constructor(
    readonly status: number,
    statusText: string,
    url: string,
  ) {
    super(`RuntimePulse API request failed: ${status} ${statusText} (${url})`);
  }
}
