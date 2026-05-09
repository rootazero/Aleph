//! Git worktree isolation primitives for subagent strict isolation (P3 Stage H).
//!
//! `WorktreeHandle::create` provisions a fresh detached-HEAD worktree under
//! `$TMPDIR/aleph-subagent-<label>-<uuid>/`. Cleanup is RAII-guarded:
//! `cleanup()` is the explicit happy path; `Drop` is the safety net.
//!
//! See: docs/superpowers/specs/2026-05-09-subagent-uplift-p3-design.md § 2

use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum WorktreeError {
    #[error("git worktree add failed: {0}")]
    Create(String),
    #[error("git worktree remove failed at {path}: {message}")]
    Cleanup { path: PathBuf, message: String },
    #[error("io error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("not a git repository: {0}")]
    NotAGitRepo(PathBuf),
}

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::process::Command;

use crate::harness::TraceSink;

/// RAII handle to a git worktree. Call `cleanup()` to remove it; `Drop` is the safety net.
pub struct WorktreeHandle {
    path: PathBuf,
    repo_root: PathBuf,
    cleaned_up: Arc<AtomicBool>,
    trace_sink: Option<Arc<dyn TraceSink>>,
}

impl std::fmt::Debug for WorktreeHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorktreeHandle")
            .field("path", &self.path)
            .field("repo_root", &self.repo_root)
            .field("cleaned_up", &self.cleaned_up.load(Ordering::Relaxed))
            .finish()
    }
}

impl WorktreeHandle {
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn repo_root(&self) -> &Path {
        &self.repo_root
    }
}

/// Create a fresh detached-HEAD worktree under `$TMPDIR/aleph-subagent-<label>-<uuid>/`.
///
/// Performance contract: ≤ 200ms typical (git worktree add).
/// Errors: `NotAGitRepo` if `repo_root` has no `.git`; `Create` for any git failure.
pub async fn create(
    repo_root: &Path,
    label: &str,
    trace_sink: Option<Arc<dyn TraceSink>>,
) -> Result<WorktreeHandle, WorktreeError> {
    if !repo_root.join(".git").exists() {
        return Err(WorktreeError::NotAGitRepo(repo_root.to_path_buf()));
    }

    let id = uuid::Uuid::new_v4();
    let safe_label: String = label
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '-' })
        .collect();
    let path = std::env::temp_dir().join(format!("aleph-subagent-{safe_label}-{id}"));

    let output = Command::new("git")
        .arg("-C")
        .arg(repo_root)
        .arg("worktree")
        .arg("add")
        .arg("--detach")
        .arg(&path)
        .arg("HEAD")
        .output()
        .await
        .map_err(|e| WorktreeError::Create(format!("spawn git: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        return Err(WorktreeError::Create(stderr));
    }

    // Trace event added in Task 6; placeholder here keeps signature stable.
    let _ = trace_sink.as_ref();

    Ok(WorktreeHandle {
        path,
        repo_root: repo_root.to_path_buf(),
        cleaned_up: Arc::new(AtomicBool::new(false)),
        trace_sink,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worktree_error_displays_create_message() {
        let e = WorktreeError::Create("git command not found".into());
        assert!(format!("{e}").contains("git worktree add failed"));
        assert!(format!("{e}").contains("git command not found"));
    }

    #[tokio::test]
    async fn create_succeeds_in_a_git_repo() {
        let repo_root = std::env::current_dir().expect("cwd");
        // Aleph repo is itself a git repo; safe to use as parent.
        let h = create(&repo_root, "task3-create", None)
            .await
            .expect("create");
        assert!(h.path().exists(), "worktree dir should exist");
        assert!(
            h.path().join(".git").exists(),
            "worktree must have .git pointer"
        );
        // Cleanup so this test does not leak.
        // We don't have cleanup() yet (Task 4); use raw git command for now.
        let _ = tokio::process::Command::new("git")
            .arg("-C")
            .arg(&repo_root)
            .arg("worktree")
            .arg("remove")
            .arg("--force")
            .arg(h.path())
            .output()
            .await;
    }

    #[tokio::test]
    async fn create_in_non_git_dir_fails_with_not_a_git_repo() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let err = create(tmp.path(), "task3-non-git", None)
            .await
            .expect_err("must fail outside git repo");
        assert!(
            matches!(err, WorktreeError::NotAGitRepo(_)),
            "got {err:?}"
        );
    }
}
