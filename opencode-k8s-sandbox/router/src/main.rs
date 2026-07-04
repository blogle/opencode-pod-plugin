mod config;
mod health;
mod index;
mod proxy;

use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::sync::Semaphore;
use tracing_subscriber::EnvFilter;

use config::Config;
use health::Metrics;

#[tokio::main]
async fn main() {
    let config = Config::from_env();

    // Initialize tracing with env-filter
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(&config.log_level)),
        )
        .init();

    tracing::info!("starting opencode-k8s-sandbox-router");
    tracing::info!("proxy addr: {}", config.proxy_addr);
    tracing::info!("health addr: {}", config.health_addr);

    if let Some(ref base_domain) = config.base_domain {
        tracing::info!("base domain: {}", base_domain);
    } else {
        tracing::warn!(
            "ROUTER_BASE_DOMAIN not set - Host header domain validation is disabled. \
             This is insecure for production use. Set ROUTER_BASE_DOMAIN to enable validation."
        );
    }

    // Build k8s client (in-cluster by default, fallback to kubeconfig)
    let k8s_client = match kube::Client::try_default().await {
        Ok(client) => {
            tracing::info!("kubernetes client initialized (in-cluster)");
            client
        }
        Err(e) => {
            tracing::warn!("in-cluster config failed ({}), trying kubeconfig", e);
            kube::Client::try_from(
                kube::config::Config::from_kubeconfig(&Default::default())
                    .await
                    .unwrap(),
            )
            .expect("failed to create kubernetes client")
        }
    };

    // Create the sandbox index
    let index = index::new_index();

    // Create metrics (before reflector so we can pass it)
    let metrics = Arc::new(Metrics::new());

    // Start the reflector
    if let Err(e) = index::start_reflector(
        k8s_client.clone(),
        config.namespace.as_deref(),
        &config.label_key,
        index.clone(),
        metrics.clone(),
    )
    .await
    {
        tracing::error!("failed to start reflector: {}", e);
        std::process::exit(1);
    }

    // Start health listener
    let health_metrics = Arc::clone(&metrics);
    let health_addr = config.health_addr.clone();
    tokio::spawn(async move {
        let listener = TcpListener::bind(&health_addr)
            .await
            .expect("failed to bind health addr");
        tracing::info!("health listener on {}", health_addr);

        let app = health::router(health_metrics);
        axum::serve(listener, app)
            .await
            .expect("health server failed");
    });

    // Start proxy listener
    let proxy_listener = TcpListener::bind(&config.proxy_addr)
        .await
        .expect("failed to bind proxy addr");
    tracing::info!("proxy listener on {}", config.proxy_addr);

    // Create semaphore for connection limiting
    let semaphore = Arc::new(Semaphore::new(config.max_connections));

    loop {
        let (stream, addr) = proxy_listener
            .accept()
            .await
            .expect("failed to accept connection");

        tracing::debug!("accepted connection from {}", addr);

        let permit = match semaphore.clone().acquire_owned().await {
            Ok(permit) => permit,
            Err(_) => {
                tracing::warn!("semaphore closed, rejecting connection");
                drop(stream);
                continue;
            }
        };

        let index = index.clone();
        let config = config.clone();
        let metrics = Arc::clone(&metrics);

        tokio::spawn(async move {
            let _permit = permit;
            proxy::handle_connection(stream, index, config, metrics).await;
        });
    }
}
