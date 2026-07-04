import { execInPod } from "../k8s/exec.js";

export interface ToolContext {
  podName: string;
  namespace: string;
}

export async function bashOverride(
  ctx: ToolContext,
  command: string
): Promise<string> {
  const result = await execInPod(ctx.podName, ctx.namespace, [
    "/bin/bash",
    "-c",
    command,
  ]);

  if (result.exitCode !== 0) {
    throw new Error(`Command failed with exit code ${result.exitCode}: ${result.stderr}`);
  }

  return result.stdout;
}
