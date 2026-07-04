#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(dirname "$(dirname "$SCRIPT_DIR")")"
PLUGIN_DIR="$ROOT_DIR/plugin"

echo "Testing plugin pod lifecycle..."

# Wait for test pods to be ready
echo "Waiting for test pods..."
kubectl wait --for=condition=Ready pod/test-pod-1 --namespace=opencode-sandbox --timeout=120s
kubectl wait --for=condition=Ready pod/test-pod-2 --namespace=opencode-sandbox --timeout=120s

# Build the plugin
echo "Building plugin..."
cd "$PLUGIN_DIR"
npm install
npm run build

# Write test harness in the plugin directory so module resolution works
cat > "$PLUGIN_DIR/_test_lifecycle.ts" << 'ENDOFFILE'
import { createPlugin } from "./src/index.js";

const config = {
  namespace: "opencode-sandbox",
  sandboxImage: "opencode-sandbox:latest",
  repos: {
    "test-repo": "https://github.com/octocat/Hello-World.git",
  },
  baseDomain: "opencode.example.com",
};

const plugin = createPlugin(config);

async function test() {
  const testSessionId = "test-session-lifecycle-001";

  console.log("Test 1: Creating session...");
  await plugin.hooks["session.created"]({ sessionId: testSessionId });

  // Get the record from session store
  const record = plugin.sessionStore.get(testSessionId);
  if (!record) {
    console.log("✗ Test 1 failed: No session record found");
    process.exit(1);
  }
  console.log(`✓ Test 1 passed: Pod created: ${record.podName}`);
  console.log(`Sandbox ID: ${record.sandboxId}`);

  // Wait for pod to be running by polling
  console.log("Waiting for pod to be running...");
  for (let i = 0; i < 30; i++) {
    // Use bash tool to check pod status via kubectl
    try {
      const status = await plugin.tools.bash({
        sessionId: testSessionId,
        command: "echo running",
      });
      if (status.trim() === "running") {
        console.log("Pod is running");
        break;
      }
    } catch {
      // Pod might not be ready yet
    }
    await new Promise(resolve => setTimeout(resolve, 1000));
  }

  // Test write tool
  console.log("Test 2: Testing write tool...");
  await plugin.tools.write({
    sessionId: testSessionId,
    path: "/tmp/test.txt",
    content: "Hello from lifecycle test!",
  });

  // Verify write landed in the pod via kubectl (independent of plugin)
  console.log("Test 3: Verifying write via kubectl exec...");
  const { execSync } = await import("child_process");
  const kubectlCat = execSync(
    `kubectl exec ${record.podName} -n opencode-sandbox -- cat /tmp/test.txt`,
    { encoding: "utf-8" }
  ).trim();
  if (kubectlCat === "Hello from lifecycle test!") {
    console.log("✓ Test 3 passed: File verified via kubectl exec");
  } else {
    console.log(`✗ Test 3 failed: Expected "Hello from lifecycle test!", got "${kubectlCat}"`);
    process.exit(1);
  }

  // Cleanup
  console.log("Test 4: Cleaning up session...");
  await plugin.hooks["session.deleted"]({ sessionId: testSessionId });

  await new Promise(resolve => setTimeout(resolve, 2000));

  // Verify cleanup by trying to use the session
  try {
    await plugin.tools.bash({
      sessionId: testSessionId,
      command: "echo should-fail",
    });
    console.log("✗ Test 4 failed: Session still active after cleanup");
    process.exit(1);
  } catch (error: any) {
    if (error.message?.includes("No sandbox found")) {
      console.log("✓ Test 4 passed: Session cleaned up");
    } else {
      console.log(`✗ Test 4 failed: Unexpected error: ${error.message}`);
      process.exit(1);
    }
  }

  console.log("All plugin lifecycle tests passed!");
}

test().catch(error => {
  console.error("Test failed:", error);
  process.exit(1);
});
ENDOFFILE

# Run from plugin directory so imports resolve
npx tsx "$PLUGIN_DIR/_test_lifecycle.ts"

# Cleanup
rm -f "$PLUGIN_DIR/_test_lifecycle.ts"

echo "Plugin lifecycle tests passed!"
