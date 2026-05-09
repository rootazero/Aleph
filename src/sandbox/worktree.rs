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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worktree_error_displays_create_message() {
        let e = WorktreeError::Create("git command not found".into());
        assert!(format!("{e}").contains("git worktree add failed"));
        assert!(format!("{e}").contains("git command not found"));
    }
}
