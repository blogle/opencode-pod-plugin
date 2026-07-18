use std::time::Duration;

use anyhow::{bail, Context, Result};
use reqwest::{Client, Url};
use serde::{Deserialize, Serialize};

#[derive(Clone)]
pub struct CentralClient {
    client: Client,
    base_url: Url,
}

#[derive(Clone, Debug)]
pub struct LaunchSpec<'a> {
    pub project_key: &'a str,
    pub git_ref: &'a str,
    pub owner: &'a str,
    pub session_name: &'a str,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LaunchResult {
    pub session_id: String,
    pub workspace_id: String,
    pub session_url: String,
}

#[derive(Deserialize)]
struct Identified {
    id: String,
}

#[derive(Serialize)]
struct SessionCreate<'a> {
    title: &'a str,
}

#[derive(Serialize)]
struct WorkspaceCreate<'a> {
    #[serde(rename = "type")]
    workspace_type: &'static str,
    branch: &'a str,
    extra: WorkspaceExtra<'a>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkspaceExtra<'a> {
    project_key: &'a str,
    owner: &'a str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkspaceWarp<'a> {
    id: &'a str,
    #[serde(rename = "sessionID")]
    session_id: &'a str,
    copy_changes: bool,
}

impl CentralClient {
    pub fn new(base_url: &str) -> Result<Self> {
        let base_url = Url::parse(base_url).context("opencode.centralUrl is invalid")?;
        if !matches!(base_url.scheme(), "http" | "https")
            || base_url.host_str().is_none()
            || !base_url.username().is_empty()
            || base_url.password().is_some()
            || base_url.query().is_some()
            || base_url.fragment().is_some()
        {
            bail!("opencode.centralUrl must be an HTTP URL without credentials or query data");
        }
        let client = Client::builder()
            .timeout(Duration::from_secs(600))
            .build()
            .context("build central OpenCode client")?;
        Ok(Self { client, base_url })
    }

    pub async fn launch(&self, spec: LaunchSpec<'_>) -> Result<LaunchResult> {
        let directory = format!("/catalog/{}", spec.project_key);
        let session: Identified = self
            .post_json(
                "session",
                &directory,
                &SessionCreate {
                    title: spec.session_name,
                },
                "create central session",
            )
            .await?;
        let workspace: Identified = self
            .post_json(
                "experimental/workspace",
                &directory,
                &WorkspaceCreate {
                    workspace_type: "kubernetes",
                    branch: spec.git_ref,
                    extra: WorkspaceExtra {
                        project_key: spec.project_key,
                        owner: spec.owner,
                    },
                },
                "create Kubernetes workspace",
            )
            .await?;
        self.post_no_content(
            "experimental/workspace/warp",
            &directory,
            &WorkspaceWarp {
                id: &workspace.id,
                session_id: &session.id,
                copy_changes: false,
            },
            "warp central session into workspace",
        )
        .await?;
        Ok(LaunchResult {
            session_url: self
                .base_url
                .join(&format!("session/{}", session.id))?
                .to_string(),
            session_id: session.id,
            workspace_id: workspace.id,
        })
    }

    pub fn session_url(&self, session_id: &str) -> Result<String> {
        Ok(self
            .base_url
            .join(&format!("session/{session_id}"))?
            .to_string())
    }

    async fn post_json<T: Serialize + ?Sized, R: for<'de> Deserialize<'de>>(
        &self,
        path: &str,
        directory: &str,
        body: &T,
        operation: &str,
    ) -> Result<R> {
        let response = self
            .client
            .post(self.base_url.join(path)?)
            .query(&[("directory", directory)])
            .json(body)
            .send()
            .await
            .with_context(|| format!("{operation}: central OpenCode is unavailable"))?;
        if !response.status().is_success() {
            bail!(
                "{operation}: central OpenCode returned HTTP {}",
                response.status()
            );
        }
        response
            .json()
            .await
            .with_context(|| format!("{operation}: invalid central OpenCode response"))
    }

    async fn post_no_content<T: Serialize + ?Sized>(
        &self,
        path: &str,
        directory: &str,
        body: &T,
        operation: &str,
    ) -> Result<()> {
        let response = self
            .client
            .post(self.base_url.join(path)?)
            .query(&[("directory", directory)])
            .json(body)
            .send()
            .await
            .with_context(|| format!("{operation}: central OpenCode is unavailable"))?;
        if !response.status().is_success() {
            bail!(
                "{operation}: central OpenCode returned HTTP {}",
                response.status()
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use axum::{
        extract::State,
        http::{StatusCode, Uri},
        routing::post,
        Json, Router,
    };
    use serde_json::{json, Value};

    use super::*;

    type Calls = Arc<Mutex<Vec<(String, Value)>>>;

    async fn record(State(calls): State<Calls>, uri: Uri, Json(body): Json<Value>) -> Json<Value> {
        calls.lock().unwrap().push((uri.to_string(), body));
        let id = if uri.path() == "/session" {
            "ses_1"
        } else {
            "wrk_1"
        };
        Json(json!({"id": id}))
    }

    async fn record_warp(
        State(calls): State<Calls>,
        uri: Uri,
        Json(body): Json<Value>,
    ) -> StatusCode {
        calls.lock().unwrap().push((uri.to_string(), body));
        StatusCode::NO_CONTENT
    }

    #[tokio::test]
    async fn performs_directory_scoped_create_workspace_and_warp() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let app = Router::new()
            .route("/session", post(record))
            .route("/experimental/workspace", post(record))
            .route("/experimental/workspace/warp", post(record_warp))
            .with_state(calls.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let client = CentralClient::new(&format!("http://{address}/")).unwrap();
        let result = client
            .launch(LaunchSpec {
                project_key: "demo",
                git_ref: "feature/test",
                owner: "dev@example.test",
                session_name: "Test session",
            })
            .await
            .unwrap();
        server.abort();

        assert_eq!(result.session_id, "ses_1");
        assert_eq!(result.workspace_id, "wrk_1");
        assert_eq!(calls.lock().unwrap().len(), 3);
        let calls = calls.lock().unwrap();
        assert!(calls[0].0.contains("directory=%2Fcatalog%2Fdemo"));
        assert_eq!(calls[1].1["type"], "kubernetes");
        assert_eq!(calls[1].1["extra"]["projectKey"], "demo");
        assert_eq!(calls[2].1["sessionID"], "ses_1");
    }
}
