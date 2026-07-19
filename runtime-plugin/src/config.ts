export const RUNTIME_ENV = {
  gatewayUrl: "OPENCODE_GATEWAY_URL",
  gatewayToken: "OPENCODE_GATEWAY_TOKEN",
  workspaceId: "OPENCODE_WORKSPACE_ID",
  baseDomain: "OPENCODE_BASE_DOMAIN",
  checkpointEndpoint: "OPENCODE_CHECKPOINT_ENDPOINT",
  supervisorEndpoint: "OPENCODE_SUPERVISOR_ENDPOINT",
  direnvPath: "OPENCODE_DIRENV_PATH",
  checkpointIntervalSeconds: "OPENCODE_CHECKPOINT_INTERVAL_SECONDS",
} as const;

export interface RuntimeConfig {
  gatewayUrl: string;
  gatewayToken: string;
  workspaceId: string;
  baseDomain: string;
  checkpointEndpoint: string;
  supervisorEndpoint: string;
  direnvPath: string;
  checkpointIntervalSeconds: number;
}

function required(env: NodeJS.ProcessEnv, name: string): string {
  const value = env[name]?.trim();
  if (!value) throw new Error(`Runtime plugin requires ${name}`);
  return value;
}

function httpUrl(value: string, name: string): string {
  let url: URL;
  try {
    url = new URL(value);
  } catch {
    throw new Error(`${name} must be a valid HTTP URL`);
  }
  if (url.protocol !== "http:" && url.protocol !== "https:") {
    throw new Error(`${name} must use http or https`);
  }
  if (url.username || url.password || url.search || url.hash) {
    throw new Error(`${name} must not contain credentials, a query, or a fragment`);
  }
  return url.toString().replace(/\/$/, "");
}

export function loadRuntimeConfig(env: NodeJS.ProcessEnv = process.env): RuntimeConfig {
  const baseDomain = required(env, RUNTIME_ENV.baseDomain).replace(/\.$/, "");
  if (
    baseDomain.includes("://") ||
    baseDomain.includes("/") ||
    baseDomain.includes(":") ||
    !/^(?=.{1,253}$)(?:[a-zA-Z0-9](?:[a-zA-Z0-9-]{0,61}[a-zA-Z0-9])?\.)*[a-zA-Z0-9](?:[a-zA-Z0-9-]{0,61}[a-zA-Z0-9])?$/.test(baseDomain)
  ) {
    throw new Error(`${RUNTIME_ENV.baseDomain} must be a DNS domain`);
  }

  const workspaceId = required(env, RUNTIME_ENV.workspaceId);
  if (!/^[a-zA-Z0-9][a-zA-Z0-9_-]{0,127}$/.test(workspaceId)) {
    throw new Error(`${RUNTIME_ENV.workspaceId} contains invalid characters`);
  }
  const checkpointIntervalSeconds = Number(
    env[RUNTIME_ENV.checkpointIntervalSeconds]?.trim() || "120",
  );
  if (!Number.isSafeInteger(checkpointIntervalSeconds) || checkpointIntervalSeconds <= 0) {
    throw new Error(`${RUNTIME_ENV.checkpointIntervalSeconds} must be a positive integer`);
  }

  return {
    gatewayUrl: httpUrl(required(env, RUNTIME_ENV.gatewayUrl), RUNTIME_ENV.gatewayUrl),
    gatewayToken: required(env, RUNTIME_ENV.gatewayToken),
    workspaceId,
    baseDomain: baseDomain.toLowerCase(),
    checkpointEndpoint: httpUrl(
      env[RUNTIME_ENV.checkpointEndpoint]?.trim() || "http://127.0.0.1:4098",
      RUNTIME_ENV.checkpointEndpoint,
    ),
    supervisorEndpoint: httpUrl(
      env[RUNTIME_ENV.supervisorEndpoint]?.trim() || "http://127.0.0.1:4097",
      RUNTIME_ENV.supervisorEndpoint,
    ),
    direnvPath: env[RUNTIME_ENV.direnvPath]?.trim() || "/opt/opencode/bin/direnv",
    checkpointIntervalSeconds,
  };
}
