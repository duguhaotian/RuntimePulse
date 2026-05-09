export function formatDuration(ms: number): string {
  if (ms < 1000) return `${Math.round(ms)} ms`;
  return `${(ms / 1000).toFixed(ms > 10_000 ? 1 : 2)} s`;
}

export function formatBytes(bytes: number): string {
  const units = ['B', 'KiB', 'MiB', 'GiB', 'TiB'];
  let value = bytes;
  let index = 0;
  while (value >= 1024 && index < units.length - 1) {
    value /= 1024;
    index += 1;
  }
  return `${value.toFixed(index === 0 ? 0 : 1)} ${units[index]}`;
}

export function formatRatio(value: number): string {
  return `${(value * 100).toFixed(1)}%`;
}

export function formatMetricValue(value: number, unit: string): string {
  if (unit === 'bytes') return formatBytes(value);
  if (unit === 'ratio') return formatRatio(value);
  if (unit === 'ms') return formatDuration(value);
  if (unit === 'bytes/s') return `${formatBytes(value)}/s`;
  return `${value.toFixed(2)} ${unit}`.trim();
}
