use crate::PINNED_OPENCODE_VERSION;
use anyhow::{bail, ensure, Context, Result};
use axum::extract::State;
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use nix::sys::signal::{killpg, Signal};
use nix::unistd::Pid;
use reqwest::Client;
use serde::Serialize;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;
use subtle::ConstantTimeEq;
use tokio::process::{Child, Command};
use tokio::sync::{mpsc, watch, RwLock};
use tokio::time::{sleep, timeout, Instant};
use tracing::{error, info, warn};

#[derive(Debug, Clone)]
pub struct SupervisorConfig {
    pub workspace: PathBuf,
    pub opencode: PathBuf,
    pub direnv: PathBuf,
    pub expected_version: String,
    pub listen: String,
    pub control_token: String,
    pub host: String,
    pub port: u16,
    pub graceful_timeout: Duration,
    pub health_interval: Duration,
    pub startup_timeout: Duration,
    pub checkpoint_url: Option<String>,
    pub checkpoint_token: Option<String>,
    pub checkpoint_timeout: Duration,
}

impl Default for SupervisorConfig {
    fn default() -> Self {
        Self {
            workspace: PathBuf::from("/workspace"),
            opencode: PathBuf::from("/opt/opencode/bin/opencode"),
            direnv: PathBuf::from("/opt/opencode/bin/direnv"),
            expected_version: PINNED_OPENCODE_VERSION.to_owned(),
            listen: "127.0.0.1:4097".to_owned(),
            control_token: String::new(),
            host: "0.0.0.0".to_owned(),
            port: 4096,
            graceful_timeout: Duration::from_secs(20),
            health_interval: Duration::from_secs(2),
            startup_timeout: Duration::new(60, 0),
            checkpoint_url: None,
            checkpoint_token: None,
            checkpoint_timeout: Duration::new(120, 0),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct RuntimeStatus {
    child_running: bool,
    ready: bool,
    child_pid: Option<u32>,
    restart_count: u64,
    expected_version: String,
    observed_version: Option<String>,
    last_error: Option<String>,
}

#[derive(Clone)]
struct ControlState {
    token: Arc<String>,
    status: Arc<RwLock<RuntimeStatus>>,
    restart: mpsc::Sender<()>,
    checkpoint: Arc<SupervisorConfig>,
}

/// Runs child `OpenCode` and its authenticated lifecycle control endpoint until termination.
///
/// # Errors
///
/// Returns an error when configuration or version validation fails, the control listener cannot
/// start, child process management fails, or child health reports a version mismatch.
#[allow(clippy::too_many_lines)]
pub async fn run(config: SupervisorConfig) -> Result<()> {
    validate_config(&config)?;
    check_binary_version(&config).await?;
    authorize_managed_envrc(&config).await?;
    check_project_environment(&config).await?;

    let status = Arc::new(RwLock::new(RuntimeStatus {
        expected_version: config.expected_version.clone(),
        ..RuntimeStatus::default()
    }));
    let (restart_tx, mut restart_rx) = mpsc::channel(4);
    let (server_shutdown_tx, server_shutdown_rx) = watch::channel(false);
    let control_state = ControlState {
        token: Arc::new(config.control_token.clone()),
        status: Arc::clone(&status),
        restart: restart_tx,
        checkpoint: Arc::new(config.clone()),
    };
    let listener = tokio::net::TcpListener::bind(&config.listen)
        .await
        .with_context(|| format!("bind supervisor control API at {}", config.listen))?;
    let app = Router::new()
        .route("/healthz", get(control_health))
        .route("/restart", post(control_restart))
        .route("/checkpoint", post(control_checkpoint))
        .with_state(control_state);
    let control_server = tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(async move {
                let mut shutdown = server_shutdown_rx;
                while !*shutdown.borrow() && shutdown.changed().await.is_ok() {}
            })
            .await
    });

    let client = Client::builder()
        .timeout(Duration::from_secs(3))
        .build()
        .context("build health client")?;
    let mut terminating = false;
    let mut termination_checkpoint_error = None;
    let mut start_count = 0_u64;
    while !terminating {
        let mut child = spawn_child(&config)?;
        let pid = child.id().context("child process has no PID")?;
        let restart_count = start_count;
        start_count = start_count.saturating_add(1);
        {
            let mut current = status.write().await;
            current.child_running = true;
            current.ready = false;
            current.child_pid = Some(pid);
            current.restart_count = restart_count;
            current.observed_version = None;
            current.last_error = None;
        }
        info!(
            operation = "child_start",
            child_pid = pid,
            "started child OpenCode"
        );
        let started = Instant::now();
        let mut health_tick = tokio::time::interval(config.health_interval);
        health_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut failed_checks = 0_u8;
        let mut ever_ready = false;
        let restart_reason = loop {
            tokio::select! {
                result = child.wait() => {
                    let exit = result.context("wait for child OpenCode")?;
                    break format!("child exited with {exit}");
                }
                request = restart_rx.recv() => {
                    if request.is_some() {
                        break "authenticated restart request".to_owned();
                    }
                }
                () = shutdown_signal() => {
                    terminating = true;
                    break "termination signal".to_owned();
                }
                _ = health_tick.tick() => {
                    match check_server_health(&client, &config).await {
                        Ok(version) => {
                            failed_checks = 0;
                            ever_ready = true;
                            let mut current = status.write().await;
                            current.ready = true;
                            current.observed_version = Some(version);
                            current.last_error = None;
                        }
                        Err(error) => {
                            failed_checks = failed_checks.saturating_add(1);
                            let message = format!("{error:#}");
                            let version_mismatch = message.contains("version mismatch");
                            let mut current = status.write().await;
                            current.ready = false;
                            current.last_error = Some(message.clone());
                            drop(current);
                            if version_mismatch {
                                terminate_child(&mut child, pid, config.graceful_timeout).await?;
                                let _ = server_shutdown_tx.send(true);
                                let _ = control_server.await;
                                bail!("{message}");
                            }
                            if (!ever_ready && started.elapsed() >= config.startup_timeout)
                                || (ever_ready && failed_checks >= 5)
                            {
                                break format!("health check failed: {message}");
                            }
                        }
                    }
                }
            }
        };
        info!(operation = "child_stop", reason = %restart_reason, child_pid = pid, "stopping child OpenCode");
        if child.try_wait().context("inspect child status")?.is_none() {
            terminate_child(&mut child, pid, config.graceful_timeout).await?;
        }
        {
            let mut current = status.write().await;
            current.child_running = false;
            current.ready = false;
            current.child_pid = None;
            current.last_error = Some(restart_reason);
        }
        if terminating {
            if let Err(error) = request_final_checkpoint(&config).await {
                error!(
                    operation = "final_checkpoint",
                    error = %error,
                    "final checkpoint failed during graceful termination"
                );
                termination_checkpoint_error = Some(error);
            } else if config.checkpoint_url.is_some() {
                info!(
                    operation = "final_checkpoint",
                    "final checkpoint completed during graceful termination"
                );
            }
        }
        if !terminating {
            sleep(Duration::from_secs(1)).await;
            authorize_managed_envrc(&config).await?;
            check_project_environment(&config).await?;
        }
    }

    let _ = server_shutdown_tx.send(true);
    control_server
        .await
        .context("join supervisor control server")?
        .context("run supervisor control server")?;
    if let Some(error) = termination_checkpoint_error {
        return Err(error).context("graceful termination final checkpoint failed");
    }
    Ok(())
}

fn spawn_child(config: &SupervisorConfig) -> Result<Child> {
    let mut command = Command::new(&config.direnv);
    command
        .arg("exec")
        .arg(&config.workspace)
        .arg(&config.opencode)
        .args([
            "serve",
            "--hostname",
            &config.host,
            "--port",
            &config.port.to_string(),
        ])
        .current_dir(&config.workspace)
        .process_group(0)
        .kill_on_drop(true);
    command.spawn().context("launch OpenCode through direnv")
}

async fn terminate_child(child: &mut Child, pid: u32, grace: Duration) -> Result<()> {
    match killpg(Pid::from_raw(pid.cast_signed()), Signal::SIGTERM) {
        Ok(()) | Err(nix::errno::Errno::ESRCH) => {}
        Err(error) => return Err(error).context("send SIGTERM to child process group"),
    }
    if timeout(grace, child.wait()).await.is_err() {
        warn!(
            operation = "child_kill",
            child_pid = pid,
            "child exceeded graceful shutdown timeout"
        );
        match killpg(Pid::from_raw(pid.cast_signed()), Signal::SIGKILL) {
            Ok(()) | Err(nix::errno::Errno::ESRCH) => {}
            Err(error) => return Err(error).context("send SIGKILL to child process group"),
        }
        child.wait().await.context("reap killed child")?;
    }
    Ok(())
}

async fn check_binary_version(config: &SupervisorConfig) -> Result<()> {
    let output = Command::new(&config.opencode)
        .arg("--version")
        .output()
        .await
        .context("execute injected OpenCode version check")?;
    ensure!(
        output.status.success(),
        "injected OpenCode --version failed: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    );
    let observed = parse_version(&String::from_utf8_lossy(&output.stdout));
    ensure!(
        observed.as_deref() == Some(config.expected_version.as_str()),
        "injected OpenCode version mismatch: expected {}, observed {}",
        config.expected_version,
        observed.as_deref().unwrap_or("unknown")
    );
    Ok(())
}

async fn check_server_health(client: &Client, config: &SupervisorConfig) -> Result<String> {
    let mut request = client.get(format!("http://127.0.0.1:{}/global/health", config.port));
    if let (Ok(username), Ok(password)) = (
        std::env::var("OPENCODE_SERVER_USERNAME"),
        std::env::var("OPENCODE_SERVER_PASSWORD"),
    ) {
        request = request.basic_auth(username, Some(password));
    }
    let response = request.send().await.context("request child health")?;
    ensure!(
        response.status().is_success(),
        "child health returned {}",
        response.status()
    );
    let value: serde_json::Value = response
        .json()
        .await
        .context("parse child health response")?;
    ensure!(
        value.get("healthy").and_then(serde_json::Value::as_bool) != Some(false),
        "child reports unhealthy"
    );
    let version = value
        .get("version")
        .and_then(serde_json::Value::as_str)
        .and_then(parse_version)
        .context("child health response omitted version")?;
    ensure!(
        version == config.expected_version,
        "child OpenCode version mismatch: expected {}, observed {version}",
        config.expected_version
    );
    Ok(version)
}

async fn request_final_checkpoint(config: &SupervisorConfig) -> Result<()> {
    let Some(url) = &config.checkpoint_url else {
        return Ok(());
    };
    let token = config
        .checkpoint_token
        .as_deref()
        .context("checkpoint sidecar token is not configured")?;
    let client = Client::builder()
        .timeout(config.checkpoint_timeout)
        .build()
        .context("build final checkpoint client")?;
    let response = client
        .post(url)
        .bearer_auth(token)
        .send()
        .await
        .context("request final checkpoint from sidecar")?;
    ensure!(
        response.status().is_success(),
        "checkpoint sidecar returned {}",
        response.status()
    );
    Ok(())
}

async fn authorize_managed_envrc(config: &SupervisorConfig) -> Result<()> {
    let envrc = config.workspace.join(".envrc");
    if !envrc.is_symlink() {
        return Ok(());
    }
    let status = Command::new(&config.direnv)
        .args(["allow", "."])
        .current_dir(&config.workspace)
        .status()
        .await
        .context("authorize managed environment profile")?;
    ensure!(
        status.success(),
        "direnv failed to authorize managed environment profile"
    );
    Ok(())
}

async fn check_project_environment(config: &SupervisorConfig) -> Result<()> {
    let status = Command::new(&config.direnv)
        .arg("exec")
        .arg(&config.workspace)
        .arg(&config.opencode)
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
        .context("evaluate project environment")?;
    ensure!(
        status.success(),
        "project environment evaluation failed; check the managed .envrc or use a compatible project environment mode"
    );
    Ok(())
}

fn validate_config(config: &SupervisorConfig) -> Result<()> {
    ensure!(
        !config.control_token.is_empty(),
        "supervisor control token is required"
    );
    ensure!(
        config.workspace.is_dir(),
        "workspace directory does not exist"
    );
    ensure_executable(&config.opencode, "OpenCode")?;
    ensure_executable(&config.direnv, "direnv")?;
    ensure!(
        config.expected_version == PINNED_OPENCODE_VERSION,
        "expected version must equal crate pin {PINNED_OPENCODE_VERSION}"
    );
    ensure!(
        control_listen_allowed(&config.listen),
        "control API must bind to IPv4 loopback or the Pod network interface"
    );
    ensure!(
        config.checkpoint_url.is_some() == config.checkpoint_token.is_some(),
        "checkpoint sidecar URL and token must be configured together"
    );
    if let Some(url) = &config.checkpoint_url {
        let parsed = reqwest::Url::parse(url).context("checkpoint sidecar URL is invalid")?;
        ensure!(
            parsed.scheme() == "http"
                && parsed.host_str() == Some("127.0.0.1")
                && parsed.username().is_empty()
                && parsed.password().is_none(),
            "checkpoint sidecar URL must be an unauthenticated HTTP loopback URL"
        );
    }
    Ok(())
}

fn control_listen_allowed(listen: &str) -> bool {
    listen
        .parse::<std::net::SocketAddr>()
        .is_ok_and(|address| match address.ip() {
            std::net::IpAddr::V4(ip) => ip.is_loopback() || ip.is_unspecified(),
            std::net::IpAddr::V6(_) => false,
        })
}

fn ensure_executable(path: &Path, name: &str) -> Result<()> {
    let metadata = path
        .metadata()
        .with_context(|| format!("inspect injected {name} binary"))?;
    ensure!(
        metadata.is_file() && metadata.permissions().mode() & 0o111 != 0,
        "injected {name} is not executable"
    );
    Ok(())
}

fn parse_version(value: &str) -> Option<String> {
    value
        .split_whitespace()
        .map(|part| part.trim_start_matches('v'))
        .find(|part| {
            let mut pieces = part.split('.');
            pieces.clone().count() == 3
                && pieces.all(|piece| {
                    !piece.is_empty() && piece.bytes().all(|byte| byte.is_ascii_digit())
                })
        })
        .map(ToOwned::to_owned)
}

async fn control_health(State(state): State<ControlState>, headers: HeaderMap) -> Response {
    if !authorized(&headers, &state.token) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let current = state.status.read().await.clone();
    let status = if current.ready {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    (status, Json(current)).into_response()
}

async fn control_restart(State(state): State<ControlState>, headers: HeaderMap) -> Response {
    if !authorized(&headers, &state.token) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    match state.restart.try_send(()) {
        Ok(()) | Err(mpsc::error::TrySendError::Full(())) => StatusCode::ACCEPTED.into_response(),
        Err(mpsc::error::TrySendError::Closed(())) => {
            StatusCode::SERVICE_UNAVAILABLE.into_response()
        }
    }
}

async fn control_checkpoint(State(state): State<ControlState>, headers: HeaderMap) -> Response {
    if !authorized(&headers, &state.token) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    match request_final_checkpoint(&state.checkpoint).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => {
            error!(operation = "requested_checkpoint", %error, "requested checkpoint failed");
            StatusCode::BAD_GATEWAY.into_response()
        }
    }
}

pub(crate) fn authorized(headers: &HeaderMap, token: &str) -> bool {
    let Some(value) = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
    else {
        return false;
    };
    let Some(candidate) = value.strip_prefix("Bearer ") else {
        return false;
    };
    candidate.as_bytes().ct_eq(token.as_bytes()).into()
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        let mut terminate =
            match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
                Ok(signal) => signal,
                Err(error) => {
                    error!(%error, "failed to install SIGTERM handler");
                    std::future::pending::<()>().await;
                    return;
                }
            };
        tokio::select! {
            _ = terminate.recv() => {}
            result = tokio::signal::ctrl_c() => {
                if let Err(error) = result {
                    error!(%error, "failed to receive interrupt signal");
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        check_project_environment, control_listen_allowed, parse_version, SupervisorConfig,
    };
    use std::{os::unix::fs::PermissionsExt, path::PathBuf};

    #[tokio::test]
    async fn rejects_invalid_project_environment_without_leaking_output() {
        let directory = tempfile::tempdir().unwrap();
        let direnv = directory.path().join("direnv");
        std::fs::write(
            &direnv,
            "#!/bin/sh\necho private-profile-value >&2\nexit 42\n",
        )
        .unwrap();
        let mut permissions = std::fs::metadata(&direnv).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&direnv, permissions).unwrap();
        let config = SupervisorConfig {
            workspace: directory.path().to_owned(),
            opencode: PathBuf::from("/bin/true"),
            direnv,
            ..SupervisorConfig::default()
        };

        let error = check_project_environment(&config).await.unwrap_err();
        let message = format!("{error:#}");
        assert!(message.contains("project environment evaluation failed"));
        assert!(!message.contains("private-profile-value"));
    }

    #[test]
    fn parses_supported_version_outputs() {
        assert_eq!(parse_version("1.18.3\n").as_deref(), Some("1.18.3"));
        assert_eq!(parse_version("opencode v1.18.3").as_deref(), Some("1.18.3"));
        assert_eq!(parse_version("development").as_deref(), None);
    }

    #[test]
    fn restricts_control_listener_to_loopback_or_pod_interfaces() {
        assert!(control_listen_allowed("127.0.0.1:4097"));
        assert!(control_listen_allowed("0.0.0.0:4097"));
        assert!(!control_listen_allowed("10.0.0.8:4097"));
        assert!(!control_listen_allowed("[::]:4097"));
        assert!(!control_listen_allowed("invalid"));
    }
}
