import type { RuntimePulseApi } from './RuntimePulseApi';
import type { SandboxQuery } from '../domain/model';
import {
  eventsForSandbox,
  images,
  metricsForSandbox,
  nodes,
  profilesForSandbox,
  runtimeComparison,
  sandboxes,
  traceForSandbox,
} from '../mock/data';

function filterSandboxes(query?: SandboxQuery) {
  return sandboxes.filter((sandbox) => {
    const runtimeMatch = !query?.runtimeType || query.runtimeType === 'all' || sandbox.runtimeType === query.runtimeType;
    const statusMatch = !query?.status || query.status === 'all' || sandbox.status === query.status;
    const text = query?.text?.trim().toLowerCase();
    const textMatch = !text || [sandbox.id, sandbox.nodeId, sandbox.workloadName, sandbox.imageRef, sandbox.namespace].some((value) => value.toLowerCase().includes(text));
    return runtimeMatch && statusMatch && textMatch;
  });
}

export const mockRuntimePulseApi: RuntimePulseApi = {
  async listSandboxes(query) {
    return filterSandboxes(query);
  },
  async getSandbox(id) {
    return sandboxes.find((sandbox) => sandbox.id === id);
  },
  async getNode(id) {
    return nodes.find((node) => node.id === id);
  },
  async getImage(id) {
    return images.find((image) => image.id === id);
  },
  async getSandboxMetrics(id) {
    return metricsForSandbox(id);
  },
  async getSandboxEvents(id) {
    return eventsForSandbox(id);
  },
  async getSandboxTrace(id) {
    return traceForSandbox(id);
  },
  async getSandboxProfiles(id) {
    return profilesForSandbox(id);
  },
  async compareRuntimes() {
    return runtimeComparison;
  },
};
