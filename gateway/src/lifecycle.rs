use std::time::Duration;

use anyhow::{bail, Context, Result};

use crate::{
    state::{Workspace, WorkspaceState},
    AppState,
};

pub async fn suspend(state: &AppState, id: &str) -> Result<Workspace> {
    let _operation = state.operations.lock().await;
    let workspace = state.store.workspace(id)?.context("workspace not found")?;
    if workspace.state == WorkspaceState::Suspended {
        return Ok(workspace);
    }
    if workspace.state != WorkspaceState::Running {
        bail!("cannot suspend workspace in {} state", workspace.state);
    }
    let workspace = state
        .store
        .transition(id, WorkspaceState::Suspending, None)?;
    if let Err(error) = state.k8s.suspend(&workspace).await {
        let _ = state
            .store
            .transition(id, WorkspaceState::Error, Some(&error.to_string()));
        return Err(error).context("graceful sandbox suspension failed");
    }
    state.store.transition(id, WorkspaceState::Suspended, None)
}

pub async fn reconcile_idle_once(state: &AppState) -> Result<usize> {
    let candidates = state
        .store
        .idle_running(state.config.lifecycle.suspend_after_idle_seconds)?;
    let mut suspended = 0;
    for workspace in candidates {
        match suspend(state, &workspace.id).await {
            Ok(_) => suspended += 1,
            Err(error) => {
                tracing::error!(workspace_id=%workspace.id, project_key=%workspace.project_key, owner=%workspace.owner, operation="idle-suspend", %error, "idle workspace suspension failed")
            }
        }
    }
    Ok(suspended)
}

pub async fn run_idle_reconciler(state: AppState) {
    let mut interval =
        tokio::time::interval(Duration::from_secs(state.config.lifecycle.poll_seconds));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    interval.tick().await;
    loop {
        interval.tick().await;
        if let Err(error) = reconcile_idle_once(&state).await {
            tracing::error!(operation="idle-reconcile", %error, "idle reconciliation failed");
        }
    }
}
