import { execInPod } from "../k8s/exec.js";
import { sandboxPath } from "./paths.js";

export interface FileOpsContext {
  podName: string;
  namespace: string;
}

export async function readFile(
  ctx: FileOpsContext,
  path: string
): Promise<string> {
  const result = await execInPod(ctx.podName, ctx.namespace, [
    "/bin/cat",
    "--",
    sandboxPath(path),
  ]);

  if (result.exitCode !== 0) {
    throw new Error(`Failed to read file ${path}: ${result.stderr}`);
  }

  return result.stdout;
}

export async function writeFile(
  ctx: FileOpsContext,
  path: string,
  content: string
): Promise<void> {
  const encoded = Buffer.from(content).toString("base64");
  const result = await execInPod(
    ctx.podName,
    ctx.namespace,
    ["/bin/bash", "-c", "base64 -d > \"$1\"", "--", sandboxPath(path)],
    encoded
  );

  if (result.exitCode !== 0) {
    throw new Error(`Failed to write file ${path}: ${result.stderr}`);
  }
}

export async function editFile(
  ctx: FileOpsContext,
  path: string,
  oldString: string,
  newString: string
): Promise<void> {
  const content = await readFile(ctx, path);

  if (!content.includes(oldString)) {
    throw new Error(`String not found in file ${path}`);
  }

  const updatedContent = content.replace(oldString, newString);
  await writeFile(ctx, path, updatedContent);
}
