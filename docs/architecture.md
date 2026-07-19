# Architecture

The persistent control plane consists of central OpenCode and the Rust gateway.
Central owns conversation history and upstream workspace records. Gateway owns
project registration, explicit lifecycle state, environment profiles, SQLite
metadata, Git checkpoint blobs, Kubernetes resources, and preview routing.

Each task is an upstream `experimental_workspace` whose target is a stable
ClusterIP Service. The Service selects one disposable sandbox Pod containing a
project container, injected stock child OpenCode, supervisor, and checkpoint
sidecar. `/workspace` is `emptyDir`; no sandbox receives a PVC or Kubernetes
service-account token.

Central OpenCode proxies workspace-scoped HTTP, SSE, and WebSocket requests to
the child. Global routes and durable session history stay central. The platform
does not implement replacements for OpenCode's built-in tools and does not use
Kubernetes exec as an execution transport.

Suspension requests a final authenticated checkpoint before deleting the Pod.
The checkpoint contains exact Git HEAD, index, working tree, and nonignored
untracked state. Resume recreates the Pod from repository, immutable image
identity, current private environment profile, and latest verified checkpoint.
Central replays the durable session projection before routing traffic.

Gateway continuously reconciles interrupted provisioning/resume operations and
recreates a missing Pod for a workspace recorded as running. Environment
fingerprint changes checkpoint and restart only child OpenCode at an idle
boundary; the Pod, Service, workspace, and central session identities remain.

Preview HTTP/WebSocket traffic enters gateway on the wildcard hostname
`<preview-key>-<port>.<base-domain>` and is sent directly to the current ready
Pod IP. OpenCode, supervisor, and checkpoint control ports are reserved.

OpenCode is pinned to version `1.18.3`, upstream commit
`127bdb30784d508cc556c71a0f32b508a3061517`. Central and child use the same
narrowly patched binary. `tests/compat/run.sh` fails if the pinned upstream
workspace contract or patches change.
