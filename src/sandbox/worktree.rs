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

    /// Explicit cleanup. Removes the worktree via `git worktree remove --force`,
    /// then marks the handle as cleaned up so `Drop` skips its safety-net work.
    /// Performance contract: ≤ 100ms typical.
    pub async fn cleanup(self) -> Result<(), WorktreeError> {
        let result = remove_worktree(&self.repo_root, &self.path).await;
        self.cleaned_up.store(true, Ordering::Release);

        if let Some(sink) = self.trace_sink.as_ref() {
            sink.on_trace(&crate::harness::trace::LoopTraceEvent::WorktreeCleanedUp {
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
        // Safety net: spawn blocking task to run `git worktree remove --force`.
        // Errors are logged via tracing; we never panic from Drop.
        let repo_root = self.repo_root.clone();
        let path = self.path.clone();
        tracing::error!(
            path = %path.display(),
            "WorktreeHandle leaked — Drop safety-net removing"
        );
        if let Some(sink) = self.trace_sink.as_ref() {
            sink.on_trace(&crate::harness::trace::LoopTraceEvent::WorktreeCleanedUp {
                path: self.path.clone(),
                leaked: true,
            });
        }
        std::thread::spawn(move || {
            let status = std::process::Command::new("git")
                .arg("-C")
                .arg(&repo_root)
                .arg("worktree")
                .arg("remove")
                .arg("--force")
                .arg(&path)
                .no_window()
                .status();
            if let Err(e) = status {
                tracing::error!(
                    path = %path.display(),
                    error = %e,
                    "Drop safety-net cleanup failed"
                );
            }
        });
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
            path: path.clone(),
        });
    }

    Ok(WorktreeHandle {
        path,
        repo_root: repo_root.to_path_buf(),
        cleaned_up: Arc::new(AtomicBool::new(false)),
        trace_sink,
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

/// Minimal Sandbox impl for Stage H — runs commands at worktree path with
/// `CARGO_TARGET_DIR=<worktree>/target` injected. No seatbelt, no capability
/// enforcement (Stage H scope is workspace isolation only — see § 2.2.1
/// Architectural Scope Lock).
pub struct WorktreeSandbox {
    worktree_path: std::path::PathBuf,
}

impl WorktreeSandbox {
    #[must_use]
    pub const fn new(worktree_path: std::path::PathBuf) -> Self {
        Self { worktree_path }
    }
}

#[async_trait::async_trait]
impl crate::sandbox::Sandbox for WorktreeSandbox {
    fn summary(&self) -> Option<crate::sandbox::summary::SandboxSummary> {
        // Worktree isolation is workspace-tree only — there is no OS-level
        // process sandbox layered on top. The LLM should know this so it
        // does not assume seatbelt/landlock enforcement when a subagent
        // delegates here.
        Some(crate::sandbox::summary::SandboxSummary::isolated_worktree(
            self.worktree_path.clone(),
        ))
    }

    async fn execute(
        &self,
        command: crate::sandbox::SandboxCommand,
    ) -> Result<crate::sandbox::SandboxOutput, crate::sandbox::SandboxError> {
        let started = std::time::Instant::now();

        let mut cmd = tokio::process::Command::new(&command.program);
        cmd.args(&command.args)
            .current_dir(&self.worktree_path)
            .envs(command.env.iter())
            .env("CARGO_TARGET_DIR", self.worktree_path.join("target"));

        let exec = if let Some(timeout) = command.timeout {
            match tokio::time::timeout(timeout, cmd.no_window().output()).await {
                Ok(Ok(out)) => out,
                Ok(Err(e)) => return Err(crate::sandbox::SandboxError::Io(e.to_string())),
                Err(_) => {
                    // Worktree-isolated commands use `cmd.output()` which
                    // can't surface partial stdout/stderr on timeout — the
                    // future is dropped before we can split the pipes.
                    // Treat both partial buffers as empty here; callers
                    // that need partial output should go through the
                    // sandbox driver path which uses run_child_with_drain.
                    return Err(crate::sandbox::SandboxError::Timeout {
                        elapsed_ms: started.elapsed().as_millis().try_into().unwrap_or(u64::MAX),
                        partial_stdout: Vec::new(),
                        partial_stderr: Vec::new(),
                    });
                }
            }
        } else {
            cmd.no_window()
                .output()
                .await
                .map_err(|e| crate::sandbox::SandboxError::Io(e.to_string()))?
        };

        #[cfg(unix)]
        let signal = {
            use std::os::unix::process::ExitStatusExt;
            exec.status.signal()
        };
        #[cfg(not(unix))]
        let signal: Option<i32> = None;

        Ok(crate::sandbox::SandboxOutput {
            stdout: exec.stdout,
            stderr: exec.stderr,
            exit_code: exec.status.code(),
            signal,
            truncated: false,
            stdout_truncated_bytes: 0,
            stderr_truncated_bytes: 0,
            duration_ms: started.elapsed().as_millis().try_into().unwrap_or(u64::MAX),
        })
    }
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
        h.cleanup().await.expect("cleanup");
    }

    #[tokio::test]
    async fn cleanup_removes_worktree_dir() {
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
    async fn drop_without_cleanup_logs_and_removes_dir() {
        let repo_root = std::env::current_dir().expect("cwd");
        let path = {
            let h = create(&repo_root, "task5-drop", None)
                .await
                .expect("create");
            h.path().to_path_buf()
            // h dropped here without cleanup() called
        };
        // Drop spawns blocking removal; allow time for it.
        for _ in 0..50 {
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

    #[tokio::test]
    async fn worktree_sandbox_executes_at_worktree_path() {
        let repo_root = std::env::current_dir().expect("cwd");
        let h = create(&repo_root, "task7-sandbox", None)
            .await
            .expect("create");
        let expected_path = h.path().to_path_buf();
        let sandbox = WorktreeSandbox::new(expected_path.clone());

        let cmd = crate::sandbox::SandboxCommand {
            session_id: crate::session::service::SessionId::main("task7-sandbox-test"),
            program: "pwd".into(),
            args: vec![],
            env: std::collections::HashMap::new(),
            stdin: None,
            cwd: None,
            capabilities: crate::sandbox::SandboxCapabilities::default(),
            timeout: None,
        };
        use crate::sandbox::Sandbox as _;
        let out = sandbox.execute(cmd).await.expect("execute");

        let stdout_str = String::from_utf8_lossy(&out.stdout);
        let actual = stdout_str.trim();
        // The test fixture passes if pwd's output ends with the worktree
        // dirname OR exactly equals the canonicalized path. macOS resolves
        // /var/.../T to /private/var/.../T so we accept both shapes.
        let expected_basename = expected_path.file_name().unwrap().to_str().unwrap();
        assert!(
            actual.ends_with(expected_basename) || actual == expected_path.to_string_lossy(),
            "pwd output {actual:?} should match worktree path {expected_path:?}"
        );

        h.cleanup().await.expect("cleanup");
    }
}
