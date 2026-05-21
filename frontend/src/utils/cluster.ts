import type { Cluster } from '../domain/model';

const localClusterIds = new Set(['cluster-prod', 'runtimepulse-prod', 'runtimepulse-local']);

export function clusterDisplayName(cluster?: Cluster, fallbackId?: string) {
  const id = cluster?.id ?? fallbackId ?? '';
  const name = cluster?.name ?? '';

  if (localClusterIds.has(id) || localClusterIds.has(name)) {
    return 'Local observed cluster';
  }

  return name || id || 'Unknown cluster';
}

export function clusterDescription(cluster?: Cluster, fallbackId?: string) {
  const id = cluster?.id ?? fallbackId ?? 'unknown';
  const environment = cluster?.environment;

  if (localClusterIds.has(id) || localClusterIds.has(cluster?.name ?? '')) {
    return `Single-node collector group - cluster id: ${id}`;
  }

  return [environment, `cluster id: ${id}`].filter(Boolean).join(' - ');
}
