#!/usr/bin/env bash
set -Eeuo pipefail
IFS=$'\n\t'

kind_create() {
  local cluster_name=$1
  local config=$2
  local kubeconfig=$3
  kind create cluster --name "$cluster_name" --config "$config" --kubeconfig "$kubeconfig" --wait 120s
}

kind_destroy() {
  local cluster_name=$1
  local kubeconfig=$2
  kind delete cluster --name "$cluster_name" --kubeconfig "$kubeconfig"
}

kind_load_images() {
  local cluster_name=$1
  shift
  local image
  for image in "$@"; do
    kind load docker-image --name "$cluster_name" "$image"
  done
}
