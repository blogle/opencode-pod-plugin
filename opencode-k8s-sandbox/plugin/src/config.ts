import { z } from "zod";

const ResourceRequirementsSchema = z.object({
  cpu: z.string().default("250m"),
  memory: z.string().default("256Mi"),
});

const ConfigSchema = z.object({
  namespace: z.string().describe("Kubernetes namespace for sandbox pods"),
  sandboxImage: z.string().describe("Container image for sandbox pods"),
  repos: z
    .record(z.string(), z.string())
    .describe("Map of project names to git repository URLs"),
  repoBaseDir: z
    .string()
    .default("repos")
    .describe("Subdirectory under worktree for cloned repos"),
  baseDomain: z.string().describe("Base domain for sandbox URLs"),
  resources: z
    .object({
      requests: ResourceRequirementsSchema.default({}),
      limits: z
        .object({
          cpu: z.string().default("2"),
          memory: z.string().default("2Gi"),
        })
        .default({}),
    })
    .default({}),
  persistWorkspace: z.boolean().default(false),
  storageClassName: z.string().optional(),
  idleTimeoutMinutes: z.number().default(60),
  podStartupTimeoutSeconds: z.number().default(30),
});

export type Config = z.infer<typeof ConfigSchema>;

export function loadConfig(pluginConfig: Record<string, unknown>): Config {
  // Load from plugin config with env var overrides
  const envConfig = {
    namespace:
      process.env.SANDBOX_NAMESPACE || (pluginConfig.namespace as string),
    sandboxImage:
      process.env.SANDBOX_IMAGE || (pluginConfig.sandboxImage as string),
    repos: (() => {
      // Support JSON string via env var
      const envRepos = process.env.SANDBOX_REPOS;
      if (envRepos) {
        return JSON.parse(envRepos) as Record<string, string>;
      }
      return pluginConfig.repos as Record<string, string>;
    })(),
    repoBaseDir:
      process.env.SANDBOX_REPO_BASE_DIR ||
      (pluginConfig.repoBaseDir as string),
    baseDomain:
      process.env.SANDBOX_BASE_DOMAIN || (pluginConfig.baseDomain as string),
    resources: pluginConfig.resources as Record<string, unknown> | undefined,
    persistWorkspace:
      process.env.SANDBOX_PERSIST_WORKSPACE === "true" ||
      (pluginConfig.persistWorkspace as boolean),
    storageClassName:
      process.env.SANDBOX_STORAGE_CLASS ||
      (pluginConfig.storageClassName as string),
    idleTimeoutMinutes: pluginConfig.idleTimeoutMinutes as number | undefined,
    podStartupTimeoutSeconds:
      pluginConfig.podStartupTimeoutSeconds as number | undefined,
  };

  return ConfigSchema.parse(envConfig);
}
