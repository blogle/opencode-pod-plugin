#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(dirname "$(dirname "$SCRIPT_DIR")")"
PLUGIN_DIR="$ROOT_DIR/plugin"

echo "Testing repo selection and init container git clone..."

# Wait for test pods to be ready
echo "Waiting for test pods..."
kubectl wait --for=condition=Ready pod/test-pod-1 --namespace=opencode-sandbox --timeout=120s
kubectl wait --for=condition=Ready pod/test-pod-2 --namespace=opencode-sandbox --timeout=120s

# Build the plugin
echo "Building plugin..."
cd "$PLUGIN_DIR"
npm install
npm run build

# Write test harness that tests repo cloning via init container
cat > "$PLUGIN_DIR/_test_repo_selection.ts" << 'ENDOFFILE'
import { loadConfig } from "./src/config.js";
import { SessionStore } from "./src/sessionStore.js";
import { sessionCreated, sessionDeleted } from "./src/hooks.js";
import { cloneRepos } from "./src/repos.js";
import { execSync } from "child_process";

async function main() {
  const pluginConfig = {
    namespace: "opencode-sandbox",
    sandboxImage: "opencode-sandbox:latest",
    repos: {
      "hello-world": "https://github.com/octocat/Hello-World.git",
    },
    baseDomain: "opencode.example.com",
  };

  const config = loadConfig(pluginConfig);
  const sessionStore = new SessionStore();

  const mockShell = async (strings: TemplateStringsArray, ...values: unknown[]) => {
    let cmd = "";
    strings.forEach((str, i) => {
      cmd += str;
      if (i < values.length) {
        cmd += values[i];
      }
    });
    return execSync(cmd, { encoding: "utf-8" });
  };

  const repoMapping = await cloneRepos(config, process.env.HOME + "/.opencode", mockShell);

  let testRepoUrl: string | undefined;
  for (const [, repo] of repoMapping) {
    if (repo.name === "hello-world") {
      testRepoUrl = repo.url;
      break;
    }
  }

  const testSessionId = "test-session-repo-001";

  console.log("Test 1: Creating session with repo...");
  await sessionCreated({
    sessionId: testSessionId,
    config,
    sessionStore,
    repoUrl: testRepoUrl,
  });

  const record = sessionStore.get(testSessionId);
  if (!record) {
    console.log("✗ Test 1 failed: No session record found");
    process.exit(1);
  }
  console.log(`✓ Test 1 passed: Pod created: ${record.podName}`);
  console.log(`  Repo URL: ${record.repoUrl}`);

  // Wait for pod to be running (init container needs time to clone)
  console.log("Waiting for pod to be running (including init container)...");
  for (let i = 0; i < 60; i++) {
    try {
      const output = execSync(`kubectl exec -n opencode-sandbox ${record.podName} -- echo running`, { encoding: "utf-8" });
      if (output.trim() === "running") {
        console.log("Pod is running");
        break;
      }
    } catch {
      // Pod might not be ready yet
    }
    if (i === 59) {
      console.log("✗ Pod did not become ready within timeout");
      process.exit(1);
    }
    await new Promise(resolve => setTimeout(resolve, 1000));
  }

  // Test 2: Verify repo was cloned via init container
  console.log("Test 2: Verifying repo was cloned...");
  const lsOutput = execSync(`kubectl exec -n opencode-sandbox ${record.podName} -- ls -la /workspace`, { encoding: "utf-8" });
  console.log(`  /workspace contents: ${lsOutput.trim()}`);

  const readmeCheck = execSync(`kubectl exec -n opencode-sandbox ${record.podName} -- test -f /workspace/README && echo exists || echo missing`, { encoding: "utf-8" });
  if (readmeCheck.trim() === "exists") {
    console.log("✓ Test 2 passed: Repo cloned successfully (README found)");
  } else {
    console.log(`✗ Test 2 failed: README not found in /workspace`);
    console.log(`  Output of ls /workspace: ${lsOutput}`);
    process.exit(1);
  }

  // Test 3: Verify we can read repo contents
  console.log("Test 3: Reading repo contents...");
  const readmeContent = execSync(`kubectl exec -n opencode-sandbox ${record.podName} -- cat /workspace/README`, { encoding: "utf-8" });
  if (readmeContent.includes("Hello World") || readmeContent.includes("hello")) {
    console.log("✓ Test 3 passed: Repo contents readable");
  } else {
    console.log(`✗ Test 3 failed: Unexpected README content: ${readmeContent.substring(0, 100)}`);
    process.exit(1);
  }

  // Test 4: Verify we can write to the repo
  console.log("Test 4: Writing to repo...");
  execSync(`kubectl exec -n opencode-sandbox ${record.podName} -- sh -c 'echo "Written by opencode test" > /workspace/test-file.txt'`, { encoding: "utf-8" });
  const writeVerify = execSync(`kubectl exec -n opencode-sandbox ${record.podName} -- cat /workspace/test-file.txt`, { encoding: "utf-8" });
  if (writeVerify.trim() === "Written by opencode test") {
    console.log("✓ Test 4 passed: Write to repo works");
  } else {
    console.log(`✗ Test 4 failed: Write verification failed`);
    process.exit(1);
  }

  // Cleanup
  console.log("Cleaning up session...");
  await sessionDeleted({
    sessionId: testSessionId,
    config,
    sessionStore,
  });

  console.log("All repo selection tests passed!");
}

main().catch(error => {
  console.error("Test failed:", error);
  process.exit(1);
});
ENDOFFILE

# Run from plugin directory so imports resolve
npx tsx "$PLUGIN_DIR/_test_repo_selection.ts"

# Cleanup
rm -f "$PLUGIN_DIR/_test_repo_selection.ts"

echo "Repo selection tests passed!"
