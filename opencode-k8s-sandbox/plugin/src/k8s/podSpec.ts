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

  // Add tmp volume for writable temp directories (needed for readOnlyRootFilesystem)
  volumes.push({
    name: "tmp",
    emptyDir: { sizeLimit: "100Mi" },
  });

  // Init container for git clone (if repo URL is provided)
  // Use pinned version instead of :latest for reproducibility
  const initContainers: k8s.V1Container[] = repoUrl
    ? [
        {
          name: "git-clone",
          image: "alpine/git:2.45.2-r0",
          imagePullPolicy: "IfNotPresent",
          command: ["git", "clone", "--depth=1", repoUrl, "/workspace"],
          volumeMounts: [{ name: "workspace", mountPath: "/workspace" }],
          securityContext: {
            runAsNonRoot: false, // git clone needs to write
            runAsUser: 0,
            allowPrivilegeEscalation: false,
            readOnlyRootFilesystem: false, // needs to write to workspace
            capabilities: { drop: ["ALL"] },
          },
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
          imagePullPolicy: config.imagePullPolicy ?? "IfNotPresent",
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
          volumeMounts: [
            ...volumeMounts,
            { name: "tmp", mountPath: "/tmp" },
          ],
          securityContext: {
            runAsNonRoot: true,
            runAsUser: 1000,
            runAsGroup: 1000,
            readOnlyRootFilesystem: true,
            allowPrivilegeEscalation: false,
            capabilities: { drop: ["ALL"] },
            seccompProfile: {
              type: "RuntimeDefault",
            },
          },
          env: [
            {
              name: "TMPDIR",
              value: "/tmp",
            },
          ],
        },
      ],
      volumes,
      securityContext: {
        seccompProfile: {
          type: "RuntimeDefault",
        },
      },
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
          storage: config.storageSize,
        },
      },
      storageClassName: config.storageClassName,
    },
  };
}
