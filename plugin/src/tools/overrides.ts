import { execInPod } from "../k8s/exec.js";
import { sandboxPath } from "./paths.js";
import { parsePatch, applyHunks } from "./patchParser.js";
import { readFile, writeFile } from "./fileOps.js";

export interface ToolContext {
  podName: string;
  namespace: string;
}

export async function lsOverride(
  ctx: ToolContext,
  path: string
): Promise<string> {
  const result = await execInPod(ctx.podName, ctx.namespace, [
    "/bin/ls",
    "-la",
    "--",
    sandboxPath(path),
  ]);

  if (result.exitCode !== 0) {
    throw new Error(`ls failed with exit code ${result.exitCode}: ${result.stderr}`);
  }

  return result.stdout;
}

export async function globOverride(
  ctx: ToolContext,
  pattern: string
): Promise<string> {
  const result = await execInPod(ctx.podName, ctx.namespace, [
    "/bin/bash",
    "-c",
    "cd /workspace && rg --files -g \"$1\" 2>/dev/null | head -200",
    "--",
    pattern,
  ]);

  if (result.exitCode !== 0) {
    throw new Error(`glob failed with exit code ${result.exitCode}: ${result.stderr}`);
  }

  return result.stdout;
}

export async function grepOverride(
  ctx: ToolContext,
  pattern: string,
  path: string,
  include?: string
): Promise<string> {
  const searchPath = sandboxPath(path);
  const includeArg = include ? `-g "${include}"` : "";

  const result = await execInPod(ctx.podName, ctx.namespace, [
    "/bin/bash",
    "-c",
    `rg --no-heading -n ${includeArg} "$1" "$2" 2>/dev/null | head -200`,
    "--",
    pattern,
    searchPath,
  ]);

  // grep returns exit code 1 when no matches found, which is not an error
  if (result.exitCode > 1) {
    throw new Error(`grep failed with exit code ${result.exitCode}: ${result.stderr}`);
  }

  return result.stdout;
}

export function previewLinkOverride(
  sandboxId: string,
  baseDomain: string,
  port: number
): string {
  return `https://${port}-${sandboxId}.${baseDomain}`;
}

export interface EditOperation {
  path: string;
  oldString: string;
  newString: string;
}

export async function multieditOverride(
  ctx: ToolContext,
  operations: EditOperation[]
): Promise<void> {
  for (const op of operations) {
    const filePath = sandboxPath(op.path);
    const content = await readFile(ctx, op.path);
    if (!content.includes(op.oldString)) {
      throw new Error(`String not found in ${op.path}`);
    }
    const updatedContent = content.replace(op.oldString, op.newString);
    await writeFile(ctx, op.path, updatedContent);
  }
}

/**
 * Apply an OpenCode marker-format patch inside the sandbox.
 * Parses the *** marker format, then applies each hunk via file operations.
 */
export async function applyPatchOverride(
  ctx: ToolContext,
  patchContent: string
): Promise<string> {
  const { hunks } = parsePatch(patchContent);
  const results: string[] = [];

  for (const hunk of hunks) {
    switch (hunk.type) {
      case "add": {
        await writeFile(ctx, hunk.path, hunk.contents ?? "");
        results.push(`Added: ${hunk.path}`);
        break;
      }
      case "delete": {
        const deleteResult = await execInPod(ctx.podName, ctx.namespace, [
          "/bin/rm",
          "-f",
          "--",
          sandboxPath(hunk.path),
        ]);
        if (deleteResult.exitCode !== 0) {
          throw new Error(`Failed to delete ${hunk.path}: ${deleteResult.stderr}`);
        }
        results.push(`Deleted: ${hunk.path}`);
        break;
      }
      case "update": {
        const existing = await readFile(ctx, hunk.path);
        const updated = applyHunks(existing, [hunk]);
        await writeFile(ctx, hunk.path, updated);

        // Handle Move to
        if (hunk.movePath) {
          const moveResult = await execInPod(ctx.podName, ctx.namespace, [
            "/bin/mv",
            "--",
            sandboxPath(hunk.path),
            sandboxPath(hunk.movePath),
          ]);
          if (moveResult.exitCode !== 0) {
            throw new Error(
              `Failed to move ${hunk.path} to ${hunk.movePath}: ${moveResult.stderr}`
            );
          }
          results.push(`Moved: ${hunk.path} -> ${hunk.movePath}`);
        } else {
          results.push(`Updated: ${hunk.path}`);
        }
        break;
      }
    }
  }

  return results.join("\n");
}
