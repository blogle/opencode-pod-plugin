use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::sync::Mutex;

use crate::state::{CheckpointRecord, Store};

#[derive(Clone)]
pub struct CheckpointStorage {
    root: PathBuf,
    store: Store,
    write_lock: Arc<Mutex<()>>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CheckpointMetadata {
    pub workspace_id: String,
    pub created_at: String,
    pub head: String,
    pub branch: Option<String>,
    pub status_sha256: String,
    pub state_sha256: String,
    pub bundle_sha256: String,
    pub checkpoint_oid: String,
    pub bundle_ref: String,
    pub head_ref: String,
    pub has_changes: bool,
    pub format_version: u32,
}

impl CheckpointStorage {
    pub fn new(root: impl AsRef<Path>, store: Store) -> Result<Self> {
        std::fs::create_dir_all(root.as_ref()).context("create checkpoint storage")?;
        Ok(Self {
            root: root.as_ref().to_owned(),
            store,
            write_lock: Arc::new(Mutex::new(())),
        })
    }

    pub async fn put(
        &self,
        workspace_id: &str,
        metadata: CheckpointMetadata,
        bundle: &[u8],
    ) -> Result<CheckpointMetadata> {
        if metadata.workspace_id != workspace_id {
            bail!("checkpoint workspaceId does not match route");
        }
        if metadata.format_version != 1 {
            bail!("unsupported checkpoint formatVersion");
        }
        validate_sha(&metadata.status_sha256)?;
        validate_sha(&metadata.state_sha256)?;
        validate_sha(&metadata.bundle_sha256)?;
        let actual = format!("{:x}", Sha256::digest(bundle));
        if actual != metadata.bundle_sha256 {
            bail!("checkpoint bundle SHA-256 mismatch");
        }
        if metadata.head.len() != 40 && metadata.head.len() != 64 {
            bail!("checkpoint head is not a full object ID");
        }
        if metadata.checkpoint_oid.len() != 40 && metadata.checkpoint_oid.len() != 64 {
            bail!("checkpointOid is not a full object ID");
        }
        if !metadata
            .bundle_ref
            .starts_with("refs/opencode/checkpoints/")
        {
            bail!("bundleRef is not an OpenCode checkpoint ref");
        }
        if !metadata.head_ref.starts_with("refs/opencode/heads/") {
            bail!("headRef is not an OpenCode checkpoint HEAD ref");
        }

        let _guard = self.write_lock.lock().await;
        let directory = self.root.join(safe_component(workspace_id));
        tokio::fs::create_dir_all(&directory).await?;
        let destination = directory.join(format!("{}.bundle", metadata.bundle_sha256));
        let temporary = directory.join(format!(".{}.tmp", uuid::Uuid::new_v4()));
        tokio::fs::write(&temporary, bundle).await?;
        tokio::fs::rename(&temporary, &destination).await?;
        let record = CheckpointRecord {
            workspace_id: workspace_id.into(),
            bundle_sha256: metadata.bundle_sha256.clone(),
            metadata_json: Some(
                serde_json::to_string(&metadata).context("serialize checkpoint metadata")?,
            ),
            blob_path: destination.to_string_lossy().into_owned(),
        };
        if let Err(error) = self.store.put_checkpoint(&record) {
            let _ = tokio::fs::remove_file(&destination).await;
            return Err(error);
        }
        Ok(metadata)
    }

    pub fn latest(&self, workspace_id: &str) -> Result<Option<CheckpointMetadata>> {
        self.store
            .latest_checkpoint(workspace_id)?
            .map(|record| {
                let metadata_json = record
                    .metadata_json
                    .context("checkpoint predates metadata_json migration")?;
                let metadata = serde_json::from_str(&metadata_json)
                    .context("parse persisted checkpoint metadata")?;
                Ok(metadata)
            })
            .transpose()
    }

    pub async fn latest_blob(&self, workspace_id: &str) -> Result<Option<Vec<u8>>> {
        let Some(record) = self.store.latest_checkpoint(workspace_id)? else {
            return Ok(None);
        };
        let bytes = tokio::fs::read(&record.blob_path)
            .await
            .context("read checkpoint blob")?;
        let actual = format!("{:x}", Sha256::digest(&bytes));
        if actual != record.bundle_sha256 {
            bail!("stored checkpoint bundle is corrupt");
        }
        Ok(Some(bytes))
    }

    pub async fn purge(&self, workspace_id: &str) -> Result<()> {
        for path in self.store.checkpoint_paths(workspace_id)? {
            let _ = tokio::fs::remove_file(path).await;
        }
        let _ = tokio::fs::remove_dir(self.root.join(safe_component(workspace_id))).await;
        self.store.delete_checkpoints(workspace_id)?;
        Ok(())
    }
}

fn validate_sha(value: &str) -> Result<()> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        bail!("SHA-256 must be 64 lowercase hexadecimal characters");
    }
    Ok(())
}

fn safe_component(value: &str) -> String {
    format!("{:x}", Sha256::digest(value.as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{Workspace, WorkspaceState};
    use tempfile::tempdir;

    #[tokio::test]
    async fn atomically_stores_and_verifies_blob() {
        let store = Store::open_memory().unwrap();
        store
            .insert_workspace(&Workspace {
                id: "wrk_1".into(),
                project_key: "demo".into(),
                git_ref: "main".into(),
                owner: "dev".into(),
                state: WorkspaceState::Running,
                service_name: "workspace-one".into(),
                preview_key: "one".into(),
                password: "p".into(),
                runtime_token: "t".into(),
                upstream_environment: "{}".into(),
                image_ref: "image:dev".into(),
                image_digest: None,
                last_activity: String::new(),
                error: None,
            })
            .unwrap();
        let bundle = b"git bundle";
        let sha = format!("{:x}", Sha256::digest(bundle));
        let metadata = CheckpointMetadata {
            workspace_id: "wrk_1".into(),
            created_at: "2026-07-17T00:00:00Z".into(),
            head: "a".repeat(40),
            branch: Some("main".into()),
            status_sha256: "b".repeat(64),
            state_sha256: "c".repeat(64),
            bundle_sha256: sha,
            checkpoint_oid: "d".repeat(40),
            bundle_ref: "refs/opencode/checkpoints/wrk_1-1".into(),
            head_ref: "refs/opencode/heads/wrk_1-1".into(),
            has_changes: true,
            format_version: 1,
        };
        let storage = CheckpointStorage::new(tempdir().unwrap().path(), store).unwrap();
        storage
            .put("wrk_1", metadata.clone(), bundle)
            .await
            .unwrap();
        assert_eq!(storage.latest("wrk_1").unwrap(), Some(metadata));
        assert_eq!(storage.latest_blob("wrk_1").await.unwrap().unwrap(), bundle);
    }
}
