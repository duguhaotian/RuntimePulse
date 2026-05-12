# RuntimePulse Storage Design

This document defines the Phase 2 storage model and Query API boundaries. It is a design contract for later collector and database work; the current implementation still serves mock data.

## Storage Split

RuntimePulse separates metadata, high-volume telemetry, and diagnostic artifacts.

| Store | Purpose | Example data |
| --- | --- | --- |
| PostgreSQL | Stable metadata and object relationships | clusters, nodes, sandboxes, images, layers, profile indexes |
| ClickHouse | Time-series and analytical records | metrics, events, trace spans, lifecycle stage samples |
| Object storage | Large or semi-structured diagnostic payloads | pprof files, flamegraph JSON, raw bundles |

The Query API composes these stores. The frontend never reads storage directly.

## PostgreSQL Metadata Model

PostgreSQL owns relationships, static data, and slowly changing summaries.

### `clusters`

| Column | Type | Notes |
| --- | --- | --- |
| `id` | text primary key | Stable cluster id |
| `name` | text | Display name |
| `environment` | text | prod, staging, lab, or local |
| `created_at` | timestamptz | Creation time |
| `updated_at` | timestamptz | Last metadata update |

### `nodes`

| Column | Type | Notes |
| --- | --- | --- |
| `id` | text primary key | Stable node id |
| `cluster_id` | text | References `clusters.id` |
| `name` | text | Hostname or display name |
| `kernel_version` | text | Static node metadata |
| `cpu_cores` | integer | Static CPU capacity |
| `memory_bytes` | bigint | Static memory capacity |
| `status` | text | Current node state |
| `labels` | jsonb | Kubernetes or platform labels |
| `attributes` | jsonb | RuntimePulse-specific static fields |
| `observed_at` | timestamptz | Last inventory observation |

### `images`

| Column | Type | Notes |
| --- | --- | --- |
| `id` | text primary key | RuntimePulse image id |
| `ref` | text | Image reference |
| `digest` | text | Content digest |
| `loading_mode` | text | `lazy` or `eager` |
| `size_bytes` | bigint | Total image size |
| `layer_count` | integer | Number of layers |
| `metadata` | jsonb | Registry, media type, platform, annotations |
| `observed_at` | timestamptz | Last image metadata observation |

### `image_layers`

| Column | Type | Notes |
| --- | --- | --- |
| `id` | text primary key | Layer id |
| `image_id` | text | References `images.id` |
| `layer_index` | integer | Display and pull order |
| `command` | text | Build command or layer label |
| `size_bytes` | bigint | Layer size |
| `block_size_bytes` | integer | Lazy-load block size, when available |
| `block_count` | integer | Total layer block count |
| `requested_block_count` | integer | Blocks requested by containers |
| `cache_hit_block_count` | integer | Blocks served from cache |
| `local_read_bytes` | bigint | Local/cache bytes |
| `remote_read_bytes` | bigint | Remote bytes |
| `pull_duration_ms` | integer | Eager pull duration |
| `unpack_duration_ms` | integer | Eager unpack duration |

For lazy images, block-level cache hit is tracked through time-series data in ClickHouse. PostgreSQL keeps only summary counters for list and detail pages.

### `image_download_steps`

| Column | Type | Notes |
| --- | --- | --- |
| `id` | text primary key | Step id |
| `image_id` | text | References `images.id` |
| `step_index` | integer | Timeline order |
| `name` | text | Specific stage name, such as resolve, pull layer, unpack |
| `phase` | text | Coarse phase for grouping |
| `duration_ms` | integer | Stage duration |
| `bytes` | bigint | Optional bytes for context |
| `detail` | jsonb | Registry, layer, retry, or error detail |

This table is used mainly for non-lazy images, where image availability is driven by download, verify, extract, and unpack stages.

### `sandboxes`

| Column | Type | Notes |
| --- | --- | --- |
| `id` | text primary key | Stable sandbox id |
| `cluster_id` | text | References `clusters.id` |
| `node_id` | text | References `nodes.id` |
| `namespace` | text | Workload namespace |
| `workload_id` | text | Deployment, pod, job, or owner id |
| `workload_name` | text | Display name |
| `image_id` | text | References `images.id` when known |
| `image_ref` | text | Image reference observed on the container |
| `runtime_type` | text | runc, gVisor, Kata, Firecracker |
| `runtime_version` | text | Runtime version |
| `status` | text | running, stopped, failed |
| `created_at` | timestamptz | Sandbox creation time |
| `started_at` | timestamptz | User workload start time |
| `stopped_at` | timestamptz | Stop time, when available |
| `startup_duration_ms` | integer | Summary startup duration |
| `cpu_avg` | double precision | Summary CPU average |
| `memory_peak_bytes` | bigint | Summary memory peak |
| `event_count` | integer | Summary event count |
| `labels` | jsonb | Workload labels |
| `attributes` | jsonb | Static or slowly changing runtime fields |

### `profile_artifacts`

| Column | Type | Notes |
| --- | --- | --- |
| `id` | text primary key | Profile id |
| `sandbox_id` | text | References `sandboxes.id` |
| `timestamp` | timestamptz | Capture time |
| `profile_type` | text | cpu, off-cpu, memory, block-io, syscall |
| `process_role` | text | shim, monitor, sentry, guest agent, workload |
| `duration_ms` | integer | Capture duration |
| `sample_count` | integer | Number of samples |
| `object_uri` | text | Raw artifact location |
| `flamegraph_uri` | text | Precomputed flamegraph location |
| `attributes` | jsonb | Capture command, symbols, kernel, node metadata |

## ClickHouse Analytical Model

ClickHouse owns high-cardinality time-window analysis.

### `metric_points`

| Column | Type | Notes |
| --- | --- | --- |
| `ts` | DateTime64 | Measurement timestamp |
| `sandbox_id` | String | Optional sandbox dimension |
| `node_id` | String | Optional node dimension |
| `image_id` | String | Optional image dimension |
| `runtime_type` | LowCardinality(String) | Runtime dimension |
| `metric_name` | LowCardinality(String) | CPU, IO, PSI, cache hit, etc. |
| `metric_label` | String | Display label |
| `metric_group` | LowCardinality(String) | cpu, memory, io, image, lifecycle |
| `unit` | LowCardinality(String) | %, bytes, ms, count |
| `value` | Float64 | Numeric value |
| `attributes` | JSON or String | Layer id, device, process role, cache source |

Examples:

- Node IO pressure over time.
- Container count changes over time.
- Lazy image block cache hit ratio over time.
- Sandbox CPU, memory, IO, and network metrics.
- Lifecycle phase duration samples grouped by stage and timestamp.

### `events`

| Column | Type | Notes |
| --- | --- | --- |
| `ts` | DateTime64 | Event timestamp |
| `id` | String | Event id |
| `severity` | LowCardinality(String) | info, warning, critical |
| `event_type` | LowCardinality(String) | lifecycle, image, runtime, node |
| `event_name` | String | Display name |
| `sandbox_id` | String | Optional sandbox dimension |
| `node_id` | String | Optional node dimension |
| `runtime_type` | LowCardinality(String) | Runtime dimension |
| `reason` | String | Machine-readable reason |
| `message` | String | Human-readable summary |
| `source` | String | Collector or runtime source |
| `attributes` | JSON or String | Raw dimensions |

### `trace_spans`

| Column | Type | Notes |
| --- | --- | --- |
| `trace_id` | String | Trace id |
| `span_id` | String | Span id |
| `parent_span_id` | String | Parent span id, empty for roots |
| `sandbox_id` | String | Sandbox dimension |
| `span_name` | String | Stage name |
| `start_time` | DateTime64 | Span start |
| `end_time` | DateTime64 | Span end |
| `duration_ms` | Float64 | Calculated duration |
| `status` | LowCardinality(String) | ok, warning, error |
| `attributes` | JSON or String | Runtime, node, image, retry, error details |

## Object Storage Layout

Object storage holds large payloads and generated diagnostic files.

```text
profiles/{sandbox_id}/{profile_id}.pprof
flamegraphs/{sandbox_id}/{profile_id}.json
diagnostics/{sandbox_id}/{timestamp}/bundle.tar.zst
diagnostics/{sandbox_id}/{timestamp}/metadata.json
```

The Query API returns metadata and signed or proxied URLs. Artifact lifecycle and access control should be implemented at the Query API layer.

## Query API Boundaries

| Endpoint family | Primary source | Query behavior |
| --- | --- | --- |
| `GET /api/clusters` | PostgreSQL | Cluster list and static summaries |
| `GET /api/nodes` | PostgreSQL + ClickHouse | Node metadata plus current summary rollups |
| `GET /api/nodes/{id}` | PostgreSQL + ClickHouse | Static node data, dynamic node curves, image/container rollups |
| `GET /api/images` | PostgreSQL | Image list, loading mode, layer summaries |
| `GET /api/images/{id}` | PostgreSQL + ClickHouse | Static image data, eager download stages, lazy block cache curves |
| `GET /api/sandboxes` | PostgreSQL | Sandbox inventory and summary fields |
| `GET /api/sandboxes/{id}` | PostgreSQL | Sandbox detail metadata and current summaries |
| `GET /api/sandboxes/{id}/metrics` | ClickHouse | Time-range metric series |
| `GET /api/sandboxes/{id}/events` | ClickHouse | Time-range event records |
| `GET /api/sandboxes/{id}/trace` | ClickHouse | Trace spans overlapping the requested time range |
| `GET /api/sandboxes/{id}/profiles` | PostgreSQL + object storage | Profile indexes and artifact locations |
| `GET /api/runtimes/compare` | ClickHouse + PostgreSQL | Aggregations grouped by runtime, node, image, and cache state |

The API owns joins, authorization, tenant scoping, downsampling, and time-range validation. Clients should request domain data, not storage-shaped records.

## Ingestion Boundaries

Collectors should write through future ingest endpoints or streaming pipelines, not the Query API.

The Phase 3 skeleton exposes `POST /api/ingest/batch` as a validation-only collector contract. It accepts metadata, metrics, events, traces, and profile artifact indexes, returns accepted counts, and intentionally does not persist records yet. `GET /api/ingest/status` exposes in-memory acceptance counters for collector smoke testing, and `GET /api/ingest/recent` exposes bounded recent sample previews. These counters and samples reset when the Query API process restarts.

- Metadata writes are idempotent by stable ids such as cluster id, node id, image digest, sandbox id, and profile id.
- Metric points are idempotent by series identity plus timestamp.
- Events are idempotent by event id.
- Trace spans are idempotent by trace id and span id.
- Object artifacts are written once, then referenced by metadata rows.

## Indexing and Partitioning

PostgreSQL recommended indexes:

- `nodes(cluster_id, status)`
- `images(digest)` and `images(loading_mode)`
- `image_layers(image_id, layer_index)`
- `sandboxes(node_id, created_at desc)`
- `sandboxes(image_id, created_at desc)`
- `sandboxes(runtime_type, status, created_at desc)`
- `profile_artifacts(sandbox_id, timestamp desc)`

ClickHouse recommended layout:

- Partition metrics, events, and traces by date.
- Order metrics by `(sandbox_id, metric_name, ts)` for sandbox detail queries.
- Add projections or materialized views for node-level curves ordered by `(node_id, metric_name, ts)`.
- Order events by `(sandbox_id, ts)` and traces by `(sandbox_id, start_time)`.
- Keep lifecycle stage samples queryable by `(node_id, ts, stage_name)` for merged lifecycle views.

## Retention

Retention should preserve diagnosis usefulness while controlling high-volume cost.

- Metadata: retain long term unless the cluster or tenant is deleted.
- Raw high-resolution metrics: retain for a short window, then downsample.
- Events and traces: retain longer than raw metrics because they explain timeline changes.
- Profiles and diagnostics: use object storage TTL and tiering; retain metadata after raw artifact expiry when useful.
- Aggregated runtime comparison data: retain as materialized summaries for trend analysis.

## Open Decisions

- Tenant and RBAC model.
- Exact retention windows per data class.
- Whether ClickHouse JSON columns use native JSON, Map, or encoded strings in the first implementation.
- Whether profile artifacts are served through signed URLs or streamed through the Query API.
- Ingest transport choice: HTTP batch, message queue, OpenTelemetry collector path, or a hybrid.
