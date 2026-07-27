//! Process-execution helpers shared by every platform's `AutomationCapability`.
//!
//! Two execution shapes, deliberately separated:
//!
//! * [`output_capped`] — run a command to completion under a hard timeout,
//!   killing the child on elapse. This is for scripts that are *expected to
//!   terminate* (`ls`, `osascript "return 2+2"`). Without the cap a script
//!   that never exits (a dev server invoked by mistake) blocks the caller
//!   until the harness's 300s per-turn timeout, leaking a hung child.
//!
//! * [`spawn_background`] — launch a *long-running* process detached, with its
//!   stdout/stderr redirected to a log file, and return its PID immediately.
//!   This is the correct path for dev servers / watchers / daemons that the
//!   model wants running so it can then open a browser against them.

use std::process::{Output, Stdio};
use std::time::Duration;

use tokio::process::Command;

use crate::error::{DesktopError, Result};

/// Hard ceiling for a synchronous `run_script` call. Scripts are expected to
/// terminate; anything that runs indefinitely must use [`spawn_background`].
///
/// Chosen well under the harness's 300s per-turn timeout so a hung script
/// fails fast with an actionable error instead of a silent five-minute stall,
/// yet generous enough not to kill ordinary slow-but-terminating commands
/// (`npm install`, a build, a test run).
pub const RUN_SCRIPT_TIMEOUT: Duration = Duration::from_secs(120);

/// Prefix of the error [`output_capped`] returns when the binary itself could
/// not be launched (as opposed to a timeout or a non-zero script exit). Kept as
/// a shared const so its producer ([`output_capped`]) and its classifier
/// ([`is_spawn_failure`]) can never drift apart.
const SPAWN_FAILURE_PREFIX: &str = "failed to spawn process";

/// True when `e` is the spawn-failure error [`output_capped`] produces — the
/// interpreter/binary could not be launched. Callers that try a sequence of
/// candidate binaries (e.g. `pwsh` → `powershell`) use this to fall through
/// *only* on a missing binary, never on a timeout of one that does exist.
#[must_use]
pub fn is_spawn_failure(e: &DesktopError) -> bool {
    matches!(e, DesktopError::InputFailed(m) if m.starts_with(SPAWN_FAILURE_PREFIX))
}

/// Build a [`Command`] that never flashes a console window.
///
/// Every desktop capability that shells out — PowerShell for toasts, shortcuts
/// and registry reads, `cmd /C start` for Settings deep links, `ffmpeg` for
/// capture — is a console program. When `aleph-server` runs as a detached daemon
/// it owns no console, so each such child **allocates its own**: a black window
/// that pops up and vanishes on the user's screen, for a call the user did not
/// make. `check_all` alone did that six times in a row.
///
/// This is the desktop crates' counterpart to `src/utils/no_window.rs`, which
/// the core uses for the same reason and which these crates cannot reach.
///
/// Note the Win32 semantics the caller must not break: a later
/// `creation_flags(..)` call **replaces** this value rather than OR-ing into it.
/// Build the command here and add arguments, not flags.
#[must_use]
pub fn hidden_command(program: &str) -> Command {
    #[allow(unused_mut)]
    let mut cmd = Command::new(program);
    #[cfg(windows)]
    {
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

/// [`hidden_command`] for the blocking `std` API, used by the capture paths that
/// already run inside `spawn_blocking`.
#[must_use]
pub fn hidden_std_command(program: &str) -> std::process::Command {
    #[allow(unused_mut)]
    let mut cmd = std::process::Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

/// `CREATE_NO_WINDOW` — create the process without allocating a console.
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Run `cmd` to completion, but never longer than `timeout`. On elapse the
/// child is killed (`kill_on_drop`) so we never leak a hung process, and the
/// caller gets a clear error that points at the background path.
pub async fn output_capped(mut cmd: Command, timeout: Duration) -> Result<Output> {
    // kill_on_drop ensures the SIGKILL fires when the timed-out future is
    // dropped below — otherwise the child outlives the call as an orphan.
    cmd.kill_on_drop(true);
    match tokio::time::timeout(timeout, cmd.output()).await {
        Ok(Ok(output)) => Ok(output),
        Ok(Err(e)) => Err(DesktopError::InputFailed(format!(
            "{SPAWN_FAILURE_PREFIX}: {e}"
        ))),
        Err(_elapsed) => Err(DesktopError::InputFailed(format!(
            "script exceeded {}s and was terminated. For a long-running process \
             (dev server, file watcher, daemon) use the automation `run_background` \
             action instead, then read its log file.",
            timeout.as_secs()
        ))),
    }
}

/// Spawn `cmd` as a detached background process: stdin is `/dev/null`, stdout
/// and stderr are redirected to `log_path` (truncated/created), and the call
/// returns the child PID without waiting for it to exit.
///
/// A lightweight task owns the [`tokio::process::Child`] and reaps it when the
/// process eventually exits, so a finished background process never lingers as
/// a zombie. The process itself runs independently of that task.
pub async fn spawn_background(mut cmd: Command, log_path: &str) -> Result<u32> {
    let log = tokio::fs::File::create(log_path).await.map_err(|e| {
        DesktopError::InputFailed(format!("cannot create log file {log_path}: {e}"))
    })?;
    let log_err = log
        .try_clone()
        .await
        .map_err(|e| DesktopError::InputFailed(format!("cannot duplicate log handle: {e}")))?;
    let log = log.into_std().await;
    let log_err = log_err.into_std().await;

    cmd.stdin(Stdio::null())
        .stdout(Stdio::from(log))
        .stderr(Stdio::from(log_err));

    let mut child = cmd.spawn().map_err(|e| {
        DesktopError::InputFailed(format!("failed to spawn background process: {e}"))
    })?;
    let pid = child
        .id()
        .ok_or_else(|| DesktopError::InputFailed("spawned background process has no PID".into()))?;

    // Detach + reap: keep the process running independently; this task only
    // owns the handle so the child is reaped (no zombie) when it exits.
    tokio::spawn(async move {
        let _ = child.wait().await;
    });

    Ok(pid)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sh(script: &str) -> Command {
        let mut c = Command::new("sh");
        c.arg("-c").arg(script);
        c
    }

    #[tokio::test]
    async fn output_capped_returns_output_for_fast_command() {
        let out = output_capped(sh("echo hi"), Duration::from_secs(5))
            .await
            .expect("fast command should succeed");
        assert!(String::from_utf8_lossy(&out.stdout).contains("hi"));
    }

    #[tokio::test]
    async fn output_capped_times_out_and_reports_background_hint() {
        // `sleep 5` would block far past the 200ms cap; the call must return
        // promptly with an error that steers the caller to run_background.
        let err = output_capped(sh("sleep 5"), Duration::from_millis(200))
            .await
            .expect_err("a command exceeding the cap must error");
        let msg = err.to_string();
        assert!(msg.contains("exceeded"), "got: {msg}");
        assert!(msg.contains("run_background"), "got: {msg}");
    }

    #[tokio::test]
    async fn spawn_background_returns_pid_and_writes_log() {
        let dir = std::env::temp_dir();
        let log_path = dir
            .join("aleph-script-exec-test.log")
            .to_string_lossy()
            .into_owned();
        let pid = spawn_background(sh("echo bg-hello"), &log_path)
            .await
            .expect("background spawn should succeed");
        assert!(pid > 0);

        // The process runs detached; poll the log until it flushes the line.
        let mut contents = String::new();
        for _ in 0..40 {
            contents = tokio::fs::read_to_string(&log_path).await.unwrap_or_default();
            if contents.contains("bg-hello") {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert!(contents.contains("bg-hello"), "log was: {contents:?}");
        let _ = tokio::fs::remove_file(&log_path).await;
    }
}
