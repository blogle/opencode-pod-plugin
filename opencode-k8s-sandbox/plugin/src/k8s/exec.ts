import * as stream from "stream";
import { getExecApi, getCoreV1Api } from "./client.js";

export interface ExecResult {
  stdout: string;
  stderr: string;
  exitCode: number;
}

export async function execInPod(
  podName: string,
  namespace: string,
  command: string[]
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
    throw new Error(`Failed to check pod status: ${error}`);
  }

  return new Promise((resolve, reject) => {
    const stdoutStream = new stream.PassThrough();
    const stderrStream = new stream.PassThrough();
    let stdout = "";
    let stderr = "";

    stdoutStream.on("data", (data: Buffer) => {
      stdout += data.toString();
    });

    stderrStream.on("data", (data: Buffer) => {
      stderr += data.toString();
    });

    exec
      .exec(
        namespace,
        podName,
        "sandbox",
        command,
        stdoutStream,
        stderrStream,
        null, // stdin
        false, // tty
        (status) => {
          resolve({
            stdout,
            stderr,
            exitCode: status.status === "Success" ? 0 : 1,
          });
        }
      )
      .then(
        () => {
          // WebSocket connected
        },
        (error) => {
          reject(new Error(`Exec failed: ${error}`));
        }
      );
  });
}
