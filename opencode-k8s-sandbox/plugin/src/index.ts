import { loadConfig, Config } from "./config.js";
import { SessionStore, SandboxRecord } from "./sessionStore.js";
import {
  sessionCreated,
  sessionDeleted,
  setupIdleSweep,
  getSystemPromptTransform,
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

/**
 * Standard opencode plugin entry point.
 * Receives context from the opencode runtime and returns hooks/tools.
 */
export const K8sSandboxPlugin = async (ctx: OpenCodeContext) => {
  const { project, client, $, directory, worktree } = ctx;
  const config = loadConfig(project.config);
  const sessionStore = new SessionStore();

  // Wrap Bun shell $ for use with cloneRepos
  const shell = (strings: TemplateStringsArray, ...values: unknown[]) =>
    $.raw(strings, ...values);

  // Clone repos on init — these become selectable projects in OpenCode UI
  const repoMapping = await cloneRepos(config, worktree, shell);

  const pluginCtx: PluginContext = {
    config,
    sessionStore,
    repoMapping,
  };

  const idleSweepInterval = setupIdleSweep(config, sessionStore);

  return {
    hooks: {
      "session.created": async (input: { sessionId: string }) => {
        // Detect which project the user selected by checking current directory
        const pathResponse = await client.path.get();
        const currentDir = pathResponse.data?.path || directory;

        // Find the repo for this directory
        const repo = findRepoForDirectory(currentDir, repoMapping);

        await sessionCreated({
          ...input,
          ...pluginCtx,
          repoUrl: repo?.url,
        });
      },
      "session.deleted": async (input: { sessionId: string }) => {
        await sessionDeleted({ ...input, ...pluginCtx });
      },
      "tool.execute.before": (input: { sessionId: string }) => {
        sessionStore.updateLastActive(input.sessionId);
      },
      "system.prompt.transform": getSystemPromptTransform(config, sessionStore),
    },
    tools: {
      bash: async (input: { sessionId: string; command: string }) => {
        const record = sessionStore.get(input.sessionId);
        if (!record) {
          throw new Error("No sandbox found for this session");
        }
        return bashOverride(
          { podName: record.podName, namespace: config.namespace },
          input.command
        );
      },
      read: async (input: { sessionId: string; path: string }) => {
        const record = sessionStore.get(input.sessionId);
        if (!record) {
          throw new Error("No sandbox found for this session");
        }
        return readFile(
          { podName: record.podName, namespace: config.namespace },
          input.path
        );
      },
      write: async (input: {
        sessionId: string;
        path: string;
        content: string;
      }) => {
        const record = sessionStore.get(input.sessionId);
        if (!record) {
          throw new Error("No sandbox found for this session");
        }
        await writeFile(
          { podName: record.podName, namespace: config.namespace },
          input.path,
          input.content
        );
      },
      edit: async (input: {
        sessionId: string;
        path: string;
        oldString: string;
        newString: string;
      }) => {
        const record = sessionStore.get(input.sessionId);
        if (!record) {
          throw new Error("No sandbox found for this session");
        }
        await editFile(
          { podName: record.podName, namespace: config.namespace },
          input.path,
          input.oldString,
          input.newString
        );
      },
      list: async (input: { sessionId: string; path: string }) => {
        const record = sessionStore.get(input.sessionId);
        if (!record) {
          throw new Error("No sandbox found for this session");
        }
        return lsOverride(
          { podName: record.podName, namespace: config.namespace },
          input.path
        );
      },
      glob: async (input: { sessionId: string; pattern: string }) => {
        const record = sessionStore.get(input.sessionId);
        if (!record) {
          throw new Error("No sandbox found for this session");
        }
        return globOverride(
          { podName: record.podName, namespace: config.namespace },
          input.pattern
        );
      },
      grep: async (input: {
        sessionId: string;
        pattern: string;
        path: string;
      }) => {
        const record = sessionStore.get(input.sessionId);
        if (!record) {
          throw new Error("No sandbox found for this session");
        }
        return grepOverride(
          { podName: record.podName, namespace: config.namespace },
          input.pattern,
          input.path
        );
      },
      "preview-link": async (input: {
        sessionId: string;
        port: number;
      }) => {
        const record = sessionStore.get(input.sessionId);
        if (!record) {
          throw new Error("No sandbox found for this session");
        }
        return previewLinkOverride(
          record.sandboxId,
          config.baseDomain,
          input.port
        );
      },
      multiedit: async (input: {
        sessionId: string;
        operations: Array<{
          path: string;
          oldString: string;
          newString: string;
        }>;
      }) => {
        const record = sessionStore.get(input.sessionId);
        if (!record) {
          throw new Error("No sandbox found for this session");
        }
        await multieditOverride(
          { podName: record.podName, namespace: config.namespace },
          input.operations
        );
      },
      apply_patch: async (input: {
        sessionId: string;
        patch: string;
      }) => {
        const record = sessionStore.get(input.sessionId);
        if (!record) {
          throw new Error("No sandbox found for this session");
        }
        return applyPatchOverride(
          { podName: record.podName, namespace: config.namespace },
          input.patch
        );
      },
    },
    cleanup: () => {
      clearInterval(idleSweepInterval);
    },
    sessionStore,
    repoMapping,
  };
};

// Legacy factory function for backward compatibility in tests
export function createPlugin(pluginConfig: Record<string, unknown>) {
  const config = loadConfig(pluginConfig);
  const sessionStore = new SessionStore();
  const idleSweepInterval = setupIdleSweep(config, sessionStore);

  const pluginCtx: PluginContext = {
    config,
    sessionStore,
    repoMapping: new Map(),
  };

  return {
    hooks: {
      "session.created": async (input: { sessionId: string }) => {
        await sessionCreated({ ...input, ...pluginCtx });
      },
      "session.deleted": async (input: { sessionId: string }) => {
        await sessionDeleted({ ...input, ...pluginCtx });
      },
      "tool.execute.before": (input: { sessionId: string }) => {
        sessionStore.updateLastActive(input.sessionId);
      },
      "system.prompt.transform": getSystemPromptTransform(config, sessionStore),
    },
    tools: {
      bash: async (input: { sessionId: string; command: string }) => {
        const record = sessionStore.get(input.sessionId);
        if (!record) {
          throw new Error("No sandbox found for this session");
        }
        return bashOverride(
          { podName: record.podName, namespace: config.namespace },
          input.command
        );
      },
      read: async (input: { sessionId: string; path: string }) => {
        const record = sessionStore.get(input.sessionId);
        if (!record) {
          throw new Error("No sandbox found for this session");
        }
        return readFile(
          { podName: record.podName, namespace: config.namespace },
          input.path
        );
      },
      write: async (input: {
        sessionId: string;
        path: string;
        content: string;
      }) => {
        const record = sessionStore.get(input.sessionId);
        if (!record) {
          throw new Error("No sandbox found for this session");
        }
        await writeFile(
          { podName: record.podName, namespace: config.namespace },
          input.path,
          input.content
        );
      },
      edit: async (input: {
        sessionId: string;
        path: string;
        oldString: string;
        newString: string;
      }) => {
        const record = sessionStore.get(input.sessionId);
        if (!record) {
          throw new Error("No sandbox found for this session");
        }
        await editFile(
          { podName: record.podName, namespace: config.namespace },
          input.path,
          input.oldString,
          input.newString
        );
      },
      list: async (input: { sessionId: string; path: string }) => {
        const record = sessionStore.get(input.sessionId);
        if (!record) {
          throw new Error("No sandbox found for this session");
        }
        return lsOverride(
          { podName: record.podName, namespace: config.namespace },
          input.path
        );
      },
      glob: async (input: { sessionId: string; pattern: string }) => {
        const record = sessionStore.get(input.sessionId);
        if (!record) {
          throw new Error("No sandbox found for this session");
        }
        return globOverride(
          { podName: record.podName, namespace: config.namespace },
          input.pattern
        );
      },
      grep: async (input: {
        sessionId: string;
        pattern: string;
        path: string;
      }) => {
        const record = sessionStore.get(input.sessionId);
        if (!record) {
          throw new Error("No sandbox found for this session");
        }
        return grepOverride(
          { podName: record.podName, namespace: config.namespace },
          input.pattern,
          input.path
        );
      },
      "preview-link": async (input: {
        sessionId: string;
        port: number;
      }) => {
        const record = sessionStore.get(input.sessionId);
        if (!record) {
          throw new Error("No sandbox found for this session");
        }
        return previewLinkOverride(
          record.sandboxId,
          config.baseDomain,
          input.port
        );
      },
      multiedit: async (input: {
        sessionId: string;
        operations: Array<{
          path: string;
          oldString: string;
          newString: string;
        }>;
      }) => {
        const record = sessionStore.get(input.sessionId);
        if (!record) {
          throw new Error("No sandbox found for this session");
        }
        await multieditOverride(
          { podName: record.podName, namespace: config.namespace },
          input.operations
        );
      },
      apply_patch: async (input: {
        sessionId: string;
        patch: string;
      }) => {
        const record = sessionStore.get(input.sessionId);
        if (!record) {
          throw new Error("No sandbox found for this session");
        }
        return applyPatchOverride(
          { podName: record.podName, namespace: config.namespace },
          input.patch
        );
      },
    },
    cleanup: () => {
      clearInterval(idleSweepInterval);
    },
    sessionStore,
  };
}

export { Config } from "./config.js";
export { SessionStore, SandboxRecord } from "./sessionStore.js";
export { RepoMapping, findRepoForDirectory } from "./repos.js";
