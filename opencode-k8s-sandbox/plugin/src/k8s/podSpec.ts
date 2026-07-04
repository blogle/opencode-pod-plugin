import * as k8s from "@kubernetes/client-node";
import type { Config } from "../config.js";

const MANAGED_BY_LABEL = "opencode-k8s-sandbox";
const SANDBOX_ID_LABEL = "opencode.dev/sandbox-id";
const SESSION_ID_ANNOTATION = "opencode.dev/session-id";

export interface PodSpecInput {
  sandboxId: string;
  sessionId: string;
  config: Config;
  repoUrl?: string;
}

export function buildPodManifest(input: PodSpecInput): k8s.V1Pod {
  const { sandboxId, sessionId, config, repoUrl } = input;
  const podName = `opencode-sbx-${sandboxId}`;

  const labels: Record<string, string> = {
    "app.kubernetes.io/managed-by": MANAGED_BY_LABEL,
    [SANDBOX_ID_LABEL]: sandboxId,
  };

  const annotations: Record<string, string> = {
    [SESSION_ID_ANNOTATION]: sessionId,
  };

  const volumes: k8s.V1Volume[] = [];
  const volumeMounts: k8s.V1VolumeMount[] = [];

  // Always create workspace volume for init container to clone into
  if (config.persistWorkspace) {
    const pvcName = `opencode-sbx-${sandboxId}-workspace`;
    volumes.push({
      name: "workspace",
      persistentVolumeClaim: { claimName: pvcName },
    });
  } else {
    // Ephemeral emptyDir for git clone
    volumes.push({
      name: "workspace",
      emptyDir: {},
    });
  }
  volumeMounts.push({
    name: "workspace",
    mountPath: "/workspace",
  });

  // Init container for git clone (if repo URL is provided)
  const initContainers: k8s.V1Container[] = repoUrl
    ? [
        {
          name: "git-clone",
          image: "alpine/git:latest",
          imagePullPolicy: "Never",
          command: ["git", "clone", "--depth=1", repoUrl, "/workspace"],
          volumeMounts: [{ name: "workspace", mountPath: "/workspace" }],
        },
      ]
    : [];

  const pod: k8s.V1Pod = {
    metadata: {
      name: podName,
      namespace: config.namespace,
      labels,
      annotations,
    },
    spec: {
      restartPolicy: "Never",
      initContainers,
      containers: [
        {
          name: "sandbox",
          image: config.sandboxImage,
          imagePullPolicy: "Never",
          command: ["sleep", "infinity"],
          resources: {
            requests: {
              cpu: config.resources.requests.cpu,
              memory: config.resources.requests.memory,
            },
            limits: {
              cpu: config.resources.limits.cpu,
              memory: config.resources.limits.memory,
            },
          },
          volumeMounts,
        },
      ],
      volumes,
    },
  };

  return pod;
}

export function buildPvcManifest(
  sandboxId: string,
  config: Config
): k8s.V1PersistentVolumeClaim | null {
  if (!config.persistWorkspace) {
    return null;
  }

  return {
    metadata: {
      name: `opencode-sbx-${sandboxId}-workspace`,
      namespace: config.namespace,
      labels: {
        "app.kubernetes.io/managed-by": MANAGED_BY_LABEL,
        [SANDBOX_ID_LABEL]: sandboxId,
      },
    },
    spec: {
      accessModes: ["ReadWriteOnce"],
      resources: {
        requests: {
          storage: "1Gi",
        },
      },
      storageClassName: config.storageClassName,
    },
  };
}
