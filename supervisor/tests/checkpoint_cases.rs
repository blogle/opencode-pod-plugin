use opencode_supervisor::checkpoint::{capture, restore};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use tempfile::TempDir;

#[derive(Debug, PartialEq, Eq)]
struct ExactState {
    status: Vec<u8>,
    cached_diff: Vec<u8>,
    worktree_diff: Vec<u8>,
    files: Vec<(PathBuf, Vec<u8>, u32)>,
}

struct Fixture {
    root: TempDir,
    remote: PathBuf,
    repo: PathBuf,
    artifacts: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().expect("temporary fixture root");
        let remote = root.path().join("remote.git");
        let seed = root.path().join("seed");
        let repo = root.path().join("working");
        let artifacts = root.path().join("artifacts");
        fs::create_dir(&seed).expect("create seed");
        git(root.path(), ["init", "--bare", path(&remote)]);
        git(&seed, ["init", "--initial-branch=main"]);
        configure_identity(&seed);
        for (name, content) in [
            ("staged.txt", "staged base\n"),
            ("unstaged.txt", "unstaged base\n"),
            ("other-a.txt", "A base\n"),
            ("other-b.txt", "B base\n"),
            ("same.txt", "same base\n"),
            ("delete-staged.txt", "delete staged\n"),
            ("delete-unstaged.txt", "delete unstaged\n"),
            ("executable.sh", "#!/bin/sh\nexit 0\n"),
        ] {
            fs::write(seed.join(name), content).expect("write fixture file");
        }
        git(&seed, ["add", "."]);
        git(&seed, ["commit", "-m", "base"]);
        git(&seed, ["remote", "add", "origin", path(&remote)]);
        git(&seed, ["push", "origin", "main"]);
        git(&remote, ["symbolic-ref", "HEAD", "refs/heads/main"]);
        git(root.path(), ["clone", path(&remote), path(&repo)]);
        configure_identity(&repo);
        Self {
            root,
            remote,
            repo,
            artifacts,
        }
    }

    fn capture_restore(&self) {
        let before = exact_state(&self.repo);
        let stashes_before = output(&self.repo, ["stash", "list", "--format=%H%x00%gs%x00"]);
        let artifact =
            capture(&self.repo, "wrk_test", &self.artifacts).expect("capture checkpoint");
        assert_eq!(
            exact_state(&self.repo),
            before,
            "capture changed live checkout"
        );
        assert_eq!(
            output(&self.repo, ["stash", "list", "--format=%H%x00%gs%x00"]),
            stashes_before,
            "capture changed unrelated stashes"
        );

        let restored = self.root.path().join("restored");
        git(
            self.root.path(),
            ["clone", path(&self.remote), path(&restored)],
        );
        configure_identity(&restored);
        restore(&restored, &artifact.metadata_path, &artifact.bundle_path)
            .expect("restore checkpoint");
        assert_eq!(exact_state(&restored), before, "fresh restore differs");
    }
}

#[test]
fn only_staged_change() {
    let fixture = Fixture::new();
    fs::write(fixture.repo.join("staged.txt"), "staged changed\n").unwrap();
    git(&fixture.repo, ["add", "staged.txt"]);
    fixture.capture_restore();
}

#[test]
fn only_unstaged_change() {
    let fixture = Fixture::new();
    fs::write(fixture.repo.join("unstaged.txt"), "unstaged changed\n").unwrap();
    fixture.capture_restore();
}

#[test]
fn staged_and_unstaged_different_files() {
    let fixture = Fixture::new();
    fs::write(fixture.repo.join("other-a.txt"), "A staged\n").unwrap();
    git(&fixture.repo, ["add", "other-a.txt"]);
    fs::write(fixture.repo.join("other-b.txt"), "B unstaged\n").unwrap();
    fixture.capture_restore();
}

#[test]
fn staged_and_unstaged_same_file() {
    let fixture = Fixture::new();
    fs::write(fixture.repo.join("same.txt"), "same base\nstaged line\n").unwrap();
    git(&fixture.repo, ["add", "same.txt"]);
    fs::write(
        fixture.repo.join("same.txt"),
        "same base\nstaged line\nunstaged line\n",
    )
    .unwrap();
    fixture.capture_restore();
}

#[test]
fn untracked_file() {
    let fixture = Fixture::new();
    fs::create_dir(fixture.repo.join("new-dir")).unwrap();
    fs::write(
        fixture.repo.join("new-dir/untracked.bin"),
        b"untracked\0bytes\n",
    )
    .unwrap();
    fixture.capture_restore();
}

#[test]
fn staged_new_file() {
    let fixture = Fixture::new();
    fs::write(fixture.repo.join("new-staged.txt"), "new staged\n").unwrap();
    git(&fixture.repo, ["add", "new-staged.txt"]);
    fixture.capture_restore();
}

#[test]
fn staged_deletion() {
    let fixture = Fixture::new();
    fs::remove_file(fixture.repo.join("delete-staged.txt")).unwrap();
    git(&fixture.repo, ["add", "delete-staged.txt"]);
    fixture.capture_restore();
}

#[test]
fn unstaged_deletion() {
    let fixture = Fixture::new();
    fs::remove_file(fixture.repo.join("delete-unstaged.txt")).unwrap();
    fixture.capture_restore();
}

#[test]
fn executable_bit_change() {
    let fixture = Fixture::new();
    let script = fixture.repo.join("executable.sh");
    let mut permissions = fs::metadata(&script).unwrap().permissions();
    permissions.set_mode(permissions.mode() | 0o111);
    fs::set_permissions(script, permissions).unwrap();
    fixture.capture_restore();
}

#[test]
fn existing_unrelated_user_stash() {
    let fixture = Fixture::new();
    fs::write(fixture.repo.join("staged.txt"), "user stash content\n").unwrap();
    git(&fixture.repo, ["stash", "push", "-m", "personal work"]);
    fs::write(fixture.repo.join("unstaged.txt"), "checkpoint content\n").unwrap();
    fixture.capture_restore();
}

#[test]
fn clean_worktree_checkpoint() {
    let fixture = Fixture::new();
    fixture.capture_restore();
}

#[test]
fn corrupted_bundle_is_rejected_without_touching_checkout() {
    let fixture = Fixture::new();
    fs::write(fixture.repo.join("unstaged.txt"), "checkpoint content\n").unwrap();
    let artifact = capture(&fixture.repo, "wrk_test", &fixture.artifacts).unwrap();
    let restored = fixture.root.path().join("corrupt-restore");
    git(
        fixture.root.path(),
        ["clone", path(&fixture.remote), path(&restored)],
    );
    fs::write(&artifact.bundle_path, b"not a bundle").unwrap();
    let clean = exact_state(&restored);
    let error = restore(&restored, &artifact.metadata_path, &artifact.bundle_path).unwrap_err();
    assert!(error.to_string().contains("SHA-256 mismatch"));
    assert_eq!(exact_state(&restored), clean);
}

#[test]
fn restore_replaces_checkout_head() {
    let fixture = Fixture::new();
    fs::write(fixture.repo.join("unstaged.txt"), "checkpoint content\n").unwrap();
    let artifact = capture(&fixture.repo, "wrk_test", &fixture.artifacts).unwrap();
    let restored = fixture.root.path().join("wrong-head");
    git(
        fixture.root.path(),
        ["clone", path(&fixture.remote), path(&restored)],
    );
    configure_identity(&restored);
    fs::write(restored.join("later.txt"), "later\n").unwrap();
    git(&restored, ["add", "later.txt"]);
    git(&restored, ["commit", "-m", "later"]);
    restore(&restored, &artifact.metadata_path, &artifact.bundle_path).unwrap();
    assert_eq!(
        String::from_utf8(output(&restored, ["rev-parse", "HEAD"]))
            .unwrap()
            .trim(),
        artifact.metadata.head
    );
}

#[test]
fn local_commit_absent_from_origin_is_self_contained() {
    let fixture = Fixture::new();
    fs::write(fixture.repo.join("local-commit.txt"), "local commit\n").unwrap();
    git(&fixture.repo, ["add", "local-commit.txt"]);
    git(&fixture.repo, ["commit", "-m", "local only"]);
    fs::write(
        fixture.repo.join("unstaged.txt"),
        "dirty after local commit\n",
    )
    .unwrap();
    let before = exact_state(&fixture.repo);
    let artifact = capture(&fixture.repo, "wrk_local", &fixture.artifacts).unwrap();
    assert!(
        !command(&fixture.remote, ["cat-file", "-e", &artifact.metadata.head])
            .status
            .success(),
        "local checkpoint HEAD unexpectedly exists in origin"
    );

    fs::remove_dir_all(&fixture.repo).unwrap();
    let restored = fixture.root.path().join("local-restored");
    git(
        fixture.root.path(),
        ["clone", path(&fixture.remote), path(&restored)],
    );
    configure_identity(&restored);
    restore(&restored, &artifact.metadata_path, &artifact.bundle_path).unwrap();
    assert_eq!(exact_state(&restored), before);
    assert_eq!(
        String::from_utf8(output(&restored, ["rev-parse", "HEAD"]))
            .unwrap()
            .trim(),
        artifact.metadata.head
    );
}

fn exact_state(repo: &Path) -> ExactState {
    let mut files = Vec::new();
    collect_files(repo, repo, &mut files);
    files.sort_by(|left, right| left.0.cmp(&right.0));
    ExactState {
        status: output(
            repo,
            ["status", "--porcelain=v2", "-z", "--untracked-files=all"],
        ),
        cached_diff: output(repo, ["diff", "--binary", "--cached", "--no-ext-diff"]),
        worktree_diff: output(repo, ["diff", "--binary", "--no-ext-diff"]),
        files,
    }
}

fn collect_files(root: &Path, directory: &Path, files: &mut Vec<(PathBuf, Vec<u8>, u32)>) {
    for entry in fs::read_dir(directory).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        if path == root.join(".git") {
            continue;
        }
        let metadata = fs::symlink_metadata(&path).unwrap();
        if metadata.is_dir() {
            collect_files(root, &path, files);
        } else if metadata.is_file() {
            files.push((
                path.strip_prefix(root).unwrap().to_owned(),
                fs::read(&path).unwrap(),
                metadata.permissions().mode() & 0o111,
            ));
        }
    }
}

fn configure_identity(repo: &Path) {
    git(repo, ["config", "user.name", "Checkpoint Test"]);
    git(repo, ["config", "user.email", "checkpoint@example.invalid"]);
}

fn git<I, S>(repo: &Path, args: I)
where
    I: IntoIterator<Item = S>,
    S: AsRef<std::ffi::OsStr>,
{
    let result = command(repo, args);
    assert!(
        result.status.success(),
        "git failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
}

fn output<I, S>(repo: &Path, args: I) -> Vec<u8>
where
    I: IntoIterator<Item = S>,
    S: AsRef<std::ffi::OsStr>,
{
    let result = command(repo, args);
    assert!(
        result.status.success(),
        "git failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    result.stdout
}

fn command<I, S>(repo: &Path, args: I) -> Output
where
    I: IntoIterator<Item = S>,
    S: AsRef<std::ffi::OsStr>,
{
    Command::new("git")
        .args(args)
        .current_dir(repo)
        .env("LC_ALL", "C")
        .output()
        .unwrap()
}

fn path(value: &Path) -> &str {
    value.to_str().unwrap()
}
