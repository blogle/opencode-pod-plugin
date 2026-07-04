#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(dirname "$(dirname "$SCRIPT_DIR")")"
PLUGIN_DIR="$ROOT_DIR/plugin"

echo "Testing plugin exec operations..."

# Wait for test pods to be ready
echo "Waiting for test pods..."
kubectl wait --for=condition=Ready pod/test-pod-1 --namespace=opencode-sandbox --timeout=120s
kubectl wait --for=condition=Ready pod/test-pod-2 --namespace=opencode-sandbox --timeout=120s

# Build the plugin
echo "Building plugin..."
cd "$PLUGIN_DIR"
npm install
npm run build

# Write test harness in the plugin directory
cat > "$PLUGIN_DIR/_test_exec.ts" << 'ENDOFFILE'
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
  const testSessionId = "test-session-exec-001";

  console.log("Creating session...");
  await plugin.hooks["session.created"]({ sessionId: testSessionId });

  const record = plugin.sessionStore.get(testSessionId);
  if (!record) {
    console.log("✗ Failed to get session record");
    process.exit(1);
  }
  console.log(`Sandbox pod: ${record.podName}`);

  // Wait for pod to be running
  console.log("Waiting for pod to be running...");
  for (let i = 0; i < 30; i++) {
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

  // Test 1: Write a file
  console.log("Test 1: Writing file via write tool...");
  await plugin.tools.write({
    sessionId: testSessionId,
    path: "/tmp/exec-test.txt",
    content: "Content from exec test",
  });
  console.log("✓ Test 1 passed: File written");

  // Test 2: Verify write landed in the pod via kubectl (independent of plugin)
  console.log("Test 2: Verifying write via kubectl exec...");
  const { execSync } = await import("child_process");
  const kubectlCat = execSync(
    `kubectl exec ${record.podName} -n opencode-sandbox -- cat /tmp/exec-test.txt`,
    { encoding: "utf-8" }
  ).trim();
  if (kubectlCat === "Content from exec test") {
    console.log("✓ Test 2 passed: File verified via kubectl exec");
  } else {
    console.log(`✗ Test 2 failed: Expected "Content from exec test", got "${kubectlCat}"`);
    process.exit(1);
  }

  // Test 3: Edit the file via plugin
  console.log("Test 3: Editing file via edit tool...");
  await plugin.tools.edit({
    sessionId: testSessionId,
    path: "/tmp/exec-test.txt",
    oldString: "Content from exec test",
    newString: "Updated content from exec test",
  });
  console.log("✓ Test 3 passed: File edited");

  // Test 4: Verify edit landed in the pod via kubectl (independent of plugin)
  console.log("Test 4: Verifying edit via kubectl exec...");
  const kubectlCatEdited = execSync(
    `kubectl exec ${record.podName} -n opencode-sandbox -- cat /tmp/exec-test.txt`,
    { encoding: "utf-8" }
  ).trim();
  if (kubectlCatEdited === "Updated content from exec test") {
    console.log("✓ Test 4 passed: Edited file verified via kubectl exec");
  } else {
    console.log(`✗ Test 4 failed: Expected "Updated content from exec test", got "${kubectlCatEdited}"`);
    process.exit(1);
  }

  // Test 5: Bash command execution via plugin
  console.log("Test 5: Running bash command via plugin...");
  const bashOutput = await plugin.tools.bash({
    sessionId: testSessionId,
    command: "echo 'Hello from bash'",
  });
  if (bashOutput.trim() === "Hello from bash") {
    console.log("✓ Test 5 passed: Bash command works");
  } else {
    console.log(`✗ Test 5 failed: Expected "Hello from bash", got "${bashOutput.trim()}"`);
    process.exit(1);
  }

  // Test 6: Verify bash execution landed in the pod via kubectl (independent)
  console.log("Test 6: Verifying bash output via kubectl exec...");
  const marker = `marker-${Date.now()}`;
  await plugin.tools.bash({
    sessionId: testSessionId,
    command: `echo '${marker}' > /tmp/bash-verify.txt`,
  });
  const kubectlBash = execSync(
    `kubectl exec ${record.podName} -n opencode-sandbox -- cat /tmp/bash-verify.txt`,
    { encoding: "utf-8" }
  ).trim();
  if (kubectlBash === marker) {
    console.log("✓ Test 6 passed: Bash execution verified via kubectl exec");
  } else {
    console.log(`✗ Test 6 failed: Expected "${marker}", got "${kubectlBash}"`);
    process.exit(1);
  }

  // Cleanup
  console.log("Cleaning up session...");
  await plugin.hooks["session.deleted"]({ sessionId: testSessionId });

  console.log("All exec tests passed!");
}

test().catch(error => {
  console.error("Test failed:", error);
  process.exit(1);
});
ENDOFFILE

# Run from plugin directory
npx tsx "$PLUGIN_DIR/_test_exec.ts"

# Cleanup
rm -f "$PLUGIN_DIR/_test_exec.ts"

echo "Plugin exec tests passed!"
