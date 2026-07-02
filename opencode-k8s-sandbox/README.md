# opencode-k8s-sandbox

Ephemeral sandbox environments for opencode in Kubernetes.

## What This Is

A system that provides isolated, on-demand sandbox environments for opencode, managed via Kubernetes. Sandboxes are created on demand, used, and destroyed.

## Quickstart

### Prerequisites

- Nix with direnv
- Rust toolchain
- Node.js + npm
- Kubernetes cluster with ingress controller

### Development

```bash
# Enter dev environment
direnv allow

# Build router
cd router && cargo build

# Build plugin
cd plugin && npm install
```

### Deploy

See [docs/cluster-setup.md](docs/cluster-setup.md) for cluster prerequisites.

## Architecture

See [docs/architecture.md](docs/architecture.md) for component design and rationale.

## Documentation

- [Architecture](docs/architecture.md) - Component design
- [Cluster Setup](docs/cluster-setup.md) - K8s prerequisites
- [Sandbox Image](docs/sandbox-image.md) - Dockerfile for sandboxes
