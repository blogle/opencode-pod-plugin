import { describe, expect, it } from "vitest";
import { loadRuntimeConfig } from "../src/config.js";

const requiredEnv = {
  OPENCODE_GATEWAY_URL: "http://gateway.internal",
  OPENCODE_GATEWAY_TOKEN: "runtime-token",
  OPENCODE_WORKSPACE_ID: "wrk_test",
  OPENCODE_BASE_DOMAIN: "preview.test",
};

describe("runtime configuration", () => {
  it("defaults checkpoint and supervisor to their separate loopback services", () => {
    const config = loadRuntimeConfig(requiredEnv);

    expect(config.checkpointEndpoint).toBe("http://127.0.0.1:4098");
    expect(config.supervisorEndpoint).toBe("http://127.0.0.1:4097");
    expect(config.checkpointIntervalSeconds).toBe(120);
  });

  it("requires a positive periodic checkpoint interval", () => {
    expect(() =>
      loadRuntimeConfig({
        ...requiredEnv,
        OPENCODE_CHECKPOINT_INTERVAL_SECONDS: "0",
      }),
    ).toThrow("positive integer");
    expect(
      loadRuntimeConfig({
        ...requiredEnv,
        OPENCODE_CHECKPOINT_INTERVAL_SECONDS: "17",
      }).checkpointIntervalSeconds,
    ).toBe(17);
  });

  it("accepts configured checkpoint and supervisor endpoints", () => {
    const config = loadRuntimeConfig({
      ...requiredEnv,
      OPENCODE_CHECKPOINT_ENDPOINT: "http://127.0.0.1:5098/control/",
      OPENCODE_SUPERVISOR_ENDPOINT: "http://127.0.0.1:5097/control/",
    });

    expect(config.checkpointEndpoint).toBe("http://127.0.0.1:5098/control");
    expect(config.supervisorEndpoint).toBe("http://127.0.0.1:5097/control");
  });
});
