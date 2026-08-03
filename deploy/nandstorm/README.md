# Nandstorm PoC overlay

This overlay replaces the deprecated `opencode` deployment with the shared
production base, Dojo in Nix mode, Tinyauth's `remote-email` identity, and the
existing `opencode.thejeffer.net` domain.

Before rendering or applying it:

1. Replace every `REPLACE_WITH_COMMIT_SHA` with the exact SHA tag published by
   the `Build and Publish` workflow.
2. Ensure the four GHCR packages are publicly pullable.
3. Create `adapter-internal-token` out of band in `opencode-system`; do not add
   its plaintext value to this overlay.
4. Validate the new control plane through port-forward before removing the old
   `opencode` ingresses.

Upload Dojo's ignored `.env` through a gateway port-forward so the supplied
identity exactly matches Tinyauth's `remote-email` value:

```bash
ocws env push --project dojo --file .env \
  --gateway-url http://127.0.0.1:8080 \
  --identity-header remote-email \
  --identity you@example.com
```

The initial PoC intentionally uses the cluster's default disposable
`local-path` storage. NetworkPolicy enforcement, snapshots, quotas, and
admission policy are follow-up hardening rather than installation prerequisites.
