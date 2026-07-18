import { describe, expect, it, vi } from "vitest";
import type { RuntimeConfig } from "../src/config.js";
import type { EnvironmentFingerprint } from "../src/environment.js";
import { LifecycleCoordinator, RuntimeClient, redact } from "../src/lifecycle.js";

const config: RuntimeConfig = {
  gatewayUrl: "http://gateway.internal",
  gatewayToken: "very-secret-token",
  workspaceId: "wrk_test",
  baseDomain: "preview.test",
  checkpointEndpoint: "http://127.0.0.1:4098",
  supervisorEndpoint: "http://127.0.0.1:4097",
  direnvPath: "/opt/opencode/bin/direnv",
};
const fingerprint: EnvironmentFingerprint = {
  hash: "fingerprint",
  files: { ".envrc": "env", "flake.nix": null, "flake.lock": null },
};

describe("runtime lifecycle", () => {
  it("reports activity with workspace authentication", async () => {
    const fetcher = vi.fn<typeof fetch>().mockResolvedValue(new Response(null, { status: 204 }));
    const client = new RuntimeClient(config, fetcher);
    await client.reportActivity("tool", "ses_1", "bash");

    expect(fetcher).toHaveBeenCalledOnce();
    const [url, init] = fetcher.mock.calls[0];
    expect(url).toBe("http://gateway.internal/v1/workspaces/wrk_test/activity");
    expect(new Headers(init?.headers).get("authorization")).toBe("Bearer very-secret-token");
    expect(JSON.parse(String(init?.body))).toMatchObject({
      kind: "tool",
      sessionId: "ses_1",
      tool: "bash",
    });
  });

  it("checkpoints dirty work and restarts only for environment changes", async () => {
    const requests: Array<{ url: string; authorization: string | null }> = [];
    const fetcher = vi.fn<typeof fetch>().mockImplementation(async (url, init) => {
      requests.push({
        url: String(url),
        authorization: new Headers(init?.headers).get("authorization"),
      });
      return new Response(null, { status: 204 });
    });
    const urls = () => requests.map((request) => request.url);
    const lifecycle = new LifecycleCoordinator(new RuntimeClient(config, fetcher), vi.fn());

    lifecycle.markDirty();
    await lifecycle.idle("ses_1", fingerprint);
    expect(urls()).toEqual([
      "http://gateway.internal/v1/workspaces/wrk_test/activity",
      "http://127.0.0.1:4098/checkpoint",
    ]);

    requests.length = 0;
    lifecycle.markEnvironmentDirty();
    await lifecycle.idle("ses_1", fingerprint);
    expect(urls()).toEqual([
      "http://gateway.internal/v1/workspaces/wrk_test/activity",
      "http://127.0.0.1:4098/checkpoint",
      "http://127.0.0.1:4097/restart",
    ]);
    expect(requests.every((request) => request.authorization === "Bearer very-secret-token")).toBe(
      true,
    );
  });

  it("contains activity fetch and logger failures", async () => {
    const fetcher = vi.fn<typeof fetch>().mockRejectedValue(new Error("network unavailable"));
    const lifecycle = new LifecycleCoordinator(new RuntimeClient(config, fetcher), () => {
      throw new Error("logger unavailable");
    });

    await expect(lifecycle.activity("tool", "ses_1", "bash")).resolves.toBeUndefined();
  });

  it("does not restart or clear dirty state when checkpointing fails", async () => {
    const urls: string[] = [];
    let checkpointFails = true;
    const fetcher = vi.fn<typeof fetch>().mockImplementation(async (url) => {
      urls.push(String(url));
      if (String(url).endsWith("/checkpoint") && checkpointFails) {
        return new Response("token=very-secret-token", { status: 500 });
      }
      return new Response(null, { status: 204 });
    });
    const logs: Record<string, unknown>[] = [];
    const lifecycle = new LifecycleCoordinator(
      new RuntimeClient(config, fetcher),
      (record) => logs.push(record),
    );
    lifecycle.markEnvironmentDirty();

    await lifecycle.idle("ses_1", fingerprint);
    expect(urls.some((url) => url.endsWith("/restart"))).toBe(false);
    expect(JSON.stringify(logs)).not.toContain("very-secret-token");

    checkpointFails = false;
    urls.length = 0;
    await lifecycle.idle("ses_1", fingerprint);
    expect(urls.some((url) => url.endsWith("/checkpoint"))).toBe(true);
    expect(urls.some((url) => url.endsWith("/restart"))).toBe(true);
  });

  it("retries a failed restart without losing the environment-dirty state", async () => {
    const urls: string[] = [];
    let restartFails = true;
    const fetcher = vi.fn<typeof fetch>().mockImplementation(async (url) => {
      urls.push(String(url));
      if (String(url).endsWith("/restart") && restartFails) {
        return new Response(null, { status: 503 });
      }
      return new Response(null, { status: 204 });
    });
    const lifecycle = new LifecycleCoordinator(new RuntimeClient(config, fetcher), vi.fn());
    lifecycle.markEnvironmentDirty();
    await lifecycle.idle("ses_1", fingerprint);

    restartFails = false;
    urls.length = 0;
    await lifecycle.idle("ses_1", fingerprint);
    expect(urls).toEqual([
      "http://gateway.internal/v1/workspaces/wrk_test/activity",
      "http://127.0.0.1:4097/restart",
    ]);
  });

  it("redacts authorization and explicit secrets", () => {
    expect(redact("Authorization: Bearer abc token=secret", ["secret"])).toBe(
      "Authorization: [REDACTED] token=[REDACTED]",
    );
  });
});
