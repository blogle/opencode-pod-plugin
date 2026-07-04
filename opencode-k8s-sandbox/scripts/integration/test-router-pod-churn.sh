#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(dirname "$(dirname "$SCRIPT_DIR")")"

echo "Testing router pod churn resilience..."

# Wait for router to be ready
echo "Waiting for router deployment..."
kubectl wait --for=condition=Available deployment/opencode-k8s-sandbox-router \
  --namespace=opencode-sandbox --timeout=120s

# Initial test - should work
echo "Initial test: Routing to sandbox aaaa1111..."
RESPONSE=$(curl -s -o /dev/null -w "%{http_code}" \
  --resolve "8080-aaaa1111.opencode.example.com:30080:127.0.0.1" \
  "http://8080-aaaa1111.opencode.example.com:30080/")

if [ "$RESPONSE" = "200" ]; then
  echo "✓ Initial test passed: Got 200 response"
else
  echo "✗ Initial test failed: Expected 200, got $RESPONSE"
  exit 1
fi

# Delete and recreate test pod 1
echo "Deleting test pod 1..."
kubectl delete pod test-pod-1 --namespace=opencode-sandbox

echo "Recreating test pod 1..."
cat <<'EOF' | kubectl apply -f -
apiVersion: v1
kind: Pod
metadata:
  name: test-pod-1
  namespace: opencode-sandbox
  labels:
    opencode.dev/sandbox-id: aaaa1111
spec:
  containers:
    - name: server
      image: python:3.11-slim
      command: ["/bin/bash", "-c"]
      args: ["python -m http.server 8080 --directory /tmp"]
EOF

echo "Waiting for new pod to be ready..."
kubectl wait --for=condition=Ready pod/test-pod-1 --namespace=opencode-sandbox --timeout=60s

# Poll until the router routes successfully to the new pod (max 30 seconds)
echo "Waiting for router to route to new pod..."
MAX_RETRIES=30
for i in $(seq 1 $MAX_RETRIES); do
  RESPONSE=$(curl -s -o /dev/null -w "%{http_code}" \
    --resolve "8080-aaaa1111.opencode.example.com:30080:127.0.0.1" \
    "http://8080-aaaa1111.opencode.example.com:30080/")
  if [ "$RESPONSE" = "200" ]; then
    echo "✓ Final test passed: Got 200 response after pod restart (attempt $i)"
    exit 0
  fi
  sleep 1
done

echo "✗ Final test failed: Expected 200 after $MAX_RETRIES attempts, got $RESPONSE"
exit 1
