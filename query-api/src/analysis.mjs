export function buildSandboxAnalysis({ sandbox, image, metrics, events, spans, profiles }) {
  const findings = [
    startupDurationFinding(sandbox, spans),
    traceBottleneckFinding(sandbox, spans),
    startupCallchainFinding(sandbox, metrics, spans),
    nodePressureFinding(metrics),
    imageAccessFinding(image, sandbox),
    eventFinding(events),
    runtimeCpuFinding(sandbox, metrics),
    profileFinding(profiles),
  ].filter(Boolean);

  const severityRank = { critical: 3, warning: 2, info: 1 };
  findings.sort((left, right) => severityRank[right.severity] - severityRank[left.severity]);

  const topFinding = findings[0];

  return {
    sandboxId: sandbox.id,
    generatedAt: new Date().toISOString(),
    summary: topFinding
      ? `${topFinding.title}. ${topFinding.summary}`
      : 'No obvious bottleneck detected from the current metrics, events, traces, and profile metadata.',
    bottleneckStage: dominantSpan(spans)?.spanName,
    findings,
  };
}

function startupDurationFinding(sandbox, spans) {
  if (sandbox.startupDurationMs <= 7_000 && sandbox.status !== 'failed') return undefined;

  const severity = sandbox.status === 'failed' || sandbox.startupDurationMs > 15_000 ? 'critical' : 'warning';
  const stage = dominantSpan(spans);

  return {
    id: `${sandbox.id}-startup-duration`,
    severity,
    category: 'startup',
    title: severity === 'critical' ? 'Startup path is critically slow' : 'Startup path is slower than expected',
    summary: `${sandbox.runtimeType} startup took ${formatDuration(sandbox.startupDurationMs)}${stage ? `; the largest trace span is ${stage.spanName}` : ''}.`,
    evidence: [
      `startup.duration_ms=${Math.round(sandbox.startupDurationMs)}`,
      sandbox.status === 'failed' ? 'sandbox.status=failed' : `sandbox.status=${sandbox.status}`,
      stage ? `${stage.spanName}=${formatDuration(stage.durationMs)}` : undefined,
    ].filter(Boolean),
    recommendedActions: [
      stage ? `Inspect trace span ${stage.spanName} and compare with nearby lifecycle events.` : 'Inspect startup trace spans for phase-level delay.',
      'Compare this sandbox with a warm start for the same image and node.',
    ],
    relatedSpanIds: stage ? [stage.spanId] : [],
  };
}

function traceBottleneckFinding(sandbox, spans) {
  const stage = dominantSpan(spans);
  if (!stage) return undefined;

  const share = stage.durationMs / Math.max(sandbox.startupDurationMs, 1);
  if (share < 0.35 && stage.durationMs < 3_000) return undefined;

  const category = stage.spanName.startsWith('image.') || stage.spanName.includes('snapshot') ? 'image' : 'runtime';

  return {
    id: `${sandbox.id}-trace-bottleneck-${stage.spanId}`,
    severity: share > 0.55 || stage.status === 'error' ? 'critical' : 'warning',
    category,
    title: `${stage.spanName} dominates startup`,
    summary: `${stage.spanName} accounts for ${formatRatio(share)} of the observed startup duration.`,
    evidence: [
      `${stage.spanName}=${formatDuration(stage.durationMs)}`,
      `startup=${formatDuration(sandbox.startupDurationMs)}`,
      `span.status=${stage.status}`,
    ],
    recommendedActions: [
      category === 'image' ? 'Review image layer access and download/cache behavior for this container.' : 'Review runtime process metrics and profile artifacts during this span.',
      'Open the span detail modal to inspect attributes and nearby events.',
    ],
    relatedSpanIds: [stage.spanId],
  };
}

function startupCallchainFinding(sandbox, metrics, spans) {
  const totalMs = startupTotalMs(sandbox, metrics, spans);
  const candidates = startupCallchainCandidates(metrics, spans);
  if (candidates.length === 0) return undefined;

  const best = candidates
    .filter((candidate) => candidate.durationMs > 0 || candidate.count >= candidate.countThreshold)
    .sort((left, right) => callchainScore(right, totalMs) - callchainScore(left, totalMs))[0];
  if (!best) return undefined;

  const share = best.durationMs / Math.max(totalMs, 1);
  if (best.durationMs < best.durationThresholdMs && share < best.shareThreshold && best.count < best.countThreshold) return undefined;

  const severity = share >= 0.55 || best.durationMs >= 5_000 || best.errorSpan ? 'critical' : 'warning';

  return {
    id: `${sandbox.id}-startup-callchain-${best.key}`,
    severity,
    category: best.category,
    title: best.title,
    summary: best.durationMs > 0
      ? `${best.label} accounts for ${formatRatio(share)} of the measured startup call chain.`
      : `${best.label} executed ${best.count} times during startup; inspect per-command attribution for hidden latency.`,
    evidence: [
      best.durationMs > 0 ? `${best.durationMetricName}=${formatDuration(best.durationMs)}` : undefined,
      totalMs > 0 ? `startup.callchain=${formatDuration(totalMs)}` : undefined,
      best.count > 0 ? `${best.countMetricName}=${best.count}` : undefined,
      best.errorSpan ? `span.status=error in ${best.errorSpan.spanName}` : undefined,
    ].filter(Boolean),
    recommendedActions: best.recommendedActions,
    relatedMetricNames: [...new Set([best.durationMetricName, best.countMetricName].filter(Boolean))],
    relatedSpanIds: best.relatedSpanIds,
  };
}

function startupCallchainCandidates(metrics, spans) {
  const phaseDefinitions = [
    {
      key: 'cni',
      label: 'CNI setup',
      title: 'CNI setup dominates sandbox startup',
      category: 'startup',
      durationMetricName: 'sandbox.startup.cni_duration_ms',
      countMetricName: 'sandbox.startup.cni_plugin_count',
      spanPatterns: [/\bcni\b/i, /network/i],
      durationThresholdMs: 1_000,
      shareThreshold: 0.25,
      countThreshold: 4,
      recommendedActions: [
        'Inspect CNI plugin spans and compare ADD latency across bridge, IPAM, iptables/nft, and tc commands.',
        'Check whether concurrent pod starts or node network rule churn overlap this RunPodSandbox call.',
      ],
    },
    {
      key: 'oci',
      label: 'OCI runtime calls',
      title: 'OCI runtime calls dominate sandbox startup',
      category: 'runtime',
      durationMetricName: 'sandbox.startup.oci_duration_ms',
      countMetricName: 'sandbox.startup.oci_call_count',
      spanPatterns: [/\boci\b/i, /runc/i, /runtime\.(create|start)/i],
      durationThresholdMs: 800,
      shareThreshold: 0.25,
      countThreshold: 3,
      recommendedActions: [
        'Compare OCI create/start spans between runc and the configured secure runtime class.',
        'Inspect runtime process CPU/profile artifacts that overlap the slow OCI boundary.',
      ],
    },
    {
      key: 'kata',
      label: 'Kata VM startup',
      title: 'Kata VM startup dominates sandbox startup',
      category: 'runtime',
      durationMetricName: 'sandbox.startup.kata_duration_ms',
      countMetricName: undefined,
      spanPatterns: [/kata/i, /vm\.(boot|start|ready)/i, /hypervisor/i],
      durationThresholdMs: 1_500,
      shareThreshold: 0.25,
      countThreshold: Number.POSITIVE_INFINITY,
      recommendedActions: [
        'Break down Kata VM boot, agent ready, and shim/runtime spans before tuning Kubernetes or image settings.',
        'Compare this secure sandbox with the same image on runc to isolate the VM/runtime tax.',
      ],
    },
    {
      key: 'helper-binaries',
      label: 'startup helper binaries',
      title: 'Helper binary execution is high during startup',
      category: 'runtime',
      durationMetricName: 'sandbox.startup.helper_binary_duration_ms',
      countMetricName: 'sandbox.startup.helper_binary_count',
      spanPatterns: [/exec/i, /iptables/i, /nft/i, /\bip\b/i, /\btc\b/i],
      durationThresholdMs: 700,
      shareThreshold: 0.20,
      countThreshold: 12,
      recommendedActions: [
        'Inspect helper-binary spans to identify repeated iptables/nft/ip/tc invocations inside one RunPodSandbox call.',
        'If CNI dominates, compare CNI plugin configuration and rule programming behavior before changing runtime class.',
      ],
    },
    {
      key: 'binary-exec',
      label: 'startup binary executions',
      title: 'Startup invokes many helper processes',
      category: 'runtime',
      durationMetricName: 'sandbox.startup.binary_exec_duration_ms',
      countMetricName: 'sandbox.startup.binary_exec_count',
      spanPatterns: [/exec/i, /process/i],
      durationThresholdMs: 1_000,
      shareThreshold: 0.20,
      countThreshold: 20,
      recommendedActions: [
        'Review per-command exec attribution from the uprobe/eBPF exporter to find repeated helpers.',
        'Group the binary executions by parent CNI/OCI span to confirm which RunPodSandbox phase owns the cost.',
      ],
    },
  ];

  return phaseDefinitions.map((definition) => {
    const relatedSpans = spans.filter((span) => definition.spanPatterns.some((pattern) => pattern.test(span.spanName)));
    const spanDurationMs = relatedSpans.reduce((total, span) => total + Number(span.durationMs ?? 0), 0);
    return {
      ...definition,
      durationMs: maxMetricValue(metrics, definition.durationMetricName) ?? spanDurationMs,
      count: definition.countMetricName ? maxMetricValue(metrics, definition.countMetricName) ?? 0 : 0,
      relatedSpanIds: relatedSpans.map((span) => span.spanId),
      errorSpan: relatedSpans.find((span) => span.status === 'error'),
    };
  });
}

function startupTotalMs(sandbox, metrics, spans) {
  return Number(sandbox.startupDurationMs || 0)
    || maxMetricValue(metrics, 'sandbox.startup.callchain_duration_ms')
    || maxMetricValue(metrics, 'sandbox.startup.e2e_duration_ms')
    || maxMetricValue(metrics, 'sandbox.startup.duration_ms')
    || Math.max(0, ...spans
      .filter((span) => ['sandbox.startup.callchain', 'sandbox.startup.e2e', 'sandbox.startup'].includes(span.spanName))
      .map((span) => Number(span.durationMs ?? 0)));
}

function callchainScore(candidate, totalMs) {
  const share = candidate.durationMs / Math.max(totalMs, 1);
  const countWeight = Number.isFinite(candidate.countThreshold) && candidate.countThreshold > 0
    ? Math.min(candidate.count / candidate.countThreshold, 2) * 0.1
    : 0;
  return share + (candidate.durationMs / 10_000) + countWeight + (candidate.errorSpan ? 1 : 0);
}

function nodePressureFinding(metrics) {
  const series = metrics.find((item) => item.name === 'node.psi.io.some');
  const peak = maxPoint(series);
  if (!peak || peak.value < 0.35) return undefined;

  return {
    id: 'node-io-pressure',
    severity: peak.value > 0.7 ? 'critical' : 'warning',
    category: 'node',
    title: 'Node IO pressure overlaps sandbox startup',
    summary: `Node IO PSI peaked at ${formatRatio(peak.value)} during the sampled window.`,
    evidence: [
      `node.psi.io.some peak=${formatRatio(peak.value)}`,
      `peak timestamp=${peak.timestamp}`,
    ],
    recommendedActions: [
      'Compare concurrent sandbox starts on the same node during this time range.',
      'Check block IO profiles and image remote reads before blaming runtime overhead.',
    ],
    relatedMetricNames: [series.name],
  };
}

function imageAccessFinding(image, sandbox) {
  if (!image) return undefined;

  if (image.loadingMode === 'lazy') {
    const requested = sum(image.layers, 'requestedBlockCount');
    const hit = sum(image.layers, 'cacheHitBlockCount');
    const remoteReadBytes = sum(image.layers, 'remoteReadBytes');
    const hitRatio = requested > 0 ? hit / requested : 1;
    if (hitRatio > 0.65 && remoteReadBytes < 256 * 1024 ** 2) return undefined;

    return {
      id: `${sandbox.id}-lazy-image-cache`,
      severity: hitRatio < 0.45 || remoteReadBytes > 1024 * 1024 ** 2 ? 'warning' : 'info',
      category: 'image',
      title: 'Lazy image block cache is adding remote IO',
      summary: `Block cache hit ratio is ${formatRatio(hitRatio)} with ${formatBytes(remoteReadBytes)} remote reads.`,
      evidence: [
        `requested blocks=${requested}`,
        `cache hit blocks=${hit}`,
        `remote read=${formatBytes(remoteReadBytes)}`,
      ],
      recommendedActions: [
        'Inspect the lazy image cache hit curve instead of relying on whole-layer cache summaries.',
        'Compare layer access for COPY application bundle and model/assets layers.',
      ],
    };
  }

  const downloadMs = (image.downloadTimeline ?? []).reduce((total, step) => total + step.durationMs, 0);
  if (downloadMs < Math.max(1_500, sandbox.startupDurationMs * 0.35)) return undefined;

  return {
    id: `${sandbox.id}-eager-image-download`,
    severity: downloadMs > sandbox.startupDurationMs * 0.6 ? 'warning' : 'info',
    category: 'image',
    title: 'Non-lazy image download is a startup contributor',
    summary: `Image download stages total ${formatDuration(downloadMs)} before container start.`,
    evidence: [
      `download total=${formatDuration(downloadMs)}`,
      `startup=${formatDuration(sandbox.startupDurationMs)}`,
      `image.size=${formatBytes(image.sizeBytes)}`,
    ],
    recommendedActions: [
      'Use the stacked image download chart to identify the longest pull/verify/unpack stage.',
      'Compare with a warm start for the same image digest.',
    ],
  };
}

function eventFinding(events) {
  const error = events.find((event) => event.severity === 'error');
  const warnings = events.filter((event) => event.severity === 'warning');
  const target = error ?? warnings[0];
  if (!target) return undefined;

  return {
    id: `event-${target.id}`,
    severity: error ? 'critical' : 'warning',
    category: target.eventType === 'node' ? 'node' : 'runtime',
    title: error ? 'Error event observed during lifecycle' : 'Warning event observed during lifecycle',
    summary: target.message,
    evidence: [
      `${target.eventName} at ${target.timestamp}`,
      target.reason ? `reason=${target.reason}` : undefined,
      warnings.length > 1 ? `${warnings.length} warning events in this window` : undefined,
    ].filter(Boolean),
    recommendedActions: [
      'Jump from the timeline event into metrics around the same timestamp.',
      'Review trace spans that overlap this event before changing runtime configuration.',
    ],
    relatedEventIds: [target.id],
  };
}

function runtimeCpuFinding(sandbox, metrics) {
  const series = metrics.find((item) => item.name === 'runtime.process.cpu.usage_ratio');
  const peak = maxPoint(series);
  if (!series || !peak || peak.value < 0.22) return undefined;

  return {
    id: `${sandbox.id}-runtime-cpu`,
    severity: peak.value > 0.45 ? 'warning' : 'info',
    category: 'runtime',
    title: 'Runtime process CPU is elevated',
    summary: `${sandbox.runtimeType} runtime process CPU peaked at ${formatRatio(peak.value)}.`,
    evidence: [
      `${series.name} peak=${formatRatio(peak.value)}`,
      `runtime=${sandbox.runtimeType}`,
    ],
    recommendedActions: [
      'Open CPU profile artifacts for runtime-side process roles.',
      'Compare runtime overhead against the same image on runc if available.',
    ],
    relatedMetricNames: [series.name],
  };
}

function profileFinding(profiles) {
  const profile = [...profiles].sort((left, right) => right.sampleCount - left.sampleCount)[0];
  if (!profile || profile.sampleCount < 10_000) return undefined;

  return {
    id: `profile-${profile.id}`,
    severity: profile.profileType === 'block_io' ? 'warning' : 'info',
    category: profile.profileType === 'block_io' ? 'node' : 'runtime',
    title: 'Profile artifact has enough samples for drill-down',
    summary: `${profile.processRole} ${profile.profileType} profile contains ${profile.sampleCount.toLocaleString()} samples.`,
    evidence: [
      `profile=${profile.id}`,
      `type=${profile.profileType}`,
      `duration=${formatDuration(profile.durationMs)}`,
    ],
    recommendedActions: [
      'Open the profile flame graph modal and inspect the widest frames.',
      'Correlate profile capture time with metrics and lifecycle trace stages.',
    ],
  };
}

function dominantSpan(spans) {
  return [...spans].sort((left, right) => right.durationMs - left.durationMs)[0];
}

function maxPoint(series) {
  if (!series || series.points.length === 0) return undefined;
  return [...series.points].sort((left, right) => right.value - left.value)[0];
}

function maxMetricValue(metrics, name) {
  if (!name) return undefined;
  const values = metrics
    .filter((series) => series.name === name)
    .flatMap((series) => series.points ?? [])
    .map((point) => Number(point.value))
    .filter((value) => Number.isFinite(value));
  if (values.length === 0) return undefined;
  return Math.max(...values);
}

function sum(rows = [], key) {
  return rows.reduce((total, row) => total + Number(row[key] ?? 0), 0);
}

function formatDuration(ms) {
  if (ms >= 1000) return `${(ms / 1000).toFixed(ms >= 10_000 ? 1 : 2)}s`;
  return `${Math.round(ms)}ms`;
}

function formatBytes(bytes) {
  if (bytes >= 1024 ** 3) return `${(bytes / 1024 ** 3).toFixed(2)}GiB`;
  if (bytes >= 1024 ** 2) return `${(bytes / 1024 ** 2).toFixed(1)}MiB`;
  if (bytes >= 1024) return `${(bytes / 1024).toFixed(1)}KiB`;
  return `${Math.round(bytes)}B`;
}

function formatRatio(value) {
  return `${Math.round(value * 100)}%`;
}
