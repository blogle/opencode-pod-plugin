#!/usr/bin/env bash
set -euo pipefail

CLUSTER_NAME="opencode-sandbox-dev"

echo "Deleting kind cluster: $CLUSTER_NAME"

kind delete cluster --name "$CLUSTER_NAME" 2>/dev/null || true

echo "Cluster deleted successfully"
