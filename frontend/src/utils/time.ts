export function minutesAgo(minutes: number, now = new Date('2026-05-09T04:00:00.000Z')): string {
  return new Date(now.getTime() - minutes * 60_000).toISOString();
}

export function formatTime(value: string): string {
  return new Intl.DateTimeFormat('zh-CN', {
    hour: '2-digit',
    minute: '2-digit',
    second: '2-digit',
    hour12: false,
  }).format(new Date(value));
}

export function formatDateTime(value: string): string {
  return new Intl.DateTimeFormat('zh-CN', {
    month: '2-digit',
    day: '2-digit',
    hour: '2-digit',
    minute: '2-digit',
    second: '2-digit',
    hour12: false,
  }).format(new Date(value));
}

export function toMs(value: string): number {
  return new Date(value).getTime();
}
