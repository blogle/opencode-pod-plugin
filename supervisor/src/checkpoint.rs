use anyhow::{bail, ensure, Context, Result};
use chrono::{SecondsFormat, Utc};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::ffi::OsStr;
use std::fs::{self, File};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
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

#[derive(Debug, Clone)]
pub struct CheckpointArtifact {
    pub metadata: CheckpointMetadata,
    pub metadata_path: PathBuf,
    pub bundle_path: PathBuf,
}

#[derive(Debug, Clone)]
struct StateSnapshot {
    status: Vec<u8>,
    bytes: Vec<u8>,
}

struct RepoLock {
    file: File,
}

impl RepoLock {
    fn acquire(repo: &Path) -> Result<Self> {
        let git_dir = git_stdout(repo, ["rev-parse", "--absolute-git-dir"])?;
        let git_dir = PathBuf::from(trim_ascii(&git_dir));
        let file = File::options()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(git_dir.join("opencode-checkpoint.lock"))
            .context("open checkpoint lock")?;
        file.lock_exclusive().context("acquire checkpoint lock")?;
        Ok(Self { file })
    }
}

impl Drop for RepoLock {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

/// Captures the repository's exact ordinary Git state into a bundle and metadata file.
///
/// # Errors
///
/// Returns an error if the repository is invalid, Git cannot complete the stash transaction,
/// the live state changes during capture, or the artifact cannot be written.
#[allow(clippy::too_many_lines)]
pub fn capture(repo: &Path, workspace_id: &str, output_dir: &Path) -> Result<CheckpointArtifact> {
    validate_workspace_id(workspace_id)?;
    ensure_repo(repo)?;
    fs::create_dir_all(output_dir).context("create checkpoint output directory")?;
    let canonical_repo = repo.canonicalize().context("resolve repository path")?;
    let canonical_output = output_dir
        .canonicalize()
        .context("resolve checkpoint output directory")?;
    ensure!(
        !canonical_output.starts_with(&canonical_repo),
        "checkpoint output directory must be outside the repository"
    );
    let _lock = RepoLock::acquire(repo)?;

    let before = snapshot(repo)?;
    let stash_before = stash_entries(repo)?;
    let head = text(git_stdout(repo, ["rev-parse", "HEAD"])?);
    let branch_output = git(repo, ["symbolic-ref", "--quiet", "--short", "HEAD"])?;
    let branch = branch_output
        .status
        .success()
        .then(|| text(branch_output.stdout));
    let nonce = format!(
        "{}-{}",
        std::process::id(),
        Utc::now().timestamp_nanos_opt().unwrap_or(0)
    );
    let bundle_ref = format!("refs/opencode/checkpoints/{workspace_id}-{nonce}");
    let head_ref = format!("refs/opencode/heads/{workspace_id}-{nonce}");
    let bundle_path = output_dir.join(format!("checkpoint-{nonce}.bundle"));
    let metadata_path = output_dir.join(format!("checkpoint-{nonce}.json"));

    let has_changes = !before.status.is_empty();
    let checkpoint_oid = if has_changes {
        let stash_push = git(
            repo,
            [
                "stash",
                "push",
                "--include-untracked",
                "--message",
                &format!("opencode checkpoint {workspace_id}"),
            ],
        )?;
        ensure_success(stash_push, "create checkpoint stash")?;

        let stash_oid = text(git_stdout(repo, ["rev-parse", "--verify", "refs/stash"])?);
        ensure!(
            !stash_before.iter().any(|entry| entry.0 == stash_oid),
            "git did not create a new checkpoint stash"
        );
        let transaction = (|| -> Result<()> {
            git_success(repo, ["update-ref", &bundle_ref, &stash_oid])?;
            git_success(repo, ["update-ref", &head_ref, &head])?;
            let bundle_arg = bundle_path
                .to_str()
                .context("checkpoint bundle path is not valid UTF-8")?;
            git_success(
                repo,
                ["bundle", "create", bundle_arg, &bundle_ref, &head_ref],
            )?;
            git_success(repo, ["stash", "apply", "--index", &stash_oid])?;
            remove_stash_entry(repo, &stash_oid)?;
            git_success(repo, ["update-ref", "-d", &bundle_ref])?;
            git_success(repo, ["update-ref", "-d", &head_ref])?;
            Ok(())
        })();

        if let Err(error) = transaction {
            let _ = git(repo, ["update-ref", "-d", &bundle_ref]);
            let _ = git(repo, ["update-ref", "-d", &head_ref]);
            if snapshot(repo).is_ok_and(|current| current.bytes != before.bytes) {
                let _ = git(repo, ["stash", "apply", "--index", &stash_oid]);
            }
            return Err(error).context(format!(
                "checkpoint transaction failed; recovery stash is {stash_oid}"
            ));
        }
        stash_oid
    } else {
        git_success(repo, ["update-ref", &bundle_ref, &head])?;
        git_success(repo, ["update-ref", &head_ref, &head])?;
        let bundle_arg = bundle_path
            .to_str()
            .context("checkpoint bundle path is not valid UTF-8")?;
        let result = git_success(
            repo,
            ["bundle", "create", bundle_arg, &bundle_ref, &head_ref],
        );
        let checkpoint_cleanup = git_success(repo, ["update-ref", "-d", &bundle_ref]);
        let head_cleanup = git_success(repo, ["update-ref", "-d", &head_ref]);
        result?;
        checkpoint_cleanup?;
        head_cleanup?;
        head.clone()
    };

    let after = snapshot(repo)?;
    ensure!(
        before.status == after.status,
        "checkpoint changed git status byte output"
    );
    ensure!(
        before.bytes == after.bytes,
        "checkpoint changed index, worktree, or untracked file bytes"
    );
    ensure!(
        stash_before == stash_entries(repo)?,
        "checkpoint changed unrelated stash entries"
    );

    let bundle_bytes = fs::read(&bundle_path).context("read checkpoint bundle")?;
    let metadata = CheckpointMetadata {
        workspace_id: workspace_id.to_owned(),
        created_at: Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
        head,
        branch,
        status_sha256: sha256(&before.status),
        state_sha256: sha256(&before.bytes),
        bundle_sha256: sha256(&bundle_bytes),
        checkpoint_oid,
        bundle_ref,
        head_ref,
        has_changes,
        format_version: 1,
    };
    let metadata_bytes =
        serde_json::to_vec_pretty(&metadata).context("serialize checkpoint metadata")?;
    fs::write(&metadata_path, metadata_bytes).context("write checkpoint metadata")?;

    Ok(CheckpointArtifact {
        metadata,
        metadata_path,
        bundle_path,
    })
}

/// Restores and verifies a checkpoint into a clean checkout at the recorded `HEAD`.
///
/// # Errors
///
/// Returns an error before applying when the checkout, base, metadata, or bundle is invalid, and
/// after applying when the resulting status or content fingerprint does not match the checkpoint.
pub fn restore(repo: &Path, metadata_path: &Path, bundle_path: &Path) -> Result<()> {
    ensure_repo(repo)?;
    let _lock = RepoLock::acquire(repo)?;
    let metadata: CheckpointMetadata =
        serde_json::from_slice(&fs::read(metadata_path).context("read checkpoint metadata")?)
            .context("parse checkpoint metadata")?;
    ensure!(
        metadata.format_version == 1,
        "unsupported checkpoint format"
    );
    validate_workspace_id(&metadata.workspace_id)?;
    ensure!(
        sha256(&fs::read(bundle_path).context("read checkpoint bundle")?) == metadata.bundle_sha256,
        "checkpoint bundle SHA-256 mismatch"
    );
    ensure!(
        git_stdout(
            repo,
            ["status", "--porcelain=v2", "-z", "--untracked-files=all"]
        )?
        .is_empty(),
        "restore requires a clean checkout"
    );
    let bundle = bundle_path
        .to_str()
        .context("checkpoint bundle path is not valid UTF-8")?;
    git_success(repo, ["bundle", "verify", bundle])?;

    let restore_id = format!(
        "{}-{}",
        std::process::id(),
        Utc::now().timestamp_nanos_opt().unwrap_or(0)
    );
    let local_checkpoint_ref = format!("refs/opencode/restore/checkpoint-{restore_id}");
    let local_head_ref = format!("refs/opencode/restore/head-{restore_id}");
    let checkpoint_refspec = format!("{}:{local_checkpoint_ref}", metadata.bundle_ref);
    let head_refspec = format!("{}:{local_head_ref}", metadata.head_ref);
    git_success(
        repo,
        [
            "fetch",
            "--no-tags",
            bundle,
            &checkpoint_refspec,
            &head_refspec,
        ],
    )?;
    let result = (|| -> Result<()> {
        ensure!(
            text(git_stdout(repo, ["rev-parse", &local_checkpoint_ref])?)
                == metadata.checkpoint_oid,
            "bundle checkpoint OID mismatch"
        );
        ensure!(
            text(git_stdout(repo, ["rev-parse", &local_head_ref])?) == metadata.head,
            "bundle HEAD OID mismatch"
        );
        if let Some(branch) = &metadata.branch {
            git_success(repo, ["checkout", "-B", branch, &metadata.head])?;
        } else {
            git_success(repo, ["checkout", "--detach", &metadata.head])?;
        }
        git_success(repo, ["reset", "--hard", &metadata.head])?;
        if metadata.has_changes {
            git_success(
                repo,
                ["stash", "apply", "--index", &metadata.checkpoint_oid],
            )?;
        }
        let restored = snapshot(repo)?;
        ensure!(
            sha256(&restored.status) == metadata.status_sha256,
            "restored git status fingerprint mismatch"
        );
        ensure!(
            sha256(&restored.bytes) == metadata.state_sha256,
            "restored worktree fingerprint mismatch"
        );
        Ok(())
    })();
    let checkpoint_cleanup = git(repo, ["update-ref", "-d", &local_checkpoint_ref])?;
    let head_cleanup = git(repo, ["update-ref", "-d", &local_head_ref])?;
    ensure_success(
        checkpoint_cleanup,
        "remove temporary checkpoint restore ref",
    )?;
    ensure_success(head_cleanup, "remove temporary HEAD restore ref")?;
    result
}

fn snapshot(repo: &Path) -> Result<StateSnapshot> {
    let status = git_stdout(
        repo,
        ["status", "--porcelain=v2", "-z", "--untracked-files=all"],
    )?;
    let mut bytes = Vec::new();
    append_section(&mut bytes, b"status", &status);
    append_section(
        &mut bytes,
        b"cached-diff",
        &git_stdout(repo, ["diff", "--binary", "--cached", "--no-ext-diff"])?,
    );
    append_section(
        &mut bytes,
        b"worktree-diff",
        &git_stdout(repo, ["diff", "--binary", "--no-ext-diff"])?,
    );
    let untracked = git_stdout(repo, ["ls-files", "--others", "--exclude-standard", "-z"])?;
    for path in untracked
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
    {
        let relative =
            std::str::from_utf8(path).context("non-UTF-8 untracked paths are unsupported")?;
        let full_path = repo.join(relative);
        let metadata = fs::symlink_metadata(&full_path).context("inspect untracked file")?;
        append_section(&mut bytes, b"path", path);
        append_section(
            &mut bytes,
            b"mode",
            format!("{:o}", metadata.permissions().mode() & 0o7777).as_bytes(),
        );
        let content = if metadata.file_type().is_symlink() {
            fs::read_link(&full_path)
                .context("read untracked symlink")?
                .as_os_str()
                .as_encoded_bytes()
                .to_vec()
        } else {
            ensure!(metadata.is_file(), "unsupported untracked path: {relative}");
            fs::read(&full_path).context("read untracked file")?
        };
        append_section(&mut bytes, b"content", &content);
    }
    Ok(StateSnapshot { status, bytes })
}

fn append_section(target: &mut Vec<u8>, name: &[u8], value: &[u8]) {
    target.extend_from_slice(&(name.len() as u64).to_be_bytes());
    target.extend_from_slice(name);
    target.extend_from_slice(&(value.len() as u64).to_be_bytes());
    target.extend_from_slice(value);
}

fn stash_entries(repo: &Path) -> Result<Vec<(String, String)>> {
    let output = git_stdout(repo, ["stash", "list", "--format=%H%x00%gs%x00"])?;
    let fields: Vec<_> = output.split(|byte| *byte == 0).collect();
    Ok(fields
        .chunks_exact(2)
        .filter(|pair| !pair[0].is_empty())
        .map(|pair| (text(pair[0]), text(pair[1])))
        .collect())
}

fn remove_stash_entry(repo: &Path, oid: &str) -> Result<()> {
    let entries = stash_entries(repo)?;
    let positions: Vec<_> = entries
        .iter()
        .enumerate()
        .filter_map(|(index, entry)| (entry.0 == oid).then_some(index))
        .collect();
    ensure!(
        positions.len() == 1,
        "cannot uniquely identify checkpoint stash"
    );
    git_success(
        repo,
        ["stash", "drop", &format!("stash@{{{}}}", positions[0])],
    )
}

fn ensure_repo(repo: &Path) -> Result<()> {
    ensure!(repo.is_dir(), "repository path does not exist");
    ensure!(
        text(git_stdout(repo, ["rev-parse", "--is-inside-work-tree"])?).as_str() == "true",
        "path is not a Git worktree"
    );
    Ok(())
}

fn validate_workspace_id(value: &str) -> Result<()> {
    ensure!(!value.is_empty(), "workspace ID must not be empty");
    ensure!(
        value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_')),
        "workspace ID contains invalid characters"
    );
    Ok(())
}

fn git<I, S>(repo: &Path, args: I) -> Result<Output>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    Command::new("git")
        .args(args)
        .current_dir(repo)
        .env("LC_ALL", "C")
        .output()
        .context("execute git")
}

fn git_stdout<I, S>(repo: &Path, args: I) -> Result<Vec<u8>>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let output = git(repo, args)?;
    ensure_success(output, "git command")
}

fn git_success<I, S>(repo: &Path, args: I) -> Result<()>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let output = git(repo, args)?;
    ensure_success(output, "git command").map(|_| ())
}

fn ensure_success(output: Output, operation: &str) -> Result<Vec<u8>> {
    if !output.status.success() {
        bail!(
            "{operation} failed ({}): {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(output.stdout)
}

fn text(value: impl AsRef<[u8]>) -> String {
    String::from_utf8_lossy(value.as_ref()).trim().to_owned()
}

fn trim_ascii(value: &[u8]) -> String {
    String::from_utf8_lossy(value).trim().to_owned()
}

fn sha256(value: &[u8]) -> String {
    hex::encode(Sha256::digest(value))
}
