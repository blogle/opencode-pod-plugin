import * as stream from "stream";
import { getExecApi, getCoreV1Api } from "./client.js";

export interface ExecResult {
  stdout: string;
  stderr: string;
  exitCode: number;
}

/**
 * Extract a human-readable message from any thrown value.
 * Handles Error, ErrorEvent, and plain objects without useful toString().
 */
export function unwrapError(error: unknown): string {
  if (error == null) return "Unknown error";

  // Standard JS Error
  if (error instanceof Error) {
    return error.message || String(error);
  }

  // ErrorEvent from WebSocket / browser APIs
  if (typeof error === "object" && "type" in error) {
    const evt = error as { type?: string; message?: string; error?: unknown };
    if (evt.message) return `[${evt.type ?? "ErrorEvent"}] ${evt.message}`;
    if (evt.error instanceof Error)
      return `[${evt.type ?? "ErrorEvent"}] ${evt.error.message}`;
    if (evt.error != null) return `[${evt.type ?? "ErrorEvent"}] ${String(evt.error)}`;
    return `[${evt.type ?? "ErrorEvent"}] (no details)`;
  }

  // String or anything else
  return String(error);
}

export async function execInPod(
  podName: string,
  namespace: string,
  command: string[],
  stdin?: string
): Promise<ExecResult> {
  const exec = getExecApi();
  const coreApi = getCoreV1Api();

  // First, check if the pod is running
  try {
    const pod = await coreApi.readNamespacedPod({ name: podName, namespace });
    if (pod.status?.phase !== "Running") {
      throw new Error(
        `Pod ${podName} is not running (phase: ${pod.status?.phase})`
      );
    }
  } catch (error) {
    throw new Error(`Failed to check pod status: ${unwrapError(error)}`);
  }

  return new Promise((resolve, reject) => {
    const stdoutStream = new stream.PassThrough();
    const stderrStream = new stream.PassThrough();
    const stdinStream = stdin ? new stream.PassThrough() : null;
    let stdout = "";
    let stderr = "";

    stdoutStream.on("data", (data: Buffer) => {
      stdout += data.toString();
    });

    stderrStream.on("data", (data: Buffer) => {
      stderr += data.toString();
    });

    // Write stdin content if provided
    if (stdinStream && stdin !== undefined) {
      stdinStream.write(stdin);
      stdinStream.end();
    }

    exec
      .exec(
        namespace,
        podName,
        "sandbox",
        command,
        stdoutStream,
        stderrStream,
        stdinStream,
        false, // tty
        (status) => {
          // The exec callback receives V1Status. Exit code is in status.status
          // "Success" means exit code 0, otherwise extract from message or default to 1
          let exitCode = 1;
          if (status.status === "Success") {
            exitCode = 0;
          } else if (status.message) {
            // Try to extract exit code from message like "command terminated with exit code 137"
            const match = status.message.match(/exit code (\d+)/);
            if (match) {
              exitCode = parseInt(match[1], 10);
            }
          }
          resolve({
            stdout,
            stderr,
            exitCode,
          });
        }
      )
      .then(
        () => {
          // WebSocket connected
        },
        (error) => {
          reject(new Error(`Exec failed: ${unwrapError(error)}`));
        }
      );
  });
}
