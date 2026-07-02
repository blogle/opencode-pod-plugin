# Architecture

## Overview

opencode-k8s-sandbox provides ephemeral sandbox environments for opencode running in Kubernetes.

## Components

```
┌─────────────────┐     ┌─────────────────┐     ┌─────────────────┐
│  opencode CLI   │────▶│  Router (Rust)  │────▶│  Sandbox Pod    │
└─────────────────┘     └─────────────────┘     └─────────────────┘
                              │
                              ▼
                        ┌─────────────────┐
                        │  K8s API        │
                        └─────────────────┘
```

### Router (Rust)

Imperative shell that:
- Accepts requests from opencode CLI
- Manages sandbox lifecycle via K8s API
- Routes traffic to appropriate sandbox pods

### Plugin (npm)

Functional core that:
- Defines sandbox configuration schema
- Provides TypeScript types for sandbox interaction
- Handles client-side sandbox management

## Design Rationale

- **Parse don't validate**: Router parses incoming requests into typed structs
- **Singular responsibility**: Router handles routing, plugin handles client logic
- **Simplicity**: Minimal dependencies, clear data flow
- **Boolean blindness**: Use enums for state instead of booleans
