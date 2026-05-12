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
  Sandbox,
  SandboxQuery,
  TimeRange,
  TraceSpan,
} from '../domain/model';

export interface RuntimePulseApi {
  listClusters(): Promise<Cluster[]>;
  listNodes(): Promise<Node[]>;
  listImages(): Promise<Image[]>;
  listSandboxes(query?: SandboxQuery): Promise<Sandbox[]>;
  getSandbox(id: string): Promise<Sandbox | undefined>;
  getNode(id: string): Promise<Node | undefined>;
  getImage(id: string): Promise<Image | undefined>;
  getSandboxMetrics(id: string, range?: TimeRange): Promise<MetricSeries[]>;
  getSandboxEvents(id: string, range?: TimeRange): Promise<EventRecord[]>;
  getSandboxTrace(id: string): Promise<TraceSpan[]>;
  getSandboxProfiles(id: string): Promise<ProfileArtifact[]>;
  compareRuntimes(range?: TimeRange): Promise<RuntimeCompareRow[]>;
  getIngestStatus(): Promise<IngestStatus>;
  getIngestRecent(): Promise<IngestRecent>;
}
