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
use crate::utils::no_window::NoWindow;

/// RAII handle to a git worktree. Call `cleanup()` to remove it; `Drop` is the safety net.
pub struct WorktreeHandle {
    path: PathBuf,
    repo_root: PathBuf,
    cleaned_up: Arc<AtomicBool>,
    trace_sink: Option<Arc<dyn TraceSink>>,
    parent: Option<Arc<Self>>,
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
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    #[must_use]
    pub fn repo_root(&self) -> &Path {
        &self.repo_root
    }

    pub(crate) fn exec_workspace(self: &Arc<Self>) -> super::context::ExecWorkspace {
        super::context::ExecWorkspace::OwnedWorktree(Arc::clone(self))
    }

    /// Nested checkout cleanup uses Git through its parent's checkout.
    pub(crate) fn retain_parent(&mut self, parent: Arc<Self>) {
        self.parent = Some(parent);
    }

    #[cfg(test)]
    pub(crate) fn fixture(path: PathBuf, repo_root: PathBuf) -> Self {
        Self {
            path,
            repo_root,
            cleaned_up: Arc::new(AtomicBool::new(true)),
            trace_sink: None,
            parent: None,
        }
    }

    /// Explicit cleanup. Removes the worktree via `git worktree remove --force`,
    /// then marks the handle as cleaned up so `Drop` skips its safety-net work.
    /// Performance contract: one `git worktree remove`, nothing on top — the
    /// wall clock is git deleting the checkout and scales with repo size and
    /// disk, so `h_t5` asserts it against a measured raw-git floor, not a
    /// constant.
    pub async fn cleanup(self) -> Result<(), WorktreeError> {
        let result = remove_worktree(&self.repo_root, &self.path).await;
        self.cleaned_up.store(result.is_ok(), Ordering::Release);

        if let Some(sink) = self.trace_sink.as_ref() {
            sink.on_trace(&crate::harness::trace::LoopTraceEvent::WorktreeCleanedUp {
                // rust-doctor-disable-next-line excessive-clone
                path: self.path.clone(),
                leaked: false,
            });
        }

        result
    }
}

impl Drop for WorktreeHandle {
    fn drop(&mut self) {
        if self.cleaned_up.load(Ordering::Acquire) {
            return;
        }
        // Safety net: hand the `git worktree remove --force` to the tokio
        // blocking pool so the calling executor thread is not stalled by a
        // synchronous child process. The JoinHandle is intentionally dropped
        // because `Drop` itself is sync — the blocking task is fire-and-forget
        // and logs its own result. If no runtime is reachable (e.g. very
        // late shutdown), fall back to a plain std::thread so we still try
        // the cleanup once. We never panic from Drop.
        // rust-doctor-disable-next-line excessive-clone
        let repo_root = self.repo_root.clone();
        // rust-doctor-disable-next-line excessive-clone
        let path = self.path.clone();
        // rust-doctor-disable-next-line excessive-clone
        let trace_sink = self.trace_sink.clone();
        tracing::error!(
            path = %path.display(),
            "WorktreeHandle leaked — Drop safety-net removing"
        );
        if let Some(sink) = trace_sink.as_ref() {
            sink.on_trace(&crate::harness::trace::LoopTraceEvent::WorktreeCleanedUp {
                // rust-doctor-disable-next-line excessive-clone
                path: path.clone(),
                leaked: true,
            });
        }
        let parent = self.parent.take();
        let outcome = move || {
            let _parent = parent;
            let result = std::process::Command::new("git")
                .arg("-C")
                .arg(&repo_root)
                .arg("worktree")
                .arg("remove")
                .arg("--force")
                .arg(&path)
                .status();
            match result {
                Ok(status) if status.success() => {}
                Ok(status) => {
                    tracing::error!(
                        path = %path.display(),
                        code = ?status.code(),
                        "Drop safety-net cleanup returned non-zero exit code"
                    );
                }
                Err(e) => {
                    tracing::error!(
                        path = %path.display(),
                        error = %e,
                        "Drop safety-net cleanup failed"
                    );
                }
            }
        };
        if tokio::runtime::Handle::try_current().is_ok() {
            tokio::task::spawn_blocking(outcome);
        } else {
            // Last-ditch fallback when the runtime has already been dropped.
            // std::thread::spawn is fine here because Drop already runs
            // off the executor; we just need the work off the current call.
            let _ = std::thread::Builder::new()
                .name("aleph-worktree-cleanup".into())
                .spawn(outcome);
        }
        let _ = trace_sink;
    }
}

/// Create a fresh detached-HEAD worktree under `$TMPDIR/aleph-subagent-<label>-<uuid>/`.
///
/// Performance contract: one `git worktree add`, nothing on top — see
/// [`WorktreeHandle::cleanup`] for why that is stated relatively.
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
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '-'
            }
        })
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
        .no_window()
        .output()
        .await
        .map_err(|e| WorktreeError::Create(format!("spawn git: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        return Err(WorktreeError::Create(stderr));
    }

    if let Some(sink) = trace_sink.as_ref() {
        sink.on_trace(&crate::harness::trace::LoopTraceEvent::WorktreeCreated {
            // rust-doctor-disable-next-line excessive-clone
            path: path.clone(),
        });
    }

    Ok(WorktreeHandle {
        path,
        repo_root: repo_root.to_path_buf(),
        cleaned_up: Arc::new(AtomicBool::new(false)),
        trace_sink,
        parent: None,
    })
}

async fn remove_worktree(repo_root: &Path, path: &Path) -> Result<(), WorktreeError> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo_root)
        .arg("worktree")
        .arg("remove")
        .arg("--force")
        .arg(path)
        .no_window()
        .output()
        .await
        .map_err(|e| WorktreeError::Cleanup {
            path: path.to_path_buf(),
            message: format!("spawn git: {e}"),
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        return Err(WorktreeError::Cleanup {
            path: path.to_path_buf(),
            message: stderr,
        });
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // Serialize the tests that create real git worktrees in *this* repo:
    // concurrent `git worktree add`/`remove` contend on the same `.git` locks,
    // which under heavy load (the `test-proptest` stage) makes those git
    // invocations slow or flaky. One shared mutex makes the shared-repo fixture
    // a serial section; the pure-logic tests below don't take it and still run
    // in parallel.
    static WORKTREE_REPO_SERIAL: std::sync::LazyLock<tokio::sync::Mutex<()>> =
        std::sync::LazyLock::new(|| tokio::sync::Mutex::new(()));

    #[test]
    fn worktree_error_displays_create_message() {
        let e = WorktreeError::Create("git command not found".into());
        assert!(format!("{e}").contains("git worktree add failed"));
        assert!(format!("{e}").contains("git command not found"));
    }

    #[tokio::test]
    async fn create_succeeds_in_a_git_repo() {
        let _serial = WORKTREE_REPO_SERIAL.lock().await;
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
        h.cleanup().await.expect("cleanup");
    }

    #[tokio::test]
    async fn cleanup_removes_worktree_dir() {
        let _serial = WORKTREE_REPO_SERIAL.lock().await;
        let repo_root = std::env::current_dir().expect("cwd");
        let h = create(&repo_root, "task4-cleanup", None)
            .await
            .expect("create");
        let path = h.path().to_path_buf();
        assert!(path.exists(), "precondition: dir exists");
        h.cleanup().await.expect("cleanup");
        assert!(!path.exists(), "cleanup must remove worktree dir");
    }

    #[tokio::test]
    async fn create_in_non_git_dir_fails_with_not_a_git_repo() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let err = create(tmp.path(), "task3-non-git", None)
            .await
            .expect_err("must fail outside git repo");
        assert!(matches!(err, WorktreeError::NotAGitRepo(_)), "got {err:?}");
    }

    #[tokio::test]
    async fn owned_worktree_survives_owner_timeout_until_harness_drops_binding() {
        let _serial = WORKTREE_REPO_SERIAL.lock().await;
        let repo_root = std::env::current_dir().expect("cwd");
        let handle = Arc::new(create(&repo_root, "active-checkout", None).await.unwrap());
        let path = handle.path().to_path_buf();
        let binding = handle.exec_workspace();
        let (started, ready) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(super::super::context::with_exec_workspace_binding(
            Some(binding),
            async move {
                started.send(()).unwrap();
                std::future::pending::<()>().await;
            },
        ));
        ready.await.unwrap();

        assert!(Arc::try_unwrap(handle).is_err());
        assert!(
            path.join(".git").is_file(),
            "a live harness still owns the checkout"
        );
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        tokio::time::timeout(std::time::Duration::from_secs(30), async {
            while path.exists() {
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
        })
        .await
        .expect("last owner releases the checkout for removal");
    }

    #[tokio::test]
    async fn drop_without_cleanup_logs_and_removes_dir() {
        let _serial = WORKTREE_REPO_SERIAL.lock().await;
        let repo_root = std::env::current_dir().expect("cwd");
        let path = {
            let h = create(&repo_root, "task5-drop", None)
                .await
                .expect("create");
            h.path().to_path_buf()
            // h dropped here without cleanup() called
        };
        // Drop spawns a detached `git worktree remove` thread (fire-and-forget,
        // best-effort). Under heavy parallel load — e.g. the `test-proptest`
        // stage saturating every core while sibling worktree tests contend on
        // this repo's `.git` locks — that git invocation can take many seconds.
        // Poll with a generous ceiling; the early break keeps the common case
        // sub-second, so the high bound only buys patience under contention.
        for _ in 0..300 {
            if !path.exists() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        assert!(
            !path.exists(),
            "Drop safety-net must remove leaked worktree at {path:?}"
        );
    }
}
