# Production overlay

`deploy/base` intentionally contains no credentials and no registered projects.
Before applying this example, create the adapter credential out of band and
replace the project ConfigMap with your registry- and repository-specific data.

```bash
kubectl create namespace opencode-system --dry-run=client -o yaml | kubectl apply -f -
kubectl -n opencode-system create secret generic adapter-internal-token \
  --from-literal=token="$(openssl rand -hex 32)"
kustomize build deploy/examples | kubectl apply -f -
```

For GitOps, manage `adapter-internal-token` with SealedSecrets, SOPS, or an external
secret controller. Never commit the plaintext token. The auth proxy must strip
client-supplied identity headers and set the configured trusted header itself.

Copy `deploy/base/config/projects.yaml` into a private overlay and replace the
empty mapping with project registrations. Use immutable image digests for the
gateway, central, runtime, generic Nix runner, and project development images.

Both central and gateway PVCs contain durable state. Snapshot them together
while the deployments are scaled to zero. Restore gateway SQLite and checkpoint
files as one unit before starting gateway; restore central state before allowing
users to reconnect.
