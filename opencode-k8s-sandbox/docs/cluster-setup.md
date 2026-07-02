# Cluster Setup

## Prerequisites

- Kubernetes cluster (1.24+)
- cert-manager for TLS certificates
- nginx-ingress or similar ingress controller

## DNS Configuration

Configure wildcard DNS for sandbox subdomains:

```
*.sandbox.example.com -> Ingress controller IP
```

## Certificate Management

cert-manager ClusterIssuer for wildcard certs:

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
    - http01:
        ingress:
          class: nginx
```

## Per-Controller Examples

### nginx-ingress

```yaml
apiVersion: networking.k8s.io/v1
kind: Ingress
metadata:
  name: sandbox-router
  annotations:
    cert-manager.io/cluster-issuer: letsencrypt-prod
spec:
  tls:
  - hosts:
    - sandbox.example.com
    secretName: sandbox-tls
  rules:
  - host: sandbox.example.com
    http:
      paths:
      - path: /
        pathType: Prefix
        backend:
          service:
            name: sandbox-router
            port:
              number: 3000
```
