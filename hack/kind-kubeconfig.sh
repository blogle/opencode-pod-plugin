#!/usr/bin/env bash
set -Eeuo pipefail
IFS=$'\n\t'

ROOT=$(CDPATH= cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
readonly ROOT
readonly CLUSTER_NAME=${1:?usage: hack/kind-kubeconfig.sh <kind-cluster-name>}
readonly KUBECONFIG="$ROOT/.kube/kind-config"

command -v kind >/dev/null 2>&1 || {
  printf 'kind is required\n' >&2
  exit 2
}

mkdir -p "$ROOT/.kube"
umask 077
: >"$KUBECONFIG"
kind export kubeconfig --name "$CLUSTER_NAME" --kubeconfig "$KUBECONFIG"
printf '%s\n' "$KUBECONFIG"
