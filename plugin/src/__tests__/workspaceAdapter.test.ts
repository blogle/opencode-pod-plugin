import type { WorkspaceInfo } from "@opencode-ai/plugin";
import { describe, expect, it, vi } from "vitest";
import { kubernetesAdapter } from "../adapter.js";
import { GatewayClient } from "../client.js";

const workspace: WorkspaceInfo = {
  id: "wrk_123",
  type: "kubernetes",
  name: "fixture",
  branch: "main",
  directory: null,
  projectID: "project_1",
  extra: { projectKey: "fixture", owner: "dev@example.test" },
};

describe("pinned upstream WorkspaceAdapter contract", () => {
  it("configures, creates, targets and removes a native remote workspace", async () => {
    const body = JSON.stringify({
      workspaceId: "wrk_123",
      state: "running",
      target: {
        url: "http://workspace-wrk-123.opencode-sandboxes.svc.cluster.local:4096",
        username: "opencode",
        password: "secret",
      },
    });
    const request = vi
      .fn<Parameters<typeof fetch>, ReturnType<typeof fetch>>()
      .mockResolvedValueOnce(new Response(body))
      .mockResolvedValueOnce(new Response(body))
      .mockResolvedValueOnce(new Response(null, { status: 204 }));
    const adapter = kubernetesAdapter(new GatewayClient("http://gateway.test", request));

    const configured = await adapter.configure(workspace);
    await adapter.create(configured, { OPENCODE_AUTH_CONTENT: "{}" });
    const target = await adapter.target(configured);
    await adapter.remove(configured);

    expect(request).toHaveBeenCalledTimes(3);
    expect(target).toEqual({
      type: "remote",
      url: "http://workspace-wrk-123.opencode-sandboxes.svc.cluster.local:4096",
      headers: { Authorization: "Basic b3BlbmNvZGU6c2VjcmV0" },
    });
    expect(JSON.parse(String(request.mock.calls[0]?.[1]?.body))).toMatchObject({
      workspaceId: "wrk_123",
      projectKey: "fixture",
      upstreamEnvironment: { OPENCODE_AUTH_CONTENT: "{}" },
    });
  });

  it("is idempotent when gateway deletion reports missing", async () => {
    const request = vi
      .fn<Parameters<typeof fetch>, ReturnType<typeof fetch>>()
      .mockResolvedValue(new Response(null, { status: 404 }));
    await kubernetesAdapter(new GatewayClient("http://gateway.test", request)).remove(workspace);
  });

  it("rejects workspaces without centrally selected projects", () => {
    const adapter = kubernetesAdapter(new GatewayClient("http://gateway.test"));
    expect(() => adapter.configure({ ...workspace, extra: null })).toThrow("extra.projectKey");
  });

  it("does not override any stock OpenCode tool or lifecycle event", () => {
    const adapter = kubernetesAdapter(new GatewayClient("http://gateway.test"));
    expect(adapter).not.toHaveProperty("tool");
    expect(adapter).not.toHaveProperty("event");
  });
});
