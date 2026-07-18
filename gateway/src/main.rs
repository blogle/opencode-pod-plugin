use std::{env, sync::Arc};

use anyhow::{Context, Result};
use opencode_gateway::{
    api, auth::Authenticator, central::CentralClient, checkpoint::CheckpointStorage,
    config::Config, k8s::Kubernetes, lifecycle, preview, state::Store, AppState,
};
use tokio::net::TcpListener;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .json()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let config_path =
        env::var("GATEWAY_CONFIG").unwrap_or_else(|_| "/etc/opencode-sandbox/platform.yaml".into());
    let config = Arc::new(Config::load(&config_path)?);
    config.projects().context("validate project registry")?;
    let store = Store::open(&config.state_path)?;
    let checkpoints = CheckpointStorage::new(&config.checkpoint.path, store.clone())?;
    let central = CentralClient::new(&config.opencode.central_url)?;
    let auth = Authenticator::new(&config.auth)?;
    let client = kube::Client::try_default()
        .await
        .context("initialize Kubernetes client")?;
    let kubernetes = Kubernetes::new(client, config.clone()).await?;
    let state = AppState {
        config: config.clone(),
        store,
        k8s: kubernetes,
        operations: Arc::new(tokio::sync::Mutex::new(())),
        checkpoints,
        central,
        auth,
    };

    let api_listener = TcpListener::bind(&config.listen)
        .await
        .with_context(|| format!("bind API listener {}", config.listen))?;
    let preview_listener = TcpListener::bind(&config.preview_listen)
        .await
        .with_context(|| format!("bind preview listener {}", config.preview_listen))?;
    tracing::info!(api_listen=%config.listen, preview_listen=%config.preview_listen, namespace=%config.namespace, "gateway started");

    let api_state = state.clone();
    tokio::spawn(lifecycle::run_idle_reconciler(state.clone()));
    tokio::try_join!(
        async {
            axum::serve(api_listener, api::router(api_state))
                .await
                .context("API server failed")
        },
        preview::serve(preview_listener, state),
    )?;
    Ok(())
}
