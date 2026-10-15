//! The real launcher. The child is spawned ONLY through
//! `ManagedSandbox::spawn_managed` (same approval/profile/second-check chain as
//! `Sandbox::execute`), is owned by one supervisor task until `Child::wait`
//! returns, and is registered in the existing `ProcessRegistry` so that the
//! reap acknowledgement is observable. The service PID is whatever the driver
//! vouched for; a wrapper PID is never substituted (drivers that cannot vouch
//! refuse with `UnsupportedPolicy`).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{oneshot, watch, Notify};

use super::manager::{CompanionLauncher, CompanionProcess, LaunchError, LaunchRequest};
use super::protocol::{verify_ready_line, LaunchCredentials, ProtocolError, MAX_READY_BYTES};
use crate::builtin_tools::bash_exec::session_label;
use crate::builtin_tools::code_exec::CodeExecOutput;
use crate::builtin_tools::process_registry::{
    managed_link, process_registry, ManagedCancelOutcome, ManagedSupervisorEnd, RegisterOutcome,
};
use crate::sandbox::capabilities::{NetworkPolicy, SandboxCapabilities};
use crate::sandbox::command::SandboxCommand;
use crate::sandbox::managed::{ManagedExit, ManagedProcess, ManagedStdio};
use crate::sandbox::{current_session, Sandbox};

/// Launches companions through the workspace sandbox.
pub(crate) struct SandboxLauncher {
    sandbox: Arc<dyn Sandbox>,
}

impl SandboxLauncher {
    pub(crate) fn new(sandbox: Arc<dyn Sandbox>) -> Self {
        Self { sandbox }
    }
}

/// Handle on a supervised child. `exit` flips to `true` only after the
/// supervisor's `Child::wait` returned.
pub(crate) struct SandboxProcess {
    service_pid: u32,
    exit: watch::Receiver<bool>,
    registry_id: u64,
    caller: Option<String>,
}

#[async_trait]
impl CompanionProcess for SandboxProcess {
    fn service_pid(&self) -> u32 {
        self.service_pid
    }

    fn is_alive(&self) -> bool {
        !*self.exit.borrow()
    }

    async fn exited(&self) {
        let mut rx = self.exit.clone();
        // A dropped sender means the supervisor is gone: also "exited".
        let _ = rx.wait_for(|exited| *exited).await;
    }

    async fn stop_and_reap(&self, budget: Duration) -> Result<(), String> {
        let outcome = process_registry()
            .cancel_managed(self.registry_id, self.caller.as_deref(), budget)
            .await;
        match outcome {
            ManagedCancelOutcome::Reaped => Ok(()),
            ManagedCancelOutcome::TimedOut => Err(format!(
                "the companion process did not report exit within {}s; it may still be running",
                budget.as_secs()
            )),
            ManagedCancelOutcome::NotFound if *self.exit.borrow() => Ok(()),
            ManagedCancelOutcome::NotFound => {
                Err("the companion process is not tracked and its exit was not observed".into())
            }
        }
    }
}

/// The ONE place the companion's network grant is derived from its endpoint.
///
/// Only an exact loopback literal (`127.0.0.1` or `::1`) with a non-zero port
/// is admitted; anything else (including `localhost`, whose resolution is not
/// ours to pin, and other `127/8` addresses) is refused here, before any
/// approval card or spawn. Windows gets an exact loopback-port request (the
/// managed launcher confines by AppContainer SID + port); other platforms keep
/// the loopback host allowlist their drivers enforce.
fn managed_network(req: &LaunchRequest) -> Result<NetworkPolicy, LaunchError> {
    let (host, port) = req.endpoint.host_port();
    let ip: std::net::IpAddr = host
        .trim_start_matches('[')
        .trim_end_matches(']')
        .parse()
        .map_err(|_| {
            LaunchError::Refused(format!(
                "companion endpoint host {host:?} is not a loopback IP literal"
            ))
        })?;
    let exact_loopback = match ip {
        std::net::IpAddr::V4(v4) => v4 == std::net::Ipv4Addr::LOCALHOST,
        std::net::IpAddr::V6(v6) => v6 == std::net::Ipv6Addr::LOCALHOST,
    };
    if !exact_loopback {
        return Err(LaunchError::Refused(format!(
            "companion endpoint host {ip} is not 127.0.0.1 or ::1"
        )));
    }
    if port == 0 {
        return Err(LaunchError::Refused("companion endpoint port is 0".into()));
    }
    if cfg!(windows) {
        Ok(NetworkPolicy::LoopbackPorts { ports: vec![port] })
    } else {
        Ok(NetworkPolicy::AllowHosts {
            hosts: vec![ip.to_string()],
        })
    }
}

fn capabilities(req: &LaunchRequest) -> Result<SandboxCapabilities, LaunchError> {
    Ok(SandboxCapabilities {
        // Loopback only; this is an elevation over the session baseline, so
        // the sandbox raises its normal capability approval card.
        network: managed_network(req)?,
        // A managed service is ONE process. Nothing here can prove that a
        // forked descendant is stopped (the supervisor kills and waits for the
        // service PID only, never a process tree), so forking is not granted
        // and a driver that cannot deny it refuses the spawn
        // (`ManagedSandbox::spawn_managed`, `managed_spawn_fork`).
        spawn_subprocess: false,
        timeout_secs: Some(req.lifetime.as_secs()),
        ..SandboxCapabilities::strict()
    })
}

/// Arms the launch-internal kill: if the `launch` future is dropped before it
/// returns the process (cancelled during the ready handshake or the rollback
/// reap), `Drop` wakes the supervisor, which kills and reaps the child itself.
/// Sync `Drop` only signals; the supervisor task does the waiting.
struct LaunchAbortGuard {
    kill: Arc<Notify>,
    armed: bool,
}

impl LaunchAbortGuard {
    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for LaunchAbortGuard {
    fn drop(&mut self) {
        if self.armed {
            self.kill.notify_one();
        }
    }
}

/// Owns the child until it is waited. `id` is `None` when registration was
/// refused: the child is then killed and reaped immediately. `kill` is the
/// launch-abort signal (see [`LaunchAbortGuard`]).
async fn supervise(
    mut child: Box<dyn ManagedProcess>,
    keepalive: Vec<Box<dyn std::any::Any + Send>>,
    mut link: ManagedSupervisorEnd,
    id_rx: oneshot::Receiver<u64>,
    exit_tx: watch::Sender<bool>,
    kill: Arc<Notify>,
) {
    let started = Instant::now();
    let id = id_rx.await.ok();
    let status = if id.is_none() {
        let _ = child.start_kill();
        child.wait().await
    } else {
        tokio::select! {
            status = child.wait() => status,
            cancelled = &mut link.cancel => {
                // `Err` means the registry side was dropped: not a cancel.
                if cancelled.is_ok() {
                    let _ = child.start_kill();
                }
                child.wait().await
            }
            () = kill.notified() => {
                let _ = child.start_kill();
                child.wait().await
            }
        }
    };
    let exit_code = status.as_ref().ok().and_then(|s| s.code).unwrap_or(-1);
    let success = status.as_ref().is_ok_and(ManagedExit::success);
    // The OS wait has returned: publish that fact, then record the result.
    let _ = exit_tx.send(true);
    if let Some(id) = id {
        process_registry().complete(
            id,
            CodeExecOutput {
                success,
                exit_code,
                stdout: String::new(),
                stderr: String::new(),
                duration_ms: started.elapsed().as_millis() as u64,
                language: "browser_companion".to_string(),
                truncated: None,
                stdout_truncated_bytes: 0,
                stderr_truncated_bytes: 0,
                advisory: None,
            },
        );
    }
    link.ack_reaped();
    drop(keepalive);
}

/// Read one newline-terminated line of at most `MAX_READY_BYTES` from `stdout`.
async fn read_ready_line(
    stdout: &mut (dyn tokio::io::AsyncRead + Send + Unpin),
) -> Result<Vec<u8>, ProtocolError> {
    let mut buf: Vec<u8> = Vec::with_capacity(256);
    let mut chunk = [0u8; 512];
    loop {
        let n = stdout
            .read(&mut chunk)
            .await
            .map_err(|e| ProtocolError::Transport(e.to_string()))?;
        if n == 0 {
            return Err(ProtocolError::ReadyMalformed("closed before ready"));
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(pos) = buf.iter().position(|b| *b == b'\n') {
            buf.truncate(pos);
            if buf.len() > MAX_READY_BYTES {
                return Err(ProtocolError::ReadyTooLarge);
            }
            return Ok(buf);
        }
        if buf.len() > MAX_READY_BYTES {
            return Err(ProtocolError::ReadyTooLarge);
        }
    }
}

#[async_trait]
impl CompanionLauncher for SandboxLauncher {
    fn preflight(&self) -> Result<(), LaunchError> {
        let managed = self
            .sandbox
            .managed()
            .ok_or_else(|| LaunchError::Unsupported("this sandbox has no managed spawn".into()))?;
        managed
            .preflight()
            .map_err(|e| LaunchError::Unsupported(e.to_string()))
    }

    async fn launch(
        &self,
        req: &LaunchRequest,
        creds: &LaunchCredentials,
    ) -> Result<Arc<dyn CompanionProcess>, LaunchError> {
        self.preflight()?;
        let managed = self
            .sandbox
            .managed()
            .ok_or_else(|| LaunchError::Unsupported("this sandbox has no managed spawn".into()))?;
        let session_id = current_session().ok_or(LaunchError::NoSession)?;
        let command = SandboxCommand {
            session_id,
            tool_name: "browser_companion".to_owned(),
            program: req.program.clone(),
            args: req.args.clone(),
            // Nothing secret travels in env: the launch secret is stdin-only.
            env: HashMap::new(),
            stdin: None,
            cwd: req.cwd.as_ref().map(PathBuf::from),
            capabilities: capabilities(req)?,
            timeout: Some(req.lifetime),
        };
        let spawn = managed
            .spawn_managed(command)
            .await
            .map_err(|e| LaunchError::Spawn(e.to_string()))?;
        let mut child = spawn.process;
        let service_pid = child.service_pid();
        let ManagedStdio {
            stdin,
            stdout,
            stderr,
        } = child.take_stdio();

        let (link_end, link) = managed_link();
        let (exit_tx, exit_rx) = watch::channel(false);
        let (id_tx, id_rx) = oneshot::channel();
        let kill = Arc::new(Notify::new());
        let join = tokio::spawn(supervise(
            child,
            spawn.keepalive,
            link,
            id_rx,
            exit_tx,
            Arc::clone(&kill),
        ));
        let caller = session_label();
        let registered = process_registry().register_managed(
            format!("browser_companion pid={service_pid}"),
            caller.clone(),
            join.abort_handle(),
            link_end,
        );
        let registry_id = match registered {
            RegisterOutcome::Registered(id) => id,
            RegisterOutcome::TooManyRunning { limit } => {
                // Dropping the id sender makes the supervisor kill and reap.
                drop(id_tx);
                let mut rx = exit_rx.clone();
                let reaped =
                    tokio::time::timeout(Duration::from_secs(10), rx.wait_for(|exited| *exited))
                        .await
                        .is_ok();
                return Err(LaunchError::Refused(format!(
                    "process cap of {limit} reached{}",
                    if reaped {
                        ""
                    } else {
                        "; child exit NOT observed"
                    }
                )));
            }
        };
        let _ = id_tx.send(registry_id);
        let process = Arc::new(SandboxProcess {
            service_pid,
            exit: exit_rx,
            registry_id,
            caller,
        });
        // Armed until the launch returns the process: a dropped future from
        // here on must not leave a supervised, unowned child.
        let mut abort_guard = LaunchAbortGuard { kill, armed: true };

        let handshake = async {
            let mut stdin = stdin.ok_or(ProtocolError::Transport("no stdin pipe".into()))?;
            let mut stdout = stdout.ok_or(ProtocolError::Transport("no stdout pipe".into()))?;
            stdin
                .write_all(&creds.launch_line())
                .await
                .map_err(|e| ProtocolError::Transport(e.to_string()))?;
            stdin
                .shutdown()
                .await
                .map_err(|e| ProtocolError::Transport(e.to_string()))?;
            drop(stdin);
            let line = read_ready_line(stdout.as_mut()).await?;
            verify_ready_line(&line, &req.endpoint, creds)?;
            Ok::<_, ProtocolError>(stdout)
        };
        let outcome = tokio::time::timeout(req.ready_timeout, handshake).await;
        let failure = match outcome {
            Ok(Ok(mut stdout)) => {
                // Keep the pipes drained so the service never blocks on them.
                tokio::spawn(async move {
                    let _ = tokio::io::copy(&mut stdout, &mut tokio::io::sink()).await;
                });
                if let Some(mut stderr) = stderr {
                    tokio::spawn(async move {
                        let _ = tokio::io::copy(&mut stderr, &mut tokio::io::sink()).await;
                    });
                }
                abort_guard.disarm();
                return Ok(process);
            }
            Ok(Err(e)) => LaunchError::Ready(e),
            Err(_) => LaunchError::ReadyTimeout,
        };
        // Roll back: kill, then require the real exit before reporting.
        match process.stop_and_reap(Duration::from_secs(10)).await {
            Ok(()) => Err(failure),
            Err(reap) => Err(LaunchError::ReapFailed(format!("{failure}; {reap}"))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::protocol::CanonicalEndpoint;
    use super::*;
    use crate::sandbox::managed::TokioManagedProcess;

    /// A real child run under the real supervisor + registry: cancel kills it
    /// and `Reaped` is returned only after `Child::wait`.
    #[tokio::test]
    async fn supervisor_cancel_returns_reaped_only_after_wait_and_natural_exit_is_observed() {
        #[cfg(windows)]
        let (program, args) = ("cmd", vec!["/C", "ping -n 30 127.0.0.1 >NUL"]);
        #[cfg(not(windows))]
        let (program, args) = ("sleep", vec!["30"]);
        let child = tokio::process::Command::new(program)
            .args(&args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .expect("spawn trivial child");

        let (link_end, link) = managed_link();
        let (exit_tx, exit_rx) = watch::channel(false);
        let (id_tx, id_rx) = oneshot::channel();
        let join = tokio::spawn(supervise(
            Box::new(TokioManagedProcess::new(child).expect("adapter")),
            Vec::new(),
            link,
            id_rx,
            exit_tx,
            Arc::new(Notify::new()),
        ));
        let caller = Some("supervisor-test-session".to_owned());
        let id = match process_registry().register_managed(
            "supervisor test",
            caller.clone(),
            join.abort_handle(),
            link_end,
        ) {
            RegisterOutcome::Registered(id) => id,
            RegisterOutcome::TooManyRunning { .. } => panic!("registry full"),
        };
        id_tx.send(id).expect("supervisor alive");
        let process = SandboxProcess {
            service_pid: 1,
            exit: exit_rx,
            registry_id: id,
            caller,
        };
        assert!(process.is_alive());
        process
            .stop_and_reap(Duration::from_secs(10))
            .await
            .expect("reaped");
        assert!(!process.is_alive(), "exit flag is set once wait returned");
        process.exited().await;
        // A second stop is idempotent and still proves exit.
        process
            .stop_and_reap(Duration::from_secs(1))
            .await
            .expect("idempotent");
    }

    /// The managed command asks for ONE process: no fork/subprocess grant, so
    /// the policy handed to the driver has `allow_fork == false`, and the
    /// workspace seam refuses anything else (`managed_spawn_fork`).
    #[test]
    fn launcher_capabilities_never_grant_fork() {
        let req = LaunchRequest {
            program: "dev-server".into(),
            args: vec![],
            cwd: None,
            endpoint: CanonicalEndpoint::parse("http://127.0.0.1:5173/app").expect("canonical"),
            ready_timeout: Duration::from_secs(5),
            lifetime: Duration::from_secs(60),
        };
        let caps = capabilities(&req).expect("loopback endpoint");
        assert!(!caps.spawn_subprocess);
        assert!(
            !crate::sandbox::policy::SandboxPolicy::from(&caps)
                .process
                .allow_fork
        );
    }

    fn request(endpoint: &str) -> LaunchRequest {
        LaunchRequest {
            program: "dev-server".into(),
            args: vec![],
            cwd: None,
            endpoint: CanonicalEndpoint::parse(endpoint).expect("canonical"),
            ready_timeout: Duration::from_secs(5),
            lifetime: Duration::from_secs(60),
        }
    }

    /// The network grant is derived once from the endpoint: exact loopback
    /// literal + port. On Windows that is an exact loopback-port request, never
    /// an `AllowHosts` entry the driver would have to reinterpret.
    #[test]
    fn network_grant_is_the_exact_loopback_port_of_the_endpoint() {
        for (endpoint, host, port) in [
            ("http://127.0.0.1:5173/app", "127.0.0.1", 5173u16),
            ("http://[::1]:8443/", "::1", 8443),
        ] {
            let net = managed_network(&request(endpoint)).expect("loopback");
            if cfg!(windows) {
                assert_eq!(net, NetworkPolicy::LoopbackPorts { ports: vec![port] });
            } else {
                assert_eq!(
                    net,
                    NetworkPolicy::AllowHosts {
                        hosts: vec![host.to_owned()]
                    }
                );
            }
        }
    }

    /// `localhost` and other 127/8 addresses are refused before admission.
    #[test]
    fn network_grant_refuses_anything_but_exact_loopback_literals() {
        for endpoint in ["http://localhost:5173/", "http://127.0.0.2:5173/"] {
            let err = managed_network(&request(endpoint)).expect_err(endpoint);
            assert!(
                matches!(err, LaunchError::Refused(_)),
                "{endpoint}: {err:?}"
            );
            assert!(capabilities(&request(endpoint)).is_err());
        }
    }

    /// A sandbox without a managed-spawn capability (or a driver that refuses,
    /// like the Windows wrapper) fails closed at preflight: no card, no spawn.
    #[test]
    fn launcher_without_managed_spawn_is_unsupported() {
        let sandbox: Arc<dyn Sandbox> = Arc::new(crate::sandbox::NoopSandbox);
        let launcher = SandboxLauncher::new(sandbox);
        assert!(matches!(
            launcher.preflight(),
            Err(LaunchError::Unsupported(_))
        ));
    }

    #[test]
    fn ready_reader_limits_are_the_protocol_limits() {
        assert_eq!(MAX_READY_BYTES, 4096);
    }
}
