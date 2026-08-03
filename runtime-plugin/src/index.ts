import { tool, type Plugin } from "@opencode-ai/plugin";
import { loadRuntimeConfig } from "./config.js";
import {
  EnvironmentTracker,
  evaluateEnvironment,
  type DirenvRunner,
  type FileReader,
} from "./environment.js";
import { LifecycleCoordinator, RuntimeClient, type Log } from "./lifecycle.js";

export { loadRuntimeConfig, RUNTIME_ENV, type RuntimeConfig } from "./config.js";
export * from "./environment.js";
export * from "./lifecycle.js";

export interface RuntimePluginDependencies {
  env?: NodeJS.ProcessEnv;
  fetch?: typeof fetch;
  runDirenv?: DirenvRunner;
  readFile?: FileReader;
  log?: Log;
  setInterval?: typeof setInterval;
}

function defaultLog(record: Record<string, unknown>): void {
  console.error(JSON.stringify(record));
}

export function createRuntimePlugin(dependencies: RuntimePluginDependencies = {}): Plugin {
  return async ({ worktree }) => {
    const log = dependencies.log ?? defaultLog;
    const safeLog: Log = (record) => {
      try {
        log(record);
      } catch {
        // Observability must never prevent plugin initialization or shell hooks.
      }
    };
    const config = loadRuntimeConfig(dependencies.env);
    const client = new RuntimeClient(config, dependencies.fetch);
    const lifecycle = new LifecycleCoordinator(client, safeLog);
    const environment = new EnvironmentTracker(worktree, dependencies.readFile);
    let latestSessionId: string | undefined;
    let periodicRunning = false;
    safeLog({
      level: "info",
      component: "opencode-runtime-plugin",
      operation: "loaded",
      workspaceId: config.workspaceId,
    });

    const refreshFingerprint = async () => {
      const result = await environment.refresh();
      if (result.changed) {
        lifecycle.markEnvironmentDirty();
        safeLog({
          level: "info",
          component: "opencode-runtime-plugin",
          operation: "environment-changed",
          fingerprint: result.fingerprint.hash,
        });
      }
      return result.fingerprint;
    };
    const periodicCheckpoint = async () => {
      if (periodicRunning || !latestSessionId) return;
      periodicRunning = true;
      try {
        await lifecycle.checkpointIfDirty(latestSessionId, await refreshFingerprint());
      } finally {
        periodicRunning = false;
      }
    };
    const periodicTimer = (dependencies.setInterval ?? setInterval)(
      () => void periodicCheckpoint(),
      config.checkpointIntervalSeconds * 1000,
    );
    if (typeof periodicTimer === "object") periodicTimer.unref();

    return {
      "shell.env": async ({ cwd }, output) => {
        const delta = await evaluateEnvironment(
          cwd,
          config.direnvPath,
          dependencies.runDirenv,
        );
        for (const [key, value] of Object.entries(delta)) {
          if (value === null) delete output.env[key];
          else output.env[key] = value;
        }
        await refreshFingerprint();
      },

      "tool.execute.before": async ({ tool: toolName, sessionID }) => {
        latestSessionId = sessionID;
        lifecycle.markDirty();
        await lifecycle.activity("tool", sessionID, toolName);
      },

      event: async ({ event }) => {
        if (event.type === "message.updated") {
          latestSessionId = event.properties.info.sessionID;
          await lifecycle.activity("message", event.properties.info.sessionID);
          return;
        }
        if (event.type === "file.edited") {
          lifecycle.markDirty();
          return;
        }
        if (event.type === "session.idle") {
          latestSessionId = event.properties.sessionID;
          const fingerprint = await refreshFingerprint();
          await lifecycle.idle(event.properties.sessionID, fingerprint);
        }
      },

      tool: {
        preview: tool({
          description: "Return the HTTPS preview URL for a listening workspace port",
          args: {
            port: tool.schema.number().int().min(1).max(65535),
          },
          async execute({ port }) {
            if (port === 4096 || port === 4097 || port === 4098) {
              throw new Error(`Port ${port} is reserved for workspace control services`);
            }
            return `https://${config.previewKey}-${port}.${config.baseDomain}`;
          },
        }),
      },
    };
  };
}

export const RuntimePlugin = createRuntimePlugin();
export default RuntimePlugin;
