use axum::{extract::State, routing::get, Router};
use prometheus::{IntGauge, Registry, TextEncoder};
use std::sync::Arc;

pub struct Metrics {
    pub active_connections: IntGauge,
    pub total_connections: IntGauge,
    pub lookup_misses: IntGauge,
    pub bytes_proxied: IntGauge,
    pub registry: Registry,
}

impl Metrics {
    pub fn new() -> Self {
        let registry = Registry::new();
        let active_connections =
            IntGauge::new("active_connections", "Number of active connections").unwrap();
        let total_connections =
            IntGauge::new("total_connections", "Total connections accepted").unwrap();
        let lookup_misses = IntGauge::new("lookup_misses", "Total sandbox lookup misses").unwrap();
        let bytes_proxied = IntGauge::new("bytes_proxied", "Total bytes proxied").unwrap();

        registry
            .register(Box::new(active_connections.clone()))
            .unwrap();
        registry
            .register(Box::new(total_connections.clone()))
            .unwrap();
        registry.register(Box::new(lookup_misses.clone())).unwrap();
        registry.register(Box::new(bytes_proxied.clone())).unwrap();

        Self {
            active_connections,
            total_connections,
            lookup_misses,
            bytes_proxied,
            registry,
        }
    }
}

// Wrap in Arc for cloning in axum state
pub type SharedMetrics = Arc<Metrics>;

async fn healthz() -> &'static str {
    "ok"
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
