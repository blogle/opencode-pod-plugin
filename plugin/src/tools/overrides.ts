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
    "--",
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
  // Use find with argument array to avoid shell interpretation
  const result = await execInPod(ctx.podName, ctx.namespace, [
    "/bin/bash",
    "-c",
    "find . -name \"$1\" -type f 2>/dev/null | head -100",
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
  path: string
): Promise<string> {
  const result = await execInPod(ctx.podName, ctx.namespace, [
    "/bin/grep",
    "-r",
    "--include=*",
    "--",
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
  // Read the file once, apply all replacements in memory, write once
  // This is safer than shell-based sed/perl and handles all edge cases
  for (const op of operations) {
    // Read the file
    const readResult = await execInPod(ctx.podName, ctx.namespace, [
      "/bin/cat",
      "--",
      op.path,
    ]);

    if (readResult.exitCode !== 0) {
      throw new Error(`Failed to read ${op.path}: ${readResult.stderr}`);
    }

    const content = readResult.stdout;
    if (!content.includes(op.oldString)) {
      throw new Error(`String not found in ${op.path}`);
    }

    // Replace in memory
    const updatedContent = content.replace(op.oldString, op.newString);

    // Write back using stdin
    const writeResult = await execInPod(
      ctx.podName,
      ctx.namespace,
      ["/bin/bash", "-c", "cat > \"$1\"", "--", op.path],
      updatedContent
    );

    if (writeResult.exitCode !== 0) {
      throw new Error(`Failed to write ${op.path}: ${writeResult.stderr}`);
    }
  }
}

export async function applyPatchOverride(
  ctx: ToolContext,
  patchContent: string
): Promise<string> {
  // Use stdin to pass patch content safely, avoiding shell injection
  const result = await execInPod(
    ctx.podName,
    ctx.namespace,
    ["/usr/bin/patch", "-p1"],
    patchContent
  );

  if (result.exitCode !== 0) {
    throw new Error(`apply_patch failed with exit code ${result.exitCode}: ${result.stderr}`);
  }

  return result.stdout;
}
