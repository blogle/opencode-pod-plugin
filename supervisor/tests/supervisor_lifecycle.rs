use nix::sys::signal::{kill, Signal};
use nix::unistd::Pid;
use serde_json::Value;
use std::fs;
use std::io::Read;
use std::net::TcpListener;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tempfile::TempDir;

struct ChildGuard(Child);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if self.0.try_wait().ok().flatten().is_none() {
            let _ = kill(Pid::from_raw(self.0.id().cast_signed()), Signal::SIGTERM);
            let _ = self.0.wait();
        }
    }
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn authenticates_restarts_and_terminates_gracefully() {
    let fixture = TempDir::new().unwrap();
    let (opencode, direnv) = write_runtime_scripts(fixture.path());
    let child_port = unused_port();
    let control_port = unused_port();
    let checkpoint_port = unused_port();
    let checkpoint_count = Arc::new(AtomicUsize::new(0));
    let checkpoint_state = Arc::clone(&checkpoint_count);
    let checkpoint_listener = tokio::net::TcpListener::bind(("127.0.0.1", checkpoint_port))
        .await
        .unwrap();
    let checkpoint_server = tokio::spawn(async move {
        let app = axum::Router::new().route(
            "/checkpoint",
            axum::routing::post(move |headers: axum::http::HeaderMap| {
                let checkpoint_state = Arc::clone(&checkpoint_state);
                async move {
                    assert_eq!(
                        headers.get(axum::http::header::AUTHORIZATION).unwrap(),
                        "Bearer checkpoint-test-token"
                    );
                    checkpoint_state.fetch_add(1, Ordering::SeqCst);
                    axum::http::StatusCode::CREATED
                }
            }),
        );
        axum::serve(checkpoint_listener, app).await.unwrap();
    });
    let child = Command::new(env!("CARGO_BIN_EXE_supervisor"))
        .env("WORKSPACE_PATH", fixture.path())
        .env("OPENCODE_BINARY", &opencode)
        .env("DIRENV_BINARY", &direnv)
        .env("SUPERVISOR_LISTEN", format!("127.0.0.1:{control_port}"))
        .env("SUPERVISOR_CONTROL_TOKEN", "test-control-token")
        .env("OPENCODE_PORT", child_port.to_string())
        .env("SUPERVISOR_HEALTH_SECONDS", "1")
        .env("SUPERVISOR_STARTUP_SECONDS", "5")
        .env("SUPERVISOR_GRACE_SECONDS", "1")
        .env(
            "CHECKPOINT_SIDECAR_URL",
            format!("http://127.0.0.1:{checkpoint_port}/checkpoint"),
        )
        .env("CHECKPOINT_CONTROL_TOKEN", "checkpoint-test-token")
        .env("SUPERVISOR_CHECKPOINT_TIMEOUT_SECONDS", "3")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut supervisor = ChildGuard(child);
    let client = reqwest::Client::new();
    let health_url = format!("http://127.0.0.1:{control_port}/healthz");
    let restart_url = format!("http://127.0.0.1:{control_port}/restart");

    let first = wait_ready(&client, &health_url, None).await;
    assert_eq!(first.0, reqwest::StatusCode::UNAUTHORIZED);
    let first = wait_ready(&client, &health_url, Some("test-control-token")).await;
    assert_eq!(first.0, reqwest::StatusCode::OK);
    let first_pid = first.1["childPid"].as_u64().unwrap();

    let response = client
        .post(&restart_url)
        .bearer_auth("test-control-token")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::ACCEPTED);
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let response = client
            .get(&health_url)
            .bearer_auth("test-control-token")
            .send()
            .await;
        if let Ok(response) = response {
            if response.status() == reqwest::StatusCode::OK {
                let body: Value = response.json().await.unwrap();
                if body["childPid"]
                    .as_u64()
                    .is_some_and(|pid| pid != first_pid)
                {
                    break;
                }
            }
        }
        assert!(Instant::now() < deadline, "child did not restart");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(
        checkpoint_count.load(Ordering::SeqCst),
        0,
        "child restart unexpectedly requested a checkpoint"
    );

    kill(
        Pid::from_raw(supervisor.0.id().cast_signed()),
        Signal::SIGTERM,
    )
    .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(status) = supervisor.0.try_wait().unwrap() {
            assert!(status.success(), "supervisor exited with {status}");
            break;
        }
        assert!(Instant::now() < deadline, "supervisor did not terminate");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(checkpoint_count.load(Ordering::SeqCst), 1);
    checkpoint_server.abort();
}

#[tokio::test]
async fn failed_final_checkpoint_makes_graceful_exit_nonzero() {
    let fixture = TempDir::new().unwrap();
    let (opencode, direnv) = write_runtime_scripts(fixture.path());
    let child_port = unused_port();
    let control_port = unused_port();
    let checkpoint_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let checkpoint_address = checkpoint_listener.local_addr().unwrap();
    let checkpoint_server = tokio::spawn(async move {
        let app = axum::Router::new().route(
            "/checkpoint",
            axum::routing::post(|| async { axum::http::StatusCode::INTERNAL_SERVER_ERROR }),
        );
        axum::serve(checkpoint_listener, app).await.unwrap();
    });
    let child = Command::new(env!("CARGO_BIN_EXE_supervisor"))
        .args([
            "run",
            "--workspace",
            fixture.path().to_str().unwrap(),
            "--opencode",
            opencode.to_str().unwrap(),
            "--direnv",
            direnv.to_str().unwrap(),
            "--listen",
            &format!("127.0.0.1:{control_port}"),
            "--control-token",
            "test-control-token",
            "--port",
            &child_port.to_string(),
            "--health-seconds",
            "1",
            "--startup-seconds",
            "5",
            "--graceful-seconds",
            "1",
            "--checkpoint-url",
            &format!("http://{checkpoint_address}/checkpoint"),
            "--checkpoint-token",
            "checkpoint-test-token",
            "--checkpoint-timeout-seconds",
            "3",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut supervisor = ChildGuard(child);
    let client = reqwest::Client::new();
    wait_ready(
        &client,
        &format!("http://127.0.0.1:{control_port}/healthz"),
        Some("test-control-token"),
    )
    .await;
    kill(
        Pid::from_raw(supervisor.0.id().cast_signed()),
        Signal::SIGTERM,
    )
    .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(status) = supervisor.0.try_wait().unwrap() {
            assert!(!status.success(), "checkpoint failure was not propagated");
            break;
        }
        assert!(Instant::now() < deadline, "supervisor did not terminate");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let mut stderr = String::new();
    supervisor
        .0
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut stderr)
        .unwrap();
    assert!(stderr.contains("final checkpoint failed"));
    assert!(!stderr.contains("checkpoint-test-token"));
    checkpoint_server.abort();
}

#[test]
fn binary_exposes_sidecar_subcommand_with_loopback_default() {
    let output = Command::new(env!("CARGO_BIN_EXE_supervisor"))
        .args(["sidecar", "--help"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("127.0.0.1:4098"));
}

async fn wait_ready(
    client: &reqwest::Client,
    url: &str,
    token: Option<&str>,
) -> (reqwest::StatusCode, Value) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let mut request = client.get(url);
        if let Some(token) = token {
            request = request.bearer_auth(token);
        }
        if let Ok(response) = request.send().await {
            let status = response.status();
            if status == reqwest::StatusCode::UNAUTHORIZED {
                return (status, Value::Null);
            }
            if status == reqwest::StatusCode::OK {
                return (status, response.json().await.unwrap());
            }
        }
        assert!(Instant::now() < deadline, "supervisor did not become ready");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

fn unused_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn write_executable(path: &Path, content: &str) {
    fs::write(path, content).unwrap();
    let mut permissions = fs::metadata(path).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).unwrap();
}

fn write_runtime_scripts(directory: &Path) -> (std::path::PathBuf, std::path::PathBuf) {
    let opencode = directory.join("opencode");
    let direnv = directory.join("direnv");
    write_executable(
        &opencode,
        r#"#!/bin/sh
if [ "$1" = "--version" ]; then
  printf '%s\n' '1.18.3'
  exit 0
fi
exec python3 -c 'import http.server,json,sys
port=int(sys.argv[sys.argv.index("--port")+1])
class H(http.server.BaseHTTPRequestHandler):
 def do_GET(self):
  body=json.dumps({"healthy":True,"version":"1.18.3"}).encode()
  self.send_response(200); self.send_header("Content-Type","application/json"); self.send_header("Content-Length",str(len(body))); self.end_headers(); self.wfile.write(body)
 def log_message(self,*args): pass
http.server.ThreadingHTTPServer(("127.0.0.1",port),H).serve_forever()' "$@"
"#,
    );
    write_executable(
        &direnv,
        r#"#!/bin/sh
if [ "$1" = "allow" ]; then exit 0; fi
if [ "$1" = "exec" ]; then shift 2; exec "$@"; fi
exit 2
"#,
    );
    (opencode, direnv)
}
