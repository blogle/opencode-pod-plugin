# Cluster Setup

## Prerequisites

- Kubernetes cluster (1.24+)
- cert-manager for TLS certificates
- Ingress controller (nginx-ingress, Traefik, or similar)
- Wildcard DNS record pointing to the ingress controller

## Important: Security Considerations

This system runs arbitrary code in sandbox pods. Before deploying:

1. **Network Isolation**: Deploy behind a private network or auth proxy
2. **Authentication**: Put an auth proxy in front of both OpenCode and the router
3. **Resource Quotas**: Set appropriate ResourceQuotas and LimitRanges in the namespace
4. **Pod Security**: The provided manifests include security contexts, but review them for your environment

## DNS Configuration

Configure wildcard DNS for sandbox subdomains:

```
*.opencode.example.com -> Ingress controller IP
```

For local development with k3s, you can use nip.io or sslip.io:

```
*.127.0.0.1.nip.io -> 127.0.0.1
```

## Certificate Management

cert-manager ClusterIssuer for wildcard certs (DNS-01 challenge required for wildcards):

```yaml
apiVersion: cert-manager.io/v1
kind: ClusterIssuer
metadata:
  name: letsencrypt-prod
spec:
  acme:
    server: https://acme-v02.api.letsencrypt.org/directory
    email: admin@example.com
    privateKeySecretRef:
      name: letsencrypt-prod
    solvers:
    - dns01:
        cloudDNS:
          project: my-gcp-project
```

## K3s Specific Setup

K3s comes with Traefik pre-installed. To use it with this system:

1. **Disable K3s Traefik** (optional, if you want nginx-ingress):
   ```bash
   curl -sfL https://get.k3s.io | INSTALL_K3S_EXEC="--disable=traefik" sh -
   ```

2. **Configure Traefik** (if using built-in Traefik):
   - Traefik automatically handles wildcard TLS with cert-manager
   - Create an IngressRoute or standard Ingress pointing to the router service

3. **Service Type**: The default deployment uses ClusterIP. For K3s without a load balancer, you may need to:
   - Change service.yaml to use `type: NodePort` or `type: LoadBalancer`
   - Or install MetalLB for bare-metal clusters

## Per-Controller Examples

### nginx-ingress

```yaml
apiVersion: networking.k8s.io/v1
kind: Ingress
metadata:
  name: sandbox-router
  namespace: opencode-sandbox
  annotations:
    cert-manager.io/cluster-issuer: letsencrypt-prod
spec:
  tls:
  - hosts:
    - "*.opencode.example.com"
    secretName: opencode-wildcard-tls
  rules:
  - host: "*.opencode.example.com"
    http:
      paths:
      - path: /
        pathType: Prefix
        backend:
          service:
            name: opencode-k8s-sandbox-router
            port:
              number: 8080
```

### Traefik (K3s native)

```yaml
apiVersion: networking.k8s.io/v1
kind: Ingress
metadata:
  name: sandbox-router
  namespace: opencode-sandbox
  annotations:
    cert-manager.io/cluster-issuer: letsencrypt-prod
spec:
  tls:
  - hosts:
    - "*.opencode.example.com"
    secretName: opencode-wildcard-tls
  rules:
  - host: "*.opencode.example.com"
    http:
      paths:
      - path: /
        pathType: Prefix
        backend:
          service:
            name: opencode-k8s-sandbox-router
            port:
              number: 8080
```

## Auth Proxy Setup

For production use, put an auth proxy in front:

### Using oauth2-proxy

```yaml
apiVersion: apps/v1
kind: Deployment
metadata:
  name: oauth2-proxy
  namespace: opencode-sandbox
spec:
  replicas: 1
  selector:
    matchLabels:
      app: oauth2-proxy
  template:
    metadata:
      labels:
        app: oauth2-proxy
    spec:
      containers:
      - name: oauth2-proxy
        image: quay.io/oauth2-proxy/oauth2-proxy:v7.6.0
        args:
        - --provider=google
        - --email-domain=example.com
        - --upstream=http://opencode-k8s-sandbox-router:8080
        - --http-address=0.0.0.0:4180
        env:
        - name: OAUTH2_PROXY_CLIENT_ID
          valueFrom:
            secretKeyRef:
              name: oauth2-proxy
              key: client-id
        - name: OAUTH2_PROXY_CLIENT_SECRET
          valueFrom:
            secretKeyRef:
              name: oauth2-proxy
              key: client-secret
        - name: OAUTH2_PROXY_COOKIE_SECRET
          valueFrom:
            secretKeyRef:
              name: oauth2-proxy
              key: cookie-secret
        ports:
        - containerPort: 4180
```

Then point your Ingress to the oauth2-proxy service instead of the router directly.

## Deployment

Apply the manifests:

```bash
kubectl apply -k opencode-k8s-sandbox/deploy/
```

Verify the deployment:

```bash
kubectl get pods -n opencode-sandbox
kubectl get svc -n opencode-sandbox
kubectl get ingress -n opencode-sandbox
```

## Testing

1. Create a test pod with the sandbox label:
   ```bash
   kubectl run test-sandbox --image=nginx --namespace=opencode-sandbox \
     --labels="opencode.dev/sandbox-id=abcd1234,app.kubernetes.io/managed-by=opencode-k8s-sandbox"
   ```

2. Verify the router can reach it:
   ```bash
   curl -H "Host: 80-abcd1234.opencode.example.com" http://router-service:8080
   ```
