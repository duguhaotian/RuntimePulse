import type { RuntimeType, Severity } from '../domain/model';

export const runtimeColors: Record<RuntimeType, string> = {
  runc: '#3b82f6',
  gvisor: '#8b5cf6',
  kata: '#f97316',
  firecracker: '#ef4444',
};

export const severityColors: Record<Severity, string> = {
  debug: '#94a3b8',
  info: '#38bdf8',
  warning: '#f59e0b',
  error: '#ef4444',
  critical: '#dc2626',
};

export const chartPalette = ['#38bdf8', '#a78bfa', '#fb923c', '#f87171', '#34d399', '#facc15'];
