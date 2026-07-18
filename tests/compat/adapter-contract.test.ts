import type { WorkspaceAdapter, WorkspaceInfo } from "@opencode-ai/plugin";
import { KubernetesSandboxPlugin } from "../../plugin/src/central.js";

const workspace: WorkspaceInfo = {
  id: "wrk_compat",
  type: "kubernetes",
  name: "compatibility",
  branch: "main",
  directory: null,
  extra: { projectKey: "demo", owner: "developer@example.test" },
  projectID: "project_compat",
};

describe("OpenCode 1.18.3 public workspace contract", () => {
  it("registers and executes configure/create/target/remove", async () => {
    const response = {
      workspaceId: workspace.id,
      state: "running",
      target: {
        url: "http://workspace-wrk-compat.sandboxes.svc.cluster.local:4096",
        username: "opencode",
        password: "compat-secret",
      },
    };
    const request = vi
      .fn<typeof fetch>()
      .mockResolvedValueOnce(new Response(JSON.stringify(response)))
      .mockResolvedValueOnce(new Response(JSON.stringify(response)))
      .mockResolvedValueOnce(new Response(null, { status: 204 }));
    vi.stubGlobal("fetch", request);

    let type = "";
    let adapter: WorkspaceAdapter | undefined;
    await KubernetesSandboxPlugin(
      {
        experimental_workspace: {
          register(registeredType, registeredAdapter) {
            type = registeredType;
            adapter = registeredAdapter;
          },
        },
      } as never,
      { gatewayUrl: "http://gateway.test", gatewayToken: "gateway-token" },
    );

    expect(type).toBe("kubernetes");
    expect(adapter).toBeDefined();
    const configured = await adapter!.configure(workspace);
    await adapter!.create(configured, { OPENCODE_AUTH_CONTENT: "{}" });
    await expect(adapter!.target(configured)).resolves.toEqual({
      type: "remote",
      url: response.target.url,
      headers: { Authorization: "Basic b3BlbmNvZGU6Y29tcGF0LXNlY3JldA==" },
    });
    await adapter!.remove(configured);

    expect(request).toHaveBeenCalledTimes(3);
    expect(request.mock.calls.map(([url, init]) => [String(url), init?.method ?? "GET"])).toEqual([
      ["http://gateway.test/v1/workspaces", "POST"],
      ["http://gateway.test/v1/workspaces/wrk_compat", "GET"],
      ["http://gateway.test/v1/workspaces/wrk_compat", "DELETE"],
    ]);
    expect(new Headers(request.mock.calls[0]?.[1]?.headers).get("authorization")).toBe("Bearer gateway-token");
  });
});
