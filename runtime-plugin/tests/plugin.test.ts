import { describe, expect, it, vi } from "vitest";
import { createRuntimePlugin } from "../src/index.js";

const env = {
  OPENCODE_GATEWAY_URL: "http://gateway.internal",
  OPENCODE_GATEWAY_TOKEN: "token",
  OPENCODE_WORKSPACE_ID: "wrk_ABC",
  OPENCODE_BASE_DOMAIN: "Preview.Test.",
  OPENCODE_CHECKPOINT_ENDPOINT: "http://127.0.0.1:4098",
  OPENCODE_SUPERVISOR_ENDPOINT: "http://127.0.0.1:4097",
  OPENCODE_DIRENV_PATH: "/injected/direnv",
};

function input() {
  return {
    worktree: "/workspace",
  } as Parameters<ReturnType<typeof createRuntimePlugin>>[0];
}

describe("runtime plugin", () => {
  it("exposes only preview and does not override built-in tools", async () => {
    const plugin = createRuntimePlugin({
      env,
      fetch: vi.fn<typeof fetch>().mockResolvedValue(new Response(null, { status: 204 })),
      readFile: async () => {
        throw Object.assign(new Error("missing"), { code: "ENOENT" });
      },
    });
    const hooks = await plugin(input());

    expect(Object.keys(hooks.tool ?? {})).toEqual(["preview"]);
    const preview = hooks.tool?.preview;
    expect(await preview?.execute({ port: 5173 }, {} as never)).toBe(
      "https://wrk_ABC-5173.preview.test",
    );
    for (const port of [4096, 4097, 4098]) {
      await expect(preview?.execute({ port }, {} as never)).rejects.toThrow("reserved");
    }
  });

  it("merges a fresh direnv export for every shell cwd and removes null keys", async () => {
    const calls: string[] = [];
    const plugin = createRuntimePlugin({
      env,
      fetch: vi.fn<typeof fetch>().mockResolvedValue(new Response(null, { status: 204 })),
      runDirenv: async (cwd, executable) => {
        calls.push(`${cwd}:${executable}`);
        return JSON.stringify({ CURRENT_CWD: cwd, REMOVE_ME: null });
      },
      readFile: async () => {
        throw Object.assign(new Error("missing"), { code: "ENOENT" });
      },
    });
    const hooks = await plugin(input());
    const first = { env: { REMOVE_ME: "old" } };
    const second = { env: {} };

    await hooks["shell.env"]?.({ cwd: "/workspace/a" }, first);
    await hooks["shell.env"]?.({ cwd: "/workspace/b" }, second);

    expect(first.env).toEqual({ CURRENT_CWD: "/workspace/a" });
    expect(second.env).toEqual({ CURRENT_CWD: "/workspace/b" });
    expect(calls).toEqual([
      "/workspace/a:/injected/direnv",
      "/workspace/b:/injected/direnv",
    ]);
  });

  it("reports message, tool, and idle activity and checkpoints at idle", async () => {
    const urls: string[] = [];
    const plugin = createRuntimePlugin({
      env,
      fetch: vi.fn<typeof fetch>().mockImplementation(async (url) => {
        urls.push(String(url));
        return new Response(null, { status: 204 });
      }),
      readFile: async () => {
        throw Object.assign(new Error("missing"), { code: "ENOENT" });
      },
    });
    const hooks = await plugin(input());

    await hooks.event?.({
      event: {
        type: "message.updated",
        properties: { info: { sessionID: "ses_1" } },
      } as never,
    });
    await hooks["tool.execute.before"]?.(
      { tool: "bash", sessionID: "ses_1", callID: "call_1" },
      { args: {} },
    );
    await hooks.event?.({
      event: { type: "session.idle", properties: { sessionID: "ses_1" } },
    });

    expect(urls.filter((url) => url.endsWith("/activity"))).toHaveLength(3);
    expect(urls.some((url) => url.endsWith("/checkpoint"))).toBe(true);
  });

  it("does not let activity outages break stock tool or event hooks", async () => {
    const plugin = createRuntimePlugin({
      env,
      fetch: vi.fn<typeof fetch>().mockRejectedValue(new Error("gateway unavailable")),
      log: () => {
        throw new Error("logger unavailable");
      },
      readFile: async () => {
        throw Object.assign(new Error("missing"), { code: "ENOENT" });
      },
    });
    const hooks = await plugin(input());

    await expect(
      hooks["tool.execute.before"]?.(
        { tool: "bash", sessionID: "ses_1", callID: "call_1" },
        { args: {} },
      ),
    ).resolves.toBeUndefined();
    await expect(
      hooks.event?.({
        event: { type: "session.idle", properties: { sessionID: "ses_1" } },
      }),
    ).resolves.toBeUndefined();
  });
});
