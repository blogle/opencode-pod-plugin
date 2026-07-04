export interface SandboxRecord {
  sandboxId: string;
  podName: string;
  repoUrl?: string;
  createdAt: Date;
  lastActiveAt: Date;
}

export class SessionStore {
  private sessions = new Map<string, SandboxRecord>();

  set(sessionId: string, record: SandboxRecord): void {
    this.sessions.set(sessionId, record);
  }

  get(sessionId: string): SandboxRecord | undefined {
    return this.sessions.get(sessionId);
  }

  getAll(): Map<string, SandboxRecord> {
    return new Map(this.sessions);
  }

  delete(sessionId: string): boolean {
    return this.sessions.delete(sessionId);
  }

  has(sessionId: string): boolean {
    return this.sessions.has(sessionId);
  }

  updateLastActive(sessionId: string): void {
    const record = this.sessions.get(sessionId);
    if (record) {
      record.lastActiveAt = new Date();
    }
  }

  getExpiredSessions(timeoutMinutes: number): string[] {
    const now = new Date();
    const expired: string[] = [];

    for (const [sessionId, record] of this.sessions) {
      const inactiveMs = now.getTime() - record.lastActiveAt.getTime();
      if (inactiveMs > timeoutMinutes * 60 * 1000) {
        expired.push(sessionId);
      }
    }

    return expired;
  }
}
