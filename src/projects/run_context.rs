//! Per-run project-root inheritance via a `tokio::task_local`.
//!
//! The desktop Panel's "Enter Project" flow sets `RunRequest.workspace_override`
//! when the user picks a folder. Round 1 wired that into the agent loop's
//! own tool dispatch (via `ToolContext::from_workspace`), but child runs
//! spawned mid-loop — `session.send`, the team dispatcher's worker tasks,
//! etc. — had no easy way to read the parent's project context.
//!
//! Threading the field through every tool's constructor would force a
//! signature change on every builtin; instead we publish it as a
//! task-local that lives for the duration of the `run_loop` call. Children
//! invoked synchronously (`tokio::time::timeout`, normal await chains)
//! see the value; children spawned via `tokio::spawn` MUST capture it
//! before the spawn boundary (use [`current`] at the call site, then
//! feed the captured `PathBuf` back into the new `RunRequest`).
//!
//! Best-effort everywhere: callers outside any `scope` get `None` and the
//! sub-run falls back to its agent's default workspace — exactly the
//! pre-project-mode behaviour.

use std::path::PathBuf;

tokio::task_local! {
    static CURRENT_PROJECT_ROOT: Option<PathBuf>;
}

/// Run `fut` with the given project root visible to [`current`] for the
/// lifetime of the future. The scope is bounded by the future, so once it
/// resolves the task-local goes back to whatever the parent stack had.
pub async fn with_project_root<F, T>(root: Option<PathBuf>, fut: F) -> T
where
    F: std::future::Future<Output = T>,
{
    CURRENT_PROJECT_ROOT.scope(root, fut).await
}

/// Read the active project root, if any. Returns `None` outside a
/// [`with_project_root`] scope or when the surrounding scope explicitly
/// set `None` (non-project run).
#[must_use]
pub fn current() -> Option<PathBuf> {
    CURRENT_PROJECT_ROOT.try_with(|p| p.clone()).ok().flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn current_is_none_outside_any_scope() {
        assert!(current().is_none());
    }

    #[tokio::test]
    async fn current_returns_set_value_inside_scope() {
        let want = PathBuf::from("/tmp/aleph-project-fixture");
        let observed = with_project_root(Some(want.clone()), async { current() }).await;
        assert_eq!(observed, Some(want));
    }

    #[tokio::test]
    async fn scope_with_none_overrides_outer_some() {
        let outer = PathBuf::from("/outer");
        let inner = with_project_root(Some(outer), async {
            with_project_root(None, async { current() }).await
        })
        .await;
        assert_eq!(inner, None, "inner None must shadow outer Some");
    }

    /// Local scope is dropped after the wrapped future resolves so the
    /// outer scope's value comes back into view.
    #[tokio::test]
    async fn scope_pops_on_future_completion() {
        let outer = PathBuf::from("/outer");
        let outer_clone = outer.clone();
        let observed = with_project_root(Some(outer.clone()), async move {
            let inside_inner =
                with_project_root(Some(PathBuf::from("/inner")), async { current() }).await;
            (inside_inner, current())
        })
        .await;
        assert_eq!(observed.0, Some(PathBuf::from("/inner")));
        assert_eq!(observed.1, Some(outer_clone));
    }

    /// `tokio::spawn` creates a new task and the local does NOT propagate.
    /// This is the documented gotcha — call [`current`] BEFORE spawning
    /// and pass the captured value into the spawned future explicitly.
    #[tokio::test]
    async fn task_local_does_not_cross_spawn_boundary() {
        let want = PathBuf::from("/tmp/scoped");
        let observed = with_project_root(Some(want.clone()), async {
            tokio::spawn(async { current() }).await.unwrap()
        })
        .await;
        assert_eq!(observed, None);
    }
}
