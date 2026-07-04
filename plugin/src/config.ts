import { z } from "zod";

const ResourceRequirementsSchema = z.object({
  cpu: z.string().default("250m"),
  memory: z.string().default("256Mi"),
});

const ConfigSchema = z.object({
  namespace: z.string().describe("Kubernetes namespace for sandbox pods"),
  sandboxImage: z.string().describe("Container image for sandbox pods"),
  imagePullPolicy: z
    .enum(["Always", "IfNotPresent", "Never"])
    .default("IfNotPresent")
    .describe("Image pull policy for sandbox pods"),
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
  storageSize: z.string().default("1Gi").describe("PVC storage size when persistWorkspace is true"),
  idleTimeoutMinutes: z.number().default(60),
  podStartupTimeoutSeconds: z.number().default(30),
  // ponytail: nixCache is optional; when set, pods get writable /nix + attic push/pull.
  // No warmup job yet — every pod writes back directly. Upgrade path: add a warmup
  // controller that pre-builds common dev shells into the cache.
  nixCache: z
    .object({
      endpoint: z.string().describe("Attic server endpoint URL (e.g. https://attic.example.com)"),
      cache: z.string().default("opencode").describe("Attic cache name"),
      publicKey: z.string().describe("Attic cache public key (e.g. opencode:abc123=)"),
      tokenSecretName: z
        .string()
        .describe("Kubernetes secret containing the Attic push token"),
      tokenSecretKey: z
        .string()
        .default("attic-token")
        .describe("Key within the secret containing the token value"),
    })
    .optional(),
});

export type Config = z.infer<typeof ConfigSchema>;

export function loadConfig(pluginConfig: Record<string, unknown>): Config {
  // Load from plugin config with env var overrides
  // For persistWorkspace, env var "false" should override config true
  const persistWorkspaceEnv = process.env.SANDBOX_PERSIST_WORKSPACE;
  let persistWorkspace: boolean;
  if (persistWorkspaceEnv !== undefined) {
    persistWorkspace = persistWorkspaceEnv === "true";
  } else {
    persistWorkspace = (pluginConfig.persistWorkspace as boolean) ?? false;
  }

  const envConfig = {
    namespace:
      process.env.SANDBOX_NAMESPACE || (pluginConfig.namespace as string),
    sandboxImage:
      process.env.SANDBOX_IMAGE || (pluginConfig.sandboxImage as string),
    imagePullPolicy:
      process.env.SANDBOX_IMAGE_PULL_POLICY ||
      (pluginConfig.imagePullPolicy as string),
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
    persistWorkspace,
    storageClassName:
      process.env.SANDBOX_STORAGE_CLASS ||
      (pluginConfig.storageClassName as string),
    storageSize:
      process.env.SANDBOX_STORAGE_SIZE ||
      (pluginConfig.storageSize as string),
    idleTimeoutMinutes: pluginConfig.idleTimeoutMinutes as number | undefined,
    podStartupTimeoutSeconds:
      pluginConfig.podStartupTimeoutSeconds as number | undefined,
    nixCache: pluginConfig.nixCache as
      | {
          endpoint: string;
          cache?: string;
          publicKey: string;
          tokenSecretName: string;
          tokenSecretKey?: string;
        }
      | undefined,
  };

  return ConfigSchema.parse(envConfig);
}
