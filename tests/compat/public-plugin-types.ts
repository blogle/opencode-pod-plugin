import type {
  Plugin,
  WorkspaceAdapter,
  WorkspaceInfo,
  WorkspaceTarget,
} from "@opencode-ai/plugin";

const adapter = {
  name: "Kubernetes Sandbox",
  description: "Compatibility fixture",
  configure(info: WorkspaceInfo): WorkspaceInfo {
    return info;
  },
  async create(_info: WorkspaceInfo, _env: Record<string, string | undefined>): Promise<void> {},
  async remove(_info: WorkspaceInfo): Promise<void> {},
  target(_info: WorkspaceInfo): WorkspaceTarget {
    return { type: "remote", url: "http://workspace.test", headers: { Authorization: "Basic fixture" } };
  },
} satisfies WorkspaceAdapter;

export const compatibilityPlugin = (async ({ experimental_workspace }) => {
  experimental_workspace.register("kubernetes", adapter);
  return {};
}) satisfies Plugin;
