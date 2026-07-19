# Project images and runtime injection

An image-mode project supplies its ordinary Linux development image. It must
provide a usable process environment and support the cluster architecture, but
does not need OpenCode, Bun, direnv, Git checkpoint helpers, or platform code.
The runtime init container copies those exact-pin platform binaries into a
shared executable volume and the gateway overrides the project entrypoint with
the injected supervisor.

The published runtime image is built from `runtime/Dockerfile`; central uses
`runtime/central.Dockerfile`; the fallback Nix runner uses
`runtime/generic-nix.Dockerfile`. Release automation publishes immutable commit
tags for all three plus gateway. Production overlays should replace readable
release tags with registry digests.

The tested project-image contract is Linux x86-64 with glibc-compatible injected
binaries, writable `/workspace`, and `/bin/sh`. Distroless, non-Linux, or
incompatible-libc images are unsupported; use the generic Nix runner or publish
a compatible project development image.

For `mode: nix`, gateway chooses the generic Nix runner and synthesizes a
broker-owned direnv entry for the configured flake output. New store paths are
realized in the disposable sandbox. Configure a trusted binary cache such as
Attic in the runner image or deployment environment for predictable cold starts.

Project image tags may be mutable only at initial task creation. Kubernetes'
resolved immutable digest is recorded and reused for every resume of that task.
Do not use `latest`.
