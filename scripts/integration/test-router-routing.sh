#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(dirname "$(dirname "$SCRIPT_DIR")")"

echo "Testing router routing..."

# Wait for router to be ready
echo "Waiting for router deployment..."
kubectl wait --for=condition=Available deployment/opencode-k8s-sandbox-router \
  --namespace=opencode-sandbox --timeout=120s

# The router is exposed via kind's extraPortMappings on localhost:30080
# We use curl --resolve to simulate wildcard DNS
ROUTER_HOST="127.0.0.1:30080"

# Test 1: Route to test pod 1 (port 8080)
echo "Test 1: Routing to sandbox aaaa1111 on port 8080..."
RESPONSE=$(curl -s -o /dev/null -w "%{http_code}" \
  --resolve "8080-aaaa1111.opencode.example.com:30080:127.0.0.1" \
  "http://8080-aaaa1111.opencode.example.com:30080/")

if [ "$RESPONSE" = "200" ]; then
  echo "✓ Test 1 passed: Got 200 response"
else
  echo "✗ Test 1 failed: Expected 200, got $RESPONSE"
  exit 1
fi

# Test 2: Route to test pod 2 (port 5173)
echo "Test 2: Routing to sandbox bbbb2222 on port 5173..."
RESPONSE=$(curl -s -o /dev/null -w "%{http_code}" \
  --resolve "5173-bbbb2222.opencode.example.com:30080:127.0.0.1" \
  "http://5173-bbbb2222.opencode.example.com:30080/")

if [ "$RESPONSE" = "200" ]; then
  echo "✓ Test 2 passed: Got 200 response"
else
  echo "✗ Test 2 failed: Expected 200, got $RESPONSE"
  exit 1
fi

# Test 3: Non-existent sandbox should return 502
echo "Test 3: Non-existent sandbox should return 502..."
RESPONSE=$(curl -s -o /dev/null -w "%{http_code}" \
  --resolve "8080-deadbeef.opencode.example.com:30080:127.0.0.1" \
  "http://8080-deadbeef.opencode.example.com:30080/")

if [ "$RESPONSE" = "502" ]; then
  echo "✓ Test 3 passed: Got 502 response for non-existent sandbox"
else
  echo "✗ Test 3 failed: Expected 502, got $RESPONSE"
  exit 1
fi

# Test 4: Invalid hostname should return 400
echo "Test 4: Invalid hostname should return 400..."
RESPONSE=$(curl -s -o /dev/null -w "%{http_code}" \
  --resolve "invalid.opencode.example.com:30080:127.0.0.1" \
  "http://invalid.opencode.example.com:30080/")

if [ "$RESPONSE" = "400" ]; then
  echo "✓ Test 4 passed: Got 400 response for invalid hostname"
else
  echo "✗ Test 4 failed: Expected 400, got $RESPONSE"
  exit 1
fi

echo "All routing tests passed!"
