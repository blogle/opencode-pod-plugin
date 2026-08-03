# Production cluster setup

## Requirements

- Kubernetes 1.29+ with `SidecarContainers` enabled; Kubernetes 1.33+ is
  recommended because native sidecars are stable.
- A CNI that enforces NetworkPolicy.
- A default StorageClass with snapshot/backup support.
- Wildcard DNS/TLS and an ingress or Gateway API implementation.
- A trusted authentication proxy that strips client identity headers before
  setting the configured identity header.
- Access to immutable central, gateway, runtime, generic-Nix, and project image
  references.

## Install

Create the internal adapter token outside Git before applying the manifests:

```bash
kubectl create namespace opencode-system --dry-run=client -o yaml | kubectl apply -f -
kubectl -n opencode-system create secret generic adapter-internal-token \
  --from-literal=token="$(openssl rand -hex 32)"
kustomize build deploy/examples | kubectl apply -f -
```

Use a private Kustomize overlay to replace platform configuration, register
projects, set wildcard hosts, and pin first-party/project images by digest.
Manage the adapter token and any registry credentials with SealedSecrets, SOPS,
or an external secret controller. The checked-in base intentionally has no
credentials and no projects.

The current v1 checkout path supports repositories reachable without embedded
credentials. For private Git or private project registries, supply a controlled
organization overlay only after adding explicit deploy-key/imagePullSecret
references; never place credentials in project YAML or environment profiles.

## Network and security checks

Verify both namespaces enforce the supplied default-deny policies and that only
central can reach child OpenCode while gateway can reach preview and supervisor
ports. Sandbox Pods must have no service-account token, hostPath, Docker socket,
privileged mode, host network/PID, or added capabilities.

Apply Pod Security Admission labels appropriate for your project images and set
ResourceQuota/LimitRange values in an organization overlay. The generated
sandbox security context already drops capabilities, disables privilege
escalation, uses RuntimeDefault seccomp, and applies configured requests/limits.

## Durable state, backup, and upgrades

Central and gateway each use one PVC. Gateway SQLite and its checkpoint directory
must be backed up and restored as one unit. For a consistent backup:

1. prevent new launches;
2. suspend active workspaces;
3. scale central and gateway to zero;
4. snapshot both PVCs in the same maintenance window;
5. scale central, then gateway, back to one replica.

Restore central first, then the complete gateway PVC, and validate `/readyz`,
workspace records, and latest checkpoint metadata before admitting users. Test
every application and SQLite schema upgrade on a restored snapshot. v1 supports
one gateway replica and does not promise downgrade compatibility.

## Operations

Use workspace IDs to correlate gateway JSON logs, sandbox Pod labels, Services,
Secrets, and checkpoint metadata. A workspace in `error` preserves a sanitized
failure reason. Do not copy `.envrc`, authorization headers, Git credentials, or
runtime passwords into tickets or logs.

Run the exact local acceptance path before release:

```bash
nix develop --command just test
nix develop --command just check
nix develop --command just build
nix develop --command just e2e
```

The kind acceptance overlay consumes `deploy/base` directly and uses
port-forwards, a fake LLM, local fixture images, and development identity. It
does not require ingress, DNS, TLS, or an authentication proxy. Cluster-specific
SSO and wildcard routing remain target-cluster smoke tests.
