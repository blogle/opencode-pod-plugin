#!/usr/bin/env bash
set -euo pipefail

CLUSTER_NAME="opencode-sandbox-dev"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(dirname "$SCRIPT_DIR")"

echo "Creating kind cluster: $CLUSTER_NAME"

# Delete existing cluster if it exists
if kind get clusters 2>/dev/null | grep -q "^${CLUSTER_NAME}$"; then
  echo "Cluster $CLUSTER_NAME already exists, deleting..."
  kind delete cluster --name "$CLUSTER_NAME"
fi

# Create the cluster with port mappings for the router
kind create cluster \
  --name "$CLUSTER_NAME" \
  --config - <<EOF
kind: Cluster
apiVersion: kind.x-k8s.io/v1alpha4
nodes:
  - role: control-plane
    extraPortMappings:
      - containerPort: 30080
        hostPort: 30080
        protocol: TCP
      - containerPort: 30090
        hostPort: 30090
        protocol: TCP
EOF

echo "Cluster created successfully"

# Create namespace
kubectl create namespace opencode-sandbox

# Create default service account in the namespace
kubectl create serviceaccount default --namespace=opencode-sandbox --dry-run=client -o yaml | kubectl apply -f -

# Apply RBAC
kubectl apply -f "$ROOT_DIR/deploy/serviceaccount.yaml"
kubectl apply -f "$ROOT_DIR/deploy/clusterrole.yaml"
kubectl apply -f "$ROOT_DIR/deploy/clusterrolebinding.yaml"

echo "RBAC applied"

# Build and load sandbox image into kind
echo "Building sandbox image..."
cd "$ROOT_DIR/sandbox-image"
docker build -t opencode-sandbox:latest .
kind load docker-image opencode-sandbox:latest --name "$CLUSTER_NAME"

echo "Sandbox image loaded into kind"

# Load alpine/git image for init container
echo "Loading alpine/git image for init container..."
docker pull alpine/git:latest
kind load docker-image alpine/git:latest --name "$CLUSTER_NAME"

echo "alpine/git image loaded into kind"

# Build and load router image into kind
echo "Building router image..."
cd "$ROOT_DIR/router"
docker build -t opencode-k8s-sandbox-router:latest .
kind load docker-image opencode-k8s-sandbox-router:latest --name "$CLUSTER_NAME"

# Apply router deployment and service
kubectl apply -f "$ROOT_DIR/deploy/deployment.yaml"
kubectl apply -f "$ROOT_DIR/deploy/service.yaml"

echo "Router deployment applied"

# Create test pods for router testing
echo "Creating test pods..."

# Test pod 1: python http server on port 8080
cat <<'EOF1' | kubectl apply -f -
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
EOF1

# Test pod 2: python http server on port 5173
cat <<'EOF2' | kubectl apply -f -
apiVersion: v1
kind: Pod
metadata:
  name: test-pod-2
  namespace: opencode-sandbox
  labels:
    opencode.dev/sandbox-id: bbbb2222
spec:
  containers:
    - name: server
      image: python:3.11-slim
      command: ["/bin/bash", "-c"]
      args: ["python -m http.server 5173 --directory /tmp"]
EOF2

# Test pod 3: simple websocket echo server
cat <<'EOF' | kubectl apply -f -
apiVersion: v1
kind: Pod
metadata:
  name: test-pod-websocket
  namespace: opencode-sandbox
  labels:
    opencode.dev/sandbox-id: cccc3333
spec:
  containers:
    - name: ws-server
      image: python:3.11-slim
      command:
        - /bin/bash
        - -c
        - |
          pip install websocket-server
          python -c "
          from websocket_server import WebsocketServer
          def new_client(client, server):
              print(f'New client: {client}')
          def message_received(client, server, message):
              print(f'Message from {client}: {message}')
              server.send_message(client, message)
          server = WebsocketServer(host='0.0.0.0', port=8080)
          server.set_fn_new_client(new_client)
          server.set_fn_message_received(message_received)
          server.run_forever()
          "
EOF

echo "Waiting for pods to be ready..."
kubectl wait --for=condition=Ready pod/test-pod-1 --namespace=opencode-sandbox --timeout=120s
kubectl wait --for=condition=Ready pod/test-pod-2 --namespace=opencode-sandbox --timeout=120s
kubectl wait --for=condition=Ready pod/test-pod-websocket --namespace=opencode-sandbox --timeout=120s

echo "All test pods are ready"
kubectl get pods --namespace=opencode-sandbox -o wide
