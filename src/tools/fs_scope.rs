//! Per-run filesystem scope via a `tokio::task_local`.
//!
//! Concurrent runs (multi-channel sessions, subagents, team member tasks)
//! historically shared two process-global path anchors:
//!
//! * the `ToolContextHandle` written by every `run_agent_loop` invocation —
//!   so two concurrent runs with different workspaces clobbered each other's
//!   relative-path resolution mid-run; and
//! * nothing at all for worktree-isolated agents — `WorktreeSandbox` redirects
//!   *command* execution, but `file_read` / `file_write` / `file_edit` /
//!   `apply_patch` kept resolving paths against the parent repo, silently
//!   escaping the isolation boundary.
//!
//! `FsScope` closes both holes the same way `projects::run_context` solved
//! project-root inheritance: a task-local published for the duration of the
//! run. File tools consult it at their single path-resolution chokepoint
//! ([`crate::builtin_tools::file_ops::check_and_resolve_path`]), so no tool
//! constructor signature changes.
//!
//! Children invoked synchronously (normal await chains, `tokio::time::timeout`)
//! see the value; children spawned via `tokio::spawn` MUST re-publish their own
//! scope inside the spawned task (the subagent spawner does this — the scope is
//! created inside `spawn()` around the harness future, so it is visible
//! regardless of which task `spawn()` itself runs in).
//!
//! Best-effort everywhere: callers outside any scope get `None` and path
//! resolution falls back to the shared `ToolContextHandle` — exactly the
//! pre-scope behaviour.

use std::path::PathBuf;

/// Filesystem scope for the current run.
#[derive(Debug, Clone)]
pub struct FsScope {
    /// Base directory for resolving *relative* tool paths. For normal runs
    /// this is the workspace artifact dir (`<workspace>/output/documents`,
    /// matching the `ToolContext` convention); for worktree-isolated agents
    /// it is the worktree root (a repo checkout, so `src/foo.rs` means
    /// `<worktree>/src/foo.rs`).
    pub base: PathBuf,
    /// Optional absolute-path remap `(from, to)`. A canonical path under
    /// `from` is rebased onto `to` before the deny check. Worktree isolation
    /// uses this to redirect parent-repo absolute paths (the parent naturally
    /// phrases tasks in its own paths) into the isolated checkout — mirroring
    /// what `WorktreeSandbox` already does for command execution.
    pub rebase: Option<(PathBuf, PathBuf)>,
}

impl FsScope {
    /// Scope for a normal (non-isolated) run: relative paths land in the
    /// workspace artifact directory, no absolute remap.
    #[must_use]
    pub fn workspace(base: PathBuf) -> Self {
        Self { base, rebase: None }
    }

    /// Scope for a worktree-isolated agent: relative paths resolve at the
    /// worktree root and parent-repo absolute paths are rebased into it.
    #[must_use]
    pub fn worktree(worktree_root: PathBuf, repo_root: PathBuf) -> Self {
        Self {
            base: worktree_root.clone(),
            rebase: Some((repo_root, worktree_root)),
        }
    }

    /// Apply the rebase mapping to an already-canonicalized path. Returns the
    /// remapped path when `path` is under `rebase.from`; `None` otherwise
    /// (including when no rebase is configured).
    #[must_use]
    pub fn rebase_path(&self, path: &std::path::Path) -> Option<PathBuf> {
        let (from, to) = self.rebase.as_ref()?;
        let rel = path.strip_prefix(from).ok()?;
        Some(to.join(rel))
    }
}

tokio::task_local! {
    static CURRENT_FS_SCOPE: Option<FsScope>;
}

/// Run `fut` with the given scope visible to [`current`] for the lifetime of
/// the future. The scope pops when the future resolves, restoring whatever
/// the parent stack had.
pub async fn with_fs_scope<F, T>(scope: Option<FsScope>, fut: F) -> T
where
    F: std::future::Future<Output = T>,
{
    CURRENT_FS_SCOPE.scope(scope, fut).await
}

/// Read the active filesystem scope, if any. Returns `None` outside a
/// [`with_fs_scope`] scope or when the surrounding scope explicitly set
/// `None`.
#[must_use]
pub fn current() -> Option<FsScope> {
    CURRENT_FS_SCOPE.try_with(|s| s.clone()).ok().flatten()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[tokio::test]
    async fn current_is_none_outside_any_scope() {
        assert!(current().is_none());
    }

    #[tokio::test]
    async fn current_returns_set_value_inside_scope() {
        let scope = FsScope::workspace(PathBuf::from("/tmp/ws/output/documents"));
        let observed = with_fs_scope(Some(scope), async { current() }).await;
        assert_eq!(
            observed.map(|s| s.base),
            Some(PathBuf::from("/tmp/ws/output/documents"))
        );
    }

    #[tokio::test]
    async fn inner_none_shadows_outer_some() {
        let outer = FsScope::workspace(PathBuf::from("/outer"));
        let inner = with_fs_scope(Some(outer), async {
            with_fs_scope(None, async { current() }).await
        })
        .await;
        assert!(inner.is_none(), "inner None must shadow outer Some");
    }

    #[tokio::test]
    async fn scope_pops_on_future_completion() {
        let outer = FsScope::workspace(PathBuf::from("/outer"));
        let observed = with_fs_scope(Some(outer), async {
            let inner_base =
                with_fs_scope(Some(FsScope::workspace(PathBuf::from("/inner"))), async {
                    current().map(|s| s.base)
                })
                .await;
            (inner_base, current().map(|s| s.base))
        })
        .await;
        assert_eq!(observed.0, Some(PathBuf::from("/inner")));
        assert_eq!(observed.1, Some(PathBuf::from("/outer")));
    }

    /// `tokio::spawn` starts a fresh task; the local does NOT propagate.
    /// Spawned children must publish their own scope (the subagent spawner
    /// creates its scope inside the spawned future).
    #[tokio::test]
    async fn task_local_does_not_cross_spawn_boundary() {
        let scope = FsScope::workspace(PathBuf::from("/scoped"));
        let observed = with_fs_scope(Some(scope), async {
            tokio::spawn(async { current() }).await.unwrap()
        })
        .await;
        assert!(observed.is_none());
    }

    #[test]
    fn rebase_maps_paths_under_from() {
        let scope = FsScope::worktree(PathBuf::from("/tmp/wt"), PathBuf::from("/repo"));
        assert_eq!(
            scope.rebase_path(Path::new("/repo/src/a.rs")),
            Some(PathBuf::from("/tmp/wt/src/a.rs"))
        );
        // The repo root itself maps to the worktree root.
        assert_eq!(
            scope.rebase_path(Path::new("/repo")),
            Some(PathBuf::from("/tmp/wt"))
        );
    }

    #[test]
    fn rebase_leaves_unrelated_paths_alone() {
        let scope = FsScope::worktree(PathBuf::from("/tmp/wt"), PathBuf::from("/repo"));
        assert_eq!(scope.rebase_path(Path::new("/elsewhere/x.rs")), None);
        // Sibling sharing a string prefix is NOT under the repo root.
        assert_eq!(scope.rebase_path(Path::new("/repo-other/x.rs")), None);
        // Paths already inside the worktree are untouched.
        assert_eq!(scope.rebase_path(Path::new("/tmp/wt/src/a.rs")), None);
    }

    #[test]
    fn workspace_scope_has_no_rebase() {
        let scope = FsScope::workspace(PathBuf::from("/ws"));
        assert_eq!(scope.rebase_path(Path::new("/ws/file")), None);
    }
}
