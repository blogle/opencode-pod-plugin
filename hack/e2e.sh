#!/usr/bin/env bash
set -Eeuo pipefail
IFS=$'\n\t'

ROOT=$(CDPATH= cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
readonly ROOT
readonly KIND_CONFIG="$ROOT/tests/e2e/kind.yaml"
readonly MANIFESTS="$ROOT/tests/e2e/manifests.yaml"
readonly KEEP_KIND_CLUSTER=${KEEP_KIND_CLUSTER:-0}
readonly SKIP_IMAGE_BUILD=${SKIP_IMAGE_BUILD:-0}
readonly RUN_ID="$(date -u +%Y%m%d%H%M%S)-$$-${RANDOM}"
readonly CLUSTER_NAME="ocws-e2e-${RUN_ID}"
readonly DIAGNOSTICS_DIR="${TMPDIR:-/tmp}/${CLUSTER_NAME}-diagnostics"
readonly KUBECONFIG="$ROOT/.kube/${CLUSTER_NAME}.yaml"
export KUBECONFIG
CLUSTER_CREATED=0
KUBECONFIG_PREPARED=0
readonly -a IMAGES=(
  "ocws-e2e-gateway:rev2"
  "ocws-e2e-runtime:rev2"
  "ocws-e2e-nix:rev2"
  "ocws-e2e-central:rev2"
  "ocws-e2e-project:rev2"
  "ocws-e2e-git:rev2"
  "ocws-e2e-llm:rev2"
)

# shellcheck source=hack/kind.sh
source "$ROOT/hack/kind.sh"

prepare_kubeconfig() {
  umask 077
  if [[ -L "$ROOT/.kube" ]]; then
    printf 'refusing symlinked repository kubeconfig directory: %s\n' "$ROOT/.kube" >&2
    exit 2
  fi
  mkdir -p "$ROOT/.kube"
  chmod 700 "$ROOT/.kube"
  if [[ -e "$KUBECONFIG" || -L "$KUBECONFIG" ]]; then
    printf 'refusing existing e2e kubeconfig: %s\n' "$KUBECONFIG" >&2
    exit 2
  fi
  : >"$KUBECONFIG"
  KUBECONFIG_PREPARED=1
}

require_command() {
  command -v "$1" >/dev/null 2>&1 || {
    printf 'required command is unavailable: %s\n' "$1" >&2
    exit 2
  }
}

diagnostics() {
  (( CLUSTER_CREATED == 1 )) || return 0
  mkdir -p "$DIAGNOSTICS_DIR"
  kubectl --request-timeout=10s --context "kind-$CLUSTER_NAME" get all,pvc,configmap,secret,serviceaccount,role,rolebinding --all-namespaces -o yaml >"$DIAGNOSTICS_DIR/resources.yaml" 2>&1 || true
  kubectl --request-timeout=10s --context "kind-$CLUSTER_NAME" get events --all-namespaces --sort-by=.metadata.creationTimestamp >"$DIAGNOSTICS_DIR/events.txt" 2>&1 || true
  kubectl --request-timeout=10s --context "kind-$CLUSTER_NAME" get pods --all-namespaces -o wide >"$DIAGNOSTICS_DIR/pods.txt" 2>&1 || true
  kubectl --request-timeout=10s --context "kind-$CLUSTER_NAME" -n opencode-system get --raw /apis/apps/v1/namespaces/opencode-system/deployments >"$DIAGNOSTICS_DIR/system-deployments.json" 2>&1 || true
  local namespace pod container
  while IFS=$'\t' read -r namespace pod; do
    [[ -n "$pod" ]] || continue
    while IFS= read -r container; do
      [[ -n "$container" ]] || continue
      kubectl --request-timeout=10s --context "kind-$CLUSTER_NAME" -n "$namespace" logs "$pod" -c "$container" >"$DIAGNOSTICS_DIR/${namespace}_${pod}_${container}.log" 2>&1 || true
      kubectl --request-timeout=10s --context "kind-$CLUSTER_NAME" -n "$namespace" logs "$pod" -c "$container" --previous >"$DIAGNOSTICS_DIR/${namespace}_${pod}_${container}.previous.log" 2>&1 || true
    done < <(kubectl --request-timeout=10s --context "kind-$CLUSTER_NAME" -n "$namespace" get pod "$pod" -o jsonpath='{range .spec.initContainers[*]}{.name}{"\n"}{end}{range .spec.containers[*]}{.name}{"\n"}{end}' 2>/dev/null || true)
  done < <(kubectl --request-timeout=10s --context "kind-$CLUSTER_NAME" get pods --all-namespaces -o jsonpath='{range .items[*]}{.metadata.namespace}{"\t"}{.metadata.name}{"\n"}{end}' 2>/dev/null || true)
  printf 'failure diagnostics: %s\n' "$DIAGNOSTICS_DIR" >&2
}

cleanup() {
  local status=$?
  trap - EXIT
  if (( status != 0 )); then
    diagnostics
  fi
  if [[ "$KEEP_KIND_CLUSTER" == "1" && "$CLUSTER_CREATED" == "1" ]]; then
    printf 'preserving kind cluster %s with kubeconfig %s\n' "$CLUSTER_NAME" "$KUBECONFIG" >&2
  elif (( CLUSTER_CREATED == 1 )); then
    kind_destroy "$CLUSTER_NAME" "$KUBECONFIG" || true
    (( KUBECONFIG_PREPARED == 0 )) || rm -f "$KUBECONFIG"
  else
    (( KUBECONFIG_PREPARED == 0 )) || rm -f "$KUBECONFIG"
  fi
  exit "$status"
}
trap cleanup EXIT

for command in docker kind kubectl python3; do
  require_command "$command"
done
docker info >/dev/null
prepare_kubeconfig

if [[ "$SKIP_IMAGE_BUILD" == "1" ]]; then
  printf '[e2e] reusing explicitly requested local images\n'
  for image in "${IMAGES[@]}"; do
    docker image inspect "$image" >/dev/null
  done
else
  printf '[e2e] building exact local images\n'
  docker build --pull=false --tag ocws-e2e-gateway:rev2 --file "$ROOT/gateway/Dockerfile" "$ROOT"
  docker build --pull=false --tag ocws-e2e-central:rev2 --file "$ROOT/runtime/central.Dockerfile" "$ROOT"
  docker build --pull=false --build-arg OPENCODE_IMAGE=ocws-e2e-central:rev2 --tag ocws-e2e-runtime:rev2 --file "$ROOT/runtime/Dockerfile" "$ROOT"
  docker build --pull=false --tag ocws-e2e-generic-nix:rev2 --file "$ROOT/runtime/generic-nix.Dockerfile" "$ROOT"
  docker build --pull=false --build-arg GENERIC_NIX_IMAGE=ocws-e2e-generic-nix:rev2 --tag ocws-e2e-nix:rev2 --file "$ROOT/tests/fixtures/nix-project/Dockerfile" "$ROOT"
  docker build --pull=false --tag ocws-e2e-project:rev2 --file "$ROOT/tests/fixtures/project-dev-image/Dockerfile" "$ROOT"
  docker build --pull=false --tag ocws-e2e-git:rev2 --file "$ROOT/tests/fixtures/git-server/Dockerfile" "$ROOT"
  docker build --pull=false --tag ocws-e2e-llm:rev2 --file "$ROOT/tests/fixtures/fake-llm/Dockerfile" "$ROOT"
fi

docker run --rm --entrypoint /bin/sh ocws-e2e-project:rev2 -ec \
  'command -v python3 >/dev/null; command -v git >/dev/null; ! command -v opencode; ! command -v bun; ! command -v direnv; test ! -e /opt/opencode'

printf '[e2e] creating unique kind cluster %s\n' "$CLUSTER_NAME"
CLUSTER_CREATED=1
kind_create "$CLUSTER_NAME" "$KIND_CONFIG" "$KUBECONFIG"
kind_load_images "$CLUSTER_NAME" "${IMAGES[@]}"
# The gateway deliberately records and reuses the resolved project-image
# digest. Give kind's local containerd store the equivalent digest reference
# so a stateless replacement does not attempt an external registry pull.
project_digest="$(docker image inspect --format '{{.Id}}' ocws-e2e-project:rev2)"
docker exec "$CLUSTER_NAME-control-plane" ctr -n k8s.io images tag \
  docker.io/library/ocws-e2e-project:rev2 \
  "docker.io/library/ocws-e2e-project@$project_digest"

printf '[e2e] deploying fixture control plane\n'
kubectl --context "kind-$CLUSTER_NAME" apply -f "$MANIFESTS"
kubectl --context "kind-$CLUSTER_NAME" -n opencode-system rollout status deployment/git-server --timeout=180s
kubectl --context "kind-$CLUSTER_NAME" -n opencode-system rollout status deployment/fake-llm --timeout=180s
kubectl --context "kind-$CLUSTER_NAME" -n opencode-system rollout status deployment/central --timeout=300s
kubectl --context "kind-$CLUSTER_NAME" -n opencode-system rollout status deployment/gateway --timeout=300s

printf '[e2e] running API and lifecycle acceptance suite\n'
KIND_CLUSTER_NAME="$CLUSTER_NAME" python3 "$ROOT/tests/e2e/acceptance.py"
printf '[e2e] PASS cluster=%s\n' "$CLUSTER_NAME"
