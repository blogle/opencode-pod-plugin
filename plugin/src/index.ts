import { tool } from "@opencode-ai/plugin";
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

  function extractSessionID(event: { type: string; properties?: Record<string, unknown> }): string | undefined {
    const props = event.properties as Record<string, unknown> | undefined;
    const info = props?.info as Record<string, unknown> | undefined;
    return (
      (props?.sessionID as string) ||
      (props?.sessionId as string) ||
      (info?.sessionID as string) ||
      (info?.sessionId as string)
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

  function k8sCtx(sessionID: string) {
    const record = getRecord(sessionID);
    return { podName: record.podName, namespace: config.namespace };
  }

  return {
    event: async (input: { event: { type: string; properties?: Record<string, unknown> } }) => {
      const event = input.event;
      console.log("[plugin] event", event.type);

      if (event.type === "session.created") {
        const sessionID = extractSessionID(event);
        if (!sessionID) {
          throw new Error("session.created event missing sessionID");
        }
        await ensureSandbox(sessionID);
        return;
      }

      if (event.type === "session.deleted") {
        const sessionID = extractSessionID(event);
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
      input: { tool: string; sessionID: string; callID: string },
      output: { args: Record<string, unknown> }
    ) => {
      sessionStore.updateLastActive(input.sessionID);
      await ensureSandbox(input.sessionID);
    },

    "experimental.chat.system.transform": getSystemPromptTransform(config, sessionStore),

    // --- Tools (top level, not nested under "tools") ---

    tool: {
      bash: tool({
        description: "Execute a shell command inside the sandbox pod",
        args: {
          command: tool.schema.string().describe("The command to execute"),
        },
        async execute(args, context) {
          await ensureSandbox(context.sessionID);
          return bashOverride(k8sCtx(context.sessionID), args.command);
        },
      }),

      read: tool({
        description: "Read a file from the sandbox pod",
        args: {
          filePath: tool.schema.string().describe("Path to the file"),
        },
        async execute(args, context) {
          await ensureSandbox(context.sessionID);
          return readFile(k8sCtx(context.sessionID), args.filePath);
        },
      }),

      write: tool({
        description: "Write content to a file in the sandbox pod",
        args: {
          filePath: tool.schema.string().describe("Path to the file"),
          content: tool.schema.string().describe("Content to write"),
        },
        async execute(args, context) {
          await ensureSandbox(context.sessionID);
          await writeFile(
            k8sCtx(context.sessionID),
            args.filePath,
            args.content
          );
          return `Wrote ${args.content.length} bytes to ${args.filePath}`;
        },
      }),

      edit: tool({
        description: "Edit a file in the sandbox pod using string replacement",
        args: {
          filePath: tool.schema.string().describe("Path to the file"),
          oldString: tool.schema.string().describe("Exact string to replace"),
          newString: tool.schema.string().describe("Replacement string"),
        },
        async execute(args, context) {
          await ensureSandbox(context.sessionID);
          await editFile(
            k8sCtx(context.sessionID),
            args.filePath,
            args.oldString,
            args.newString
          );
          return `Edited ${args.filePath}`;
        },
      }),

      list: tool({
        description: "List files in a directory on the sandbox pod",
        args: {
          path: tool.schema.string().describe("Directory path"),
        },
        async execute(args, context) {
          await ensureSandbox(context.sessionID);
          return lsOverride(k8sCtx(context.sessionID), args.path);
        },
      }),

      glob: tool({
        description: "Find files by glob pattern on the sandbox pod",
        args: {
          pattern: tool.schema.string().describe("Glob pattern (e.g. **/*.ts)"),
        },
        async execute(args, context) {
          await ensureSandbox(context.sessionID);
          return globOverride(k8sCtx(context.sessionID), args.pattern);
        },
      }),

      grep: tool({
        description: "Search file contents using regex on the sandbox pod",
        args: {
          pattern: tool.schema.string().describe("Regex pattern to search for"),
          path: tool.schema.string().describe("Directory to search in"),
          include: tool.schema.string().optional().describe("File pattern to include (e.g. *.ts)"),
        },
        async execute(args, context) {
          await ensureSandbox(context.sessionID);
          return grepOverride(
            k8sCtx(context.sessionID),
            args.pattern,
            args.path,
            args.include
          );
        },
      }),

      "preview-link": tool({
        description: "Get a URL to access a service running in the sandbox",
        args: {
          port: tool.schema.number().describe("Port number"),
        },
        async execute(args, context) {
          await ensureSandbox(context.sessionID);
          const record = getRecord(context.sessionID);
          return previewLinkOverride(
            record.sandboxId,
            config.baseDomain,
            args.port
          );
        },
      }),

      multiedit: tool({
        description: "Apply multiple edits to files in the sandbox pod",
        args: {
          operations: tool.schema
            .array(
              tool.schema.object({
                path: tool.schema.string(),
                oldString: tool.schema.string(),
                newString: tool.schema.string(),
              })
            )
            .describe("List of edit operations"),
        },
        async execute(args, context) {
          await ensureSandbox(context.sessionID);
          await multieditOverride(k8sCtx(context.sessionID), args.operations);
          return `Applied ${args.operations.length} edits`;
        },
      }),

      apply_patch: tool({
        description:
          "Apply a patch to files in the sandbox pod using OpenCode's marker format",
        args: {
          patchText: tool.schema.string().describe("Patch content in OpenCode marker format"),
        },
        async execute(args, context) {
          await ensureSandbox(context.sessionID);
          return applyPatchOverride(
            k8sCtx(context.sessionID),
            args.patchText
          );
        },
      }),
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
      input: { tool: string; sessionID: string; callID: string },
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
