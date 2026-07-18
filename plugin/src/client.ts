export interface WorkspaceCreateRequest {
  workspaceId: string;
  projectKey: string;
  gitRef: string | null;
  owner?: string;
  runtimeOverrides?: Record<string, unknown>;
  upstreamEnvironment: Record<string, string | undefined>;
}

export interface GatewayWorkspace {
  workspaceId: string;
  state: string;
  target: { url: string; username: string; password: string };
}

export class GatewayClient {
  constructor(
    private readonly baseUrl: string,
    private readonly request: typeof fetch = fetch,
    private readonly token?: string,
  ) {}

  createWorkspace(request: WorkspaceCreateRequest): Promise<GatewayWorkspace> {
    return this.json("/v1/workspaces", {
      method: "POST",
      body: JSON.stringify(request),
    });
  }

  getWorkspace(workspaceId: string): Promise<GatewayWorkspace> {
    return this.json(`/v1/workspaces/${encodeURIComponent(workspaceId)}`);
  }

  async removeWorkspace(workspaceId: string): Promise<void> {
    const response = await this.request(
      new URL(`/v1/workspaces/${encodeURIComponent(workspaceId)}`, this.baseUrl),
      { method: "DELETE", headers: this.headers() },
    );
    if (!response.ok && response.status !== 404) {
      throw new Error(`gateway delete failed: ${response.status}`);
    }
  }

  private headers(init?: HeadersInit): Headers {
    const headers = new Headers(init);
    headers.set("content-type", "application/json");
    if (this.token) headers.set("authorization", `Bearer ${this.token}`);
    return headers;
  }

  private async json<T>(path: string, init: RequestInit = {}): Promise<T> {
    const response = await this.request(new URL(path, this.baseUrl), {
      ...init,
      headers: this.headers(init.headers),
    });
    if (!response.ok) throw new Error(`gateway request failed: ${response.status}`);
    return response.json() as Promise<T>;
  }
}
