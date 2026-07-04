import { execInPod } from "../k8s/exec.js";

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
    path,
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
    `find . -name "${pattern}" -type f 2>/dev/null | head -100`,
  ]);

  if (result.exitCode !== 0) {
    throw new Error(`glob failed with exit code ${result.exitCode}: ${result.stderr}`);
  }

  return result.stdout;
}

export async function grepOverride(
  ctx: ToolContext,
  pattern: string,
  path: string
): Promise<string> {
  const result = await execInPod(ctx.podName, ctx.namespace, [
    "/bin/grep",
    "-r",
    "--include=*",
    pattern,
    path,
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
  // Apply each edit sequentially through the existing execInPod primitive
  for (const op of operations) {
    // Read the file, apply the replacement, write it back
    // Using sed for in-place replacement
    const escapedOld = op.oldString.replace(/[\/&]/g, '\\$&');
    const escapedNew = op.newString.replace(/[\/&]/g, '\\$&');
    const result = await execInPod(ctx.podName, ctx.namespace, [
      "/bin/sed",
      "-i",
      `s/${escapedOld}/${escapedNew}/g`,
      op.path,
    ]);

    if (result.exitCode !== 0) {
      throw new Error(`multiedit failed on ${op.path}: ${result.stderr}`);
    }
  }
}

export async function applyPatchOverride(
  ctx: ToolContext,
  patchContent: string
): Promise<string> {
  // Apply unified diff patch using the pod's patch command
  // Write patch to temp file, apply, then clean up
  const result = await execInPod(ctx.podName, ctx.namespace, [
    "/bin/bash",
    "-c",
    `echo '${patchContent.replace(/'/g, "'\\''")}' | patch -p1`,
  ]);

  if (result.exitCode !== 0) {
    throw new Error(`apply_patch failed with exit code ${result.exitCode}: ${result.stderr}`);
  }

  return result.stdout;
}
