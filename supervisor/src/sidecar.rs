use crate::checkpoint::{capture, CheckpointArtifact};
use crate::supervisor::authorized;
use anyhow::{ensure, Context, Result};
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use reqwest::header::{HeaderValue, CONTENT_TYPE};
use serde::Serialize;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use tracing::info;

#[derive(Debug, Clone)]
pub struct SidecarConfig {
    pub workspace: PathBuf,
    pub workspace_id: String,
    pub output_dir: PathBuf,
    pub listen: String,
    pub control_token: String,
    pub gateway_url: String,
    pub gateway_token: String,
}

#[derive(Clone)]
struct SidecarState {
    config: Arc<SidecarConfig>,
    capture_lock: Arc<Mutex<()>>,
    client: reqwest::Client,
}

#[derive(Serialize)]
struct Health {
    healthy: bool,
    workspace_id: String,
}

/// Runs the authenticated, loopback-only checkpoint sidecar HTTP API.
///
/// # Errors
///
/// Returns an error for invalid configuration, listener failures, or server failures.
pub async fn run(config: SidecarConfig) -> Result<()> {
    validate(&config)?;
    let listener = tokio::net::TcpListener::bind(&config.listen)
        .await
        .with_context(|| format!("bind checkpoint sidecar API at {}", config.listen))?;
    let state = SidecarState {
        config: Arc::new(config),
        capture_lock: Arc::new(Mutex::new(())),
        client: reqwest::Client::builder()
            .timeout(Duration::new(120, 0))
            .build()
            .context("build checkpoint upload client")?,
    };
    let app = Router::new()
        .route("/healthz", get(health))
        .route("/checkpoint", post(checkpoint))
        .with_state(state);
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("run checkpoint sidecar API")
}

/// Uploads one metadata and bundle pair to the workspace-scoped gateway endpoint.
///
/// # Errors
///
/// Returns an error if the artifact cannot be read, the request fails, or the gateway rejects it.
pub async fn upload(
    artifact: &CheckpointArtifact,
    gateway_url: &str,
    gateway_token: &str,
) -> Result<()> {
    let client = reqwest::Client::builder()
        .timeout(Duration::new(120, 0))
        .build()
        .context("build checkpoint upload client")?;
    upload_with_client(&client, artifact, gateway_url, gateway_token).await
}

async fn health(State(state): State<SidecarState>, headers: HeaderMap) -> Response {
    if !authorized(&headers, &state.config.control_token) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    Json(Health {
        healthy: true,
        workspace_id: state.config.workspace_id.clone(),
    })
    .into_response()
}

async fn checkpoint(State(state): State<SidecarState>, headers: HeaderMap) -> Response {
    if !authorized(&headers, &state.config.control_token) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let _guard = state.capture_lock.lock().await;
    let config = Arc::clone(&state.config);
    let artifact = match tokio::task::spawn_blocking(move || {
        capture(&config.workspace, &config.workspace_id, &config.output_dir)
    })
    .await
    {
        Ok(Ok(artifact)) => artifact,
        Ok(Err(error)) => return error_response(&error),
        Err(error) => return error_response(&error.into()),
    };
    if let Err(error) = upload_with_client(
        &state.client,
        &artifact,
        &state.config.gateway_url,
        &state.config.gateway_token,
    )
    .await
    {
        return error_response(&error);
    }
    info!(
        workspace_id = %artifact.metadata.workspace_id,
        checkpoint_oid = %artifact.metadata.checkpoint_oid,
        operation = "checkpoint_upload",
        "checkpoint uploaded"
    );
    (StatusCode::CREATED, Json(artifact.metadata)).into_response()
}

async fn upload_with_client(
    client: &reqwest::Client,
    artifact: &CheckpointArtifact,
    gateway_url: &str,
    gateway_token: &str,
) -> Result<()> {
    let metadata = serde_json::to_vec(&artifact.metadata)?;
    let bundle = tokio::fs::read(&artifact.bundle_path).await?;
    let metadata_header = HeaderValue::from_bytes(&metadata)
        .context("checkpoint metadata cannot be represented as an HTTP header")?;
    let response = client
        .post(format!(
            "{}/v1/workspaces/{}/checkpoints",
            gateway_url.trim_end_matches('/'),
            artifact.metadata.workspace_id
        ))
        .bearer_auth(gateway_token)
        .header("x-opencode-checkpoint-metadata", metadata_header)
        .header(CONTENT_TYPE, "application/octet-stream")
        .body(bundle)
        .send()
        .await?;
    ensure!(
        response.status().is_success(),
        "gateway rejected checkpoint upload with {}",
        response.status()
    );
    Ok(())
}

fn validate(config: &SidecarConfig) -> Result<()> {
    ensure!(
        config.workspace.is_dir(),
        "workspace directory does not exist"
    );
    ensure!(!config.workspace_id.is_empty(), "workspace ID is required");
    ensure!(
        !config.control_token.is_empty(),
        "sidecar control token is required"
    );
    ensure!(
        !config.gateway_token.is_empty(),
        "gateway token is required"
    );
    ensure!(
        config.listen.starts_with("127.0.0.1:"),
        "sidecar API must bind to IPv4 loopback"
    );
    reqwest::Url::parse(&config.gateway_url).context("gateway URL is invalid")?;
    Ok(())
}

fn error_response(error: &anyhow::Error) -> Response {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(serde_json::json!({ "error": format!("{error:#}") })),
    )
        .into_response()
}

async fn shutdown_signal() {
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .expect("install SIGTERM handler");
    tokio::select! {
        _ = terminate.recv() => {}
        _ = tokio::signal::ctrl_c() => {}
    }
}
