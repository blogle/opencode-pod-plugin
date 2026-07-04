# Sandbox Image

## Minimal Dockerfile

```dockerfile
FROM rust:1.77-slim as builder

WORKDIR /app
COPY . .
RUN cargo build --release

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /app/target/release/opencode-sandbox /usr/local/bin/

EXPOSE 8080
CMD ["opencode-sandbox"]
```

## Image Requirements

- Minimal base image (slim/debian-slim)
- Non-root user for security
- Health check endpoint exposed
- Required tools pre-installed
- When Nix caching is enabled: `attic-client` must be available on PATH

## Building

```bash
docker build -t opencode-sandbox:latest .
```

## Testing Locally

```bash
docker run -p 8080:8080 opencode-sandbox:latest
```

## Nix Binary Caching (optional)

The sandbox image includes `attic-client` for push/pull to an Attic binary cache.
When `nixCache` is configured in the plugin, each sandbox pod:

1. Gets a writable `/nix/store` (via `readOnlyRootFilesystem: false`)
2. Configures Nix substituters to pull from the Attic cache + `cache.nixos.org`
3. Runs `attic watch-store` in the background to push newly-built paths back

### Setup

1. Deploy an [Attic](https://docs.attic.rs) server (e.g. backed by MinIO/S3)
2. Create a cache: `attic cache create opencode`
3. Generate a push token: `atticadm make-token --sub sandbox --validity 1y --push opencode`
4. Create a Kubernetes secret with the token:
   ```bash
   kubectl create secret generic attic-creds --from-literal=attic-token=eyJ...
   ```
5. Configure the plugin:
   ```json
   {
     "nixCache": {
       "endpoint": "https://attic.example.com",
       "cache": "opencode",
       "publicKey": "opencode:base64key=",
       "tokenSecretName": "attic-creds"
     }
   }
   ```

### Tradeoffs

- Every pod writes back to the shared cache directly (no warmup job)
- Cache pollution is accepted: arbitrary code in sandboxes can push anything
- This is safe when the cache is only trusted by sandbox pods, not laptops/CI
- Future upgrade: add a warmup controller that pre-builds common dev shells
