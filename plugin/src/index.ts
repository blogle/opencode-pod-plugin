import { loadConfig, Config } from "./config.js";
import { SessionStore, SandboxRecord } from "./sessionStore.js";
import {
  sessionCreated,
  sessionDeleted,
  setupIdleSweep,
  getSystemPromptTransform,
  reconcileExistingPods,
} from "./hooks.js";
import { bashOverride } from "./tools/bash.js";
import { readFile, writeFile, editFile } from "./tools/fileOps.js";
import {
  lsOverride,
  globOverride,
  grepOverride,
  previewLinkOverride,
  multieditOverride,
  applyPatchOverride,
} from "./tools/overrides.js";
import { cloneRepos, RepoMapping, findRepoForDirectory } from "./repos.js";

export interface PluginContext {
  config: Config;
  sessionStore: SessionStore;
  repoMapping: Map<string, RepoMapping>;
}

export interface OpenCodeContext {
  project: { name: string; path: string; config: Record<string, unknown> };
  client: {
    path: { get: () => Promise<{ data?: { path: string } }> };
    app: { log: (entry: { body: { service: string; level: string; message: string } }) => Promise<void> };
  };
  $: { raw: (strings: TemplateStringsArray, ...values: unknown[]) => Promise<string> };
  directory: string;
  worktree: string;
}

interface ToolContext {
  sessionID: string;
  messageID?: string;
  agent?: string;
  abort?: AbortSignal;
  directory: string;
  worktree: string;
}

interface PluginEventEnvelope {
  event: {
    type: string;
    sessionID?: string;
    sessionId?: string;
    properties?: {
      sessionID?: string;
      sessionId?: string;
      info?: {
        sessionID?: string;
        sessionId?: string;
      };
    };
  };
}

/**
 * Opencode plugin entry point.
 * Returns hooks and tools at top level per current opencode plugin API.
 * Tools with the same name as built-in tools take precedence.
 */
export const K8sSandboxPlugin = async (
  ctx: OpenCodeContext,
  options?: Record<string, unknown>
) => {
  const { project, client, $, directory, worktree } = ctx;
  const config = loadConfig(options ?? {});
  const sessionStore = new SessionStore();

  const shell = (strings: TemplateStringsArray, ...values: unknown[]) => {
    const fn = $.raw ?? ($ as unknown as (strings: TemplateStringsArray, ...values: unknown[]) => Promise<string>);
    return fn(strings, ...values);
  };

  const repoMapping = await cloneRepos(config, worktree, shell);

  const pluginCtx: PluginContext = {
    config,
    sessionStore,
    repoMapping,
  };
  const sandboxCreation = new Map<string, Promise<void>>();

  await reconcileExistingPods(config, sessionStore);

  const idleSweepInterval = setupIdleSweep(config, sessionStore);

  function getRecord(sessionID: string): SandboxRecord {
    const record = sessionStore.get(sessionID);
    if (!record) {
      throw new Error("No sandbox found for this session. Create a session first.");
    }
    return record;
  }

  function getEventSessionID(event: PluginEventEnvelope["event"]): string | undefined {
    return (
      event.sessionID ||
      event.sessionId ||
      event.properties?.sessionID ||
      event.properties?.sessionId ||
      event.properties?.info?.sessionID ||
      event.properties?.info?.sessionId
    );
  }

  async function ensureSandbox(sessionID: string): Promise<void> {
    if (sessionStore.has(sessionID)) {
      return;
    }

    const existing = sandboxCreation.get(sessionID);
    if (existing) {
      await existing;
      return;
    }

    const creation = (async () => {
      const pathResponse = await client.path.get();
      const currentDir = pathResponse.data?.path || directory;
      const repo = findRepoForDirectory(currentDir, repoMapping);

      await sessionCreated({
        sessionId: sessionID,
        ...pluginCtx,
        repoUrl: repo?.url,
      });
    })();

    sandboxCreation.set(sessionID, creation);

    try {
      await creation;
    } finally {
      sandboxCreation.delete(sessionID);
    }
  }

  return {
    event: async ({ event }: PluginEventEnvelope) => {
      console.log("[plugin] event", event.type);

      if (event.type === "session.created") {
        const sessionID = getEventSessionID(event);
        if (!sessionID) {
          throw new Error("session.created event missing sessionID");
        }
        await ensureSandbox(sessionID);
        return;
      }

      if (event.type === "session.deleted") {
        const sessionID = getEventSessionID(event);
        if (!sessionID) {
          return;
        }
        await sessionDeleted({
          sessionId: sessionID,
          ...pluginCtx,
        });
        return;
      }
    },

    "tool.execute.before": async (
      input: { tool: string; sessionID: string },
      output: { args: Record<string, unknown> }
    ) => {
      sessionStore.updateLastActive(input.sessionID);
    },

    "experimental.chat.system.transform": getSystemPromptTransform(config, sessionStore),

    // --- Tools (top level, not nested under "tools") ---

    tool: {
      bash: {
        description: "Execute a shell command inside the sandbox pod",
        args: {
          command: { type: "string", description: "The command to execute" },
        },
        execute: async (args: { command: string }, toolCtx: ToolContext) => {
          const record = getRecord(toolCtx.sessionID);
          return bashOverride(
            { podName: record.podName, namespace: config.namespace },
            args.command
          );
        },
      },

      read: {
        description: "Read a file from the sandbox pod",
        args: {
          filePath: { type: "string", description: "Absolute path to the file" },
        },
        execute: async (args: { filePath: string }, toolCtx: ToolContext) => {
          const record = getRecord(toolCtx.sessionID);
          return readFile(
            { podName: record.podName, namespace: config.namespace },
            args.filePath
          );
        },
      },

      write: {
        description: "Write content to a file in the sandbox pod",
        args: {
          filePath: { type: "string", description: "Absolute path to the file" },
          content: { type: "string", description: "Content to write" },
        },
        execute: async (
          args: { filePath: string; content: string },
          toolCtx: ToolContext
        ) => {
          const record = getRecord(toolCtx.sessionID);
          await writeFile(
            { podName: record.podName, namespace: config.namespace },
            args.filePath,
            args.content
          );
        },
      },

      edit: {
        description: "Edit a file in the sandbox pod using string replacement",
        args: {
          filePath: { type: "string", description: "Absolute path to the file" },
          oldString: { type: "string", description: "Exact string to replace" },
          newString: { type: "string", description: "Replacement string" },
        },
        execute: async (
          args: { filePath: string; oldString: string; newString: string },
          toolCtx: ToolContext
        ) => {
          const record = getRecord(toolCtx.sessionID);
          await editFile(
            { podName: record.podName, namespace: config.namespace },
            args.filePath,
            args.oldString,
            args.newString
          );
        },
      },

      list: {
        description: "List files in a directory on the sandbox pod",
        args: {
          path: { type: "string", description: "Directory path" },
        },
        execute: async (args: { path: string }, toolCtx: ToolContext) => {
          const record = getRecord(toolCtx.sessionID);
          return lsOverride(
            { podName: record.podName, namespace: config.namespace },
            args.path
          );
        },
      },

      glob: {
        description: "Find files by pattern on the sandbox pod",
        args: {
          pattern: { type: "string", description: "Glob pattern" },
        },
        execute: async (args: { pattern: string }, toolCtx: ToolContext) => {
          const record = getRecord(toolCtx.sessionID);
          return globOverride(
            { podName: record.podName, namespace: config.namespace },
            args.pattern
          );
        },
      },

      grep: {
        description: "Search file contents on the sandbox pod",
        args: {
          pattern: { type: "string", description: "Regex pattern" },
          path: { type: "string", description: "Directory to search in" },
        },
        execute: async (
          args: { pattern: string; path: string },
          toolCtx: ToolContext
        ) => {
          const record = getRecord(toolCtx.sessionID);
          return grepOverride(
            { podName: record.podName, namespace: config.namespace },
            args.pattern,
            args.path
          );
        },
      },

      "preview-link": {
        description: "Get a URL to access a service running in the sandbox",
        args: {
          port: { type: "number", description: "Port number" },
        },
        execute: async (args: { port: number }, toolCtx: ToolContext) => {
          const record = getRecord(toolCtx.sessionID);
          return previewLinkOverride(
            record.sandboxId,
            config.baseDomain,
            args.port
          );
        },
      },

      multiedit: {
        description: "Apply multiple edits to files in the sandbox pod",
        args: {
          operations: {
            type: "array",
            items: {
              type: "object",
              properties: {
                path: { type: "string" },
                oldString: { type: "string" },
                newString: { type: "string" },
              },
            },
            description: "List of edit operations",
          },
        },
        execute: async (
          args: {
            operations: Array<{
              path: string;
              oldString: string;
              newString: string;
            }>;
          },
          toolCtx: ToolContext
        ) => {
          const record = getRecord(toolCtx.sessionID);
          await multieditOverride(
            { podName: record.podName, namespace: config.namespace },
            args.operations
          );
        },
      },

      apply_patch: {
        description: "Apply a patch to files in the sandbox pod",
        args: {
          patchText: { type: "string", description: "Patch content" },
        },
        execute: async (args: { patchText: string }, toolCtx: ToolContext) => {
          const record = getRecord(toolCtx.sessionID);
          return applyPatchOverride(
            { podName: record.podName, namespace: config.namespace },
            args.patchText
          );
        },
      },
    },

    // --- Cleanup ---

    cleanup: () => {
      clearInterval(idleSweepInterval);
    },

    sessionStore,
    repoMapping,
  };
};

// Legacy factory for backward compat with tests
export function createPlugin(pluginConfig: Record<string, unknown>) {
  const config = loadConfig(pluginConfig);
  const sessionStore = new SessionStore();
  const idleSweepInterval = setupIdleSweep(config, sessionStore);

  return {
    event: async ({ event }: PluginEventEnvelope) => {
      const sessionID =
        event.sessionID ||
        event.sessionId ||
        event.properties?.sessionID ||
        event.properties?.sessionId ||
        event.properties?.info?.sessionID ||
        event.properties?.info?.sessionId;

      if (!sessionID) {
        return;
      }

      if (event.type === "session.created") {
        await sessionCreated({
          sessionId: sessionID,
          config,
          sessionStore,
        });
        return;
      }

      if (event.type === "session.deleted") {
        await sessionDeleted({
          sessionId: sessionID,
          config,
          sessionStore,
        });
      }
    },
    "tool.execute.before": async (
      input: { tool: string; sessionID: string },
      output: { args: Record<string, unknown> }
    ) => {
      sessionStore.updateLastActive(input.sessionID);
    },
    "experimental.chat.system.transform": getSystemPromptTransform(config, sessionStore),
    cleanup: () => {
      clearInterval(idleSweepInterval);
    },
    sessionStore,
  };
}

export { Config } from "./config.js";
export { SessionStore, SandboxRecord } from "./sessionStore.js";
export { RepoMapping, findRepoForDirectory } from "./repos.js";

export default K8sSandboxPlugin;
