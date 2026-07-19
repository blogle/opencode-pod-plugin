import { execFile } from "node:child_process";
import { createHash } from "node:crypto";
import { lstat, readFile } from "node:fs/promises";
import { dirname, join, resolve } from "node:path";
import { promisify } from "node:util";

const execFileAsync = promisify(execFile);
const FINGERPRINT_FILES = [".envrc", "flake.nix", "flake.lock"] as const;

async function governingEnvrcIsSymlink(cwd: string): Promise<boolean> {
  let directory = resolve(cwd);
  while (true) {
    try {
      return (await lstat(join(directory, ".envrc"))).isSymbolicLink();
    } catch (error) {
      if ((error as NodeJS.ErrnoException).code !== "ENOENT") throw error;
    }

    const parent = dirname(directory);
    if (parent === directory) return false;
    directory = parent;
  }
}

export type FingerprintFile = (typeof FINGERPRINT_FILES)[number];
export type EnvironmentHashes = Record<FingerprintFile, string | null>;

export interface EnvironmentFingerprint {
  hash: string;
  files: EnvironmentHashes;
}

export type DirenvRunner = (cwd: string, executable: string) => Promise<string>;
export type FileReader = (path: string) => Promise<Buffer>;

export async function runDirenv(cwd: string, executable: string): Promise<string> {
  const env: NodeJS.ProcessEnv = { ...process.env, DIRENV_LOG_FORMAT: "" };
  // The long-lived OpenCode process inherits direnv's watch cache. Clearing
  // these values forces each shell hook to evaluate projected Secret updates
  // even though Kubernetes swaps the symlink target atomically.
  delete env.DIRENV_DIFF;
  delete env.DIRENV_DIR;
  delete env.DIRENV_WATCHES;
  try {
    if (await governingEnvrcIsSymlink(cwd)) {
      await execFileAsync(executable, ["allow", cwd], {
        cwd,
        env,
        encoding: "utf8",
        maxBuffer: 1024 * 1024,
        timeout: 30_000,
      });
    }
    const result = await execFileAsync(executable, ["exec", cwd, "/usr/bin/env", "-0"], {
      cwd,
      env,
      encoding: "utf8",
      maxBuffer: 4 * 1024 * 1024,
      timeout: 120_000,
    });
    const evaluated: Record<string, string> = {};
    for (const entry of result.stdout.split("\0")) {
      if (!entry) continue;
      const separator = entry.indexOf("=");
      if (separator <= 0) continue;
      evaluated[entry.slice(0, separator)] = entry.slice(separator + 1);
    }
    return JSON.stringify(evaluated);
  } catch {
    // direnv output can contain values from the private profile, so never include it.
    throw new Error(`direnv export json failed for ${cwd}`);
  }
}

export function parseDirenvJson(value: string): Record<string, string | null> {
  let parsed: unknown;
  try {
    parsed = JSON.parse(value);
  } catch {
    throw new Error("direnv export json returned invalid JSON");
  }
  if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) {
    throw new Error("direnv export json must return an object");
  }

  const result: Record<string, string | null> = {};
  for (const [key, item] of Object.entries(parsed)) {
    if (typeof item !== "string" && item !== null) {
      throw new Error(`direnv export json returned a non-string value for ${key}`);
    }
    result[key] = item;
  }
  return result;
}

export async function evaluateEnvironment(
  cwd: string,
  executable: string,
  runner: DirenvRunner = runDirenv,
): Promise<Record<string, string | null>> {
  return parseDirenvJson(await runner(cwd, executable));
}

export async function calculateEnvironmentFingerprint(
  root: string,
  reader: FileReader = readFile,
): Promise<EnvironmentFingerprint> {
  const entries = await Promise.all(
    FINGERPRINT_FILES.map(async (name): Promise<[FingerprintFile, string | null]> => {
      try {
        const contents = await reader(join(root, name));
        return [name, createHash("sha256").update(contents).digest("hex")];
      } catch (error) {
        if ((error as NodeJS.ErrnoException).code === "ENOENT") return [name, null];
        throw error;
      }
    }),
  );
  const files = Object.fromEntries(entries) as EnvironmentHashes;
  return {
    files,
    hash: createHash("sha256").update(JSON.stringify(files)).digest("hex"),
  };
}

export class EnvironmentTracker {
  private current?: EnvironmentFingerprint;

  constructor(
    private readonly root: string,
    private readonly reader: FileReader = readFile,
  ) {}

  async refresh(): Promise<{ changed: boolean; fingerprint: EnvironmentFingerprint }> {
    const fingerprint = await calculateEnvironmentFingerprint(this.root, this.reader);
    const changed = this.current !== undefined && this.current.hash !== fingerprint.hash;
    this.current = fingerprint;
    return { changed, fingerprint };
  }
}
