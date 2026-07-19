set shell := ["bash", "-eu", "-o", "pipefail", "-c"]

# Regenerate only the ignored repository-local kind kubeconfig.
kind-kubeconfig cluster:
    ./hack/kind-kubeconfig.sh {{cluster}}

test:
    npm --prefix plugin test
    npm --prefix runtime-plugin test
    cargo test --manifest-path gateway/Cargo.toml
    cargo test --manifest-path supervisor/Cargo.toml
    ./tests/compat/test-ocws.sh
    ./tests/compat/run.sh

check:
    npm --prefix plugin run typecheck
    npm --prefix runtime-plugin run typecheck
    cargo fmt --manifest-path gateway/Cargo.toml -- --check
    cargo clippy --manifest-path gateway/Cargo.toml --all-targets --all-features -- -D warnings
    cargo fmt --manifest-path supervisor/Cargo.toml -- --check
    cargo clippy --manifest-path supervisor/Cargo.toml --all-targets --all-features -- -D warnings

build:
    npm --prefix plugin run build
    npm --prefix runtime-plugin run build
    cargo build --manifest-path gateway/Cargo.toml --locked --release
    cargo build --manifest-path supervisor/Cargo.toml --locked --release

e2e:
    ./hack/e2e.sh
