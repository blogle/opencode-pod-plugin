use std::{
    fmt,
    path::Path,
    str::FromStr,
    sync::{Arc, Mutex},
};

use anyhow::{bail, Context, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

#[derive(Clone)]
pub struct Store(Arc<Mutex<Connection>>);

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum WorkspaceState {
    Provisioning,
    Running,
    Checkpointing,
    Suspending,
    Suspended,
    Resuming,
    Deleting,
    Deleted,
    Error,
}

impl fmt::Display for WorkspaceState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}",
            serde_json::to_value(self).unwrap().as_str().unwrap()
        )
    }
}

impl FromStr for WorkspaceState {
    type Err = anyhow::Error;
    fn from_str(value: &str) -> Result<Self> {
        serde_json::from_str(&format!("\"{value}\"")).context("invalid workspace state")
    }
}

impl WorkspaceState {
    pub fn permits(self, next: Self) -> bool {
        use WorkspaceState::*;
        self == next
            || matches!(
                (self, next),
                (Provisioning, Running | Error | Deleting)
                    | (Running, Checkpointing | Suspending | Error | Deleting)
                    | (Checkpointing, Running | Suspending | Error)
                    | (Suspending, Suspended | Error)
                    | (Suspended, Resuming | Deleting)
                    | (Resuming, Running | Error | Deleting)
                    | (Error, Resuming | Deleting)
                    | (Deleting, Deleted | Error)
            )
    }
}

#[derive(Clone, Debug)]
pub struct Workspace {
    pub id: String,
    pub project_key: String,
    pub git_ref: String,
    pub owner: String,
    pub state: WorkspaceState,
    pub service_name: String,
    pub preview_key: String,
    pub password: String,
    pub runtime_token: String,
    pub upstream_environment: String,
    pub image_ref: String,
    pub image_digest: Option<String>,
    pub last_activity: String,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvProfileMeta {
    pub project_key: String,
    pub owner: String,
    pub sha256: String,
    pub updated_at: String,
}

#[derive(Clone, Debug)]
pub struct CheckpointRecord {
    pub workspace_id: String,
    pub bundle_sha256: String,
    pub metadata_json: Option<String>,
    pub blob_path: String,
}

impl Store {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        if let Some(parent) = path.as_ref().parent() {
            std::fs::create_dir_all(parent)?;
        }
        let connection = Connection::open(path).context("open gateway SQLite database")?;
        connection.pragma_update(None, "journal_mode", "WAL")?;
        connection.pragma_update(None, "foreign_keys", "ON")?;
        migrate(&connection)?;
        Ok(Self(Arc::new(Mutex::new(connection))))
    }

    pub fn open_memory() -> Result<Self> {
        let connection = Connection::open_in_memory()?;
        connection.pragma_update(None, "foreign_keys", "ON")?;
        migrate(&connection)?;
        Ok(Self(Arc::new(Mutex::new(connection))))
    }

    pub fn insert_workspace(&self, workspace: &Workspace) -> Result<(Workspace, bool)> {
        let connection = self.0.lock().unwrap();
        if let Some(existing) = get_workspace(&connection, &workspace.id)? {
            if existing.project_key != workspace.project_key
                || existing.git_ref != workspace.git_ref
                || existing.owner != workspace.owner
            {
                bail!("workspace ID already exists with different immutable configuration");
            }
            if existing.state == WorkspaceState::Deleted {
                bail!("workspace has been deleted");
            }
            return Ok((existing, false));
        }
        connection.execute(
            "INSERT INTO workspaces (id,project_key,git_ref,owner,state,service_name,preview_key,password,runtime_token,upstream_environment,image_ref) VALUES (?,?,?,?,?,?,?,?,?,?,?)",
            params![workspace.id, workspace.project_key, workspace.git_ref, workspace.owner, workspace.state.to_string(), workspace.service_name, workspace.preview_key, workspace.password, workspace.runtime_token, workspace.upstream_environment, workspace.image_ref],
        )?;
        Ok((get_workspace(&connection, &workspace.id)?.unwrap(), true))
    }

    pub fn workspace(&self, id: &str) -> Result<Option<Workspace>> {
        get_workspace(&self.0.lock().unwrap(), id)
    }

    pub fn workspaces(&self, owner: &str) -> Result<Vec<Workspace>> {
        let connection = self.0.lock().unwrap();
        let mut statement = connection.prepare(&format!(
            "{} WHERE owner=? AND state != 'deleted' ORDER BY updated_at DESC",
            SELECT_WORKSPACE
        ))?;
        let rows = statement.query_map([owner], map_workspace)?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }

    pub fn idle_running(&self, idle_seconds: u64) -> Result<Vec<Workspace>> {
        let connection = self.0.lock().unwrap();
        let threshold = format!("-{idle_seconds} seconds");
        let mut statement = connection.prepare(&format!(
            "{} WHERE state='running' AND last_activity <= datetime('now', ?) ORDER BY last_activity",
            SELECT_WORKSPACE
        ))?;
        let rows = statement.query_map([threshold], map_workspace)?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }

    pub fn workspace_by_preview_key(&self, key: &str) -> Result<Option<Workspace>> {
        let connection = self.0.lock().unwrap();
        connection
            .query_row(
                &format!("{} WHERE preview_key=?", SELECT_WORKSPACE),
                [key],
                map_workspace,
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn health(&self) -> Result<()> {
        self.0
            .lock()
            .unwrap()
            .query_row("SELECT 1", [], |_| Ok(()))?;
        Ok(())
    }

    pub fn transition(
        &self,
        id: &str,
        next: WorkspaceState,
        error: Option<&str>,
    ) -> Result<Workspace> {
        let mut connection = self.0.lock().unwrap();
        let transaction = connection.transaction()?;
        let current = get_workspace(&transaction, id)?.context("workspace not found")?;
        if !current.state.permits(next) {
            bail!("invalid workspace transition {} -> {}", current.state, next);
        }
        transaction.execute(
            "UPDATE workspaces SET state=?, error=?, updated_at=datetime('now') WHERE id=?",
            params![next.to_string(), error, id],
        )?;
        let result = get_workspace(&transaction, id)?.unwrap();
        transaction.commit()?;
        Ok(result)
    }

    pub fn record_image_digest(&self, id: &str, digest: &str) -> Result<()> {
        if digest.is_empty() {
            bail!("resolved image digest is empty");
        }
        let connection = self.0.lock().unwrap();
        connection.execute("UPDATE workspaces SET image_digest=COALESCE(image_digest, ?), updated_at=datetime('now') WHERE id=?", params![digest, id])?;
        Ok(())
    }

    pub fn record_activity(&self, id: &str, owner: &str) -> Result<bool> {
        let connection = self.0.lock().unwrap();
        Ok(connection.execute("UPDATE workspaces SET last_activity=datetime('now'), updated_at=datetime('now') WHERE id=? AND owner=? AND state != 'deleted'", params![id, owner])? == 1)
    }

    pub fn put_env_profile(&self, meta: &EnvProfileMeta) -> Result<EnvProfileMeta> {
        let connection = self.0.lock().unwrap();
        connection.execute("INSERT INTO env_profiles(owner,project_key,sha256,updated_at) VALUES(?,?,?,datetime('now')) ON CONFLICT(owner,project_key) DO UPDATE SET sha256=excluded.sha256,updated_at=datetime('now')", params![meta.owner, meta.project_key, meta.sha256])?;
        drop(connection);
        self.env_profile(&meta.owner, &meta.project_key)?
            .context("profile metadata was not stored")
    }

    pub fn env_profile(&self, owner: &str, project: &str) -> Result<Option<EnvProfileMeta>> {
        let connection = self.0.lock().unwrap();
        connection.query_row("SELECT project_key,owner,sha256,updated_at FROM env_profiles WHERE owner=? AND project_key=?", params![owner, project], |row| Ok(EnvProfileMeta { project_key: row.get(0)?, owner: row.get(1)?, sha256: row.get(2)?, updated_at: row.get(3)? })).optional().map_err(Into::into)
    }

    pub fn delete_env_profile(&self, owner: &str, project: &str) -> Result<bool> {
        Ok(self.0.lock().unwrap().execute(
            "DELETE FROM env_profiles WHERE owner=? AND project_key=?",
            params![owner, project],
        )? == 1)
    }

    pub fn put_checkpoint(&self, record: &CheckpointRecord) -> Result<()> {
        let mut connection = self.0.lock().unwrap();
        let transaction = connection.transaction()?;
        transaction.execute(
            "UPDATE checkpoints SET is_latest=0 WHERE workspace_id=?",
            [&record.workspace_id],
        )?;
        transaction.execute("INSERT INTO checkpoints(workspace_id,bundle_sha256,head,branch,status_sha256,format_version,created_at,blob_path,metadata_json,is_latest) VALUES(?,?, '', '', '', 1, '', ?, ?, 1) ON CONFLICT(workspace_id,bundle_sha256) DO UPDATE SET blob_path=excluded.blob_path,metadata_json=excluded.metadata_json,is_latest=1", params![record.workspace_id, record.bundle_sha256, record.blob_path, record.metadata_json])?;
        transaction.commit()?;
        Ok(())
    }

    pub fn latest_checkpoint(&self, workspace_id: &str) -> Result<Option<CheckpointRecord>> {
        let connection = self.0.lock().unwrap();
        connection.query_row("SELECT workspace_id,bundle_sha256,metadata_json,blob_path FROM checkpoints WHERE workspace_id=? AND is_latest=1", [workspace_id], |row| Ok(CheckpointRecord { workspace_id: row.get(0)?, bundle_sha256: row.get(1)?, metadata_json: row.get(2)?, blob_path: row.get(3)? })).optional().map_err(Into::into)
    }

    pub fn checkpoint_paths(&self, workspace_id: &str) -> Result<Vec<String>> {
        let connection = self.0.lock().unwrap();
        let mut statement =
            connection.prepare("SELECT blob_path FROM checkpoints WHERE workspace_id=?")?;
        let rows = statement.query_map([workspace_id], |row| row.get(0))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }

    pub fn delete_checkpoints(&self, workspace_id: &str) -> Result<()> {
        self.0.lock().unwrap().execute(
            "DELETE FROM checkpoints WHERE workspace_id=?",
            [workspace_id],
        )?;
        Ok(())
    }

    pub fn bind_session(&self, workspace_id: &str, session_id: &str) -> Result<()> {
        self.0.lock().unwrap().execute(
            "INSERT INTO workspace_sessions(workspace_id,session_id) VALUES(?,?) ON CONFLICT(workspace_id) DO UPDATE SET session_id=excluded.session_id",
            params![workspace_id, session_id],
        )?;
        Ok(())
    }

    pub fn session_id(&self, workspace_id: &str) -> Result<Option<String>> {
        self.0
            .lock()
            .unwrap()
            .query_row(
                "SELECT session_id FROM workspace_sessions WHERE workspace_id=?",
                [workspace_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(Into::into)
    }
}

const SELECT_WORKSPACE: &str = "SELECT id,project_key,git_ref,owner,state,service_name,preview_key,password,runtime_token,upstream_environment,image_ref,image_digest,last_activity,error FROM workspaces";

fn get_workspace(connection: &Connection, id: &str) -> Result<Option<Workspace>> {
    connection
        .query_row(
            &format!("{} WHERE id=?", SELECT_WORKSPACE),
            [id],
            map_workspace,
        )
        .optional()
        .map_err(Into::into)
}

fn map_workspace(row: &rusqlite::Row<'_>) -> rusqlite::Result<Workspace> {
    let state: String = row.get(4)?;
    Ok(Workspace {
        id: row.get(0)?,
        project_key: row.get(1)?,
        git_ref: row.get(2)?,
        owner: row.get(3)?,
        state: WorkspaceState::from_str(&state).map_err(|_| rusqlite::Error::InvalidQuery)?,
        service_name: row.get(5)?,
        preview_key: row.get(6)?,
        password: row.get(7)?,
        runtime_token: row.get(8)?,
        upstream_environment: row.get(9)?,
        image_ref: row.get(10)?,
        image_digest: row.get(11)?,
        last_activity: row.get(12)?,
        error: row.get(13)?,
    })
}

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS workspaces (
 id TEXT PRIMARY KEY, project_key TEXT NOT NULL, git_ref TEXT NOT NULL, owner TEXT NOT NULL,
 state TEXT NOT NULL CHECK(state IN ('provisioning','running','checkpointing','suspending','suspended','resuming','deleting','deleted','error')),
 service_name TEXT NOT NULL UNIQUE, preview_key TEXT NOT NULL UNIQUE, password TEXT NOT NULL, runtime_token TEXT NOT NULL,
 upstream_environment TEXT NOT NULL, image_ref TEXT NOT NULL, image_digest TEXT,
 last_activity TEXT NOT NULL DEFAULT (datetime('now')), error TEXT,
 created_at TEXT NOT NULL DEFAULT (datetime('now')), updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE INDEX IF NOT EXISTS workspaces_owner ON workspaces(owner);
CREATE TABLE IF NOT EXISTS workspace_sessions (
 workspace_id TEXT PRIMARY KEY, session_id TEXT NOT NULL UNIQUE,
 FOREIGN KEY(workspace_id) REFERENCES workspaces(id) ON DELETE CASCADE
);
CREATE TABLE IF NOT EXISTS env_profiles (
 owner TEXT NOT NULL, project_key TEXT NOT NULL, sha256 TEXT NOT NULL, updated_at TEXT NOT NULL DEFAULT (datetime('now')),
 PRIMARY KEY(owner, project_key)
);
CREATE TABLE IF NOT EXISTS checkpoints (
 workspace_id TEXT NOT NULL, bundle_sha256 TEXT NOT NULL, head TEXT NOT NULL, branch TEXT NOT NULL,
 status_sha256 TEXT NOT NULL, format_version INTEGER NOT NULL, created_at TEXT NOT NULL, blob_path TEXT NOT NULL,
 metadata_json TEXT, is_latest INTEGER NOT NULL DEFAULT 0, PRIMARY KEY(workspace_id,bundle_sha256),
 FOREIGN KEY(workspace_id) REFERENCES workspaces(id) ON DELETE CASCADE
);
CREATE UNIQUE INDEX IF NOT EXISTS checkpoint_latest ON checkpoints(workspace_id) WHERE is_latest=1;
";

fn migrate(connection: &Connection) -> Result<()> {
    connection
        .execute_batch(SCHEMA)
        .context("migrate gateway database")?;
    let mut columns = connection.prepare("PRAGMA table_info(checkpoints)")?;
    let names = columns
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if !names.iter().any(|name| name == "metadata_json") {
        connection.execute("ALTER TABLE checkpoints ADD COLUMN metadata_json TEXT", [])?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn workspace() -> Workspace {
        Workspace {
            id: "wrk_1".into(),
            project_key: "demo".into(),
            git_ref: "main".into(),
            owner: "dev".into(),
            state: WorkspaceState::Provisioning,
            service_name: "workspace-one".into(),
            preview_key: "one".into(),
            password: "p".into(),
            runtime_token: "t".into(),
            upstream_environment: "{}".into(),
            image_ref: "image:dev".into(),
            image_digest: None,
            last_activity: String::new(),
            error: None,
        }
    }

    #[test]
    fn enforces_state_machine() {
        let store = Store::open_memory().unwrap();
        store.insert_workspace(&workspace()).unwrap();
        assert!(store
            .transition("wrk_1", WorkspaceState::Suspended, None)
            .is_err());
        store
            .transition("wrk_1", WorkspaceState::Running, None)
            .unwrap();
        store
            .transition("wrk_1", WorkspaceState::Suspending, None)
            .unwrap();
        assert_eq!(
            store
                .transition("wrk_1", WorkspaceState::Suspended, None)
                .unwrap()
                .state,
            WorkspaceState::Suspended
        );
    }

    #[test]
    fn duplicate_create_is_idempotent_but_not_mutable() {
        let store = Store::open_memory().unwrap();
        assert!(store.insert_workspace(&workspace()).unwrap().1);
        assert!(!store.insert_workspace(&workspace()).unwrap().1);
        let mut changed = workspace();
        changed.git_ref = "other".into();
        assert!(store.insert_workspace(&changed).is_err());
    }

    #[test]
    fn migrates_checkpoint_metadata_json_without_recreating_table() {
        let connection = Connection::open_in_memory().unwrap();
        connection.execute_batch(
            "CREATE TABLE checkpoints (
                workspace_id TEXT NOT NULL, bundle_sha256 TEXT NOT NULL, head TEXT NOT NULL,
                branch TEXT NOT NULL, status_sha256 TEXT NOT NULL, format_version INTEGER NOT NULL,
                created_at TEXT NOT NULL, blob_path TEXT NOT NULL, is_latest INTEGER NOT NULL DEFAULT 0,
                PRIMARY KEY(workspace_id,bundle_sha256)
            );",
        ).unwrap();
        migrate(&connection).unwrap();
        let mut statement = connection
            .prepare("PRAGMA table_info(checkpoints)")
            .unwrap();
        let columns = statement
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        assert!(columns.iter().any(|column| column == "metadata_json"));
    }

    #[test]
    fn selects_only_expired_running_workspaces_for_idle_suspend() {
        let store = Store::open_memory().unwrap();
        store.insert_workspace(&workspace()).unwrap();
        store
            .transition("wrk_1", WorkspaceState::Running, None)
            .unwrap();
        store
            .0
            .lock()
            .unwrap()
            .execute(
                "UPDATE workspaces SET last_activity=datetime('now', '-2 hours') WHERE id='wrk_1'",
                [],
            )
            .unwrap();
        assert_eq!(store.idle_running(3600).unwrap()[0].id, "wrk_1");
        assert!(store.idle_running(10_000).unwrap().is_empty());
    }
}
