//! Managed (long-lived, owner-held) child execution seam.
//!
//! `Sandbox::execute` is one-shot: it spawns, drains and reaps inside one call,
//! so the child can never be handed to a supervisor. A managed companion needs
//! the opposite - the owner keeps the live `Child` (stdin for the launch
//! secret, stdout for the ready report, `wait` for the real exit). That must
//! NOT become a way around the sandbox, so this seam is deliberately narrow:
//!
//! * [`ManagedSandbox::spawn_managed`] runs the SAME admission pass as
//!   `execute` (hooks, cwd containment, approval request, second cwd
//!   revalidation, proxy/DNS, profile generation) before any spawn;
//! * the OS driver, not the caller, builds the `Command`
//!   ([`OsSandboxDriverTrait::spawn_managed`](crate::sandbox::OsSandboxDriverTrait::spawn_managed));
//! * a managed service is ONE process. The supervisor kills and waits for the
//!   service PID only; there is no process-tree kill, so descendants of a
//!   forked child could outlive a "confirmed" stop. `spawn_managed` therefore
//!   refuses any command whose capabilities allow fork/subprocesses
//!   (`UnsupportedPolicy { feature: "managed_spawn_fork" }`), before approval
//!   and again on the prepared command. Denying fork at the OS level is the
//!   driver's job (macOS seatbelt emits `(deny process-fork)` when
//!   `allow_fork == false`; that denial has not been verified on a real macOS
//!   host from this change). Linux and Windows refuse managed spawns anyway;
//!   this is a single-process contract, not a tree-kill guarantee;
//! * a driver whose wrapper process is not the service process (Windows
//!   `sandbox-init-windows`, Linux bwrap/init) refuses with
//!   `SandboxError::UnsupportedPolicy` instead of reporting a wrapper PID as
//!   the service PID. Disabling the restricted token to dodge that is not an
//!   option.

use std::any::Any;

use async_trait::async_trait;
use tokio::io::{AsyncRead, AsyncWrite};

use crate::sandbox::command::{SandboxCommand, SandboxError};

/// Owner-side write end of the service's stdin pipe.
pub type ManagedStdin = Box<dyn AsyncWrite + Send + Unpin>;
/// Owner-side read end of the service's stdout / stderr pipe.
pub type ManagedOutput = Box<dyn AsyncRead + Send + Unpin>;

/// The three pipes of a managed service. Each is handed out once.
#[derive(Default)]
pub struct ManagedStdio {
    pub stdin: Option<ManagedStdin>,
    pub stdout: Option<ManagedOutput>,
    pub stderr: Option<ManagedOutput>,
}

/// How the service process ended, as observed by [`ManagedProcess::wait`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ManagedExit {
    /// OS exit code, `None` when the platform reports none (Unix signal).
    pub code: Option<i32>,
}

impl ManagedExit {
    #[must_use]
    pub const fn success(&self) -> bool {
        matches!(self.code, Some(0))
    }
}

/// A live managed service process, independent of how the OS driver spawned
/// it (a `tokio::process::Child` on macOS, a raw process handle inside a Job
/// Object on Windows).
///
/// Contract every implementation keeps:
/// * [`Self::service_pid`] is the PID of the SERVICE process itself - never a
///   wrapper, launcher or shell in between;
/// * [`Self::wait`] returns only after the OS reports that process exited (and,
///   where the driver confines it, after that confinement is torn down), so a
///   caller that marks "reaped" after `wait` returns never lies;
/// * [`Self::start_kill`] only requests termination; the exit is still
///   observed through `wait`.
#[async_trait]
pub trait ManagedProcess: Send {
    fn service_pid(&self) -> u32;
    /// Hand out the pipes. Later calls return empty slots.
    fn take_stdio(&mut self) -> ManagedStdio;
    fn start_kill(&mut self) -> std::io::Result<()>;
    async fn wait(&mut self) -> std::io::Result<ManagedExit>;
}

/// [`ManagedProcess`] over a directly spawned `tokio::process::Child`, for
/// drivers whose spawned child IS the service (macOS `sandbox-exec` execs the
/// target in place).
pub struct TokioManagedProcess {
    child: tokio::process::Child,
    service_pid: u32,
}

impl TokioManagedProcess {
    /// Fails when the child has no PID (already reaped), so a zero or stale
    /// PID can never be reported as the service PID.
    pub fn new(child: tokio::process::Child) -> Result<Self, SandboxError> {
        let service_pid = child
            .id()
            .ok_or_else(|| SandboxError::ExecutionFailed("spawned child reported no pid".into()))?;
        Ok(Self { child, service_pid })
    }
}

#[async_trait]
impl ManagedProcess for TokioManagedProcess {
    fn service_pid(&self) -> u32 {
        self.service_pid
    }

    fn take_stdio(&mut self) -> ManagedStdio {
        ManagedStdio {
            stdin: self.child.stdin.take().map(|p| Box::new(p) as ManagedStdin),
            stdout: self
                .child
                .stdout
                .take()
                .map(|p| Box::new(p) as ManagedOutput),
            stderr: self
                .child
                .stderr
                .take()
                .map(|p| Box::new(p) as ManagedOutput),
        }
    }

    fn start_kill(&mut self) -> std::io::Result<()> {
        self.child.start_kill()
    }

    async fn wait(&mut self) -> std::io::Result<ManagedExit> {
        let status = self.child.wait().await?;
        Ok(ManagedExit {
            code: status.code(),
        })
    }
}

/// What an OS driver hands back: the live service process.
pub struct ManagedChild {
    /// Live service; its `service_pid()` is the PID the driver vouches for.
    pub process: Box<dyn ManagedProcess>,
    /// Resources that must outlive the child (e.g. profile temp file).
    pub keepalive: Option<Box<dyn Any + Send>>,
}

/// A managed spawn after sandbox admission. `keepalive` holds the driver's
/// resources plus the managed network proxy guard; drop it only after
/// `process.wait()` returned.
pub struct ManagedSpawn {
    pub process: Box<dyn ManagedProcess>,
    pub keepalive: Vec<Box<dyn Any + Send>>,
}

#[async_trait]
pub trait ManagedSandbox: Send + Sync {
    /// Admit `command` exactly like `Sandbox::execute` would, then spawn it and
    /// return the live child instead of waiting for it. `command.stdin` must be
    /// `None`: the owner writes the launch secret to the child's stdin pipe.
    async fn spawn_managed(&self, command: SandboxCommand) -> Result<ManagedSpawn, SandboxError>;

    /// Side-effect-free "can this sandbox ever do a managed spawn here?",
    /// answered before any approval card is raised.
    fn preflight(&self) -> Result<(), SandboxError>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncReadExt;

    fn trivial_child() -> tokio::process::Child {
        #[cfg(windows)]
        let mut cmd = {
            let mut c = tokio::process::Command::new("cmd");
            c.args(["/C", "echo managed"]);
            c
        };
        #[cfg(not(windows))]
        let mut cmd = {
            let mut c = tokio::process::Command::new("sh");
            c.args(["-c", "echo managed"]);
            c
        };
        cmd.stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .expect("spawn trivial child")
    }

    /// The tokio adapter reports the child's own PID, hands each pipe out
    /// once, and `wait` reports the real exit code.
    #[tokio::test]
    async fn tokio_adapter_reports_pid_pipes_once_and_real_exit() {
        let child = trivial_child();
        let pid = child.id().expect("pid");
        let mut process = TokioManagedProcess::new(child).expect("adapter");
        assert_eq!(process.service_pid(), pid);
        let stdio = process.take_stdio();
        assert!(stdio.stdin.is_some() && stdio.stderr.is_some());
        let mut stdout = stdio.stdout.expect("stdout");
        let again = process.take_stdio();
        assert!(again.stdin.is_none() && again.stdout.is_none() && again.stderr.is_none());
        drop(stdio.stdin);
        let mut out = String::new();
        stdout.read_to_string(&mut out).await.expect("read");
        assert!(out.contains("managed"), "{out:?}");
        let exit = process.wait().await.expect("wait");
        assert_eq!(exit.code, Some(0));
        assert!(exit.success());
    }

    #[test]
    fn exit_success_requires_code_zero() {
        assert!(!ManagedExit { code: None }.success());
        assert!(!ManagedExit { code: Some(1) }.success());
    }
}
