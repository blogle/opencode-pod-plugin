#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
ROOT_DIR=$(cd "$SCRIPT_DIR/../.." && pwd)
UPSTREAM_URL=${OPENCODE_UPSTREAM_URL:-https://github.com/anomalyco/opencode.git}
OPENCODE_VERSION=1.18.3
OPENCODE_COMMIT=127bdb30784d508cc556c71a0f32b508a3061517
TEMP_DIR=""

die() {
  printf 'compat: FAIL: %s\n' "$*" >&2
  exit 1
}

pass() {
  printf 'compat: PASS: %s\n' "$*"
}

cleanup() {
  [[ -z $TEMP_DIR ]] || rm -rf "$TEMP_DIR"
}
trap cleanup EXIT

for command in git npm node; do
  command -v "$command" >/dev/null 2>&1 || die "required command not found: $command"
done

if [[ -n ${OPENCODE_COMPAT_CACHE_DIR:-} ]]; then
  checkout=${OPENCODE_COMPAT_CACHE_DIR%/}/opencode-$OPENCODE_COMMIT
  mkdir -p "$OPENCODE_COMPAT_CACHE_DIR"
else
  TEMP_DIR=$(mktemp -d "${TMPDIR:-/tmp}/opencode-compat.XXXXXX")
  checkout=$TEMP_DIR/upstream
fi

if [[ ! -d $checkout/.git ]]; then
  rm -rf "$checkout"
  mkdir -p "$checkout"
  git -C "$checkout" init --quiet
  git -C "$checkout" remote add origin "$UPSTREAM_URL"
fi

printf 'compat: fetching OpenCode %s (%s)\n' "$OPENCODE_VERSION" "$OPENCODE_COMMIT"
git -C "$checkout" fetch --quiet --depth=1 origin "$OPENCODE_COMMIT" || \
  die "could not fetch pinned OpenCode commit $OPENCODE_COMMIT from $UPSTREAM_URL"
git -C "$checkout" checkout --quiet --detach FETCH_HEAD
[[ $(git -C "$checkout" rev-parse HEAD) == "$OPENCODE_COMMIT" ]] || die "upstream checkout is not the pinned commit"
[[ $(node -p "require('$checkout/packages/opencode/package.json').version") == "$OPENCODE_VERSION" ]] || \
  die "pinned commit does not contain OpenCode version $OPENCODE_VERSION"
pass "exact OpenCode commit checked out"

package_version=$(node -p "require('$ROOT_DIR/plugin/node_modules/@opencode-ai/plugin/package.json').version" 2>/dev/null || true)
if [[ $package_version != "$OPENCODE_VERSION" ]]; then
  printf 'compat: installing pinned plugin dependencies\n'
  npm ci --ignore-scripts --prefix "$ROOT_DIR/plugin" >/dev/null || die "npm ci failed for plugin"
fi
[[ $(node -p "require('$ROOT_DIR/plugin/node_modules/@opencode-ai/plugin/package.json').version") == "$OPENCODE_VERSION" ]] || \
  die "@opencode-ai/plugin is not pinned to $OPENCODE_VERSION"

"$ROOT_DIR/plugin/node_modules/.bin/tsc" -p "$SCRIPT_DIR/tsconfig.json" || \
  die "public @opencode-ai/plugin workspace types do not compile"
npm run typecheck --prefix "$ROOT_DIR/plugin" --silent || die "central adapter does not compile against public plugin types"
pass "public plugin types compile"

(
  cd "$ROOT_DIR/plugin"
  ./node_modules/.bin/vitest run --root "$SCRIPT_DIR" --no-cache --globals --environment node adapter-contract.test.ts
) || die "adapter registration lifecycle contract failed"
pass "adapter registration, configure, create, target, and remove"

workspace_group="$checkout/packages/opencode/src/server/routes/instance/httpapi/groups/workspace.ts"
session_group="$checkout/packages/opencode/src/server/routes/instance/httpapi/groups/session.ts"
event_group="$checkout/packages/opencode/src/server/routes/instance/httpapi/groups/event.ts"
routing="$checkout/packages/opencode/src/server/routes/instance/httpapi/middleware/workspace-routing.ts"
authorization="$checkout/packages/opencode/src/server/routes/instance/httpapi/middleware/authorization.ts"
shared_routing="$checkout/packages/opencode/src/server/shared/workspace-routing.ts"

for source in "$workspace_group" "$session_group" "$event_group" "$routing" "$authorization" "$shared_routing"; do
  [[ -f $source ]] || die "required upstream routing source missing: ${source#$checkout/}"
done

grep -Fq 'warp: `${root}/warp`' "$workspace_group" || die "workspace warp route is missing"
grep -Fq 'HttpApiEndpoint.post("warp"' "$workspace_group" || die "workspace warp endpoint is missing"
grep -Fq 'SessionPaths' "$session_group" || die "session routes are missing"
grep -Fq '.middleware(WorkspaceRoutingMiddleware)' "$session_group" || die "session routes lack workspace routing"
grep -Fq 'HttpApiProxy.http(client, proxyURL, target.headers, request)' "$routing" || die "remote HTTP forwarding is missing"
grep -Fq 'contentType: "text/event-stream"' "$event_group" || die "SSE event route is missing"
grep -Fq '.middleware(WorkspaceRoutingMiddleware)' "$event_group" || die "SSE event route lacks workspace routing"
grep -Fq 'proxyURL.searchParams.delete("workspace")' "$shared_routing" || die "workspace proxy URL rewriting is missing"
pass "workspace/session warp and HTTP/SSE routing sources"

git -C "$checkout" reset --quiet --hard "$OPENCODE_COMMIT"
git -C "$checkout" apply --check "$ROOT_DIR/runtime/upstream-websocket-auth.patch" || \
  die "authenticated WebSocket patch no longer applies to pinned upstream"
git -C "$checkout" apply "$ROOT_DIR/runtime/upstream-websocket-auth.patch"
grep -Fq 'proxyURL.searchParams.set("auth_token", token)' "$routing" || die "WebSocket proxy patch does not forward auth_token"
grep -Fq 'url.searchParams.get(AUTH_TOKEN_QUERY)' "$authorization" || die "upstream authorization does not accept auth_token"
grep -Fq 'HttpApiProxy.websocket(request, proxyURL)' "$routing" || die "WebSocket forwarding source is missing"
pass "authenticated WebSocket patch behavior represented"

git -C "$checkout" reset --quiet --hard "$OPENCODE_COMMIT"
git -C "$checkout" apply --check "$ROOT_DIR/runtime/upstream-workspace-warp.patch" || \
  die "session binding-order patch no longer applies to pinned upstream"
git -C "$checkout" apply "$ROOT_DIR/runtime/upstream-workspace-warp.patch"
grep -B4 -Fq 'const rows = yield* db' "$checkout/packages/opencode/src/control-plane/workspace.ts" || \
  die "workspace event replay source is missing"
grep -Fq 'yield* session.setWorkspace({ sessionID: input.sessionID, workspaceID: input.workspaceID })' \
  "$checkout/packages/opencode/src/control-plane/workspace.ts" || die "session binding patch is missing"
pass "session binding is persisted before remote event replay"

git -C "$checkout" reset --quiet --hard "$OPENCODE_COMMIT"
git -C "$checkout" apply --check "$ROOT_DIR/runtime/upstream-workspace-timeout.patch" || \
  die "workspace provisioning timeout patch no longer applies to pinned upstream"
git -C "$checkout" apply "$ROOT_DIR/runtime/upstream-workspace-timeout.patch"
grep -Fq 'const TIMEOUT = 300_000' "$checkout/packages/opencode/src/control-plane/workspace.ts" || \
  die "workspace provisioning timeout patch is missing"
pass "remote provisioning timeout supports Kubernetes cold starts"

git -C "$checkout" reset --quiet --hard "$OPENCODE_COMMIT"
git -C "$checkout" apply --check "$ROOT_DIR/runtime/upstream-child-session-directory.patch" || \
  die "child session directory patch no longer applies to pinned upstream"
git -C "$checkout" apply "$ROOT_DIR/runtime/upstream-child-session-directory.patch"
grep -Fq 'event.seq === 0 && space.directory' \
  "$checkout/packages/opencode/src/control-plane/workspace.ts" || \
  die "child session directory patch is missing"
grep -Fq 'event.seq === 0 && directory' \
  "$checkout/packages/opencode/src/server/routes/instance/httpapi/handlers/sync.ts" || \
  die "child replay directory patch is missing"
grep -Fq 'directory: envWorkspaceID' \
  "$checkout/packages/opencode/src/server/routes/instance/httpapi/middleware/workspace-routing.ts" || \
  die "child request directory routing patch is missing"
grep -Fq 'projectID,' \
  "$checkout/packages/opencode/src/server/routes/instance/httpapi/handlers/sync.ts" || \
  die "child replay project identity mapping is missing"
pass "remote replay binds sessions to the child checkout directory"

git -C "$checkout" reset --quiet --hard "$OPENCODE_COMMIT"
git -C "$checkout" apply --check "$ROOT_DIR/runtime/upstream-workspace-reconnect.patch" || \
  die "workspace sync reconnect patch no longer applies to pinned upstream"
git -C "$checkout" apply "$ROOT_DIR/runtime/upstream-workspace-reconnect.patch"
grep -Fq 'workspace sync disconnected' \
  "$checkout/packages/opencode/src/control-plane/workspace.ts" || \
  die "workspace sync reconnect patch is missing"
grep -Fq 'restoreWorkspaceSessions' \
  "$checkout/packages/opencode/src/control-plane/workspace.ts" || \
  die "workspace session replay on reconnect is missing"
grep -Fq 'return exists && connections.get(workspaceID)?.status === "connected"' \
  "$checkout/packages/opencode/src/control-plane/workspace.ts" || \
  die "workspace routing is admitted before reconnect completes"
pass "workspace sync and sessions recover after child replacement"

printf 'compat: static Phase 1 gate passed for OpenCode %s. This gate does not start central or child servers.\n' "$OPENCODE_VERSION"
