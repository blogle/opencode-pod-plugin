# Injected Runtime

The runtime image carries OpenCode `1.18.3`, Bun `1.3.14`, direnv at commit
`7590ee2442104060bb11eedebd7bd6daf3d88fcd`, the supervisor, and the child
runtime plugin. The gateway copies this payload into an `emptyDir` before the
project container starts.

The v1 image contract is Linux x86_64 with glibc 2.36 or newer. Project images
must permit execution of bind-mounted binaries and provide a writable mounted
`/workspace`. Use the generic Nix runner for musl, distroless, non-x86_64, or
otherwise incompatible project images.
