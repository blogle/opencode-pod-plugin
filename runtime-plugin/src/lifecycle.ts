import type { EnvironmentFingerprint } from "./environment.js";
import type { RuntimeConfig } from "./config.js";

export type ActivityKind = "message" | "tool" | "idle";
export type Log = (record: Record<string, unknown>) => void;

export function redact(value: string, secrets: readonly string[]): string {
  let result = value
    .replace(/(authorization\s*[:=]\s*)(?:bearer|basic)\s+[^\s,;]+/gi, "$1[REDACTED]")
    .replace(/([?&](?:token|key|password|secret)=)[^&\s]+/gi, "$1[REDACTED]");
  for (const secret of secrets) {
    if (secret) result = result.split(secret).join("[REDACTED]");
  }
  return result;
}

export class RuntimeHttpError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "RuntimeHttpError";
  }
}

export class RuntimeClient {
  constructor(
    private readonly config: RuntimeConfig,
    private readonly fetcher: typeof fetch = fetch,
  ) {}

  private async post(url: string, body: unknown): Promise<void> {
    let response: Response;
    try {
      response = await this.fetcher(url, {
        method: "POST",
        headers: {
          authorization: `Bearer ${this.config.gatewayToken}`,
          "content-type": "application/json",
        },
        body: JSON.stringify(body),
        signal: AbortSignal.timeout(15_000),
      });
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      throw new RuntimeHttpError(
        redact(`request to ${url} failed: ${message}`, [this.config.gatewayToken]),
      );
    }

    if (!response.ok) {
      throw new RuntimeHttpError(
        redact(
          `request to ${url} failed with HTTP ${response.status}`,
          [this.config.gatewayToken],
        ),
      );
    }
  }

  reportActivity(kind: ActivityKind, sessionId?: string, tool?: string): Promise<void> {
    return this.post(
      `${this.config.gatewayUrl}/v1/workspaces/${encodeURIComponent(this.config.workspaceId)}/activity`,
      {
        kind,
        sessionId,
        tool,
        timestamp: new Date().toISOString(),
      },
    );
  }

  requestCheckpoint(
    sessionId: string,
    fingerprint: EnvironmentFingerprint,
  ): Promise<void> {
    return this.post(`${this.config.checkpointEndpoint}/checkpoint`, {
      workspaceId: this.config.workspaceId,
      sessionId,
      reason: "session.idle",
      environmentFingerprint: fingerprint,
    });
  }

  requestRestart(sessionId: string): Promise<void> {
    return this.post(`${this.config.supervisorEndpoint}/restart`, {
      workspaceId: this.config.workspaceId,
      sessionId,
      reason: "environment-changed",
    });
  }
}

export class LifecycleCoordinator {
  private dirtyGeneration = 0;
  private environmentGeneration = 0;
  private idleRequest?: Promise<void>;

  constructor(
    private readonly client: RuntimeClient,
    private readonly log: Log,
  ) {}

  markDirty(): void {
    this.dirtyGeneration += 1;
  }

  markEnvironmentDirty(): void {
    this.environmentGeneration += 1;
    this.markDirty();
  }

  activity(kind: ActivityKind, sessionId?: string, tool?: string): Promise<void> {
    return this.client.reportActivity(kind, sessionId, tool).catch((error: unknown) => {
      this.logError("activity", error);
    });
  }

  idle(sessionId: string, fingerprint: EnvironmentFingerprint): Promise<void> {
    if (this.idleRequest) return this.idleRequest;
    this.idleRequest = this.performIdle(sessionId, fingerprint).finally(() => {
      this.idleRequest = undefined;
    });
    return this.idleRequest;
  }

  private async performIdle(
    sessionId: string,
    fingerprint: EnvironmentFingerprint,
  ): Promise<void> {
    await this.activity("idle", sessionId);
    if (this.dirtyGeneration === 0 && this.environmentGeneration === 0) return;

    const dirtyGeneration = this.dirtyGeneration;
    const environmentGeneration = this.environmentGeneration;
    if (dirtyGeneration !== 0) {
      try {
        await this.client.requestCheckpoint(sessionId, fingerprint);
        if (this.dirtyGeneration === dirtyGeneration) this.dirtyGeneration = 0;
      } catch (error) {
        this.logError("checkpoint", error);
        return;
      }
    }

    if (environmentGeneration === 0) return;
    try {
      await this.client.requestRestart(sessionId);
      if (this.environmentGeneration === environmentGeneration) {
        this.environmentGeneration = 0;
      }
    } catch (error) {
      this.logError("restart", error);
    }
  }

  private logError(operation: string, error: unknown): void {
    try {
      this.log({
        level: "error",
        component: "opencode-runtime-plugin",
        operation,
        message: error instanceof Error ? error.message : "unknown runtime error",
      });
    } catch {
      // Observability failures must never affect stock OpenCode operations.
    }
  }
}
