use std::collections::BTreeMap;

use axum::{
    body::Bytes,
    extract::{DefaultBodyLimit, Path, State},
    http::{header, HeaderMap, StatusCode},
    response::{Html, IntoResponse, Redirect, Response},
    routing::{get, post, put},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tower_http::trace::TraceLayer;

use crate::{
    central::LaunchSpec,
    checkpoint::CheckpointMetadata,
    config::{Project, ProjectEnvironment},
    k8s::resource_key,
    state::{EnvProfileMeta, Workspace, WorkspaceState},
    AppState,
};

const MAX_PROFILE_BYTES: usize = 1024 * 1024;
const MAX_CHECKPOINT_BYTES: usize = 64 * 1024 * 1024;

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/", get(launch_page))
        .route("/healthz", get(health))
        .route("/readyz", get(ready))
        .route("/v1/projects", get(projects))
        .route("/v1/launch", post(launch))
        .route("/ui/workspaces/:id/resume", post(ui_resume))
        .route("/ui/workspaces/:id/suspend", post(ui_suspend))
        .route("/ui/workspaces/:id/delete", post(ui_delete))
        .route(
            "/v1/workspaces",
            get(list_workspaces).post(create_workspace),
        )
        .route(
            "/v1/workspaces/:id",
            get(get_workspace).delete(delete_workspace),
        )
        .route("/v1/workspaces/:id/ensure", post(ensure_workspace))
        .route("/v1/workspaces/:id/suspend", post(suspend_workspace))
        .route("/v1/workspaces/:id/activity", post(activity))
        .route(
            "/v1/workspaces/:id/checkpoints",
            post(upload_checkpoint).layer(DefaultBodyLimit::max(MAX_CHECKPOINT_BYTES)),
        )
        .route(
            "/v1/workspaces/:id/checkpoints/latest",
            get(latest_checkpoint),
        )
        .route(
            "/v1/workspaces/:id/checkpoints/latest/blob",
            get(latest_checkpoint_blob),
        )
        .route(
            "/v1/projects/:project/env-profile",
            put(put_env_profile)
                .delete(delete_env_profile)
                .layer(DefaultBodyLimit::max(MAX_PROFILE_BYTES)),
        )
        .route(
            "/v1/projects/:project/env-profile/meta",
            get(env_profile_meta),
        )
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

#[derive(Debug)]
struct ApiError(StatusCode, String);

impl ApiError {
    fn bad(error: impl std::fmt::Display) -> Self {
        Self(StatusCode::BAD_REQUEST, error.to_string())
    }
    fn internal(error: impl std::fmt::Display) -> Self {
        tracing::error!(%error, "gateway operation failed");
        Self(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal gateway error".into(),
        )
    }
    fn not_found() -> Self {
        Self(StatusCode::NOT_FOUND, "not found".into())
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(json!({"error":self.1}))).into_response()
    }
}

#[derive(Debug)]
struct UiError(StatusCode, String);

impl IntoResponse for UiError {
    fn into_response(self) -> Response {
        let content = format!(
            "<!doctype html><html><head><meta charset=utf-8><title>Workspace error</title></head><body><main><h1>Workspace action failed</h1><p>{}</p><p><a href=\"/\">Return to workspaces</a></p></main></body></html>",
            escape_html(&self.1)
        );
        (self.0, Html(content)).into_response()
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct LaunchRequest {
    project_key: String,
    #[serde(default)]
    git_ref: String,
    session_name: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CreateRequest {
    workspace_id: String,
    project_key: String,
    git_ref: Option<String>,
    owner: Option<String>,
    #[serde(default)]
    runtime_overrides: Option<Value>,
    upstream_environment: BTreeMap<String, Option<String>>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkspaceResponse {
    workspace_id: String,
    state: WorkspaceState,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    target: TargetResponse,
}

#[derive(Serialize)]
struct TargetResponse {
    url: String,
    username: &'static str,
    password: String,
}

fn workspace_response(state: &AppState, workspace: Workspace) -> WorkspaceResponse {
    WorkspaceResponse {
        workspace_id: workspace.id,
        state: workspace.state,
        error: workspace.error,
        target: TargetResponse {
            url: format!(
                "http://{}.{}.svc.cluster.local:{}",
                workspace.service_name,
                state.config.namespace,
                crate::k8s::OPENCODE_PORT
            ),
            username: "opencode",
            password: workspace.password,
        },
    }
}

async fn health() -> &'static str {
    "ok\n"
}

async fn ready(State(state): State<AppState>) -> Result<&'static str, ApiError> {
    state.store.health().map_err(ApiError::internal)?;
    state.k8s.health().await.map_err(ApiError::internal)?;
    Ok("ready\n")
}

async fn launch_page(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Html<String>, UiError> {
    let owner = state
        .auth
        .identity(&headers)
        .map_err(|error| UiError(StatusCode::UNAUTHORIZED, error.into()))?;
    let projects = state
        .config
        .projects()
        .map_err(|error| UiError(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?;
    let workspaces = state
        .store
        .workspaces(&owner)
        .map_err(|error| UiError(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?;
    let mut project_options = String::new();
    for project in projects.values() {
        project_options.push_str(&format!(
            "<option value=\"{}\" data-ref=\"{}\">{}</option>",
            escape_html(&project.key),
            escape_html(&project.default_ref),
            escape_html(&project.name)
        ));
    }
    let mut rows = String::new();
    for workspace in workspaces {
        let session = state
            .store
            .session_id(&workspace.id)
            .map_err(|error| UiError(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?;
        let open = session
            .as_deref()
            .and_then(|id| state.central.session_url(id).ok())
            .map(|url| {
                format!(
                    "<a class=\"open\" href=\"{}\">Open session</a>",
                    escape_html(&url)
                )
            })
            .unwrap_or_default();
        let action = match workspace.state {
            WorkspaceState::Suspended | WorkspaceState::Error => format!("<form method=post action=\"/ui/workspaces/{}/resume\"><button>Resume</button></form>", escape_html(&workspace.id)),
            WorkspaceState::Running => format!("<form method=post action=\"/ui/workspaces/{}/suspend\"><button>Suspend</button></form>", escape_html(&workspace.id)),
            _ => String::new(),
        };
        rows.push_str(&format!(
            "<tr><td><strong>{}</strong><small>{}</small></td><td><code>{}</code></td><td><span class=\"state\">{}</span></td><td>{}</td><td class=\"actions\">{}{}<form method=post action=\"/ui/workspaces/{}/delete\"><button class=\"danger\">Delete</button></form></td></tr>",
            escape_html(&workspace.project_key), escape_html(&workspace.id), escape_html(&workspace.git_ref), workspace.state, escape_html(&workspace.last_activity), open, action, escape_html(&workspace.id)
        ));
    }
    if rows.is_empty() {
        rows.push_str(
            "<tr><td colspan=5 class=empty>No workspaces yet. Launch one above.</td></tr>",
        );
    }
    Ok(Html(format!(
        r#"<!doctype html><html><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>OpenCode Workspaces</title><style>
body{{margin:0;background:#f0eee8;color:#17211d;font:15px system-ui,sans-serif}}main{{max-width:1100px;margin:0 auto;padding:48px 24px}}header{{display:flex;justify-content:space-between;align-items:end;border-bottom:3px solid #17211d}}h1{{font:700 42px Georgia,serif;margin:0 0 12px}}header p{{color:#52605a}}.launch{{display:grid;grid-template-columns:1fr 1fr 1.5fr auto;gap:12px;margin:28px 0;padding:20px;background:#dce6d8;border:1px solid #9baa96}}label{{font-size:12px;font-weight:700;text-transform:uppercase}}input,select,button{{box-sizing:border-box;width:100%;margin-top:6px;padding:10px;border:1px solid #637168;background:#fff;color:inherit}}button{{cursor:pointer;background:#173f35;color:#fff;font-weight:700}}table{{width:100%;border-collapse:collapse;background:#fff}}th,td{{padding:14px;text-align:left;border-bottom:1px solid #d7d7d2}}th{{background:#17211d;color:#fff}}small{{display:block;color:#68736e;margin-top:4px}}.state{{text-transform:capitalize}}.actions{{display:flex;gap:8px;align-items:center}}.actions form{{margin:0}}.actions button,.open{{width:auto;margin:0;padding:7px 10px;display:inline-block}}.danger{{background:#872f2f}}.empty{{text-align:center;color:#68736e;padding:40px}}@media(max-width:760px){{.launch{{grid-template-columns:1fr}}table,thead,tbody,tr,th,td{{display:block}}thead{{display:none}}tr{{padding:10px}}td{{border:0;padding:6px}}.actions{{flex-wrap:wrap}}}}
</style></head><body><main><header><div><h1>Workspaces</h1><p>Disposable sandboxes, durable sessions.</p></div><p>{}</p></header><form class="launch" method="post" action="/v1/launch"><label>Project<select name="projectKey" required>{}</select></label><label>Git ref<input name="gitRef" placeholder="default branch"></label><label>Session name<input name="sessionName" required maxlength="200" placeholder="What are you working on?"></label><div><label>&nbsp;<button>Launch workspace</button></label></div></form><table><thead><tr><th>Project</th><th>Git ref</th><th>State</th><th>Last activity</th><th>Actions</th></tr></thead><tbody>{}</tbody></table></main></body></html>"#,
        escape_html(&owner),
        project_options,
        rows
    )))
}

async fn launch(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Redirect, UiError> {
    let owner = state
        .auth
        .identity(&headers)
        .map_err(|error| UiError(StatusCode::UNAUTHORIZED, error.into()))?;
    let content_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    let request: LaunchRequest = if content_type.starts_with("application/json") {
        serde_json::from_slice(&body).map_err(|error| {
            UiError(
                StatusCode::BAD_REQUEST,
                format!("invalid launch JSON: {error}"),
            )
        })?
    } else {
        serde_urlencoded::from_bytes(&body).map_err(|error| {
            UiError(
                StatusCode::BAD_REQUEST,
                format!("invalid launch form: {error}"),
            )
        })?
    };
    if request.session_name.trim().is_empty() || request.session_name.len() > 200 {
        return Err(UiError(
            StatusCode::BAD_REQUEST,
            "sessionName must be 1 to 200 characters".into(),
        ));
    }
    let projects = state
        .config
        .projects()
        .map_err(|error| UiError(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?;
    let project = projects.get(&request.project_key).ok_or_else(|| {
        UiError(
            StatusCode::BAD_REQUEST,
            "selected project is not registered".into(),
        )
    })?;
    let git_ref = if request.git_ref.trim().is_empty() {
        &project.default_ref
    } else {
        request.git_ref.trim()
    };
    let result = state
        .central
        .launch(LaunchSpec {
            project_key: &request.project_key,
            git_ref,
            owner: &owner,
            session_name: request.session_name.trim(),
        })
        .await
        .map_err(|error| UiError(StatusCode::BAD_GATEWAY, error.to_string()))?;
    state
        .store
        .bind_session(&result.workspace_id, &result.session_id)
        .map_err(|error| UiError(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?;
    Ok(Redirect::to(&result.session_url))
}

async fn ui_resume(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Redirect, UiError> {
    owned_workspace(&state, &headers, &id, false).map_err(ui_from_api)?;
    crate::lifecycle::ensure_workspace(&state, &id, true)
        .await
        .map_err(|error| UiError(StatusCode::SERVICE_UNAVAILABLE, error.to_string()))?;
    let session = state
        .store
        .session_id(&id)
        .map_err(|error| UiError(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?
        .ok_or_else(|| {
            UiError(
                StatusCode::CONFLICT,
                "workspace has no associated central session".into(),
            )
        })?;
    let url = state
        .central
        .session_url(&session)
        .map_err(|error| UiError(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?;
    Ok(Redirect::to(&url))
}

async fn ui_suspend(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Redirect, UiError> {
    owned_workspace(&state, &headers, &id, false).map_err(ui_from_api)?;
    crate::lifecycle::suspend(&state, &id)
        .await
        .map_err(|error| UiError(StatusCode::BAD_GATEWAY, error.to_string()))?;
    Ok(Redirect::to("/"))
}

async fn ui_delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Redirect, UiError> {
    let workspace = state
        .store
        .workspace(&id)
        .map_err(|error| UiError(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?
        .ok_or_else(|| UiError(StatusCode::NOT_FOUND, "workspace not found".into()))?;
    authorize(&state, &headers, &workspace, false).map_err(ui_from_api)?;
    delete_workspace_record(&state, &id)
        .await
        .map_err(ui_from_api)?;
    Ok(Redirect::to("/"))
}

fn ui_from_api(error: ApiError) -> UiError {
    UiError(error.0, error.1)
}

fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

async fn projects(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Vec<Project>>, ApiError> {
    state
        .auth
        .identity(&headers)
        .map_err(|error| ApiError(StatusCode::UNAUTHORIZED, error.into()))?;
    Ok(Json(
        state
            .config
            .projects()
            .map_err(ApiError::internal)?
            .into_values()
            .collect(),
    ))
}

async fn list_workspaces(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Vec<WorkspaceResponse>>, ApiError> {
    let owner = state
        .auth
        .identity(&headers)
        .map_err(|error| ApiError(StatusCode::UNAUTHORIZED, error.into()))?;
    let values = state
        .store
        .workspaces(&owner)
        .map_err(ApiError::internal)?
        .into_iter()
        .map(|workspace| workspace_response(&state, workspace))
        .collect();
    Ok(Json(values))
}

async fn create_workspace(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<CreateRequest>,
) -> Result<(StatusCode, Json<WorkspaceResponse>), ApiError> {
    let authenticated = if state.auth.is_internal(&headers) {
        let owner = request
            .owner
            .as_deref()
            .ok_or_else(|| ApiError::bad("owner is required for internal workspace creation"))?;
        crate::auth::validate_identity(owner).map_err(ApiError::bad)?
    } else {
        let identity = state
            .auth
            .identity(&headers)
            .map_err(|error| ApiError(StatusCode::UNAUTHORIZED, error.into()))?;
        if request
            .owner
            .as_deref()
            .is_some_and(|owner| owner != identity)
        {
            return Err(ApiError(
                StatusCode::FORBIDDEN,
                "workspace owner does not match authenticated identity".into(),
            ));
        }
        identity
    };
    validate_id(&request.workspace_id)?;
    if request
        .runtime_overrides
        .as_ref()
        .is_some_and(|value| value.as_object().is_none_or(|object| !object.is_empty()))
    {
        return Err(ApiError::bad(
            "runtimeOverrides must be an empty object; project security overrides are not enabled",
        ));
    }
    let projects = state.config.projects().map_err(ApiError::internal)?;
    let project = projects
        .get(&request.project_key)
        .cloned()
        .ok_or_else(ApiError::not_found)?;
    let git_ref = request
        .git_ref
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| project.default_ref.clone());
    let image_ref = match &project.environment {
        ProjectEnvironment::Image { image } => image.clone(),
        ProjectEnvironment::Nix { .. } => state.config.runtime.generic_nix_image.clone(),
    };
    let key = resource_key(&request.workspace_id);
    let workspace = Workspace {
        id: request.workspace_id,
        project_key: request.project_key,
        git_ref,
        owner: authenticated,
        state: WorkspaceState::Provisioning,
        service_name: format!("workspace-{key}"),
        preview_key: key,
        password: format!(
            "{}{}",
            uuid::Uuid::new_v4().simple(),
            uuid::Uuid::new_v4().simple()
        ),
        runtime_token: format!(
            "{}{}",
            uuid::Uuid::new_v4().simple(),
            uuid::Uuid::new_v4().simple()
        ),
        upstream_environment: serde_json::to_string(&request.upstream_environment)
            .map_err(ApiError::bad)?,
        image_ref,
        image_digest: None,
        last_activity: String::new(),
        error: None,
    };
    if workspace.upstream_environment.len() > MAX_PROFILE_BYTES {
        return Err(ApiError::bad("upstreamEnvironment exceeds limit"));
    }

    let _operation = state.operations.lock().await;
    let (workspace, created) = state
        .store
        .insert_workspace(&workspace)
        .map_err(ApiError::bad)?;
    if !created && workspace.state != WorkspaceState::Provisioning {
        return Ok((StatusCode::OK, Json(workspace_response(&state, workspace))));
    }
    let has_profile = state
        .store
        .env_profile(&workspace.owner, &workspace.project_key)
        .map_err(ApiError::internal)?
        .is_some();
    match state.k8s.provision(&workspace, &project, has_profile).await {
        Ok(result) => {
            state
                .store
                .record_image_digest(&workspace.id, &result.image_digest)
                .map_err(ApiError::internal)?;
            let workspace = state
                .store
                .transition(&workspace.id, WorkspaceState::Running, None)
                .map_err(ApiError::internal)?;
            Ok((
                if created {
                    StatusCode::CREATED
                } else {
                    StatusCode::OK
                },
                Json(workspace_response(&state, workspace)),
            ))
        }
        Err(error) => {
            tracing::error!(workspace_id=%workspace.id, project_key=%workspace.project_key, owner=%workspace.owner, operation="provision", %error, "workspace provisioning failed");
            let _ = state.store.transition(
                &workspace.id,
                WorkspaceState::Error,
                Some(&error.to_string()),
            );
            Err(ApiError(
                StatusCode::SERVICE_UNAVAILABLE,
                "workspace provisioning failed".into(),
            ))
        }
    }
}

async fn get_workspace(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<WorkspaceResponse>, ApiError> {
    let workspace = owned_workspace(&state, &headers, &id, true)?;
    Ok(Json(workspace_response(&state, workspace)))
}

async fn ensure_workspace(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<WorkspaceResponse>, ApiError> {
    let current = owned_workspace(&state, &headers, &id, false)?;
    if !matches!(
        current.state,
        WorkspaceState::Suspended
            | WorkspaceState::Error
            | WorkspaceState::Running
            | WorkspaceState::Provisioning
            | WorkspaceState::Resuming
    ) {
        return Err(ApiError(
            StatusCode::CONFLICT,
            format!("cannot ensure workspace in {} state", current.state),
        ));
    }
    let workspace = crate::lifecycle::ensure_workspace(&state, &id, true)
        .await
        .map_err(|_| {
            ApiError(
                StatusCode::SERVICE_UNAVAILABLE,
                "workspace resume failed".into(),
            )
        })?;
    Ok(Json(workspace_response(&state, workspace)))
}

async fn suspend_workspace(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<WorkspaceResponse>, ApiError> {
    owned_workspace(&state, &headers, &id, false)?;
    let workspace = crate::lifecycle::suspend(&state, &id)
        .await
        .map_err(ApiError::bad)?;
    Ok(Json(workspace_response(&state, workspace)))
}

async fn delete_workspace(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let Some(workspace) = state.store.workspace(&id).map_err(ApiError::internal)? else {
        return Ok(StatusCode::NO_CONTENT);
    };
    if workspace.state == WorkspaceState::Deleted {
        return Ok(StatusCode::NO_CONTENT);
    }
    authorize(&state, &headers, &workspace, true)?;
    delete_workspace_record(&state, &id).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn delete_workspace_record(state: &AppState, id: &str) -> Result<(), ApiError> {
    let Some(existing) = state.store.workspace(id).map_err(ApiError::internal)? else {
        return Ok(());
    };
    if existing.state == WorkspaceState::Deleted {
        return Ok(());
    }
    let _operation = state.operations.lock().await;
    let workspace = state
        .store
        .transition(id, WorkspaceState::Deleting, None)
        .map_err(ApiError::bad)?;
    if let Err(error) = state.k8s.delete(&workspace).await {
        let _ = state
            .store
            .transition(id, WorkspaceState::Error, Some(&error.to_string()));
        return Err(ApiError::internal(error));
    }
    state
        .checkpoints
        .purge(id)
        .await
        .map_err(ApiError::internal)?;
    state
        .store
        .transition(id, WorkspaceState::Deleted, None)
        .map_err(ApiError::internal)?;
    Ok(())
}

async fn activity(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let workspace = state
        .store
        .workspace(&id)
        .map_err(ApiError::internal)?
        .ok_or_else(ApiError::not_found)?;
    authorize(&state, &headers, &workspace, false)?;
    state
        .store
        .record_activity(&id, &workspace.owner)
        .map_err(ApiError::internal)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn upload_checkpoint(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: Bytes,
) -> Result<(StatusCode, Json<CheckpointMetadata>), ApiError> {
    let workspace = state
        .store
        .workspace(&id)
        .map_err(ApiError::internal)?
        .filter(|workspace| workspace.state != WorkspaceState::Deleted)
        .ok_or_else(ApiError::not_found)?;
    authorize_runtime(&headers, &workspace)?;
    let encoded = headers
        .get("x-opencode-checkpoint-metadata")
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| ApiError::bad("x-opencode-checkpoint-metadata header is required"))?;
    let metadata: CheckpointMetadata = serde_json::from_str(encoded).map_err(ApiError::bad)?;
    let was_running = workspace.state == WorkspaceState::Running;
    if was_running {
        state
            .store
            .transition(&id, WorkspaceState::Checkpointing, None)
            .map_err(ApiError::bad)?;
    }
    match state.checkpoints.put(&id, metadata, &body).await {
        Ok(metadata) => {
            if was_running {
                state
                    .store
                    .transition(&id, WorkspaceState::Running, None)
                    .map_err(ApiError::internal)?;
            }
            Ok((StatusCode::CREATED, Json(metadata)))
        }
        Err(error) => {
            if was_running {
                let _ =
                    state
                        .store
                        .transition(&id, WorkspaceState::Error, Some(&error.to_string()));
            }
            Err(ApiError::bad(error))
        }
    }
}

async fn latest_checkpoint(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<CheckpointMetadata>, ApiError> {
    let workspace = state
        .store
        .workspace(&id)
        .map_err(ApiError::internal)?
        .ok_or_else(ApiError::not_found)?;
    authorize(&state, &headers, &workspace, false)?;
    Ok(Json(
        state
            .checkpoints
            .latest(&id)
            .map_err(ApiError::internal)?
            .ok_or_else(ApiError::not_found)?,
    ))
}

async fn latest_checkpoint_blob(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    let workspace = state
        .store
        .workspace(&id)
        .map_err(ApiError::internal)?
        .ok_or_else(ApiError::not_found)?;
    authorize(&state, &headers, &workspace, false)?;
    let bytes = state
        .checkpoints
        .latest_blob(&id)
        .await
        .map_err(ApiError::internal)?
        .ok_or_else(ApiError::not_found)?;
    Ok(([(header::CONTENT_TYPE, "application/octet-stream")], bytes).into_response())
}

async fn put_env_profile(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(project): Path<String>,
    body: Bytes,
) -> Result<Json<EnvProfileMeta>, ApiError> {
    let owner = state
        .auth
        .identity(&headers)
        .map_err(|error| ApiError(StatusCode::UNAUTHORIZED, error.into()))?;
    require_project(&state, &project)?;
    if body.is_empty() || body.len() > MAX_PROFILE_BYTES || body.contains(&0) {
        return Err(ApiError::bad(
            "environment profile must be non-empty text up to 1 MiB without NUL bytes",
        ));
    }
    std::str::from_utf8(&body).map_err(ApiError::bad)?;
    let sha256 = format!("{:x}", Sha256::digest(&body));
    state
        .k8s
        .put_env_profile(&owner, &project, &body)
        .await
        .map_err(ApiError::internal)?;
    let metadata = state
        .store
        .put_env_profile(&EnvProfileMeta {
            project_key: project,
            owner,
            sha256,
            updated_at: String::new(),
        })
        .map_err(ApiError::internal)?;
    Ok(Json(metadata))
}

async fn env_profile_meta(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(project): Path<String>,
) -> Result<Json<EnvProfileMeta>, ApiError> {
    let owner = state
        .auth
        .identity(&headers)
        .map_err(|error| ApiError(StatusCode::UNAUTHORIZED, error.into()))?;
    require_project(&state, &project)?;
    Ok(Json(
        state
            .store
            .env_profile(&owner, &project)
            .map_err(ApiError::internal)?
            .ok_or_else(ApiError::not_found)?,
    ))
}

async fn delete_env_profile(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(project): Path<String>,
) -> Result<StatusCode, ApiError> {
    let owner = state
        .auth
        .identity(&headers)
        .map_err(|error| ApiError(StatusCode::UNAUTHORIZED, error.into()))?;
    require_project(&state, &project)?;
    state
        .k8s
        .delete_env_profile(&owner, &project)
        .await
        .map_err(ApiError::internal)?;
    state
        .store
        .delete_env_profile(&owner, &project)
        .map_err(ApiError::internal)?;
    Ok(StatusCode::NO_CONTENT)
}

fn require_project(state: &AppState, key: &str) -> Result<(), ApiError> {
    if state
        .config
        .projects()
        .map_err(ApiError::internal)?
        .contains_key(key)
    {
        Ok(())
    } else {
        Err(ApiError::not_found())
    }
}

fn owned_workspace(
    state: &AppState,
    headers: &HeaderMap,
    id: &str,
    allow_internal: bool,
) -> Result<Workspace, ApiError> {
    let workspace = state
        .store
        .workspace(id)
        .map_err(ApiError::internal)?
        .filter(|workspace| workspace.state != WorkspaceState::Deleted)
        .ok_or_else(ApiError::not_found)?;
    authorize(state, headers, &workspace, allow_internal)?;
    Ok(workspace)
}

fn authorize(
    state: &AppState,
    headers: &HeaderMap,
    workspace: &Workspace,
    allow_internal: bool,
) -> Result<(), ApiError> {
    if headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.strip_prefix("Bearer ") == Some(&workspace.runtime_token))
    {
        return Ok(());
    }
    if allow_internal && state.auth.is_internal(headers) {
        return Ok(());
    }
    let owner = state
        .auth
        .identity(headers)
        .map_err(|error| ApiError(StatusCode::UNAUTHORIZED, error.into()))?;
    if owner != workspace.owner {
        return Err(ApiError(
            StatusCode::FORBIDDEN,
            "workspace is owned by another identity".into(),
        ));
    }
    Ok(())
}

fn authorize_runtime(headers: &HeaderMap, workspace: &Workspace) -> Result<(), ApiError> {
    if headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.strip_prefix("Bearer ") == Some(&workspace.runtime_token))
    {
        return Ok(());
    }
    Err(ApiError(
        StatusCode::UNAUTHORIZED,
        "workspace runtime token is required".into(),
    ))
}

fn validate_id(id: &str) -> Result<(), ApiError> {
    if id.is_empty()
        || id.len() > 128
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        Err(ApiError::bad(
            "workspaceId must contain only ASCII letters, digits, '_' or '-'",
        ))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        checkpoint::CheckpointStorage,
        config::{
            AuthConfig, CheckpointConfig, Config, LifecycleConfig, OpenCodeConfig, RuntimeConfig,
        },
        k8s::{Orchestrator, Provisioned},
        state::Store,
    };
    use async_trait::async_trait;
    use axum::{
        body::{to_bytes, Body},
        http::Request,
    };
    use std::{fs, sync::Arc};
    use tower::ServiceExt;

    struct FakeK8s;
    #[async_trait]
    impl Orchestrator for FakeK8s {
        async fn provision(
            &self,
            _: &Workspace,
            _: &Project,
            _: bool,
        ) -> anyhow::Result<Provisioned> {
            Ok(Provisioned {
                image_digest: "docker-pullable://demo@sha256:abc".into(),
            })
        }
        async fn suspend(&self, _: &Workspace) -> anyhow::Result<()> {
            Ok(())
        }
        async fn delete(&self, _: &Workspace) -> anyhow::Result<()> {
            Ok(())
        }
        async fn sandbox_exists(&self, _: &Workspace) -> anyhow::Result<bool> {
            Ok(true)
        }
        async fn put_env_profile(&self, _: &str, _: &str, _: &[u8]) -> anyhow::Result<()> {
            Ok(())
        }
        async fn delete_env_profile(&self, _: &str, _: &str) -> anyhow::Result<()> {
            Ok(())
        }
        async fn ready_pod_ip(&self, _: &str) -> Option<String> {
            Some("127.0.0.1".into())
        }
        async fn health(&self) -> anyhow::Result<()> {
            Ok(())
        }
    }

    fn test_state() -> (AppState, tempfile::TempDir) {
        test_state_with_auth(AuthConfig::Development {
            user: "dev@example.test".into(),
            internal_token_file: None,
        })
    }

    fn test_state_with_auth(auth_config: AuthConfig) -> (AppState, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let projects = dir.path().join("projects.yaml");
        fs::write(&projects,"projects:\n  demo:\n    name: Demo\n    repository: https://git.test/demo\n    defaultRef: main\n    environment: { mode: image, image: 'demo:dev' }\n    resources:\n      requests: { cpu: 100m, memory: 128Mi }\n      limits: { cpu: '1', memory: 1Gi }\n").unwrap();
        let config = Arc::new(Config {
            namespace: "sandboxes".into(),
            base_domain: "test.invalid".into(),
            listen: "x".into(),
            preview_listen: "y".into(),
            state_path: "x".into(),
            opencode: OpenCodeConfig {
                version: "1.18.3".into(),
                central_url: "x".into(),
            },
            runtime: RuntimeConfig {
                image: "runtime:v1".into(),
                generic_nix_image: "nix:v1".into(),
                gateway_url: "http://gateway:8080".into(),
            },
            checkpoint: CheckpointConfig {
                path: dir.path().to_string_lossy().into(),
                periodic_seconds: 120,
            },
            lifecycle: LifecycleConfig {
                suspend_after_idle_seconds: 60,
                ready_timeout_seconds: 30,
                poll_seconds: 30,
                termination_grace_seconds: 180,
            },
            auth: auth_config,
            projects_file: projects.to_string_lossy().into(),
        });
        let store = Store::open_memory().unwrap();
        let checkpoints = CheckpointStorage::new(dir.path().join("blobs"), store.clone()).unwrap();
        let auth = crate::auth::Authenticator::new(&config.auth).unwrap();
        (
            AppState {
                config,
                store,
                k8s: Arc::new(FakeK8s),
                operations: Arc::new(tokio::sync::Mutex::new(())),
                checkpoints,
                central: crate::central::CentralClient::new("http://central.test:4096").unwrap(),
                auth,
            },
            dir,
        )
    }

    #[tokio::test]
    async fn create_payload_matches_plugin_and_is_idempotent() {
        let (state, _dir) = test_state();
        let app = router(state);
        let payload = r#"{"workspaceId":"wrk_123","projectKey":"demo","gitRef":"main","owner":"dev@example.test","upstreamEnvironment":{"OPENCODE_AUTH_CONTENT":"{}"}}"#;
        let first = app
            .clone()
            .oneshot(
                Request::post("/v1/workspaces")
                    .header("content-type", "application/json")
                    .body(Body::from(payload))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(first.status(), StatusCode::CREATED);
        let second = app
            .oneshot(
                Request::post("/v1/workspaces")
                    .header("content-type", "application/json")
                    .body(Body::from(payload))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(second.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn internal_adapter_token_can_create_get_and_delete_without_user_impersonation() {
        let token_dir = tempfile::tempdir().unwrap();
        let token_file = token_dir.path().join("internal-token");
        fs::write(&token_file, "adapter-secret\n").unwrap();
        let (state, _dir) = test_state_with_auth(AuthConfig::TrustedHeader {
            identity_header: "x-user".into(),
            internal_token_file: Some(token_file.to_string_lossy().into()),
        });
        let app = router(state);
        let missing_owner = app
            .clone()
            .oneshot(
                Request::post("/v1/workspaces")
                    .header("content-type", "application/json")
                    .header("authorization", "Bearer adapter-secret")
                    .body(Body::from(r#"{"workspaceId":"wrk_missing","projectKey":"demo","gitRef":"main","upstreamEnvironment":{"OPENCODE_AUTH_CONTENT":"{}"}}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(missing_owner.status(), StatusCode::BAD_REQUEST);

        let payload = r#"{"workspaceId":"wrk_service","projectKey":"demo","gitRef":"main","owner":"owner@example.test","upstreamEnvironment":{"OPENCODE_AUTH_CONTENT":"{}"}}"#;
        let created = app
            .clone()
            .oneshot(
                Request::post("/v1/workspaces")
                    .header("content-type", "application/json")
                    .header("authorization", "Bearer adapter-secret")
                    .body(Body::from(payload))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(created.status(), StatusCode::CREATED);

        let fetched = app
            .clone()
            .oneshot(
                Request::get("/v1/workspaces/wrk_service")
                    .header("authorization", "Bearer adapter-secret")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(fetched.status(), StatusCode::OK);

        let impersonation = app
            .clone()
            .oneshot(
                Request::post("/v1/workspaces")
                    .header("content-type", "application/json")
                    .header("x-user", "attacker@example.test")
                    .body(Body::from(payload.replace("wrk_service", "wrk_other")))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(impersonation.status(), StatusCode::FORBIDDEN);

        let deleted = app
            .oneshot(
                Request::delete("/v1/workspaces/wrk_service")
                    .header("authorization", "Bearer adapter-secret")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(deleted.status(), StatusCode::NO_CONTENT);
    }

    #[tokio::test]
    async fn launch_page_lists_projects_and_actions() {
        let (state, _dir) = test_state();
        let response = router(state)
            .oneshot(Request::get("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let html = String::from_utf8(
            to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap()
                .to_vec(),
        )
        .unwrap();
        assert!(html.contains("Demo"));
        assert!(html.contains("action=\"/v1/launch\""));
        assert!(html.contains("name=\"sessionName\""));
    }

    #[tokio::test]
    async fn raw_checkpoint_api_round_trips_supervisor_metadata() {
        let (state, _dir) = test_state();
        let app = router(state.clone());
        let create = r#"{"workspaceId":"wrk_123","projectKey":"demo","gitRef":"main","owner":"dev@example.test","upstreamEnvironment":{"OPENCODE_AUTH_CONTENT":"{}"}}"#;
        let response = app
            .clone()
            .oneshot(
                Request::post("/v1/workspaces")
                    .header("content-type", "application/json")
                    .body(Body::from(create))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);
        let runtime_token = state
            .store
            .workspace("wrk_123")
            .unwrap()
            .unwrap()
            .runtime_token;

        let unauthorized = app
            .clone()
            .oneshot(
                Request::post("/v1/workspaces/wrk_123/checkpoints")
                    .body(Body::from("bad"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);

        let bundle = b"supervisor git bundle";
        let metadata = CheckpointMetadata {
            workspace_id: "wrk_123".into(),
            created_at: "2026-07-17T12:00:00.000Z".into(),
            head: "a".repeat(40),
            branch: None,
            status_sha256: "b".repeat(64),
            state_sha256: "c".repeat(64),
            bundle_sha256: format!("{:x}", Sha256::digest(bundle)),
            checkpoint_oid: "d".repeat(40),
            bundle_ref: "refs/opencode/checkpoints/wrk_123-1".into(),
            head_ref: "refs/opencode/heads/wrk_123-1".into(),
            has_changes: true,
            format_version: 1,
        };
        let response = app
            .clone()
            .oneshot(
                Request::post("/v1/workspaces/wrk_123/checkpoints")
                    .header("content-type", "application/octet-stream")
                    .header("authorization", format!("Bearer {runtime_token}"))
                    .header(
                        "x-opencode-checkpoint-metadata",
                        serde_json::to_string(&metadata).unwrap(),
                    )
                    .body(Body::from(bundle.as_slice()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);

        let response = app
            .oneshot(
                Request::get("/v1/workspaces/wrk_123/checkpoints/latest")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let returned: CheckpointMetadata =
            serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        assert_eq!(returned, metadata);
    }
}
