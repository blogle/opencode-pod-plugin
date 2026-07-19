# OpenCode Kubernetes Ephemeral Workspaces

This repository runs ordinary OpenCode sessions in isolated, disposable
Kubernetes workspaces. Central OpenCode owns durable conversation history; a
Rust gateway provisions sandbox Pods, stores compact Git checkpoints and
private environment profiles, and proxies preview traffic.

Each sandbox receives a stock child OpenCode `1.18.3` server through an injected
runtime bundle. OpenCode's native experimental workspace routing remains
authoritative: the platform does not replace built-in file, shell, terminal,
LSP, formatter, or project APIs and does not use Pod exec as a tool transport.

## Components

- `plugin/`: central `experimental_workspace` Kubernetes adapter.
- `gateway/`: lifecycle, SQLite state, Kubernetes orchestration, checkpoint
  storage, environment-profile API, launch page, and preview proxy.
- `runtime-plugin/`: dynamic direnv environment, activity, checkpoint, restart,
  and additive `preview` behavior inside child OpenCode.
- `supervisor/`: child process supervision and Git checkpoint sidecar.
- `runtime/`: exact-pin central and injected runtime images.
- `deploy/`: production base, kind overlay, and deployment examples.

Sandbox checkouts use `emptyDir`; there is no per-workspace PVC. The stable
Service, runtime credentials, workspace metadata, environment profile, central
session history, and latest Git checkpoint survive suspension.

## Development

Enter the pinned development shell:

```bash
nix develop
```

Canonical commands:

```bash
just test    # unit, CLI, adapter, and pinned-upstream compatibility gates
just check   # TypeScript, rustfmt, and clippy
just build   # production component builds
just e2e     # isolated deterministic kind lifecycle acceptance
```

`just e2e` builds local images, creates a uniquely named cluster with its own
explicit kubeconfig, runs the complete offline acceptance sequence, and deletes
the cluster. Set `KEEP_KIND_CLUSTER=1` to retain a failed cluster for diagnosis.
It does not require GitHub credentials, a commercial LLM key, public DNS, or an
external image registry.

## Environment profiles

The `ocws` helper manages private project `.envrc` profiles:

```bash
ocws env push --project my-project --file .envrc
ocws env status --project my-project
ocws env delete --project my-project
```

`ocws env push` can infer a registered project from the current Git remote.
Profile content is stored outside Git and is never returned by the metadata API.

## Deployment

Start with `deploy/base` and provide exact image references, project
registration, trusted identity-proxy settings, wildcard DNS/TLS, and a
Kubernetes NetworkPolicy-capable CNI. See `deploy/examples` for integration
examples and `SPEC_REV_2.md` for the complete product and security contract.
