# opencode-k8s-sandbox Progress Report
## 2026-07-02T15:52:04-07:00

---

## Goal
- Implement `opencode-k8s-sandbox` per SPEC.md: a Rust router that proxies HTTP/WebSocket traffic to K8s sandbox pods by `{port}-{sandbox-id}.{base-domain}`, and a TypeScript OpenCode plugin that creates/destroys pods per session and overrides bash/read/write/edit tools to exec into the pod.

## Constraints & Preferences
- Functional core, imperative shell — pure functions for parsing/routing, I/O only in outer layer
- Parse, don't validate — convert to precise types at boundary, no downstream re-parsing
- No boolean blindness — use enums (PodPhase, WorkspaceMode) not bare bools
- Semantic comments — comment why, not what
- Simplicity over generality — build exactly what SPEC.md section 7 needs
- Single responsibility — all tool overrides share `execInPod`
- `flake.nix` at git root (`/home/ogle/src/oc_pods/flake.nix`) with Rust stable + musl target, Node.js, kind, kubectl, websocat
- Default to `kind` cluster for all testing; real cluster off-limits
- Validation loop: `scripts/validate.sh` must pass static + integration checks

## Progress
### Done
- **Dev environment**: `flake.nix` with rust-overlay, Node.js, kind, kubectl, websocat; `.envrc` with `use flake`
- **Router complete**: `config.rs`, `index.rs` (reflector-based pod index), `health.rs` (/healthz + /metrics), `proxy.rs` (parse_host, header sniffing, TCP splice), `main.rs`; 10 unit tests
- **Plugin complete**: `config.ts` (zod), `sessionStore.ts`, `hooks.ts` (session lifecycle + idle sweep), `k8s/client.ts`, `k8s/podSpec.ts`, `k8s/exec.ts`, `tools/bash.ts`, `tools/fileOps.ts`, `tools/overrides.ts` (ls, glob, grep, preview-link), `index.ts`
- **Deploy manifests**: ServiceAccount, ClusterRole (pods get/list/watch), ClusterRoleBinding, Deployment, Service (NodePort 30080/30090), Kustomize, ingress example
- **All static checks pass**: `cargo fmt`, `cargo clippy -D warnings`, `cargo test` (10/10), `tsc --noEmit`, `vitest` (7/7)
- **All integration tests pass** (11/11 in validate.sh):
  - Router routing (HTTP 200 for valid sandboxes, 502 for nonexistent, 400 for invalid hostnames)
  - Router WebSocket upgrade (HTTP 101)
  - Router pod churn (routing survives pod delete+recreate)
  - Plugin pod lifecycle (session created → pod running → write/read verified → session deleted → pod gone)
  - Plugin exec (write → kubectl verify → edit → kubectl verify → bash → kubectl verify)
- **Exec test fixed**: Verification steps now use kubectl exec independently, not plugin's read tool
- **Tool coverage expanded**: Added ls, glob, grep, preview-link overrides
- **System prompt transform**: Injects sandbox instructions (working directory, preview-link tool usage)
- **Manual acceptance checklist**: Documented in progress report per steering §4.4

### In Progress
- (none)

### Blocked
- (none)

## Tool-Override Audit (Daytona comparison)

| Tool | Status | Action | Reason |
|------|--------|--------|--------|
| **bash** | Already handled | — | Routes through `execInPod` |
| **read** | Already handled | — | Routes through `execInPod` with `cat` |
| **write** | Already handled | — | Routes through `execInPod` with heredoc |
| **edit** | Already handled | — | Routes through `execInPod` with sed/read-write |
| **glob** | Already handled | — | Routes through `execInPod` with `find` |
| **grep** | Already handled | — | Routes through `execInPod` with `grep -r` |
| **list** | Already handled | — | Routes through `execInPod` with `ls -la`. Registered tool name is `list` (not `ls`), confirmed from OpenCode config schema line 133. |
| **preview-link** | Already handled | — | Pure function: `{port}-{sandboxId}.{baseDomain}` |
| **multiedit** | **Added** | Override | Batch of sequential edit operations — routes through `execInPod` with sed. Verified as separate registered tool in OpenCode, not an alias of edit. |
| **apply_patch** | **Added** | Override | Unified-diff patch — routes through `execInPod` with `patch -p1`. Verified as separate registered tool in OpenCode, not an alias of edit. Exact name is `apply_patch`, not `patch`. |
| **lsp** | Intentionally excluded | — | Per SPEC.md simplicity framing; no LSP support in sandboxes for v1 |
| **System prompt** | Already handled | — | Injects sandbox instructions (working directory, preview-link tool usage) |

## Key Decisions
- **Reflector label selector**: K8s "not empty" selector is `key!=` not `key!=""` — the double quotes were treated as literal characters by the API server
- **KubeConfig loading**: `loadFromCluster()` doesn't throw in nix dev shells; it returns `https://undefined:undefined`. Must explicitly validate server URL before using cluster config, then fall back to `loadFromDefault()`
- **`kubectl run --labels` unreliable**: Pod labels silently not applied. Use YAML manifests with heredocs (`<<'EOF'`) instead for test pods
- **WebSocket test**: websocat doesn't support `--resolve`. Use Python raw socket WebSocket upgrade test with retry logic
- **Pod churn test**: Fixed sleep doesn't work — reflector timing varies. Use polling loop (curl until 200, max 30 retries)
- **Plugin test harnesses**: Raw `k8s.Exec.exec()` API signatures differ from docs — but the implementation correctly uses the Exec class, not kubectl subprocess. The earlier note about "routing around the library" was misleading.
- **`sessionStore` exposed**: Added `sessionStore` to `createPlugin` return object so integration tests can verify session state
- **kube-runtime 0.93 API**: `watcher()` takes `watcher::Config` not `ListParams`; `Writer::as_reader()` not `clone_reader()`; streams need `Box::pin()` for Unpin
- **Dockerfile `rust:slim`**: pinned `rust:1.85` failed because `home` v0.5.12 requires rustc 1.88; `rust:slim` (latest) works
- **Service type NodePort**: kind's `extraPortMappings` (30080→8080, 30090→9090) route to NodePort service

## Manual Acceptance Checklist (steering §4.4)
*These items require real cluster infrastructure, DNS/cert, OpenCode server, and a human with a browser. Not covered by validate.sh.*

### Prerequisites (operator-performed, one-time)
- [ ] Wildcard DNS record: `*.{baseDomain}` → the ingress IP
- [ ] Wildcard TLS cert covering `*.{baseDomain}` (DNS-01 challenge)
- [ ] One Ingress object routing `*.{baseDomain}` → the router Service
- [ ] RBAC applied: router ClusterRole (get/list/watch pods), plugin service account (create/get/delete pods, pods/exec, optional PVC)
- [ ] Router deployed via `kubectl apply -k deploy/`
- [ ] Plugin configured in OpenCode's plugin config with correct `namespace`, `sandboxImage`, `repoUrl`, `baseDomain`

### Verification steps (human-performed)
- [ ] **Pod lifecycle**: Create OpenCode session → confirm pod `opencode-sbx-<id>` appears in `kubectl get pods` within 30s → end session → confirm pod deleted within 10s
- [ ] **Tool routing**: In OpenCode, run `write` tool to create file → run `kubectl exec <pod> -- cat <file>` → confirm content matches
- [ ] **Port reachability**: In pod, start `python -m http.server 8080` → open `https://8080-<sandboxId>.<baseDomain>` in browser → confirm page loads
- [ ] **Multi-port test**: Repeat with ports 8888 and 5173 → confirm all three URLs resolve and respond
- [ ] **WebSocket/HMR**: Start Vite dev server in pod → open in browser → edit a file → confirm live-reload without manual refresh
- [ ] **Router restart**: Delete router pod → wait for new pod → confirm existing sandbox URLs still work (no sandbox restart needed)
- [ ] **Single Ingress**: Run `kubectl get ingress -A` → confirm only one Ingress object exists regardless of concurrent sessions
- [ ] **Idle timeout**: Leave session idle for configured timeout → confirm pod deleted automatically

### Notes
- SPEC §2 explicitly excludes: auth/authz, HA/multi-replica router, non-HTTP protocols, horizontal scaling, resource-quota policy engine
- Access control delegated to network (e.g., private tailnet) — document this loudly in README
- Any deployment on a network without equivalent isolation needs an auth proxy in front of both OpenCode server and router

## Follow-up 3: Explicit Answers

### 1. What execInPod actually does

**Answer: execInPod uses @kubernetes/client-node's Exec class programmatically. It does NOT shell out to kubectl.**

Current contents of `plugin/src/k8s/exec.ts`:
- Imports `Exec` from `@kubernetes/client-node` via `getExecApi()`
- Calls `exec.exec(namespace, podName, "sandbox", command, stdoutStream, stderrStream, null, false, callback)` (lines 44-61)
- Uses SPDY-based exec stream through the Kubernetes API server
- No subprocess spawning, no kubectl binary dependency

This is the correct implementation per SPEC.md §5.6. The earlier "API signatures differ from docs" note in the progress report was misleading — the implementation correctly uses the client library. The plugin only needs network reachability to the API server plus a credential, exactly as designed.

### 2. ls override tool name

**Answer: The registered tool name is `list`, not `ls`. Override has been corrected.**

Source: `/nix/store/rw186syhsznn15di9ywri81ir28qp3jw-opencode-1.16.2/share/opencode/config.json`, line 133:
```json
"list": {
  "$ref": "#/$defs/PermissionRuleConfig"
},
```

This is the `PermissionConfig` schema defining all tool names that can have permission rules. The directory-listing tool is registered as `list`. The override was keyed to `ls`, which would have silently never fired — ls calls would have fallen through to the host.

**Fix applied**: Changed override key from `ls` to `list` in index.ts. TypeScript compiles, unit tests pass.

### 3. test-plugin-pod-lifecycle.sh verification path

**Answer: The verification path is already correct and independent of the plugin.**

Lines 76-88 of test-plugin-pod-lifecycle.sh:
```typescript
// Verify write landed in the pod via kubectl (independent of plugin)
console.log("Test 3: Verifying write via kubectl exec...");
const { execSync } = await import("child_process");
const kubectlCat = execSync(
  `kubectl exec ${record.podName} -n opencode-sandbox -- cat /tmp/test.txt`,
  { encoding: "utf-8" }
).trim();
```

This uses `kubectl exec` directly, not the plugin's read tool. The test:
- Writes via `plugin.tools.write` (line 70-74)
- Verifies via `kubectl exec ... cat` (lines 79-81)

This is the same pattern applied to test-plugin-exec.sh. The verification is independent — it goes around the plugin, not through it.

## Lower-Priority Items

| Item | Status | Action | Reason |
|------|--------|--------|--------|
| **sessionStore leak** | Noted, not fixed | — | `sessionStore` exposed on plugin return object solely for test convenience. Should use dependency injection or debug-only export. Not fixed in this pass — left as-is with explicit note this was a test-convenience change, not an intentional design choice. |
| **Dockerfile optimization** | Noted, not fixed | — | `COPY . .` invalidates Docker cache, causing ~3-4 min rebuilds. Fix: split Dockerfile to copy `Cargo.toml`/`Cargo.lock` first, build deps in earlier layer. Not fixed in this pass — not slowing iteration significantly enough to justify the change. |

## Next Steps
- (none — all tests pass, implementation complete per SPEC.md)

## Critical Context
- Git root is `/home/ogle/src/oc_pods` (not `opencode-k8s-sandbox/`)
- kind cluster name: `opencode-sandbox-dev`
- Router proxy port on host: 30080 (via kind extraPortMappings → NodePort)
- Test pod labels: `opencode.dev/sandbox-id=aaaa1111` (port 8080), `bbbb2222` (port 5173), `cccc3333` (websocket echo)
- The `nix develop --command bash -c "..."` pattern is required since `npm`/`cargo` are not on bare PATH
- Docker image rebuild takes ~3-4 min (cargo compilation not cached due to `COPY . .` invalidating Docker cache)
- `validate.sh` total runtime ~8-10 min (cluster creation + image build + 11 tests)
- SPEC explicitly excludes: auth/authz, HA/multi-replica router, non-HTTP protocols, horizontal scaling, resource-quota policy engine

## Relevant Files
- `/home/ogle/src/oc_pods/flake.nix`: Dev environment definition
- `/home/ogle/src/oc_pods/opencode-k8s-sandbox/router/src/index.rs`: Reflector with fixed label selector `key!=`, logging on index updates
- `/home/ogle/src/oc_pods/opencode-k8s-sandbox/router/src/proxy.rs`: parse_host, 400 responses for invalid/missing Host headers
- `/home/ogle/src/oc_pods/opencode-k8s-sandbox/router/src/config.rs`: Router env config
- `/home/ogle/src/oc_pods/opencode-k8s-sandbox/router/src/health.rs`: /healthz + /metrics
- `/home/ogle/src/oc_pods/opencode-k8s-sandbox/router/src/main.rs`: Main entry
- `/home/ogle/src/oc_pods/opencode-k8s-sandbox/plugin/src/index.ts`: Plugin entry, exposes `sessionStore` on return object
- `/home/ogle/src/oc_pods/opencode-k8s-sandbox/plugin/src/k8s/client.ts`: KubeConfig with `loadFromCluster` URL validation + `loadFromDefault` fallback
- `/home/ogle/src/oc_pods/opencode-k8s-sandbox/plugin/src/k8s/exec.ts`: execInPod
- `/home/ogle/src/oc_pods/opencode-k8s-sandbox/plugin/src/hooks.ts`: session.created/deleted
- `/home/ogle/src/oc_pods/opencode-k8s-sandbox/plugin/src/tools/fileOps.ts`: read/write/edit
- `/home/ogle/src/oc_pods/opencode-k8s-sandbox/deploy/`: Kustomize manifests
- `/home/ogle/src/oc_pods/opencode-k8s-sandbox/scripts/kind-up.sh`: Cluster setup with YAML pod manifests
- `/home/ogle/src/oc_pods/opencode-k8s-sandbox/scripts/validate.sh`: Full validation suite
- `/home/ogle/src/oc_pods/opencode-k8s-sandbox/scripts/integration/test-router-routing.sh`: HTTP routing via `--resolve`
- `/home/ogle/src/oc_pods/opencode-k8s-sandbox/scripts/integration/test-router-websocket.sh`: Python WebSocket upgrade test with retry
- `/home/ogle/src/oc_pods/opencode-k8s-sandbox/scripts/integration/test-router-pod-churn.sh`: Polling-based pod churn test
- `/home/ogle/src/oc_pods/opencode-k8s-sandbox/scripts/integration/test-plugin-pod-lifecycle.sh`: Uses kubectl exec for independent verification
- `/home/ogle/src/oc_pods/opencode-k8s-sandbox/scripts/integration/test-plugin-exec.sh`: Uses kubectl exec for independent verification
