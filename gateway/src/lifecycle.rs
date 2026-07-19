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

pub async fn ensure_workspace(state: &AppState, id: &str, resume_error: bool) -> Result<Workspace> {
    let _operation = state.operations.lock().await;
    let workspace = state.store.workspace(id)?.context("workspace not found")?;
    let workspace = match workspace.state {
        WorkspaceState::Error if !resume_error => return Ok(workspace),
        WorkspaceState::Suspended | WorkspaceState::Error => {
            state.store.transition(id, WorkspaceState::Resuming, None)?
        }
        WorkspaceState::Running | WorkspaceState::Provisioning | WorkspaceState::Resuming => {
            workspace
        }
        state_value => bail!("cannot ensure workspace in {state_value} state"),
    };
    let project = state
        .config
        .projects()?
        .get(&workspace.project_key)
        .cloned()
        .context("registered project disappeared")?;
    let has_profile = state
        .store
        .env_profile(&workspace.owner, &workspace.project_key)?
        .is_some();
    match state.k8s.provision(&workspace, &project, has_profile).await {
        Ok(result) => {
            state.store.record_image_digest(id, &result.image_digest)?;
            if workspace.state == WorkspaceState::Running {
                state.store.workspace(id)?.context("workspace disappeared")
            } else {
                Ok(state.store.transition(id, WorkspaceState::Running, None)?)
            }
        }
        Err(error) => {
            tracing::error!(workspace_id=%workspace.id, project_key=%workspace.project_key, owner=%workspace.owner, operation="ensure", %error, "workspace reconciliation failed");
            let _ = state
                .store
                .transition(id, WorkspaceState::Error, Some(&error.to_string()));
            Err(error).context("workspace ensure failed")
        }
    }
}

pub async fn reconcile_workspaces_once(state: &AppState) -> Result<usize> {
    let candidates = state.store.all_workspaces()?;
    let mut reconciled = 0;
    for workspace in candidates {
        let sandbox_exists = if workspace.state == WorkspaceState::Running {
            state.k8s.sandbox_exists(&workspace).await?
        } else {
            false
        };
        let should_ensure = should_ensure_workspace(workspace.state, sandbox_exists);
        if should_ensure {
            match ensure_workspace(state, &workspace.id, false).await {
                Ok(result) if result.state != WorkspaceState::Error => reconciled += 1,
                Ok(_) => {}
                Err(error) => {
                    tracing::error!(workspace_id=%workspace.id, project_key=%workspace.project_key, owner=%workspace.owner, operation="workspace-reconcile", %error, "workspace reconciliation failed")
                }
            }
        }
    }
    Ok(reconciled)
}

fn should_ensure_workspace(state: WorkspaceState, sandbox_exists: bool) -> bool {
    matches!(
        state,
        WorkspaceState::Provisioning | WorkspaceState::Resuming
    ) || (state == WorkspaceState::Running && !sandbox_exists)
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

pub async fn run_workspace_reconciler(state: AppState) {
    let mut interval =
        tokio::time::interval(Duration::from_secs(state.config.lifecycle.poll_seconds));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    interval.tick().await;
    loop {
        interval.tick().await;
        if let Err(error) = reconcile_workspaces_once(&state).await {
            tracing::error!(operation="workspace-reconcile", %error, "workspace reconciliation failed");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reconciles_missing_and_interrupted_sandboxes_only() {
        assert!(should_ensure_workspace(WorkspaceState::Running, false));
        assert!(!should_ensure_workspace(WorkspaceState::Running, true));
        assert!(should_ensure_workspace(WorkspaceState::Provisioning, false));
        assert!(should_ensure_workspace(WorkspaceState::Resuming, false));
        assert!(!should_ensure_workspace(WorkspaceState::Error, false));
        assert!(!should_ensure_workspace(WorkspaceState::Suspended, false));
    }
}
