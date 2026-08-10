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
use std::time::Duration;
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
    /// Performance contract: one `git worktree remove`, nothing on top — the
    /// wall clock is git deleting the checkout and scales with repo size and
    /// disk, so `h_t5` asserts it against a measured raw-git floor, not a
    /// constant.
    pub async fn cleanup(self) -> Result<(), WorktreeError> {
        let result = remove_worktree(&self.repo_root, &self.path).await;
        self.cleaned_up.store(true, Ordering::Release);

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
        // Safety net: spawn blocking task to run `git worktree remove --force`.
        // Errors are logged via tracing; we never panic from Drop.
        // rust-doctor-disable-next-line excessive-clone
        let repo_root = self.repo_root.clone();
        // rust-doctor-disable-next-line excessive-clone
        let path = self.path.clone();
        tracing::error!(
            path = %path.display(),
            "WorktreeHandle leaked — Drop safety-net removing"
        );
        if let Some(sink) = self.trace_sink.as_ref() {
            sink.on_trace(&crate::harness::trace::LoopTraceEvent::WorktreeCleanedUp {
                // rust-doctor-disable-next-line excessive-clone
                path: self.path.clone(),
                leaked: true,
            });
        }
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

/// Minimal Sandbox impl for Stage H — runs commands at the worktree path with
/// `CARGO_TARGET_DIR=<worktree>/target` injected. There is no OS-level process
/// sandbox (no seatbelt/landlock — Stage H scope is workspace isolation only,
/// see § 2.2.1 Architectural Scope Lock).
///
/// **Shared floor** (so a worktree-isolated subagent is not a hole in it): the
/// catastrophic command-policy **hardline** (fork bomb / disk wipe) runs before
/// exec, and the secret-scrub + block-class gate (`scrub::scrub_and_gate_output`)
/// runs on output — the same single-source-of-truth `WorkspaceSandbox` uses.
///
/// **NOT shared** (deliberate Stage-H scope): the operator-configurable *tunable*
/// `[sandbox.command_policy]` Block/Warn rules and the rate limiter that
/// `factory::build_sandbox` layers onto `WorkspaceSandbox` are not applied here —
/// only the non-negotiable hardline is. An operator relying on a *custom* Block
/// rule to gate a delegated subagent's commands should know it does not reach
/// this path; the catastrophic floor does.
pub struct WorktreeSandbox {
    worktree_path: std::path::PathBuf,
    hooks: crate::sandbox::hooks::SandboxHooks,
}

impl WorktreeSandbox {
    #[must_use]
    pub fn new(worktree_path: std::path::PathBuf) -> Self {
        // Only the non-negotiable catastrophic floor — no tunable command policy
        // is configured on this path. Mirrors the `hardline_only()` hook that
        // `factory::build_sandbox` installs when the tunable policy is disabled.
        let hooks = crate::sandbox::hooks::SandboxHooks::new().with_before(std::sync::Arc::new(
            crate::sandbox::command_policy::CommandPolicyHook::new(
                crate::sandbox::command_policy::CommandPolicy::hardline_only(),
            ),
        ));
        Self {
            worktree_path,
            hooks,
        }
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
            // rust-doctor-disable-next-line excessive-clone
            self.worktree_path.clone(),
        ))
    }

    async fn execute(
        &self,
        command: crate::sandbox::SandboxCommand,
    ) -> Result<crate::sandbox::SandboxOutput, crate::sandbox::SandboxError> {
        // Catastrophic command-policy hardline floor — holds even here, where no
        // OS sandbox is layered on. Without it a worktree-isolated subagent could
        // run `rm -rf /` / a fork bomb directly via `tokio::process`, which the
        // "undisableable floor holds under every tier" invariant forbids.
        if let crate::sandbox::hooks::SandboxHookResult::Deny { reason } = self
            .hooks
            .run_before(&crate::sandbox::hooks::SandboxHookContext::new(
                &command.program,
                &command,
            ))
            .await
        {
            return Err(crate::sandbox::SandboxError::Other(format!(
                "hook denied: {reason}"
            )));
        }

        let mut cmd = tokio::process::Command::new(&command.program);
        cmd.args(&command.args)
            .current_dir(&self.worktree_path)
            .envs(command.env.iter())
            .env("CARGO_TARGET_DIR", self.worktree_path.join("target"))
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);

        let child = cmd
            .spawn()
            .map_err(|e| crate::sandbox::SandboxError::Io(e.to_string()))?;

        // Default 1 MiB total budget, mirroring `WorkspaceSandbox::default`.
        const MAX_OUTPUT_BYTES: usize = 1024 * 1024;

        // ONE drain implementation, deliberately. This branch used to hand-roll
        // its own `read_to_end` pair for the no-timeout case: that copy read the
        // whole stream into memory *before* truncating (the shared drain bounds
        // it while reading, CWE-400), and — once background jobs grew a live
        // output tail — it was also the one drain path that tee'd nothing, so a
        // worktree-isolated job would have polled blind. Two defects from one
        // cause: a second host for the same behaviour.
        //
        // `timeout: None` means "no wall-clock limit", which the shared drain
        // expresses as a `Duration` rather than an `Option`. A year is not a
        // timeout anyone waits out; it is the sentinel for "unbounded", chosen
        // to stay far inside tokio's timer range instead of `Duration::MAX`,
        // which saturates.
        const NO_WALL_CLOCK_LIMIT: Duration = Duration::from_secs(365 * 24 * 60 * 60);
        let result = crate::sandbox::platforms::common::run_child_with_drain(
            child,
            command.stdin.as_deref(),
            command.timeout.unwrap_or(NO_WALL_CLOCK_LIMIT),
            MAX_OUTPUT_BYTES,
        )
        .await;

        let exec = match result {
            Ok(out) => out,
            Err(crate::sandbox::SandboxError::Timeout { .. }) => return result,
            Err(e) => return Err(e),
        };

        let mut out = exec;

        // Same output content floor as WorkspaceSandbox (single source of truth):
        // redact secrets, strip invisible/bidi controls, and fail closed on
        // block-class secret material rather than return it to the model.
        let blocked = crate::sandbox::scrub::scrub_and_gate_output(&mut out);
        if !blocked.is_empty() {
            return Err(crate::sandbox::SandboxError::Other(format!(
                "command output blocked: catastrophic secret material detected ({})",
                blocked.join(", ")
            )));
        }

        Ok(out)
    }
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

    #[tokio::test]
    async fn worktree_sandbox_executes_at_worktree_path() {
        let _serial = WORKTREE_REPO_SERIAL.lock().await;
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

    /// F3 end-to-end: a worktree-isolated subagent scopes its `WorktreeSandbox`
    /// as the exec-tool override, so a real `code_exec` shell command runs
    /// *inside the worktree checkout* with `CARGO_TARGET_DIR` redirected — not
    /// through the parent's construction-time sandbox. This is the exact path
    /// that was silently inert before the fix (the override reached only the
    /// never-read `HarnessDeps.sandbox`, so `bash`/`code_exec`/`code_check` kept
    /// using the parent's sandbox). Proves routing + real cwd + the
    /// `CARGO_TARGET_DIR` redirect (the last of which was previously untested).
    #[tokio::test]
    async fn isolated_subagent_command_runs_in_worktree_via_override() {
        use crate::builtin_tools::code_exec::{CodeExecArgs, CodeExecTool, Language};
        use crate::sandbox::context::{with_sandbox_override, SESSION_ID};
        use crate::sandbox::test_util::MockSandbox;
        use crate::sandbox::{Sandbox, SandboxOutput};
        use crate::tools::AlephTool;

        let _serial = WORKTREE_REPO_SERIAL.lock().await;
        let repo_root = std::env::current_dir().expect("cwd");
        let h = create(&repo_root, "task7-e2e-override", None)
            .await
            .expect("create");
        let worktree_path = h.path().to_path_buf();

        // Construction-time ("parent") sandbox — must be bypassed while the
        // worktree override is in scope. Recording its calls lets us assert it.
        let parent = MockSandbox::new(SandboxOutput {
            exit_code: Some(0),
            ..Default::default()
        });
        let parent_dyn: Arc<dyn Sandbox> = parent.clone();
        let tool = CodeExecTool::new().with_sandbox(parent_dyn);

        let override_sb: Arc<dyn Sandbox> = Arc::new(WorktreeSandbox::new(worktree_path.clone()));

        let session = crate::routing::session_key::SessionKey::ephemeral("task7-e2e-override");
        let out = SESSION_ID
            .scope(
                session,
                with_sandbox_override(Some(override_sb), async {
                    tool.call(CodeExecArgs {
                        language: Language::Shell,
                        code: "pwd; echo \"CTD=$CARGO_TARGET_DIR\"".to_string(),
                        working_dir: None,
                        timeout_seconds: Some(30),
                        allow_network: false,
                        allow_subprocess: false,
                        extra_writable_paths: Vec::new(),
                        justification: None,
                    })
                    .await
                    .expect("tool call")
                }),
            )
            .await;

        // Routing preferred the scoped override — the parent sandbox is untouched.
        assert_eq!(
            parent.calls.lock().await.len(),
            0,
            "worktree override must bypass the construction-time sandbox"
        );
        assert!(out.success, "isolated command failed: {}", out.stderr);

        let basename = worktree_path.file_name().unwrap().to_str().unwrap();
        assert!(
            out.stdout.contains(basename),
            "pwd did not run inside the worktree: stdout={:?}",
            out.stdout
        );
        // `CARGO_TARGET_DIR` must point at <worktree>/target — the redirect that
        // keeps a subagent's cargo builds out of the parent's target dir.
        let ctd_line = out
            .stdout
            .lines()
            .find(|l| l.starts_with("CTD="))
            .unwrap_or("");
        assert!(
            ctd_line.contains(basename) && ctd_line.contains("target"),
            "CARGO_TARGET_DIR not redirected into the worktree: {ctd_line:?} (full stdout {:?})",
            out.stdout
        );

        h.cleanup().await.expect("cleanup");
    }

    #[tokio::test]
    async fn worktree_sandbox_hardline_floor_denies_catastrophic_command() {
        // The catastrophic command-policy floor must hold on the worktree path
        // too — a fork bomb is refused BEFORE any process spawns, so no real git
        // worktree is needed. This is the hole R3-3 closed: WorktreeSandbox used
        // to run `tokio::process::Command` with no hooks at all.
        let sandbox = WorktreeSandbox::new(std::env::temp_dir());
        let cmd = crate::sandbox::SandboxCommand {
            session_id: crate::session::service::SessionId::main("worktree-floor-test"),
            program: "bash".into(),
            args: vec!["-c".into(), ":(){ :|:& };:".into()],
            env: std::collections::HashMap::new(),
            stdin: None,
            cwd: None,
            capabilities: crate::sandbox::SandboxCapabilities::default(),
            timeout: None,
        };
        use crate::sandbox::Sandbox as _;
        let err = sandbox
            .execute(cmd)
            .await
            .expect_err("fork bomb must be denied by the hardline floor");
        assert!(
            matches!(err, crate::sandbox::SandboxError::Other(ref m) if m.contains("hook denied")),
            "expected hardline hook denial, got {err:?}"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn worktree_sandbox_blocks_private_key_in_output() {
        // The block-class secret floor must hold on the worktree path: a command
        // that dumps a PEM private-key header to stdout is refused, not returned
        // to the model. `printf` runs in $TMPDIR (an existing dir), so no git
        // worktree is required.
        let sandbox = WorktreeSandbox::new(std::env::temp_dir());
        let cmd = crate::sandbox::SandboxCommand {
            session_id: crate::session::service::SessionId::main("worktree-secret-test"),
            program: "printf".into(),
            args: vec!["%s".into(), "-----BEGIN PRIVATE KEY-----".into()],
            env: std::collections::HashMap::new(),
            stdin: None,
            cwd: None,
            capabilities: crate::sandbox::SandboxCapabilities::default(),
            timeout: None,
        };
        use crate::sandbox::Sandbox as _;
        let err = sandbox
            .execute(cmd)
            .await
            .expect_err("private-key output must be blocked");
        assert!(
            matches!(err, crate::sandbox::SandboxError::Other(ref m) if m.contains("catastrophic secret")),
            "expected block-class refusal, got {err:?}"
        );
    }
}
