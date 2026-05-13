export function buildSandboxAnalysis({ sandbox, image, metrics, events, spans, profiles }) {
  const findings = [
    startupDurationFinding(sandbox, spans),
    traceBottleneckFinding(sandbox, spans),
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
