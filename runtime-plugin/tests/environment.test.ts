import { createHash } from "node:crypto";
import { chmod, mkdtemp, mkdir, readFile, rm, symlink, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, describe, expect, it } from "vitest";
import {
  EnvironmentTracker,
  calculateEnvironmentFingerprint,
  evaluateEnvironment,
  parseDirenvJson,
  runDirenv,
} from "../src/environment.js";

const temporaryDirectories: string[] = [];

afterEach(async () => {
  await Promise.all(temporaryDirectories.splice(0).map((directory) => rm(directory, { recursive: true, force: true })));
});

async function fakeDirenv(): Promise<{ executable: string; calls: string }> {
  const directory = await mkdtemp(join(tmpdir(), "runtime-plugin-environment-"));
  temporaryDirectories.push(directory);
  const executable = join(directory, "direnv");
  const calls = join(directory, "calls");
  await writeFile(
    executable,
    `#!/bin/sh
if [ "$1" = allow ]; then printf 'allow %s\\n' "$2" >> '${calls}'; exit 0; fi
if [ "$1" = exec ]; then printf 'exec %s\\n' "$2" >> '${calls}'; exec /usr/bin/env -0; fi
exit 1
`,
  );
  await chmod(executable, 0o755);
  return { executable, calls };
}

async function runWithEnvrc(kind: "symlink" | "regular" | "absent") {
  const directory = await mkdtemp(join(tmpdir(), "runtime-plugin-cwd-"));
  temporaryDirectories.push(directory);
  const cwd = join(directory, "nested");
  await mkdir(cwd);
  if (kind === "symlink") {
    await symlink("profile", join(directory, ".envrc"));
    await writeFile(join(directory, "profile"), "export PROFILE=1");
  } else if (kind === "regular") {
    await writeFile(join(directory, ".envrc"), "export TRACKED=1");
  }
  const direnv = await fakeDirenv();
  await runDirenv(cwd, direnv.executable);
  return readFile(direnv.calls, "utf8");
}

describe("dynamic environment", () => {
  it("allows the nearest governing symlink .envrc", async () => {
    await expect(runWithEnvrc("symlink")).resolves.toMatch(/^allow .*\nexec .*\n$/);
  });

  it("does not auto-allow a regular governing .envrc", async () => {
    await expect(runWithEnvrc("regular")).resolves.toMatch(/^exec .*\n$/);
  });

  it("does not auto-allow when .envrc is absent", async () => {
    await expect(runWithEnvrc("absent")).resolves.toMatch(/^exec .*\n$/);
  });

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
