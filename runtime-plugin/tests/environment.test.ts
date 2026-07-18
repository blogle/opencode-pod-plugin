import { createHash } from "node:crypto";
import { describe, expect, it } from "vitest";
import {
  EnvironmentTracker,
  calculateEnvironmentFingerprint,
  evaluateEnvironment,
  parseDirenvJson,
} from "../src/environment.js";

describe("dynamic environment", () => {
  it("parses and returns every direnv delta for the requested cwd", async () => {
    const calls: Array<[string, string]> = [];
    const result = await evaluateEnvironment("/workspace/subdir", "/injected/direnv", async (cwd, bin) => {
      calls.push([cwd, bin]);
      return JSON.stringify({ ADDED: "value", REMOVED: null });
    });

    expect(calls).toEqual([["/workspace/subdir", "/injected/direnv"]]);
    expect(result).toEqual({ ADDED: "value", REMOVED: null });
  });

  it("rejects malformed and non-string direnv output", () => {
    expect(() => parseDirenvJson("not json")).toThrow("invalid JSON");
    expect(() => parseDirenvJson('{"SECRET":12}')).toThrow("non-string");
  });

  it("hashes .envrc and both flake files without retaining contents", async () => {
    const files = new Map([
      ["/workspace/.envrc", Buffer.from("export SECRET=private")],
      ["/workspace/flake.nix", Buffer.from("flake")],
    ]);
    const fingerprint = await calculateEnvironmentFingerprint("/workspace", async (path) => {
      const value = files.get(path);
      if (!value) throw Object.assign(new Error("missing"), { code: "ENOENT" });
      return value;
    });

    expect(fingerprint.files[".envrc"]).toBe(
      createHash("sha256").update("export SECRET=private").digest("hex"),
    );
    expect(fingerprint.files["flake.nix"]).toHaveLength(64);
    expect(fingerprint.files["flake.lock"]).toBeNull();
    expect(JSON.stringify(fingerprint)).not.toContain("private");
  });

  it("detects changes after establishing a baseline", async () => {
    let contents = Buffer.from("first");
    const tracker = new EnvironmentTracker("/workspace", async (path) => {
      if (!path.endsWith(".envrc")) {
        throw Object.assign(new Error("missing"), { code: "ENOENT" });
      }
      return contents;
    });

    expect((await tracker.refresh()).changed).toBe(false);
    contents = Buffer.from("second");
    expect((await tracker.refresh()).changed).toBe(true);
    expect((await tracker.refresh()).changed).toBe(false);
  });
});
