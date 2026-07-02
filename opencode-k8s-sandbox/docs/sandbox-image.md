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

## Building

```bash
docker build -t opencode-sandbox:latest .
```

## Testing Locally

```bash
docker run -p 8080:8080 opencode-sandbox:latest
```
