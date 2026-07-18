import type { WorkspaceAdapter, WorkspaceInfo } from "@opencode-ai/plugin";
import type { GatewayClient } from "./client.js";

export interface KubernetesWorkspaceExtra {
  projectKey: string;
  gitRef?: string;
  owner?: string;
  runtimeOverrides?: Record<string, unknown>;
}

function metadata(value: unknown): KubernetesWorkspaceExtra {
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    throw new Error("kubernetes workspace requires extra.projectKey");
  }
  const extra = value as Record<string, unknown>;
  if (typeof extra.projectKey !== "string" || !extra.projectKey) {
    throw new Error("kubernetes workspace requires extra.projectKey");
  }
  if (extra.gitRef !== undefined && typeof extra.gitRef !== "string") {
    throw new Error("kubernetes workspace extra.gitRef must be a string");
  }
  if (extra.owner !== undefined && typeof extra.owner !== "string") {
    throw new Error("kubernetes workspace extra.owner must be a string");
  }
  if (
    extra.runtimeOverrides !== undefined &&
    (!extra.runtimeOverrides || typeof extra.runtimeOverrides !== "object" || Array.isArray(extra.runtimeOverrides))
  ) {
    throw new Error("kubernetes workspace extra.runtimeOverrides must be an object");
  }
  return extra as unknown as KubernetesWorkspaceExtra;
}

/** Registers only native workspace routing; stock OpenCode owns all workspace tools. */
export function kubernetesAdapter(gateway: GatewayClient): WorkspaceAdapter {
  return {
    name: "Kubernetes Sandbox",
    description: "Disposable Kubernetes sandbox backed by the workspace gateway",
    configure(info: WorkspaceInfo): WorkspaceInfo {
      const extra = metadata(info.extra);
      return {
        ...info,
        type: "kubernetes",
        branch: extra.gitRef ?? info.branch,
        extra: {
          projectKey: extra.projectKey,
          gitRef: extra.gitRef ?? info.branch ?? undefined,
          ...(extra.owner ? { owner: extra.owner } : {}),
          ...(extra.runtimeOverrides ? { runtimeOverrides: extra.runtimeOverrides } : {}),
        },
      };
    },
    async create(info, environment): Promise<void> {
      const extra = metadata(info.extra);
      await gateway.createWorkspace({
        workspaceId: info.id,
        projectKey: extra.projectKey,
        gitRef: extra.gitRef ?? info.branch,
        owner: extra.owner,
        runtimeOverrides: extra.runtimeOverrides,
        upstreamEnvironment: environment,
      });
    },
    async remove(info): Promise<void> {
      await gateway.removeWorkspace(info.id);
    },
    async target(info) {
      const workspace = await gateway.getWorkspace(info.id);
      const basic = Buffer.from(`${workspace.target.username}:${workspace.target.password}`).toString("base64");
      return {
        type: "remote" as const,
        url: workspace.target.url,
        headers: { Authorization: `Basic ${basic}` },
      };
    },
  };
}
