import { execFile } from "node:child_process";
import { createHash } from "node:crypto";
import { readFile } from "node:fs/promises";
import { join } from "node:path";
import { promisify } from "node:util";

const execFileAsync = promisify(execFile);
const FINGERPRINT_FILES = [".envrc", "flake.nix", "flake.lock"] as const;

export type FingerprintFile = (typeof FINGERPRINT_FILES)[number];
export type EnvironmentHashes = Record<FingerprintFile, string | null>;

export interface EnvironmentFingerprint {
  hash: string;
  files: EnvironmentHashes;
}

export type DirenvRunner = (cwd: string, executable: string) => Promise<string>;
export type FileReader = (path: string) => Promise<Buffer>;

export async function runDirenv(cwd: string, executable: string): Promise<string> {
  try {
    const result = await execFileAsync(executable, ["export", "json"], {
      cwd,
      env: { ...process.env, DIRENV_LOG_FORMAT: "" },
      encoding: "utf8",
      maxBuffer: 4 * 1024 * 1024,
      timeout: 120_000,
    });
    return result.stdout;
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
