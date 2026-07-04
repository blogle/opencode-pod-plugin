use std::env;

#[derive(Debug, Clone)]
pub struct Config {
    pub proxy_addr: String,
    pub health_addr: String,
    pub namespace: Option<String>,
    pub label_key: String,
    pub log_level: String,
    pub connect_timeout_ms: u64,
    pub max_header_bytes: usize,
    pub base_domain: Option<String>,
    pub header_timeout_ms: u64,
    pub max_connections: usize,
}

impl Config {
    pub fn from_env() -> Self {
        let base_domain = env::var("ROUTER_BASE_DOMAIN").ok().filter(|s| !s.is_empty());
        if base_domain.is_none() {
            tracing::warn!(
                "ROUTER_BASE_DOMAIN not set - Host header domain validation is disabled. \
                 This is insecure for production use. Set ROUTER_BASE_DOMAIN to enable validation."
            );
        }

        Self {
            proxy_addr: env::var("ROUTER_PROXY_ADDR").unwrap_or_else(|_| "0.0.0.0:8080".into()),
            health_addr: env::var("ROUTER_HEALTH_ADDR").unwrap_or_else(|_| "0.0.0.0:9090".into()),
            namespace: env::var("ROUTER_NAMESPACE").ok().filter(|s| !s.is_empty()),
            label_key: env::var("ROUTER_LABEL_KEY")
                .unwrap_or_else(|_| "opencode.dev/sandbox-id".into()),
            log_level: env::var("ROUTER_LOG").unwrap_or_else(|_| "info".into()),
            connect_timeout_ms: env::var("ROUTER_CONNECT_TIMEOUT_MS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(3000),
            max_header_bytes: env::var("ROUTER_MAX_HEADER_BYTES")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(8192),
            base_domain,
            header_timeout_ms: env::var("ROUTER_HEADER_TIMEOUT_MS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(10000),
            max_connections: env::var("ROUTER_MAX_CONNECTIONS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(1024),
        }
    }
}
