use axum::{extract::State, routing::get, Router};
use prometheus::{IntCounter, IntGauge, Registry, TextEncoder};
use std::sync::Arc;

pub struct Metrics {
    pub active_connections: IntGauge,
    pub total_connections: IntCounter,
    pub lookup_misses: IntCounter,
    pub bytes_proxied: IntCounter,
    pub reflector_healthy: IntGauge,
    pub registry: Registry,
}

impl Metrics {
    pub fn new() -> Self {
        let registry = Registry::new();
        let active_connections =
            IntGauge::new("active_connections", "Number of active connections").unwrap();
        let total_connections =
            IntCounter::new("total_connections", "Total connections accepted").unwrap();
        let lookup_misses = IntCounter::new("lookup_misses", "Total sandbox lookup misses").unwrap();
        let bytes_proxied = IntCounter::new("bytes_proxied", "Total bytes proxied").unwrap();
        let reflector_healthy =
            IntGauge::new("reflector_healthy", "Whether the reflector is healthy (1=healthy)").unwrap();

        registry
            .register(Box::new(active_connections.clone()))
            .unwrap();
        registry
            .register(Box::new(total_connections.clone()))
            .unwrap();
        registry.register(Box::new(lookup_misses.clone())).unwrap();
        registry.register(Box::new(bytes_proxied.clone())).unwrap();
        registry.register(Box::new(reflector_healthy.clone())).unwrap();

        Self {
            active_connections,
            total_connections,
            lookup_misses,
            bytes_proxied,
            reflector_healthy,
            registry,
        }
    }
}

// Wrap in Arc for cloning in axum state
pub type SharedMetrics = Arc<Metrics>;

async fn healthz(State(metrics): State<SharedMetrics>) -> axum::http::StatusCode {
    if metrics.reflector_healthy.get() == 0 {
        return axum::http::StatusCode::SERVICE_UNAVAILABLE;
    }
    axum::http::StatusCode::OK
}

async fn metrics_handler(State(metrics): State<SharedMetrics>) -> String {
    let encoder = TextEncoder::new();
    let metric_families = metrics.registry.gather();
    let mut buffer = String::new();
    encoder.encode_utf8(&metric_families, &mut buffer).unwrap();
    buffer
}

pub fn router(metrics: SharedMetrics) -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/metrics", get(metrics_handler))
        .with_state(metrics)
}
