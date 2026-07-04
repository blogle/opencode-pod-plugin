#!/usr/bin/env bash
set -euo pipefail

# Creates a minimal test git repo with a flake.nix for integration testing.
# The repo is created in a temp directory and served via a simple HTTP server.

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(dirname "$SCRIPT_DIR")"
TEST_REPO_DIR="/tmp/opencode-test-repo"

echo "Creating minimal test repository..."

# Clean up any previous test repo
rm -rf "$TEST_REPO_DIR"
mkdir -p "$TEST_REPO_DIR"
cd "$TEST_REPO_DIR"

# Initialize git repo
git init
git config user.email "test@opencode.dev"
git config user.name "OpenCode Test"

# Create a minimal flake.nix
cat > flake.nix << 'EOF'
{
  description = "Minimal test dev environment for opencode-k8s-sandbox";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, flake-utils }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = import nixpkgs { inherit system; };
      in
      {
        devShells.default = pkgs.mkShell {
          buildInputs = [
            pkgs.hello
            pkgs.jq
            pkgs.curl
          ];

          shellHook = ''
            echo "opencode test dev environment loaded"
            echo "  hello: $(hello --version 2>/dev/null || echo 'available')"
          '';
        };
      });
}
EOF

# Create a simple README
cat > README.md << 'EOF'
# Test Repository

Minimal repository for testing opencode-k8s-sandbox plugin.

## Development

```bash
nix develop   # or: direnv allow
```
EOF

# Create a simple .envrc for direnv
cat > .envrc << 'EOF'
use flake
EOF

# Commit everything
git add -A
git commit -m "Initial commit: minimal test repo"

echo "Test repo created at: $TEST_REPO_DIR"
echo "Files:"
ls -la
echo ""
echo "To use this repo in your opencode config:"
echo "  \"repos\": {"
echo "    \"test\": \"file://$TEST_REPO_DIR\""
echo "  }"
