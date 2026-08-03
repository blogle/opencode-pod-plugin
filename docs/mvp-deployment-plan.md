# MVP Deployment Plan

## 1. MVP Goal

Deploy the new workspace architecture to nandstorm and prove:

- Tinyauth-protected access.
- Dojo launches in its Nix development environment.
- Private `.env` injection works without Git storage.
- Normal OpenCode tools work remotely.
- Preview URLs work.
- Suspend/resume preserves conversation and Git state.

Deferred: NetworkPolicy enforcement, backups, quotas, metrics, HA, and lifecycle race hardening.

## 2. Prepare the Release

1. Review and commit the current changes.
2. Push through a branch and PR.
3. Require `just test`, `just check`, and `just build` to pass.
4. Run `just e2e` on the exact release commit.
5. Merge to `master`.
6. Wait for the publish workflow to build:
   - `opencode-central`
   - `opencode-gateway`
   - `opencode-runtime`
   - `opencode-generic-nix`
7. Confirm all four SHA-tagged GHCR images are publicly pullable.

Record the merged commit SHA:

```bash
git rev-parse HEAD
```

Replace every `REPLACE_WITH_COMMIT_SHA` under `deploy/nandstorm`, then verify:

```bash
grep -R "REPLACE_WITH_COMMIT_SHA" deploy/nandstorm
```

The command should return no matches.

## 3. Add a Preflight Overlay

Before cutover, split the nandstorm deployment into:

```text
deploy/nandstorm/core/
deploy/nandstorm/
```

`core` should contain:

- `deploy/base`
- Platform and project configuration
- Image substitutions
- Certificate
- No Ingress resources

The top-level overlay should add the three Ingress resources to `core`.

This permits deploying and testing the new control plane before replacing the old public routes.

## 4. Validate the Rendered Manifests

```bash
kustomize build deploy/nandstorm/core >/tmp/opencode-core.yaml
kustomize build deploy/nandstorm >/tmp/opencode-nandstorm.yaml
```

Check:

- No placeholder image tags remain.
- `publicUrl` is `https://opencode.thejeffer.net`.
- `identityHeader` is `remote-email`.
- Dojo has `profileTarget: .env`.
- Dojo has `trustTrackedEnvrc: true`.
- No plaintext adapter token or `.env` content appears.

Run target-cluster validation:

```bash
kubectl apply --dry-run=server -f /tmp/opencode-core.yaml
kubectl apply --dry-run=server -f /tmp/opencode-nandstorm.yaml
```

## 5. Create the Internal Credential

Create the namespace and adapter token outside Git:

```bash
kubectl create namespace opencode-system \
  --dry-run=client -o yaml | kubectl apply -f -

kubectl -n opencode-system create secret generic adapter-internal-token \
  --from-literal=token="$(openssl rand -hex 32)"
```

For the MVP, this imperative Secret is acceptable. Migrate it to SealedSecrets later.

## 6. Verify External Dependencies

Confirm the existing resources:

```bash
kubectl -n auth get middleware.traefik.io sso-auth
kubectl get clusterissuer letsencrypt
kubectl get ingressclass traefik
```

Tinyauth must return `remote-email`. The new Ingress resources must reference:

```text
auth-sso-auth@kubernetescrd
```

## 7. Deploy the Core

```bash
kubectl apply -k deploy/nandstorm/core
```

Wait for the control plane:

```bash
kubectl -n opencode-system rollout status deployment/central --timeout=10m
kubectl -n opencode-system rollout status deployment/gateway --timeout=5m
kubectl -n opencode-system get pods,pvc
```

Expected:

- Central is Ready.
- Gateway is Ready.
- Both PVCs are Bound.
- No sandbox exists yet.

## 8. Validate Through Port-Forward

Start temporary forwards:

```bash
kubectl -n opencode-system port-forward service/gateway 8080:8080
kubectl -n opencode-system port-forward service/central 4096:4096
```

Verify:

```bash
curl http://127.0.0.1:8080/healthz
curl http://127.0.0.1:8080/readyz
curl http://127.0.0.1:4096/global/health
```

Query projects using the identity Tinyauth will supply:

```bash
curl -H 'remote-email: YOUR_EMAIL' \
  http://127.0.0.1:8080/v1/projects
```

Confirm Dojo is returned.

## 9. Upload Dojo Secrets

From a local Dojo checkout containing the ignored `.env`:

```bash
ocws env push \
  --project dojo \
  --file .env \
  --gateway-url http://127.0.0.1:8080 \
  --identity-header remote-email \
  --identity YOUR_EMAIL
```

Verify metadata only:

```bash
ocws env status \
  --project dojo \
  --gateway-url http://127.0.0.1:8080 \
  --identity-header remote-email \
  --identity YOUR_EMAIL
```

The API must not return Secret content.

## 10. Internal Smoke Test

Through the gateway port-forward:

1. Open the workspace launch page with the `remote-email` header or call the launch endpoint.
2. Launch Dojo at `master`.
3. Confirm a sandbox Pod becomes Ready.
4. Confirm `node`, Python, `pnpm`, and Nix-shell variables are available.
5. Confirm `/workspace/.envrc` is the tracked file.
6. Confirm `/workspace/.env` is a Secret-backed symlink.
7. Confirm `.env` does not appear in Git status.
8. Create and modify a test file.
9. Suspend and resume the workspace.
10. Verify the file and conversation survive.

Do not proceed to public cutover if this fails.

## 11. Cut Over Public Routing

The old public Ingresses conflict with the replacement hosts. Remove only the old Ingresses first:

```bash
kubectl -n opencode delete ingress opencode opencode-sandbox-router
```

Apply the complete nandstorm overlay:

```bash
kubectl apply -k deploy/nandstorm
```

Wait for the certificate and routes:

```bash
kubectl -n opencode-system wait \
  --for=condition=Ready certificate/opencode-tls \
  --timeout=5m

kubectl -n opencode-system get ingress
```

Keep the old namespace and PVCs temporarily, but scale its obsolete workloads down:

```bash
kubectl -n opencode scale deployment/opencode --replicas=0
kubectl -n opencode scale deployment/opencode-k8s-sandbox-router --replicas=0
```

## 12. Public Acceptance

Verify through Tinyauth:

- `https://opencode.thejeffer.net`
- `https://workspaces.opencode.thejeffer.net`

Run this acceptance sequence:

1. Unauthenticated access redirects to SSO.
2. Authenticated access reaches central and the workspace page.
3. Launch Dojo `master`.
4. Use normal OpenCode file and shell tools.
5. Verify `.env` values are available to commands.
6. Start a development HTTP server.
7. Open the URL returned by the `preview` tool.
8. Verify HTTP and WebSocket traffic.
9. Create staged, unstaged, and untracked changes.
10. Suspend the workspace.
11. Resume it.
12. Verify exact Git state and conversation continuity.
13. Delete the workspace.
14. Verify its Pod, Service, runtime Secret, and checkpoint are removed.

## 13. Go/No-Go Criteria

The MVP is successful when:

- Both control-plane deployments remain Ready.
- SSO protects all three public routes.
- Dojo launches reliably.
- Secret-backed `.env` works without leakage.
- Preview routing works.
- Suspend/resume preserves state.
- Workspace deletion cleans up resources.
- No critical errors appear in central, gateway, or sandbox logs.

## 14. Immediate Post-MVP Backlog

After the PoC is functional:

1. Bound checkpoint retention.
2. Recover workspaces stranded in transitional states.
3. Serialize checkpoint/suspend/delete operations.
4. Enable and test NetworkPolicy on nandstorm.
5. Add PVC backup procedures.
6. Add ResourceQuota and admission controls.
7. Add gateway metrics and disk-pressure alerts.
8. Replace SHA tags with image digests.
9. Move the adapter token to SealedSecrets.
10. Delete the deprecated `opencode` namespace after the retention period.

Continue using Kustomize for the MVP. Reconsider Helm only if this must be installed across multiple independent clusters.
