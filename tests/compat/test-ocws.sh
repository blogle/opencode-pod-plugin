#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
ROOT_DIR=$(cd "$SCRIPT_DIR/../.." && pwd)
TMP=$(mktemp -d "${TMPDIR:-/tmp}/ocws-test.XXXXXX")
SERVER_PID=""

cleanup() {
  [[ -z $SERVER_PID ]] || kill "$SERVER_PID" 2>/dev/null || true
  rm -rf "$TMP"
}
trap cleanup EXIT

python3 "$SCRIPT_DIR/fake_gateway.py" "$TMP/requests.jsonl" "$TMP/port" &
SERVER_PID=$!
for _ in {1..100}; do
  [[ -s $TMP/port ]] && break
  kill -0 "$SERVER_PID" 2>/dev/null || { printf 'fake gateway exited early\n' >&2; exit 1; }
  sleep 0.02
done
[[ -s $TMP/port ]] || { printf 'fake gateway did not start\n' >&2; exit 1; }

mkdir "$TMP/repository"
git -C "$TMP/repository" init --quiet
git -C "$TMP/repository" remote add origin git@github.com:acme/demo.git
printf 'export PRIVATE_COMPAT_SECRET=do-not-print-this\n' >"$TMP/repository/.envrc"

export OCWS_GATEWAY_URL="http://127.0.0.1:$(<"$TMP/port")"
export OCWS_TOKEN=test-token
export OCWS_IDENTITY=developer@example.test
export OCWS_IDENTITY_HEADER=X-Test-Identity

push_output=$(cd "$TMP/repository" && "$ROOT_DIR/cli/ocws" env push)
status_output=$("$ROOT_DIR/cli/ocws" env status --project demo)
delete_output=$("$ROOT_DIR/cli/ocws" env delete --project demo)

combined="$push_output$status_output$delete_output"
[[ $combined != *PRIVATE_COMPAT_SECRET* && $combined != *do-not-print-this* ]] || {
  printf 'ocws printed environment profile content\n' >&2
  exit 1
}
[[ $push_output == *"project: demo"* && $status_output == *"sha256:"* ]]
[[ $delete_output == "Environment profile deleted for project demo." ]]

digest=$(sha256sum "$TMP/repository/.envrc" | cut -d' ' -f1)
jq -e -s --arg digest "$digest" '
  length == 4 and
  .[0].method == "GET" and .[0].path == "/v1/projects" and
  .[1].method == "PUT" and .[1].path == "/v1/projects/demo/env-profile" and
  .[1].bodySha256 == $digest and
  .[2].method == "GET" and .[2].path == "/v1/projects/demo/env-profile/meta" and
  .[3].method == "DELETE" and .[3].path == "/v1/projects/demo/env-profile" and
  all(.[]; .authorization == "Bearer test-token" and .identity == "developer@example.test")
' "$TMP/requests.jsonl" >/dev/null

printf 'ocws CLI compatibility tests passed\n'
