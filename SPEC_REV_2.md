# Product Specification: OpenCode Kubernetes Ephemeral Workspaces

**Status:** Implementation specification
**Target repository:** `blogle/opencode-pod-plugin`
**Primary objective:** Replace the current custom remote-tool sandbox implementation with a production-quality OpenCode-native remote workspace system using upstream OpenCode workspace routing and disposable Kubernetes sandboxes.
**Implementation mode:** The implementation agent is expected to complete the entire specification end to end in one run, including tests, manifests, documentation, and a local `kind` acceptance harness. Partial scaffolding, TODO-only implementations, or unverified architecture are not acceptable.

---

## 1. Executive directive to the implementation agent

Implement this specification as the new canonical architecture of the repository.

The current repository is a prototype and may be substantially refactored. Preserve useful code only where it still fits this design. In particular:

* Remove the custom overrides of OpenCode built-in filesystem and shell tools.
* Do not use `kubectl exec` or Kubernetes pod exec as the agent execution transport.
* Do not maintain a separate custom implementation of OpenCode's `bash`, `read`, `write`, `edit`, `glob`, `grep`, `apply_patch`, LSP, formatter, terminal, or project APIs.
* Implement a real OpenCode `experimental_workspace` adapter whose remote target is a stock child OpenCode server running inside the sandbox.
* Let upstream OpenCode workspace routing proxy workspace-scoped HTTP and WebSocket traffic to the child OpenCode server.
* Keep sandbox filesystems disposable. Do not add a per-sandbox PVC.
* Persist only control-plane state, environment profiles, and compact Git checkpoints required to recreate the sandbox exactly.
* Project repositories must not need to add OpenCode, Bun, platform plugins, or platform-specific scripts to their own development images.
* The platform must be able to inject its runtime into an existing development image.
* Projects without a registered development image must be able to use a generic Nix runner when they expose a usable flake.
* Pin all OpenCode packages and binaries to an exact version or exact upstream commit. Do not use `"latest"` anywhere in production dependencies.
* Central OpenCode and child OpenCode must run the exact same version.
* Build a deterministic end-to-end test harness that creates a local `kind` cluster and proves the complete lifecycle.

Before considering the implementation complete, run the entire validation suite described in this specification and fix all failures.

If an upstream OpenCode experimental API differs from the assumptions in this document, inspect the pinned upstream source and adapt the implementation while preserving the product behavior and invariants described here. Do not fall back to reimplementing OpenCode tools.

---

# 2. Product goal

Provide a small team with a central OpenCode experience in which each coding task executes inside its own isolated, ephemeral Kubernetes sandbox.

A user should be able to:

1. Choose a registered project.
2. Choose a Git branch or ref.
3. Create an OpenCode session backed by a dedicated Kubernetes sandbox.
4. Use ordinary stock OpenCode behavior:

   * agents,
   * built-in tools,
   * project plugins,
   * skills,
   * commands,
   * file browsing,
   * diffs,
   * terminals,
   * LSP,
   * formatters,
   * MCP,
   * approvals and permissions.
5. Start HTTP development servers and open them through a stable preview URL.
6. Leave work and allow the sandbox pod to be destroyed.
7. Return later and restore:

   * the same OpenCode conversation,
   * the same Git `HEAD`,
   * the same staged changes,
   * the same unstaged changes,
   * the same untracked non-ignored files.
8. Use their private, gitignored project `.envrc` without committing it to the repository.
9. Change `flake.nix`, `flake.lock`, or the environment profile during a session and have future commands use the current environment without rebuilding the pod.
10. Run multiple tasks against the same repository concurrently because every task receives a separate sandbox and checkout.

The central product is OpenCode. The custom platform exists only to provide project selection, workspace lifecycle, environment injection, checkpointing, and network routing.

---

# 3. Core architectural invariant

The system must preserve this split:

```text
Persistent control plane
    - central OpenCode session/event history
    - registered project metadata
    - workspace metadata
    - user/project environment profiles
    - latest workspace checkpoint bundle

Disposable workspace
    - repository checkout
    - Git index and working tree
    - OpenCode child runtime
    - project plugins and skills
    - LSP/MCP/formatter processes
    - package/build artifacts
    - development servers
```

The sandbox pod is a cacheable execution environment, not the source of durable session identity.

A sandbox may disappear at any time. The system must be able to create a replacement pod from durable inputs.

---

# 4. Terminology

## Control-plane OpenCode

The persistent OpenCode server and web application that the user connects to.

It owns durable conversation/session history and the upstream workspace registry.

## Workspace

An upstream OpenCode workspace record. A workspace corresponds to one isolated task environment.

The OpenCode workspace ID is the canonical platform workspace identifier.

## Sandbox

The Kubernetes resources used to realize a workspace at a particular moment.

A workspace may have no running sandbox while suspended.

## Child OpenCode

The stock OpenCode server running inside a sandbox pod.

It owns active workspace execution while the workspace is running.

## Gateway

The Rust control service. It combines:

* sandbox lifecycle API,
* Kubernetes orchestration,
* environment-profile API,
* checkpoint storage,
* launch/resume UI,
* preview-port reverse proxy.

The gateway is the evolution of the repository's existing Rust router.

## Runtime bundle

A platform-owned OCI image and shared filesystem payload containing:

* the exact pinned OpenCode executable,
* Bun if required by OpenCode/project plugins,
* `direnv`,
* the sandbox supervisor,
* the injected child OpenCode platform plugin,
* runtime helper binaries/scripts.

## Project development image

An optional project-owned OCI image containing the project's normal development toolchain.

It does not need to contain OpenCode, Bun, direnv, or platform integration.

## Generic Nix runner

A platform-owned fallback image containing a functional Nix installation, Git, shell utilities, and enough baseline runtime support to enter a repository flake.

## Environment profile

A user-specific, project-specific private `.envrc` supplied outside Git.

## Checkpoint

A compact Git bundle plus metadata capable of recreating the exact Git index/working-tree state of a suspended workspace.

---

# 5. Non-goals

The first implementation must not attempt to:

* Build a generic browser IDE.
* Replace OpenCode's chat UI.
* Reimplement OpenCode tools.
* Provide raw TCP preview forwarding.
* Provide Docker-in-Docker.
* Expose the Kubernetes API to sandbox pods.
* Persist arbitrary build artifacts when a sandbox is destroyed.
* Preserve running processes across suspension.
* Preserve ignored files such as `node_modules`, build directories, package caches, or other `.gitignore` content.
* Automatically build a custom project development image for every branch.
* Require repositories to commit platform-specific manifests or scripts.
* Guarantee compatibility with arbitrary non-Linux or distroless development images.
* Implement multi-region or high-availability control-plane storage.
* Add a custom Kubernetes CRD/operator unless implementation proves it is strictly necessary.

---

# 6. User experience

## 6.1 Launch page

The gateway must expose a minimal authenticated launch page.

The page lists:

* registered projects,
* active sessions/workspaces,
* suspended sessions/workspaces,
* current branch/ref,
* last activity,
* current sandbox state.

A user can create a task with:

```text
Project       [ dojo2                  ]
Git ref       [ feature/assets         ]
Session name  [ Assets implementation  ]

[ Create workspace ]
```

The launch page is intentionally thin. It must not reproduce OpenCode functionality.

The page may be simple server-rendered HTML. A JavaScript frontend framework is not required.

## 6.2 Create workflow

Creating a workspace must:

1. Resolve the authenticated user identity.
2. Resolve the registered project.
3. Ensure the central OpenCode project seed exists.
4. Create a central OpenCode session scoped to the project's seed directory.
5. Create an upstream OpenCode workspace using the `kubernetes` workspace adapter.
6. Pass:

   * project key,
   * requested Git ref,
   * authenticated owner identity,
   * environment profile identity,
   * project runtime configuration.
7. Wait for the gateway to provision the sandbox and for child OpenCode health to become ready.
8. Warp/bind the central session to the remote workspace using the upstream workspace API.
9. Redirect the browser to the normal central OpenCode session route.

Do not create sandboxes from the generic `session.created` plugin event.

The supported creation path must establish project and branch before sandbox creation.

## 6.3 Resume workflow

If a user opens a suspended task from the launch page:

1. The gateway recreates the sandbox with the same workspace ID.
2. It uses the workspace's recorded project image digest, or the configured generic runner.
3. It checks out the original checkpoint base `HEAD`.
4. It restores the latest checkpoint exactly.
5. It mounts the current environment profile.
6. It starts child OpenCode.
7. The existing stable workspace Service gains a ready endpoint.
8. Central OpenCode's existing remote-workspace synchronization reconnects.
9. The user opens the same central OpenCode session.

Conversation history must not be copied from the sandbox during normal resume; it already belongs to the central control plane.

## 6.4 Suspend workflow

A running workspace may be suspended manually or after an idle timeout.

Suspension must:

1. Request a final checkpoint.
2. Wait for checkpoint success.
3. Gracefully terminate the sandbox Pod.
4. Keep:

   * the upstream workspace record,
   * the stable Kubernetes Service,
   * the workspace runtime-auth Secret,
   * gateway workspace metadata,
   * latest checkpoint,
   * environment profile.
5. Mark the workspace `suspended`.

If final checkpoint fails, do not silently destroy the pod unless a hard administrative force-delete is requested.

## 6.5 Delete workflow

Deleting a workspace permanently must delete:

* sandbox Pod,
* stable workspace Service,
* workspace runtime-auth Secret,
* workspace metadata,
* checkpoint bundles,
* any workspace-specific ephemeral secrets.

Deleting a workspace must not delete the user's reusable project environment profile by default.

---

# 7. System architecture

```text
                        Browser / OpenCode TUI
                                 |
                                 v
                        Central OpenCode
                     persistent session history
                                 |
                     Kubernetes WorkspaceAdapter
                                 |
                                 v
                         Rust Gateway API
                +----------------+----------------+
                |                |                |
                v                v                v
         Kubernetes API   checkpoint store   env profiles
                |
                v
     stable Service per workspace
                |
                v
          ephemeral sandbox Pod
    +--------------------------------+
    | runtime init container         |
    | checkout/restore init          |
    | project development container  |
    | checkpoint sidecar             |
    +--------------------------------+
                |
                +---- child OpenCode :4096
                |
                +---- arbitrary dev servers

Preview hostname:
    <workspace-id>-<port>.<base-domain>
                  |
                  v
             Rust Gateway
                  |
                  v
              Pod IP:port
```

---

# 8. Repository target structure

Refactor toward the following logical layout. Exact package nesting may vary, but responsibilities must remain separated.

```text
.
├── plugin/
│   ├── src/
│   │   ├── central.ts
│   │   ├── adapter.ts
│   │   └── client.ts
│   ├── package.json
│   └── tests/
│
├── runtime-plugin/
│   ├── src/
│   │   ├── index.ts
│   │   ├── environment.ts
│   │   └── lifecycle.ts
│   └── tests/
│
├── gateway/
│   ├── Cargo.toml
│   └── src/
│       ├── main.rs
│       ├── api/
│       ├── auth/
│       ├── checkpoint/
│       ├── config/
│       ├── k8s/
│       ├── preview/
│       ├── state/
│       └── ui/
│
├── supervisor/
│   ├── Cargo.toml
│   └── src/
│
├── runtime/
│   ├── Dockerfile
│   ├── generic-nix.Dockerfile
│   └── entrypoint/
│
├── deploy/
│   ├── base/
│   ├── kind/
│   └── examples/
│
├── config/
│   ├── projects.example.yaml
│   └── platform.example.yaml
│
├── tests/
│   ├── e2e/
│   ├── fixtures/
│   │   ├── git-server/
│   │   ├── project-dev-image/
│   │   ├── nix-project/
│   │   └── fake-llm/
│   └── scripts/
│
├── hack/
│   ├── e2e.sh
│   └── kind.sh
│
├── flake.nix
├── README.md
└── SPEC.md
```

The old custom tool override files should be removed when no longer used.

---

# 9. Upstream OpenCode integration

## 9.1 Version pinning

The repository must define one exact OpenCode version or source commit.

The same version must be used by:

* control-plane OpenCode,
* child OpenCode runtime image,
* `@opencode-ai/plugin`,
* `@opencode-ai/sdk`.

A version mismatch between control plane and child must fail readiness with a clear error.

No production package may depend on `@opencode-ai/plugin: latest`.

## 9.2 Central workspace adapter plugin

The central plugin must register one workspace adapter:

```text
type: kubernetes
name: Kubernetes Sandbox
```

The adapter must implement upstream:

* `configure`
* `create`
* `remove`
* `target`

### `configure`

Must produce deterministic workspace metadata and retain:

* project key,
* Git ref,
* owner identity when supplied,
* optional runtime overrides.

### `create`

Must call the gateway:

```http
POST /v1/workspaces
```

It must pass the upstream environment provided to the adapter, including OpenCode auth material required by the child runtime.

The gateway must treat duplicate creates for the same workspace ID as idempotent.

### `remove`

Must call:

```http
DELETE /v1/workspaces/{workspaceID}
```

and permanently purge that workspace.

### `target`

Must return a stable in-cluster URL, not the current Pod IP:

```text
http://workspace-<workspace-id>.<sandbox-namespace>.svc.cluster.local:4096
```

It must include the child OpenCode Basic Auth header obtained from the gateway.

A Kubernetes Service must exist for the lifetime of the workspace, including while the Pod is suspended.

## 9.3 Upstream routing is authoritative

Do not implement a custom route allowlist for OpenCode workspace behavior unless upstream requires a compatibility patch.

The design depends on upstream workspace routing to forward workspace-scoped HTTP and WebSocket APIs to the child OpenCode server while keeping global/control-plane routes central.

The test harness must explicitly prove this behavior for:

* file read/list/search,
* VCS status/diff,
* session prompt execution,
* event streaming,
* WebSocket/PTY where supported,
* project skills,
* LSP/formatter status when configured.

## 9.4 Compatibility gate

Because the workspace API is experimental, create an automated compatibility test that fails clearly when the pinned upstream version no longer exposes the required adapter and remote-workspace behavior.

The implementation must not silently degrade to the old custom tool override architecture.

---

# 10. Project registration

Projects must be registered centrally. No repository change is required.

Example:

```yaml
projects:
  dojo2:
    name: Dojo
    repository: https://github.com/blogle/dojo2.git
    defaultRef: master

    environment:
      mode: image
      image: ghcr.io/blogle/dojo2:dev

    resources:
      requests:
        cpu: "500m"
        memory: "1Gi"
      limits:
        cpu: "4"
        memory: "8Gi"

  another-project:
    name: Another Project
    repository: https://github.com/example/another-project.git
    defaultRef: main

    environment:
      mode: nix
      flake: ".#default"
```

Supported environment modes:

### `image`

Use the configured existing project development image.

The platform injects its runtime.

The project image must not be required to contain OpenCode, Bun, or direnv.

### `nix`

Use the platform generic Nix runner and enter the project's flake.

### `auto`

Optional convenience mode.

If implemented, `auto` may use a configured `dev` image convention and fall back to the generic Nix runner. It must not make image resolution nondeterministic.

If automatic registry probing adds significant complexity, implement `image` and `nix` first and make project registration the explicit control point.

## 10.1 Image ownership

The platform does not build project development images in v1.

Project CI or the project owner publishes them independently.

New workspaces may start from a mutable tag such as:

```text
ghcr.io/blogle/dojo2:dev
```

Once the first Pod is running, the gateway must record the immutable resolved image digest from Kubernetes status.

A resumed workspace must use the recorded digest so an existing task does not silently change environments.

A newly created workspace may resolve the current `:dev` tag again.

---

# 11. Project seed catalog for central OpenCode

Central OpenCode needs lightweight local Git roots so sessions can be associated with real OpenCode projects before they are warped into a remote workspace.

The central OpenCode deployment must include a catalog init/sync process that creates:

```text
/catalog/<project-key>
```

for every registered project.

The seed checkout may be:

* shallow,
* blob-filtered,
* default branch only.

It is not an execution environment.

The launch page uses a directory-scoped OpenCode SDK client pointed at:

```text
/catalog/<project-key>
```

to resolve the central OpenCode project and create the initial session.

Catalog data may be recreated on central OpenCode restart and does not require a dedicated PVC in v1.

---

# 12. Sandbox Pod specification

Each running workspace must be realized by one Pod and one stable Service.

Do not create a PVC for the sandbox workspace.

## 12.1 Volumes

At minimum:

```text
workspace       emptyDir
runtime         emptyDir
runtime-state   emptyDir
env-profile     Secret volume, optional
runtime-auth    Secret volume
```

Optional temporary volumes may be added for caches.

## 12.2 Init container: runtime injection

Use the platform runtime image.

It copies the runtime payload into the shared runtime volume:

```text
/opt/opencode/
├── bin/
│   ├── opencode
│   ├── bun
│   ├── direnv
│   └── supervisor
└── config/
    └── plugins/
        └── platform-runtime.js
```

The runtime volume must be readable and executable regardless of the project image's default user.

## 12.3 Init container: checkout and restore

Use a platform-owned image with Git.

It must:

1. Clone the requested repository and exact requested ref into `/workspace`.
2. Record the resulting exact `HEAD`.
3. If a checkpoint exists:

   * reset/checkout the checkpoint's recorded base `HEAD`,
   * download the checkpoint bundle,
   * restore it,
   * verify the restored Git state fingerprint.
4. If an environment profile exists:

   * verify `.envrc` is not tracked by the repository,
   * create a symlink:

     ```text
     /workspace/.envrc -> /run/opencode-env/profile.envrc
     ```
5. Prepare permissions so the project container can write the workspace.

If a tracked `.envrc` exists and a private environment profile is configured, fail with a clear error rather than silently replacing tracked repository content.

## 12.4 Main project container

The main container image is either:

* the registered project development image, or
* the generic Nix runner.

Its normal image entrypoint is overridden with:

```text
/opt/opencode/bin/supervisor
```

The supervisor starts child OpenCode on port `4096`.

The child must be configured with:

```text
OPENCODE_WORKSPACE_ID=<workspace-id>
OPENCODE_EXPERIMENTAL_WORKSPACES=true
OPENCODE_CONFIG_DIR=/opt/opencode/config
OPENCODE_SERVER_USERNAME=opencode
OPENCODE_SERVER_PASSWORD=<random per workspace>
```

and the upstream auth content supplied by central OpenCode.

The supervisor must launch OpenCode through the current project environment when possible:

```text
direnv exec /workspace opencode serve ...
```

If no environment profile or tracked `.envrc` exists, the project may still use its normal image environment.

For `nix` mode, the generic runner must synthesize or use a direnv entry that enters the configured flake.

## 12.5 Checkpoint sidecar

Run a platform-owned checkpoint sidecar sharing `/workspace`.

It must contain Git and expose a private localhost-only API to the runtime plugin/supervisor.

Responsibilities:

* generate checkpoints,
* upload checkpoints to the gateway,
* report checkpoint health,
* restore support may remain in the checkout init container.

The checkpoint sidecar avoids requiring every arbitrary project development image to include a particular Git implementation for platform lifecycle operations.

## 12.6 Stable Service

Create one ClusterIP Service per workspace:

```text
workspace-<workspace-id>
```

selector:

```text
opencode.dev/workspace-id=<workspace-id>
```

port:

```text
4096 -> 4096
```

The Service remains while the workspace is suspended.

---

# 13. Runtime injection and project-image compatibility

The platform must prove that a project development image does not need OpenCode-specific wiring.

The fixture development image used by E2E tests must intentionally omit:

* OpenCode,
* Bun,
* direnv,
* platform plugins.

The sandbox must nevertheless start successfully because these are injected.

## 13.1 Minimum image contract

For `image` mode, v1 may require:

* Linux,
* a supported CPU architecture,
* a usable process environment,
* ability to execute the injected OpenCode binary,
* writable mounted `/workspace`.

The platform must document supported libc/architecture combinations.

If an image is incompatible with injected OpenCode, fail clearly and recommend `nix` mode or a compatible dev image.

Do not require a repository-side integration package.

## 13.2 Nix capability

If the project `.envrc` uses Nix and the project image does not provide a functional Nix installation, sandbox readiness must fail with an actionable error.

The generic Nix runner is the supported fallback.

---

# 14. Dynamic development environment behavior

A long-lived OpenCode server must not freeze shell commands to the environment that existed when the process started.

## 14.1 Dynamic shell environment

The injected child runtime plugin must implement OpenCode's `shell.env` hook.

Before each OpenCode shell-tool invocation, it must evaluate the environment for the command's working directory using injected `direnv`.

Conceptually:

```text
cd <cwd>
direnv export json
```

The resulting environment delta is merged into the shell environment returned by the hook.

This preserves the stock upstream `bash` tool and its:

* permission parsing,
* process handling,
* cancellation,
* timeout,
* output streaming,
* truncation,
* metadata.

Do not replace the `bash` tool.

## 14.2 Nix changes during a session

If an agent edits:

* `flake.nix`,
* `flake.lock`,
* the authorized environment profile,

then the next shell-tool invocation must evaluate the new environment.

The pod must not be rebuilt merely because Nix dependencies changed.

If realizing the new Nix environment downloads or builds new store paths, that happens inside the current sandbox.

## 14.3 Long-lived subprocess refresh

LSP, MCP, formatter, and other processes may inherit the environment of the child OpenCode process.

The runtime must track an environment fingerprint containing at least:

* `.envrc` content hash,
* `flake.nix` content hash when present,
* `flake.lock` content hash when present.

When the fingerprint changes:

1. Mark the OpenCode runtime `environment-dirty`.
2. Continue allowing the current agent turn to complete.
3. At the next `session.idle`:

   * checkpoint,
   * request supervisor restart of child OpenCode.
4. Restart only child OpenCode, not the Pod.
5. Start the new child process through the current `direnv` environment.
6. Allow the central workspace sync connection to reconnect.

The E2E harness must prove that a child OpenCode restart does not lose the central session.

---

# 15. Private `.envrc` environment profiles

A user may upload a gitignored local `.envrc` for a registered project.

## 15.1 API

The gateway must provide:

```http
PUT /v1/projects/{projectKey}/env-profile
DELETE /v1/projects/{projectKey}/env-profile
GET /v1/projects/{projectKey}/env-profile/meta
```

The authenticated user identity is part of the storage key.

The GET endpoint returns metadata only, never secret content.

Metadata includes:

* SHA-256,
* update timestamp,
* project key,
* owner identity.

## 15.2 CLI

Provide a small CLI:

```bash
ocws env push --project dojo2 --file .envrc
ocws env status --project dojo2
ocws env delete --project dojo2
```

The common shortcut should work from a registered project directory:

```bash
ocws env push
```

when the CLI can infer the project from the Git remote.

## 15.3 Storage

Store environment profiles as Kubernetes Secrets or an equivalently protected backend.

One profile is keyed by:

```text
user identity + project key
```

Do not put `.envrc` in:

* the project image,
* the Git checkpoint,
* the central project seed.

## 15.4 Runtime

The Secret volume is mounted read-only.

The sandbox symlinks the profile into `/workspace/.envrc`.

Because this profile is platform-authorized, the supervisor may perform the equivalent of `direnv allow` automatically.

Do not automatically authorize arbitrary repository-authored changes to a tracked `.envrc`.

## 15.5 Updates while running

When a profile Secret changes:

1. detect the new content hash,
2. re-authorize the broker-owned `.envrc`,
3. make new shell commands use the new environment,
4. restart child OpenCode at the next idle boundary.

---

# 16. Checkpoint specification

The checkpoint system must recreate the exact ordinary Git working state without a sandbox PVC.

## 16.1 State that must be preserved

Required:

* exact `HEAD` commit,
* branch/ref metadata,
* staged modifications,
* unstaged modifications,
* staged additions,
* unstaged untracked additions,
* staged deletions,
* unstaged deletions,
* executable-bit changes representable by Git.

Not required:

* ignored files,
* package caches,
* build outputs,
* running processes,
* unresolved exotic index states unless naturally supported.

## 16.2 Default implementation

Implement checkpoint creation using a temporary Git stash transaction because Git stash natively represents separate index and working-tree states.

At a checkpoint boundary:

1. Acquire a workspace checkpoint lock.
2. Capture:

   ```bash
   git status --porcelain=v2 -z --untracked-files=all
   ```

   as the pre-checkpoint state fingerprint.
3. Record exact `HEAD`.
4. Execute:

   ```bash
   git stash push --include-untracked --message "opencode checkpoint <workspace-id>"
   ```
5. Capture the resulting stash commit OID.
6. Create a temporary ref pointing at that commit.
7. Create a compact Git bundle containing the checkpoint objects and the required base prerequisite.
8. Restore the live working state:

   ```bash
   git stash apply --index <stash-oid>
   ```
9. Remove only the checkpoint-created stash entry and temporary ref.
10. Capture the post-checkpoint `git status --porcelain=v2 -z --untracked-files=all`.
11. Require the pre/post fingerprints to be byte-identical.
12. Upload the bundle and metadata to the gateway.
13. Atomically mark the uploaded checkpoint as the latest successful checkpoint.

If the stash implementation cannot satisfy the exact-state tests, replace the internal capture implementation with a non-mutating Git plumbing implementation while preserving the external checkpoint contract.

## 16.3 Checkpoint metadata

Store:

```json
{
  "workspaceId": "wrk_...",
  "createdAt": "...",
  "head": "<sha>",
  "branch": "feature/assets",
  "statusSha256": "<sha256>",
  "bundleSha256": "<sha256>",
  "formatVersion": 1
}
```

## 16.4 Storage backend

For v1, the gateway may use:

* a single control-plane PVC,
* SQLite for metadata,
* files on the same PVC for bundle blobs.

Design an internal storage interface so S3-compatible storage can be added later.

Do not create a PVC per sandbox.

## 16.5 Trigger policy

Checkpoint at:

* `session.idle` after a dirty turn,
* manual suspend,
* before idle-timeout deletion,
* periodically while dirty, default every 120 seconds,
* graceful Pod termination when possible.

The system must assume a Pod can die without a final hook. The most recent successful checkpoint is therefore the recovery point.

## 16.6 Restore verification

Restore must:

1. checkout the recorded exact `HEAD`,
2. fetch the checkpoint bundle,
3. apply the checkpoint with index restoration,
4. compute the resulting status fingerprint,
5. compare it to recorded checkpoint metadata.

If verification fails, the workspace must enter `error` and preserve diagnostic logs. Do not start child OpenCode against a silently incorrect checkout.

---

# 17. Gateway responsibilities

Evolve the current Rust router into the single gateway/control service.

## 17.1 Responsibilities

* project registry loading,
* launch page,
* workspace lifecycle API,
* Kubernetes Pod/Service/Secret orchestration,
* workspace state tracking,
* checkpoint storage,
* environment-profile API,
* preview reverse proxy,
* authentication identity extraction,
* health/readiness endpoints,
* structured logs and metrics.

## 17.2 State

Use SQLite for durable control metadata in v1.

Run one gateway replica.

SQLite and checkpoint bundles live on one control-plane PVC.

Kubernetes remains the source of truth for actual running Pod state.

## 17.3 Workspace state machine

Use explicit states:

```text
provisioning
running
checkpointing
suspending
suspended
resuming
deleting
deleted
error
```

Do not represent lifecycle using ambiguous booleans.

All lifecycle mutations must be idempotent.

## 17.4 API

Minimum API:

```text
GET    /healthz
GET    /readyz

GET    /v1/projects
GET    /v1/workspaces
POST   /v1/workspaces
GET    /v1/workspaces/:id
POST   /v1/workspaces/:id/ensure
POST   /v1/workspaces/:id/suspend
DELETE /v1/workspaces/:id

POST   /v1/workspaces/:id/activity
POST   /v1/workspaces/:id/checkpoints
GET    /v1/workspaces/:id/checkpoints/latest

PUT    /v1/projects/:project/env-profile
GET    /v1/projects/:project/env-profile/meta
DELETE /v1/projects/:project/env-profile

POST   /v1/launch
```

Internal endpoints used by child runtime may use a workspace-scoped bearer token.

Never expose raw Kubernetes credentials.

## 17.5 Workspace creation response

Return enough information for the adapter:

```json
{
  "workspaceId": "wrk_...",
  "state": "running",
  "target": {
    "url": "http://workspace-wrk-....opencode-sandboxes.svc.cluster.local:4096",
    "username": "opencode",
    "password": "..."
  }
}
```

Do not log the password.

## 17.6 Runtime authentication

Generate a random OpenCode server password per workspace.

Store it in a workspace Secret.

The central WorkspaceAdapter receives it from the gateway and passes Basic Auth headers in the remote target.

Sandbox runtime tokens used to call the gateway must be scoped to their own workspace.

---

# 18. Preview routing

The gateway must route HTTP and WebSocket preview traffic directly to sandbox Pod IPs.

Use hostnames:

```text
<workspace-id>-<port>.<base-domain>
```

The exact workspace ID may be shortened with a collision-safe mapping if necessary to fit DNS label limits.

Examples:

```text
abc123-5173.opencode.example.com
abc123-8000.opencode.example.com
```

## 18.1 Routing behavior

The gateway:

1. parses workspace and port,
2. authenticates the user when auth is enabled,
3. verifies workspace authorization,
4. resolves the current ready Pod IP from a Kubernetes watch/cache,
5. rejects reserved control ports,
6. proxies HTTP, WebSocket, and SSE,
7. returns a friendly unavailable response when no process listens.

No Kubernetes Service or Ingress is required per preview port.

## 18.2 Reserved ports

At minimum, block external preview routing to:

* child OpenCode port `4096`,
* checkpoint sidecar ports,
* supervisor control ports.

## 18.3 Runtime helper

The injected child plugin should expose a small custom tool:

```text
preview(port)
```

It returns:

```text
https://<workspace-id>-<port>.<base-domain>
```

The tool may optionally verify the port is listening.

This is additive. It must not override built-in OpenCode tools.

---

# 19. Sandbox security

Required sandbox security posture:

* no Kubernetes service account token,
* no hostPath,
* no Docker socket,
* no privileged container,
* no host PID/network namespace,
* `allowPrivilegeEscalation: false`,
* drop Linux capabilities unless explicitly required,
* seccomp `RuntimeDefault`,
* CPU/memory requests and limits,
* default-deny inbound sandbox traffic where the cluster CNI supports policy,
* allow child OpenCode ingress only from central control plane,
* allow preview ingress only from gateway,
* no direct public ingress to child OpenCode.

A project dev image may require project-specific security overrides. Those must be explicit in central project configuration and disabled by default.

Production auth may be supplied by an external auth proxy. The gateway must support a configurable trusted identity header.

Local E2E mode may use a fixed development user.

---

# 20. Idle lifecycle

The child runtime plugin must report activity to the gateway.

Activity includes:

* session messages,
* tool execution,
* session idle transitions.

Default policy:

```text
checkpointInterval: 120s while dirty
suspendAfterIdle: 60m
```

The gateway must make these configurable.

When the idle threshold is reached:

1. request/check latest checkpoint,
2. suspend Pod,
3. retain stable Service and workspace record.

The E2E test configuration should use very short intervals.

---

# 21. Central session and workspace recovery

The system must preserve the distinction:

```text
OpenCode session = durable conversation
OpenCode workspace = durable execution binding
Sandbox Pod       = disposable realization
```

A control-plane restart must not delete workspace metadata.

A child Pod restart must not create a new central OpenCode session.

A suspended workspace must be able to resume under the same upstream workspace ID and stable Service DNS name.

---

# 22. Logging and observability

All custom components must use structured logs.

Every workspace-related log should include when available:

* workspace ID,
* project key,
* owner,
* Pod name,
* operation,
* state transition.

Gateway metrics should include:

* workspaces by state,
* sandbox provisioning duration,
* sandbox resume duration,
* checkpoint success/failure counts,
* checkpoint duration and bytes,
* preview proxy connections,
* Kubernetes watch errors.

Do not log:

* `.envrc` content,
* model credentials,
* Git credentials,
* OpenCode server passwords,
* authorization headers.

---

# 23. Testing strategy

The repository must have three test layers.

## 23.1 Unit tests

Cover pure behavior without Kubernetes.

### Central plugin

* adapter registration,
* configure metadata,
* create/remove/target gateway calls,
* auth header construction,
* idempotent error handling.

### Runtime plugin

* `shell.env` direnv parsing,
* environment fingerprint changes,
* idle reload request,
* activity reporting,
* preview URL generation.

### Gateway

* config parsing,
* workspace state transitions,
* image selection,
* auth identity mapping,
* preview hostname parsing,
* reserved-port rejection,
* checkpoint metadata,
* API idempotency.

### Checkpoint helper

Use temporary Git repositories to test exact preservation of:

1. only staged change,
2. only unstaged change,
3. staged and unstaged changes to different files,
4. staged and unstaged changes to the same file,
5. untracked file,
6. staged new file,
7. staged deletion,
8. unstaged deletion,
9. executable bit change,
10. existing unrelated user stash.

For every case:

```text
capture checkpoint
destroy original checkout
fresh clone at recorded HEAD
restore checkpoint
compare:
    git status --porcelain=v2 -z --untracked-files=all
    file bytes
    executable bits
```

The test must prove staged/unstaged distinctions survive.

## 23.2 Kubernetes integration tests

Run against `kind`.

Test:

* Pod construction,
* Service stability,
* Secret mounting,
* runtime injection,
* environment profile symlink,
* child health,
* Pod deletion/recreation,
* router Pod watch.

## 23.3 Full end-to-end acceptance harness

Provide one command:

```bash
./hack/e2e.sh
```

or:

```bash
nix develop --command just e2e
```

It must create, exercise, and destroy its own `kind` cluster.

The harness must not require:

* a real LLM API key,
* GitHub credentials,
* public DNS,
* cert-manager,
* an external registry.

It may require Docker or another `kind`-supported container runtime.

---

# 24. Required `kind` end-to-end harness

## 24.1 Cluster setup

The harness must:

1. Create a uniquely named `kind` cluster.
2. Build all test images locally.
3. Load them into the cluster using `kind load docker-image` or a harness-local registry.
4. Deploy:

   * gateway,
   * central OpenCode,
   * central WorkspaceAdapter plugin,
   * fixture Git server,
   * fake LLM provider,
   * required RBAC,
   * sandbox namespace.
5. Wait for readiness.
6. Run acceptance tests.
7. On success, destroy the cluster.
8. On failure, preserve or print:

   * Pod logs,
   * events,
   * resource YAML,
   * workspace state,
   * checkpoint diagnostics.
9. Support:

   ```text
   KEEP_KIND_CLUSTER=1
   ```

   to preserve the failed cluster for debugging.

## 24.2 Fixture Git server

Do not depend on GitHub.

Run an in-cluster Git server containing a fixture repository.

The fixture repository must include:

```text
README.md
tracked.txt
server.py
.opencode/
    skills/
        fixture-skill/
            SKILL.md
    plugins/
        fixture-plugin.ts or compiled equivalent
```

The fixture project must be able to start a simple HTTP server for preview tests.

## 24.3 Fixture project development image

Build a development image that intentionally does not contain:

* OpenCode,
* Bun,
* direnv,
* the platform runtime plugin.

It should contain only the normal fixture project toolchain and basic OS/runtime dependencies.

The test must prove the platform runtime was injected rather than preinstalled.

## 24.4 Fake LLM provider

Implement a deterministic local provider compatible with the OpenCode provider configuration used by the harness.

It must support enough tool-calling behavior to run a scripted OpenCode agent turn.

The scripted turn must cause stock child OpenCode to invoke at least one built-in shell or file tool.

No network call to a commercial LLM may occur.

## 24.5 E2E acceptance sequence

The harness must execute the following sequence.

### Test A: create a remote workspace

1. Create a central session for the fixture project.
2. Create a Kubernetes workspace on branch `main`.
3. Warp the session into the workspace.
4. Assert:

   * one sandbox Pod exists,
   * no workspace PVC exists,
   * stable Service exists,
   * child OpenCode health is reachable through the Service,
   * central and child OpenCode versions match.

### Test B: prove upstream workspace routing

Through the central OpenCode SDK/server, with the session bound to the workspace:

1. read a fixture file,
2. list files,
3. run text/file search,
4. inspect VCS status.

Assert the results describe the child sandbox checkout, not the central project seed.

Where supported by the pinned upstream version, also verify a workspace-routed event or WebSocket endpoint.

### Test C: prove stock agent execution

Send a deterministic prompt through the central session.

The fake LLM must instruct child OpenCode to invoke a stock built-in tool that modifies the sandbox.

Assert:

* the tool runs in the child sandbox,
* the resulting file exists in `/workspace`,
* no custom tool override implementation was used,
* the central session receives the resulting tool event and final assistant output.

### Test D: prove project-local OpenCode content

Assert the child runtime discovers the fixture repository's:

* project skill,
* project plugin or custom tool where feasible.

This proves project-local OpenCode configuration is loaded from the actual sandbox checkout.

### Test E: prove runtime injection

Assert the base fixture dev image itself does not contain OpenCode/Bun/direnv.

Assert the running sandbox can execute the injected OpenCode runtime.

### Test F: environment profile

1. Upload a private fixture `.envrc` through the gateway API.
2. Recreate or launch a workspace for that user/project.
3. Assert:

   * `/workspace/.envrc` is a symlink to the mounted Secret,
   * the file is not part of Git status/checkpoint,
   * a stock OpenCode shell command sees the injected environment variable.
4. Update the environment profile.
5. Assert a subsequent shell command sees the new value.

### Test G: exact checkpoint and stateless resume

Inside the workspace create all of:

* one staged modification,
* one unstaged modification,
* one file with both staged and later unstaged content,
* one untracked non-ignored file,
* one staged new file,
* one deletion.

Record:

```bash
git status --porcelain=v2 -z --untracked-files=all
```

and relevant file hashes.

Trigger checkpoint.

Delete the sandbox Pod and verify:

* no workspace PVC exists,
* workspace state becomes suspended or unavailable.

Resume the same workspace.

Assert:

* a new Pod UID exists,
* workspace ID is unchanged,
* Service DNS is unchanged,
* exact `HEAD` is restored,
* Git status output is byte-identical,
* file hashes are identical,
* staged/unstaged distinctions are identical.

### Test H: central session survives sandbox replacement

After Test G, continue the same central OpenCode session.

Assert:

* prior conversation messages remain,
* the same session ID is used,
* a new agent turn can inspect the restored files.

### Test I: preview routing

Use the agent or a direct test command inside the sandbox to start:

```text
python3 -m http.server 18080
```

Access the gateway using a Host header representing:

```text
<workspace-id>-18080.test.invalid
```

Assert:

* HTTP response comes from the sandbox server,
* WebSocket proxy behavior is separately tested with a tiny fixture WebSocket server if practical,
* reserved child OpenCode port `4096` is rejected.

### Test J: concurrent isolation

Create two workspaces for the same fixture repository and branch.

Assert:

* separate Pods,
* separate workspaces,
* a file written in workspace A does not appear in workspace B,
* preview routes resolve to the correct Pod.

### Test K: child OpenCode restart

Cause the environment fingerprint to change.

Trigger/await a session idle boundary.

Assert:

* child OpenCode process identity changes,
* Pod UID does not change,
* central session ID does not change,
* workspace Service does not change,
* the session can continue.

### Test L: cleanup

Delete a workspace permanently.

Assert:

* Pod removed,
* stable Service removed,
* runtime-auth Secret removed,
* checkpoint blobs removed,
* environment profile remains unless explicitly deleted.

---

# 25. Nix-specific acceptance test

Add a dedicated Nix fixture or gated E2E case.

The goal is to prove:

1. child OpenCode starts in a project environment,
2. an agent changes the flake/environment definition,
3. the next shell command resolves the new environment without Pod recreation,
4. after idle restart, long-lived child OpenCode processes inherit the new environment.

Keep the fixture minimal and deterministic.

Prefer a fixture whose required Nix store inputs are preloaded into the test image so CI does not depend on a large external Nix download.

If that is impractical for the first pass, the test may be marked as a separate required CI job with cache support, but the implementation must still include it.

---

# 26. Failure-injection tests

The E2E harness must include at least these failure cases:

## Child Pod killed without graceful shutdown

* Kill the sandbox Pod.
* Resume from latest successful checkpoint.
* Verify no control-plane session loss.

## Gateway restart

* Restart gateway.
* Verify SQLite state and checkpoints remain.
* Verify existing running workspace routing recovers.

## Child OpenCode process crash

* Kill only child OpenCode inside the Pod.
* Supervisor restarts it.
* Central workspace reconnects.

## Invalid environment profile

* Upload an `.envrc` that fails.
* Workspace reports actionable environment error.
* No silent fallback to an incorrect environment.

## Checkpoint restore verification failure

Inject a corrupted bundle or wrong fingerprint.

The system must enter an error state and refuse to present the checkout as successfully restored.

---

# 27. Production deployment requirements

Provide Kustomize manifests for:

```text
opencode-system
    central OpenCode
    gateway
    gateway PVC
    project config
    runtime configuration

opencode-sandboxes
    ephemeral Pods created dynamically
    stable per-workspace Services
    workspace runtime Secrets
    user/project environment Secrets
```

Document integration with:

* Traefik,
* wildcard DNS,
* wildcard TLS,
* external auth proxy,
* SealedSecrets or equivalent,
* optional Attic/Nix binary cache.

Do not require Traefik or cert-manager for the `kind` harness.

---

# 28. Performance and cold-start requirements

The architecture should minimize startup work in this order:

1. Existing project `:dev` image supplies the common toolchain.
2. Kubernetes/node image cache avoids repeated pulls.
3. Runtime injection image is small and independently cacheable.
4. Checkout uses shallow/filter clone where safe.
5. Generic Nix fallback uses configured binary cache.
6. New Nix dependencies may be realized inside the running sandbox.
7. No project image is built synchronously during workspace creation.

Record provisioning timing metrics for:

* Pod scheduled,
* image ready,
* checkout complete,
* child OpenCode healthy,
* workspace connected.

Do not introduce a warm-pod pool until measurements prove it is needed.

---

# 29. Configuration

Provide typed configuration with validation.

Example platform config:

```yaml
namespace: opencode-sandboxes
baseDomain: opencode.example.com

opencode:
  version: "<exact pinned version>"
  centralUrl: http://opencode.opencode-system.svc.cluster.local:4096

runtime:
  image: ghcr.io/example/opencode-sandbox-runtime:<exact-version>
  genericNixImage: ghcr.io/example/opencode-sandbox-nix:<exact-version>

checkpoint:
  path: /var/lib/opencode-sandbox/checkpoints
  periodicSeconds: 120

lifecycle:
  suspendAfterIdleSeconds: 3600

auth:
  mode: trusted-header
  identityHeader: X-Auth-Request-Email

projectsFile: /etc/opencode-sandbox/projects.yaml
```

No secret values should be embedded in this file.

---

# 30. Migration from the current repository

The current codebase should be treated as an implementation prototype.

The migration must:

1. Keep the Rust routing foundation where useful.
2. Replace the current router's role as an imperative OpenCode request shell with the gateway responsibilities in this spec.
3. Remove plugin-side Kubernetes pod-exec tool implementations.
4. Remove custom OpenCode tool collisions for:

   * `bash`,
   * `read`,
   * `write`,
   * `edit`,
   * `glob`,
   * `grep`,
   * `apply_patch`,
   * related emulated tools.
5. Replace `session.created` provisioning with the real WorkspaceAdapter lifecycle.
6. Replace in-memory-only session-to-pod state with durable workspace metadata plus Kubernetes reconciliation.
7. Preserve the wildcard preview routing concept.
8. Rewrite obsolete architecture and sandbox-image documentation.
9. Add a real end-to-end harness rather than manual `kubectl run` smoke instructions.

Backward compatibility with the old plugin configuration is not required unless it is trivial and does not complicate the new implementation.

---

# 31. Implementation order

The implementation agent should execute in this order.

## Phase 1: upstream compatibility spike

Before large refactoring:

1. Pin an exact OpenCode version.
2. Build a minimal WorkspaceAdapter test plugin.
3. Start a central and child OpenCode pair locally.
4. Prove:

   * remote target registration,
   * workspace creation,
   * session warp,
   * file API routing,
   * event synchronization.
5. Commit a regression test representing this contract.

Do not proceed with a custom fallback if this fails. Diagnose the exact pinned upstream API first.

## Phase 2: gateway lifecycle

Implement:

* project config,
* SQLite state,
* Kubernetes Pod/Service/Secret creation,
* stable target,
* create/ensure/suspend/delete.

## Phase 3: runtime injection

Implement:

* runtime OCI image,
* init injection,
* supervisor,
* child runtime plugin,
* version checks.

## Phase 4: environment profiles

Implement:

* API,
* CLI,
* Secret mount,
* direnv authorization,
* dynamic `shell.env`.

## Phase 5: checkpointing

Implement:

* checkpoint sidecar,
* bundle creation,
* storage,
* exact restore verification.

## Phase 6: preview gateway

Complete:

* Kubernetes watch/cache,
* HTTP/WS proxy,
* authz,
* reserved ports.

## Phase 7: launch/resume UX

Implement:

* project seed catalog,
* launch page,
* central session/workspace creation,
* resume/suspend actions.

## Phase 8: full `kind` harness

Implement and run every acceptance case.

## Phase 9: documentation and cleanup

Remove dead architecture, update README, deployment docs, and examples.

---

# 32. Definition of done

The implementation is complete only when all of the following are true:

* `./hack/e2e.sh` passes from a clean checkout.
* The E2E harness requires no external LLM or Git provider.
* A sandbox fixture image without OpenCode/Bun/direnv successfully runs child OpenCode through runtime injection.
* Central OpenCode uses an upstream remote workspace rather than custom tool overrides.
* No per-sandbox PVC is created.
* A sandbox can be destroyed and recreated with byte-identical ordinary Git staged/unstaged/untracked state.
* The same central OpenCode session continues after sandbox replacement.
* Private `.envrc` is injected per user/project and is not stored in Git checkpoints.
* New shell commands observe environment changes without Pod recreation.
* Child OpenCode can restart for environment refresh without changing Pod or central session identity.
* Project-local OpenCode skills/plugins are visible from the child checkout.
* Preview HTTP routing works.
* Two concurrent workspaces for one repository remain isolated.
* Gateway restart preserves durable workspace metadata/checkpoints.
* All production dependencies are version-pinned.
* All unit, integration, E2E, lint, typecheck, and build commands pass.
* README describes the actual implemented architecture.
* No production code contains placeholder TODOs for required behavior.

---

# 33. Required developer commands

The final repository should expose a small, memorable command surface.

At minimum:

```bash
# all fast tests
just test

# build all binaries/packages/images
just build

# create/run/destroy full kind acceptance cluster
just e2e

# keep failed cluster
KEEP_KIND_CLUSTER=1 just e2e

# lint/typecheck
just check
```

A Makefile may be used instead of `just`, but there must be one canonical interface documented in the README and usable from the Nix dev shell.

The Nix development shell must contain every local tool required to run the complete test harness.

---

# 34. Agent implementation constraints

The implementation agent must:

* Inspect the actual pinned OpenCode SDK/plugin types rather than guessing API signatures.
* Prefer upstream OpenCode behavior over local emulation.
* Preserve exact testable invariants rather than approximate behavior.
* Keep platform-specific code outside project repositories.
* Avoid adding a generic workspace platform.
* Avoid introducing unnecessary databases/services.
* Use explicit errors when a project image lacks required capabilities.
* Write tests alongside each subsystem.
* Leave the repository in a state where a new contributor can run one E2E command and reproduce the full system locally.

When a design detail not explicitly specified here must be chosen, choose the smallest implementation that preserves:

```text
stock OpenCode semantics
+ task-level Kubernetes isolation
+ stateless sandbox recreation
+ exact Git working-state recovery
+ no repository-specific OpenCode integration requirement
```

