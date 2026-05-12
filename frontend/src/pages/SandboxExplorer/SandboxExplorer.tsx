import { useEffect, useMemo, useState } from 'react';
import type { RuntimePulseApi } from '../../api/RuntimePulseApi';
import type { Cluster, Image, Node, RuntimeType, Sandbox } from '../../domain/model';
import { runtimeColors } from '../../utils/colors';
import { formatDateTime } from '../../utils/time';
import { formatBytes, formatDuration, formatRatio } from '../../utils/units';

type ClusterExplorerProps = {
  api: RuntimePulseApi;
  onSelectNode: (id: string) => void;
};

const refreshIntervalMs = 5000;

export function SandboxExplorer({ api, onSelectNode }: ClusterExplorerProps) {
  const [clusters, setClusters] = useState<Cluster[]>([]);
  const [nodes, setNodes] = useState<Node[]>([]);
  const [images, setImages] = useState<Image[]>([]);
  const [sandboxes, setSandboxes] = useState<Sandbox[]>([]);
  const [lastRefreshAt, setLastRefreshAt] = useState<string>();

  useEffect(() => {
    let mounted = true;
    let timer: number | undefined;

    const refresh = () => {
      Promise.all([api.listClusters(), api.listNodes(), api.listImages(), api.listSandboxes()]).then(([nextClusters, nextNodes, nextImages, nextSandboxes]) => {
        if (!mounted) return;
        setClusters(nextClusters);
        setNodes(nextNodes);
        setImages(nextImages);
        setSandboxes(nextSandboxes);
        setLastRefreshAt(new Date().toISOString());
      });
    };

    refresh();
    timer = window.setInterval(refresh, refreshIntervalMs);

    return () => {
      mounted = false;
      if (timer) window.clearInterval(timer);
    };
  }, [api]);

  const totals = useMemo(() => {
    const running = sandboxes.filter((sandbox) => sandbox.status === 'running').length;
    const failed = sandboxes.filter((sandbox) => sandbox.status === 'failed').length;
    const slow = sandboxes.filter((sandbox) => sandbox.startupDurationMs > 7000).length;
    const capacity = nodes.reduce((sum, node) => sum + node.memoryBytes, 0);
    return { running, failed, slow, capacity };
  }, [nodes, sandboxes]);

  return (
    <section className="page-stack">
      <div className="page-header">
        <div>
          <p className="eyebrow">Cluster level</p>
          <h2>Clusters</h2>
          <p>先从集群和节点入口观察全局状态，点击节点进入该节点的沙箱和镜像明细。</p>
        </div>
        <div className="header-actions">
          {lastRefreshAt && <span className="refresh-pill">Updated {formatDateTime(lastRefreshAt)}</span>}
          <button>Export</button>
          <button className="primary">Create report</button>
        </div>
      </div>

      <div className="summary-grid">
        <SummaryCard label="Clusters" value={String(clusters.length)} caption="static topology" />
        <SummaryCard label="Nodes" value={String(nodes.length)} caption={`${formatBytes(totals.capacity)} memory`} />
        <SummaryCard label="Running sandboxes" value={String(totals.running)} caption="dynamic runtime state" />
        <SummaryCard label="Slow / failed" value={`${totals.slow} / ${totals.failed}`} caption="startup and health signals" tone={totals.failed > 0 ? 'danger' : totals.slow > 0 ? 'warning' : undefined} />
      </div>

      <div className="level-page-grid">
        {clusters.map((cluster) => {
          const clusterNodes = nodes.filter((node) => node.clusterId === cluster.id);
          const clusterSandboxes = sandboxes.filter((sandbox) => sandbox.clusterId === cluster.id);
          const clusterImages = uniqueImages(clusterSandboxes, images);
          const cpuAvg = average(clusterSandboxes.map((sandbox) => sandbox.cpuAvg));
          const avgStartup = average(clusterSandboxes.map((sandbox) => sandbox.startupDurationMs));

          return (
            <article className="level-card cluster-card" key={cluster.id}>
              <div className="level-card-header">
                <div>
                  <span className="level-kicker">Cluster</span>
                  <h3>{cluster.name}</h3>
                  <p>{cluster.environment} · {cluster.id}</p>
                </div>
                <span className="data-pill">Live query</span>
              </div>

              <DynamicStaticBlock
                dynamicItems={[
                  ['Running', String(clusterSandboxes.filter((sandbox) => sandbox.status === 'running').length)],
                  ['Failed', String(clusterSandboxes.filter((sandbox) => sandbox.status === 'failed').length)],
                  ['CPU avg', formatRatio(cpuAvg)],
                  ['Avg startup', formatDuration(avgStartup)],
                ]}
                staticItems={[
                  ['Nodes', String(clusterNodes.length)],
                  ['Images', String(clusterImages.length)],
                  ['Capacity', `${clusterNodes.reduce((sum, node) => sum + node.cpuCores, 0)} cores / ${formatBytes(clusterNodes.reduce((sum, node) => sum + node.memoryBytes, 0))}`],
                  ['Environment', cluster.environment],
                ]}
              />

              <div className="table-card level-table-card">
                <div className="table-titlebar">
                  <div>
                    <strong>Nodes</strong>
                    <span>点击节点查看节点详情</span>
                  </div>
                  <div className="column-pills">
                    <span>dynamic</span>
                    <span>static</span>
                  </div>
                </div>
                <table>
                  <thead>
                    <tr>
                      <th>Node</th>
                      <th>Status</th>
                      <th>Sandboxes</th>
                      <th>CPU Avg</th>
                      <th>Avg Startup</th>
                      <th>Kernel</th>
                      <th>Capacity</th>
                    </tr>
                  </thead>
                  <tbody>
                    {clusterNodes.map((node) => {
                      const nodeSandboxes = clusterSandboxes.filter((sandbox) => sandbox.nodeId === node.id);
                      return (
                        <tr className="clickable-row" key={node.id} onClick={() => onSelectNode(node.id)}>
                          <td><strong>{node.name}</strong><small>{node.id}</small></td>
                          <td><NodeStatus status={node.status} /></td>
                          <td>{nodeSandboxes.length}</td>
                          <td>{formatRatio(average(nodeSandboxes.map((sandbox) => sandbox.cpuAvg)))}</td>
                          <td>{formatDuration(average(nodeSandboxes.map((sandbox) => sandbox.startupDurationMs)))}</td>
                          <td>{node.kernelVersion}</td>
                          <td>{node.cpuCores} cores / {formatBytes(node.memoryBytes)}</td>
                        </tr>
                      );
                    })}
                  </tbody>
                </table>
              </div>
            </article>
          );
        })}
      </div>
    </section>
  );
}

export function RuntimeBadge({ runtimeType }: { runtimeType: RuntimeType }) {
  return <span className="runtime-badge"><i style={{ background: runtimeColors[runtimeType] }} />{runtimeType}</span>;
}

export function StatusBadge({ status }: { status: Sandbox['status'] }) {
  return <span className={`status-badge ${status}`}>{status}</span>;
}

export function NodeStatus({ status }: { status: Node['status'] }) {
  return <span className={`node-status ${status}`}>{status}</span>;
}

export function SummaryCard({ label, value, caption, tone }: { label: string; value: string; caption: string; tone?: 'warning' | 'danger' }) {
  return (
    <div className={`summary-card ${tone ?? ''}`}>
      <span>{label}</span>
      <strong>{value}</strong>
      <em>{caption}</em>
    </div>
  );
}

export function DynamicStaticBlock({
  dynamicItems,
  staticItems,
}: {
  dynamicItems: Array<[string, string]>;
  staticItems: Array<[string, string]>;
}) {
  return (
    <div className="dynamic-static-grid">
      <MetricKind title="Dynamic" tone="dynamic" items={dynamicItems} />
      <MetricKind title="Static" tone="static" items={staticItems} />
    </div>
  );
}

function MetricKind({ title, tone, items }: { title: string; tone: 'dynamic' | 'static'; items: Array<[string, string]> }) {
  return (
    <dl className={`metric-kind ${tone}`}>
      <dt>{title}</dt>
      {items.map(([label, value]) => (
        <div key={label}>
          <span>{label}</span>
          <dd>{value}</dd>
        </div>
      ))}
    </dl>
  );
}

function uniqueImages(sandboxes: Sandbox[], images: Image[]) {
  const ids = new Set(sandboxes.map((sandbox) => sandbox.imageId));
  return images.filter((image) => ids.has(image.id));
}

function average(values: number[]) {
  return values.reduce((sum, value) => sum + value, 0) / Math.max(values.length, 1);
}
