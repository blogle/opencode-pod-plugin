import * as crypto from "crypto";
import * as k8s from "@kubernetes/client-node";
import { Config } from "./config.js";
import { SessionStore, SandboxRecord } from "./sessionStore.js";
import { getCoreV1Api } from "./k8s/client.js";
import { buildPodManifest, buildPvcManifest } from "./k8s/podSpec.js";

const SANDBOX_ID_LABEL = "opencode.dev/sandbox-id";
const SESSION_ID_ANNOTATION = "opencode.dev/session-id";
const MANAGED_BY_LABEL = "app.kubernetes.io/managed-by";

export interface PluginInput {
  sessionId: string;
  config: Config;
  sessionStore: SessionStore;
  repoUrl?: string;
}

function generateSandboxId(): string {
  return crypto.randomBytes(4).toString("hex");
}

async function waitForPodRunning(
  podName: string,
  namespace: string,
  timeoutSeconds: number
): Promise<void> {
  const coreApi = getCoreV1Api();
  const startTime = Date.now();

  while (Date.now() - startTime < timeoutSeconds * 1000) {
    try {
      const pod = await coreApi.readNamespacedPod({ name: podName, namespace });
      if (pod.status?.phase === "Running") {
        return;
      }
    } catch {
      // Pod might not exist yet
    }
    await new Promise((resolve) => setTimeout(resolve, 1000));
  }

  throw new Error(`Pod ${podName} did not reach Running state within ${timeoutSeconds}s`);
}

/**
 * Reconcile existing sandbox pods on startup.
 * Lists all pods with our managed-by label and adopts any not in sessionStore.
 * This handles plugin restarts where session state was lost.
 */
export async function reconcileExistingPods(
  config: Config,
  sessionStore: SessionStore
): Promise<void> {
  const coreApi = getCoreV1Api();

  try {
    const pods = await coreApi.listNamespacedPod({
      namespace: config.namespace,
      labelSelector: `${MANAGED_BY_LABEL}=opencode-k8s-sandbox`,
    });

    for (const pod of pods.items ?? []) {
      const sandboxId = pod.metadata?.labels?.[SANDBOX_ID_LABEL];
      const sessionId = pod.metadata?.annotations?.[SESSION_ID_ANNOTATION];
      const podName = pod.metadata?.name;

      if (!sandboxId || !podName) continue;

      // Check if this pod is already tracked
      const allSessions = sessionStore.getAll();
      const existingRecord = Array.from(allSessions.values()).find(
        (r) => r.sandboxId === sandboxId
      );

      if (existingRecord) {
        // Already tracked, skip
        continue;
      }

      // Check if pod is running
      if (pod.status?.phase !== "Running") {
        console.log(
          `Cleaning up non-running orphaned pod: ${podName} (phase: ${pod.status?.phase})`
        );
        try {
          await coreApi.deleteNamespacedPod({
            name: podName,
            namespace: config.namespace,
          });
        } catch (err) {
          console.warn(`Failed to delete orphaned pod ${podName}: ${err}`);
        }
        continue;
      }

      // Adopt this orphaned pod - it's running but not tracked
      // We can't recover the sessionId, so we use a synthetic one
      const recoveredSessionId = `recovered-${Date.now()}-${sandboxId}`;
      console.log(
        `Adopting orphaned pod: ${podName} (sandbox: ${sandboxId})`
      );

      sessionStore.set(recoveredSessionId, {
        sandboxId,
        podName,
        repoUrl: undefined,
        createdAt: pod.metadata?.creationTimestamp ?? new Date(),
        lastActiveAt: new Date(),
      });
    }
  } catch (error) {
    console.warn(`Failed to reconcile existing pods: ${error}`);
  }
}

export async function sessionCreated(input: PluginInput): Promise<void> {
  const { sessionId, config, sessionStore, repoUrl } = input;
  const sandboxId = generateSandboxId();
  const podName = `opencode-sbx-${sandboxId}`;

  // Create PVC if persistence is enabled
  if (config.persistWorkspace) {
    const pvc = buildPvcManifest(sandboxId, config);
    if (pvc) {
      const coreApi = getCoreV1Api();
      try {
        await coreApi.createNamespacedPersistentVolumeClaim({
          namespace: config.namespace,
          body: pvc,
        });
      } catch (error) {
        // PVC might already exist
        console.warn(`PVC creation failed (might already exist): ${error}`);
      }
    }
  }

  // Create the pod with repo URL for init container git clone
  const pod = buildPodManifest({ sandboxId, sessionId, config, repoUrl });
  const coreApi = getCoreV1Api();

  try {
    await coreApi.createNamespacedPod({
      namespace: config.namespace,
      body: pod,
    });
  } catch (error) {
    throw new Error(`Failed to create pod: ${error}`);
  }

  // Wait for pod to be running
  await waitForPodRunning(podName, config.namespace, config.podStartupTimeoutSeconds);

  // Store session info
  sessionStore.set(sessionId, {
    sandboxId,
    podName,
    repoUrl,
    createdAt: new Date(),
    lastActiveAt: new Date(),
  });

  console.log(
    `Sandbox created: https://{port}-${sandboxId}.${config.baseDomain}${repoUrl ? ` (repo: ${repoUrl})` : ""}`
  );
}

export async function sessionDeleted(input: PluginInput): Promise<void> {
  const { sessionId, config, sessionStore } = input;
  const record = sessionStore.get(sessionId);

  if (!record) {
    return;
  }

  const coreApi = getCoreV1Api();

  // Delete the pod
  try {
    await coreApi.deleteNamespacedPod({
      name: record.podName,
      namespace: config.namespace,
    });
  } catch (error) {
    console.warn(`Failed to delete pod: ${error}`);
  }

  // Note: PVC is NOT deleted when persistWorkspace is enabled.
  // Orphaned PVCs should be cleaned up by a separate mechanism
  // (e.g., Kubernetes Job, finalizer, or manual cleanup).

  // Remove from session store
  sessionStore.delete(sessionId);
}

export function setupIdleSweep(
  config: Config,
  sessionStore: SessionStore
): NodeJS.Timeout {
  return setInterval(() => {
    const expiredSessions = sessionStore.getExpiredSessions(
      config.idleTimeoutMinutes
    );

    for (const sessionId of expiredSessions) {
      sessionDeleted({ sessionId, config, sessionStore }).catch((error) => {
        console.error(`Failed to clean up idle session ${sessionId}:`, error);
      });
    }
  }, 60000); // Check every minute
}

export function getSystemPromptTransform(
  config: Config,
  sessionStore: SessionStore
) {
  return (basePrompt: string, context: { sessionId: string }): string => {
    const record = sessionStore.get(context.sessionId);
    if (!record) {
      return basePrompt;
    }

    const sandboxInstructions = `
You are operating inside an isolated Kubernetes sandbox pod.
Working directory: /workspace
All file operations and commands execute inside the sandbox, not on the host machine.

When you start a development server that should be accessible via browser, use the preview-link tool to get the URL and inform the user.
Example: preview-link(port=5173) returns https://5173-${record.sandboxId}.${config.baseDomain}
`.trim();

    return `${basePrompt}\n\n${sandboxInstructions}`;
  };
}
