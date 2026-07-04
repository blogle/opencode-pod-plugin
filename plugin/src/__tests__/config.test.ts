import { describe, it, expect } from "vitest";
import { loadConfig } from "../config.js";
import { SessionStore } from "../sessionStore.js";

describe("Config", () => {
  it("should load config from plugin config", () => {
    const pluginConfig = {
      namespace: "test-namespace",
      sandboxImage: "test-image",
      repos: {
        "test-repo": "https://example.com/repo.git",
      },
      baseDomain: "test.example.com",
    };

    const config = loadConfig(pluginConfig);
    expect(config.namespace).toBe("test-namespace");
    expect(config.sandboxImage).toBe("test-image");
    expect(config.repos).toEqual({ "test-repo": "https://example.com/repo.git" });
    expect(config.baseDomain).toBe("test.example.com");
  });

  it("should use default values", () => {
    const pluginConfig = {
      namespace: "test-namespace",
      sandboxImage: "test-image",
      repos: {
        "test-repo": "https://example.com/repo.git",
      },
      baseDomain: "test.example.com",
    };

    const config = loadConfig(pluginConfig);
    expect(config.resources.requests.cpu).toBe("250m");
    expect(config.resources.requests.memory).toBe("256Mi");
    expect(config.resources.limits.cpu).toBe("2");
    expect(config.resources.limits.memory).toBe("2Gi");
    expect(config.persistWorkspace).toBe(false);
    expect(config.idleTimeoutMinutes).toBe(60);
    expect(config.podStartupTimeoutSeconds).toBe(30);
    expect(config.repoBaseDir).toBe("repos");
  });

  it("should allow env var overrides", () => {
    process.env.SANDBOX_NAMESPACE = "env-namespace";

    const pluginConfig = {
      namespace: "test-namespace",
      sandboxImage: "test-image",
      repos: {
        "test-repo": "https://example.com/repo.git",
      },
      baseDomain: "test.example.com",
    };

    const config = loadConfig(pluginConfig);
    expect(config.namespace).toBe("env-namespace");

    delete process.env.SANDBOX_NAMESPACE;
  });
});

describe("SessionStore", () => {
  it("should store and retrieve sessions", () => {
    const store = new SessionStore();
    const record = {
      sandboxId: "abc12345",
      podName: "opencode-sbx-abc12345",
      createdAt: new Date(),
      lastActiveAt: new Date(),
    };

    store.set("session-1", record);
    expect(store.get("session-1")).toEqual(record);
  });

  it("should delete sessions", () => {
    const store = new SessionStore();
    store.set("session-1", {
      sandboxId: "abc12345",
      podName: "opencode-sbx-abc12345",
      createdAt: new Date(),
      lastActiveAt: new Date(),
    });

    expect(store.has("session-1")).toBe(true);
    store.delete("session-1");
    expect(store.has("session-1")).toBe(false);
  });

  it("should update last active time", () => {
    const store = new SessionStore();
    const past = new Date(Date.now() - 10000);
    store.set("session-1", {
      sandboxId: "abc12345",
      podName: "opencode-sbx-abc12345",
      createdAt: past,
      lastActiveAt: past,
    });

    store.updateLastActive("session-1");
    const record = store.get("session-1");
    expect(record?.lastActiveAt.getTime()).toBeGreaterThan(past.getTime());
  });

  it("should detect expired sessions", () => {
    const store = new SessionStore();
    const past = new Date(Date.now() - 120 * 60 * 1000); // 2 hours ago
    store.set("session-1", {
      sandboxId: "abc12345",
      podName: "opencode-sbx-abc12345",
      createdAt: past,
      lastActiveAt: past,
    });

    store.set("session-2", {
      sandboxId: "def67890",
      podName: "opencode-sbx-def67890",
      createdAt: new Date(),
      lastActiveAt: new Date(),
    });

    const expired = store.getExpiredSessions(60); // 60 minutes timeout
    expect(expired).toContain("session-1");
    expect(expired).not.toContain("session-2");
  });
});
