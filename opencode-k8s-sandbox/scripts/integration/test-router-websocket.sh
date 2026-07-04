#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(dirname "$(dirname "$SCRIPT_DIR")")"

echo "Testing router WebSocket support..."

# Wait for router to be ready
echo "Waiting for router deployment..."
kubectl wait --for=condition=Available deployment/opencode-k8s-sandbox-router \
  --namespace=opencode-sandbox --timeout=120s

# Wait for websocket pod to be ready
echo "Waiting for websocket pod..."
kubectl wait --for=condition=Ready pod/test-pod-websocket --namespace=opencode-sandbox --timeout=120s

# Give the websocket server time to start (pip install + python startup)
echo "Waiting for websocket server to be ready..."
sleep 10

# Use Python to test WebSocket upgrade (websocat doesn't support --resolve)
echo "Testing WebSocket upgrade to cccc3333 on port 8080..."

MAX_RETRIES=5
for i in $(seq 1 $MAX_RETRIES); do
  if python3 -c "
import socket, os, sys, base64

host = '8080-cccc3333.opencode.example.com'
try:
    sock = socket.create_connection(('127.0.0.1', 30080), timeout=5)
except Exception as e:
    print(f'Connection failed: {e}', file=sys.stderr)
    sys.exit(1)

key = base64.b64encode(os.urandom(16)).decode()

req = (
    f'GET / HTTP/1.1\r\n'
    f'Host: {host}\r\n'
    f'Upgrade: websocket\r\n'
    f'Connection: Upgrade\r\n'
    f'Sec-WebSocket-Key: {key}\r\n'
    f'Sec-WebSocket-Version: 13\r\n'
    f'\r\n'
)
sock.sendall(req.encode())

resp = b''
while b'\r\n\r\n' not in resp:
    chunk = sock.recv(4096)
    if not chunk:
        break
    resp += chunk

resp_str = resp.decode(errors='replace')
if '101 Switching Protocols' in resp_str:
    print('WebSocket upgrade successful (HTTP 101)')
    sock.close()
    sys.exit(0)
else:
    print(f'WebSocket upgrade failed: {resp_str[:200]}', file=sys.stderr)
    sock.close()
    sys.exit(1)
" 2>&1; then
    echo "✓ WebSocket test passed"
    break
  else
    echo "Retry $i/$MAX_RETRIES..."
    sleep 3
    if [ "$i" -eq "$MAX_RETRIES" ]; then
      echo "✗ WebSocket test failed after $MAX_RETRIES retries"
      exit 1
    fi
  fi
done

echo "WebSocket test passed!"
