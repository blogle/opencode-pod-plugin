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

  // ponytail: nixCache requires writable /nix/store + attic push/pull.
  // No warmup job — pods write back directly. Accept cache pollution risk for now.
  const nixCacheEnabled = !!config.nixCache;
  if (nixCacheEnabled) {
    volumes.push({
      name: "attic-token",
      secret: { secretName: config.nixCache!.tokenSecretName },
    });
  }

  // Init container for git clone (if repo URL is provided)
  // Runs as root to chown /workspace to the main container's UID (1000)
  const initContainers: k8s.V1Container[] = repoUrl
    ? [
        {
          name: "git-clone",
          image: "alpine/git:2.45.2-r0",
          imagePullPolicy: "IfNotPresent",
          command: [
            "sh",
            "-c",
            "git clone --depth=1 \"$REPO_URL\" /workspace && chown -R 1000:1000 /workspace",
          ],
          env: [{ name: "REPO_URL", value: repoUrl }],
          volumeMounts: [{ name: "workspace", mountPath: "/workspace" }],
          securityContext: {
            runAsNonRoot: false,
            runAsUser: 0,
            allowPrivilegeEscalation: false,
            readOnlyRootFilesystem: false,
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
          command: nixCacheEnabled
            ? [
                "sh",
                "-c",
                [
                  // Write nix.conf with Attic as a substituter
                  'mkdir -p ~/.config/nix',
                  'cat > ~/.config/nix/nix.conf <<NIXEOF',
                  'experimental-features = nix-command flakes',
                  `substituters = https://cache.nixos.org ${config.nixCache!.endpoint}/${config.nixCache!.cache}`,
                  `trusted-public-keys = cache.nixos.org-1:6NCHdD59X431o0gWypbMrAURkbJ16ZPMQFGspcDShjY= ${config.nixCache!.cache}:${config.nixCache!.publicKey}`,
                  'NIXEOF',
                  // Login to Attic and start watching the store in background
                  `attic login opencode "${config.nixCache!.endpoint}" "$ATTIC_TOKEN"`,
                  `attic watch-store "opencode:${config.nixCache!.cache}" &`,
                  'exec sleep infinity',
                ].join("\n"),
              ]
            : ["sleep", "infinity"],
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
            ...(nixCacheEnabled
              ? [{ name: "attic-token", mountPath: "/var/run/secrets/attic", readOnly: true }]
              : []),
          ],
          securityContext: {
            runAsNonRoot: true,
            runAsUser: 1000,
            runAsGroup: 1000,
            readOnlyRootFilesystem: !nixCacheEnabled,
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
            {
              name: "HOME",
              value: "/workspace",
            },
            ...(nixCacheEnabled
              ? [
                  {
                    name: "ATTIC_TOKEN",
                    valueFrom: {
                      secretKeyRef: {
                        name: config.nixCache!.tokenSecretName,
                        key: config.nixCache!.tokenSecretKey,
                      },
                    },
                  },
                ]
              : []),
          ],
        },
      ],
      volumes,
      securityContext: {
        fsGroup: 1000,
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
