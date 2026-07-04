#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(dirname "$SCRIPT_DIR")"

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

# Track test results
TESTS_PASSED=0
TESTS_FAILED=0
TESTS_SKIPPED=0

# Cleanup function
cleanup() {
  echo -e "\n${YELLOW}Cleaning up...${NC}"
  bash "$SCRIPT_DIR/kind-down.sh" 2>/dev/null || true
}

# Set up trap for cleanup
trap cleanup EXIT

# Run a test and track results
run_test() {
  local test_name="$1"
  local test_script="$2"

  echo -e "\n${YELLOW}Running test: ${test_name}${NC}"
  echo "=========================================="

  if bash "$test_script"; then
    echo -e "${GREEN}✓ ${test_name} passed${NC}"
    TESTS_PASSED=$((TESTS_PASSED + 1))
  else
    echo -e "${RED}✗ ${test_name} failed${NC}"
    TESTS_FAILED=$((TESTS_FAILED + 1))
    return 1
  fi
}

# Static checks (fast, no cluster needed)
echo -e "${YELLOW}=== Phase 1: Static Checks ===${NC}"

# Router checks
echo -e "\n${YELLOW}Router static checks:${NC}"

echo "Checking cargo fmt..."
cd "$ROOT_DIR/router"
if cargo fmt --check; then
  echo -e "${GREEN}✓ cargo fmt check passed${NC}"
  TESTS_PASSED=$((TESTS_PASSED + 1))
else
  echo -e "${RED}✗ cargo fmt check failed${NC}"
  TESTS_FAILED=$((TESTS_FAILED + 1))
  cargo fmt
  echo "  Auto-formatted code"
fi

echo "Checking cargo clippy..."
if cargo clippy --all-targets -- -D warnings; then
  echo -e "${GREEN}✓ cargo clippy check passed${NC}"
  TESTS_PASSED=$((TESTS_PASSED + 1))
else
  echo -e "${RED}✗ cargo clippy check failed${NC}"
  TESTS_FAILED=$((TESTS_FAILED + 1))
fi

echo "Running cargo test..."
if cargo test; then
  echo -e "${GREEN}✓ cargo test passed${NC}"
  TESTS_PASSED=$((TESTS_PASSED + 1))
else
  echo -e "${RED}✗ cargo test failed${NC}"
  TESTS_FAILED=$((TESTS_FAILED + 1))
fi

# Plugin checks
echo -e "\n${YELLOW}Plugin static checks:${NC}"
cd "$ROOT_DIR/plugin"

echo "Installing plugin dependencies..."
npm install

echo "Checking TypeScript..."
if npx tsc --noEmit; then
  echo -e "${GREEN}✓ TypeScript check passed${NC}"
  TESTS_PASSED=$((TESTS_PASSED + 1))
else
  echo -e "${RED}✗ TypeScript check failed${NC}"
  TESTS_FAILED=$((TESTS_FAILED + 1))
fi

echo "Running npm test..."
if npm test; then
  echo -e "${GREEN}✓ npm test passed${NC}"
  TESTS_PASSED=$((TESTS_PASSED + 1))
else
  echo -e "${RED}✗ npm test failed${NC}"
  TESTS_FAILED=$((TESTS_FAILED + 1))
fi

# Integration checks (against kind)
echo -e "\n${YELLOW}=== Phase 2: Integration Checks ===${NC}"

# Create kind cluster and test pods
run_test "kind-up" "$SCRIPT_DIR/kind-up.sh" || true

# Router tests
run_test "router-routing" "$SCRIPT_DIR/integration/test-router-routing.sh" || true
run_test "router-websocket" "$SCRIPT_DIR/integration/test-router-websocket.sh" || true
run_test "router-pod-churn" "$SCRIPT_DIR/integration/test-router-pod-churn.sh" || true

# Plugin tests
run_test "plugin-pod-lifecycle" "$SCRIPT_DIR/integration/test-plugin-pod-lifecycle.sh" || true
run_test "plugin-exec" "$SCRIPT_DIR/integration/test-plugin-exec.sh" || true
run_test "plugin-repo-selection" "$SCRIPT_DIR/integration/test-repo-selection.sh" || true

# Print results
echo -e "\n${YELLOW}=== Test Results ===${NC}"
echo -e "${GREEN}Passed: ${TESTS_PASSED}${NC}"
echo -e "${RED}Failed: ${TESTS_FAILED}${NC}"

if [ $TESTS_FAILED -gt 0 ]; then
  echo -e "\n${RED}Some tests failed!${NC}"
  exit 1
else
  echo -e "\n${GREEN}All tests passed!${NC}"
  exit 0
fi
