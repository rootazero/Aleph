//! Shared local-server startup I/O, with no runtime or process at import.
//! HTTP readiness is not binary identity or permission to stop a service.
use std::path::{Path, PathBuf};
use std::time::Duration;

use tokio::net::TcpStream;

const DAEMON_HOST: &str = "127.0.0.1";
const DAEMON_PORT: u16 = 18790;
/// How long to wait for the daemon to report ready before giving up.
const READY_TIMEOUT: Duration = Duration::from_secs(40);
/// Interval between `/ready` polls.
const POLL_INTERVAL: Duration = Duration::from_millis(400);
/// Upper bound on a single localhost HTTP probe (connect + write + read).
/// A process that accepts the connection but never replies cannot hang it.
const PROBE_TIMEOUT: Duration = Duration::from_secs(2);
/// Winsock can take just over two seconds to report a refused loopback
/// connection. Allow that explicit refusal without widening the HTTP probe.
const CONNECT_TIMEOUT: Duration = if cfg!(windows) {
    Duration::from_secs(4)
} else {
    PROBE_TIMEOUT
};

#[cfg(feature = "gateway-admission")]
mod admission;

/// Strict NSH startup. A listener must serve compatible Gateway documents;
/// only a refused connection can authorize a launch, and only of the sibling binary.
#[cfg(feature = "gateway-admission")]
pub async fn ensure_compatible_ready() -> Result<(), String> {
    admission::ensure(DAEMON_PORT, async || {
        let bin = bundled_daemon_binary()
            .ok_or_else(|| "could not locate the bundled aleph-server binary".to_string())?;
        spawn_detached(&bin, &["--bundled-content-only"]).await
    })
    .await
}

/// Give the detached server time to release its listener after a stop request.
pub async fn wait_until_port_closed() {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(6);
    while port_open(DAEMON_PORT).await != Some(false) {
        if tokio::time::Instant::now() >= deadline {
            tracing::warn!("daemon port still open after stop; continuing anyway");
            return;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

// Some(false) proves refusal; None is unknown and must not grant a launch.
fn classify_connect(result: Result<(), std::io::ErrorKind>) -> Option<bool> {
    match result {
        Ok(()) => Some(true),
        Err(std::io::ErrorKind::ConnectionRefused) => Some(false),
        Err(_) => None,
    }
}

async fn port_open(port: u16) -> Option<bool> {
    let result = tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect((DAEMON_HOST, port)))
        .await
        .ok()?;
    classify_connect(result.map(|_| ()).map_err(|error| error.kind()))
}

/// Platform-specific daemon binary file name.
const fn daemon_bin_name() -> &'static str {
    if cfg!(windows) {
        "aleph-server.exe"
    } else {
        "aleph-server"
    }
}

/// Resolve the `aleph-server` binary: the copy bundled next to the shell
/// executable, falling back to `PATH` for development service commands.
pub fn resolve_daemon_binary() -> Option<PathBuf> {
    let name = daemon_bin_name();

    if let Some(candidate) = bundled_daemon_binary() {
        return Some(candidate);
    }
    // 2. Anything on `PATH` (covers unusual dev setups).
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(name))
        .find(|p| p.is_file())
}

fn bundled_daemon_binary() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let candidate = exe.parent()?.join(daemon_bin_name());
    candidate.is_file().then_some(candidate)
}

#[cfg(unix)]
async fn settle_launcher(
    launcher: &mut tokio::process::Child,
    deadline: Duration,
) -> Result<(), String> {
    match tokio::time::timeout(deadline, launcher.wait()).await {
        Ok(Ok(status)) if status.success() => Ok(()),
        Ok(Ok(_)) => Err("daemon launcher exited unsuccessfully".to_string()),
        Ok(Err(_)) => Err("could not wait for daemon launcher".to_string()),
        Err(_) => {
            // Tokio kill awaits wait/reap, unlike std::process::Child::kill.
            launcher
                .kill()
                .await
                .map_err(|_| "daemon launcher timed out and cleanup failed".to_string())?;
            Err("daemon launcher timed out".to_string())
        }
    }
}

/// Spawn the daemon detached on Unix. `--daemon` makes `aleph-server`
/// double-fork and `setsid` itself, so the real daemon detaches; the
/// short-lived launcher is bounded and reaped without blocking the executor.
#[cfg(unix)]
async fn spawn_detached(bin: &Path, startup_options: &[&str]) -> Result<(), String> {
    use std::process::Stdio;

    let mut launcher = tokio::process::Command::new(bin)
        .arg("--daemon")
        .args(startup_options)
        .arg("start")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| format!("failed to spawn {}: {e}", bin.display()))?;
    settle_launcher(&mut launcher, READY_TIMEOUT).await
}

/// Spawn the daemon detached on Windows. There is no fork-based
/// daemonization, so we detach the process and a new process group and
/// never wait on it, letting it outlive the shell.
#[cfg(windows)]
async fn spawn_detached(bin: &Path, startup_options: &[&str]) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    use std::process::{Command, Stdio};

    const DETACHED_PROCESS: u32 = 0x0000_0008;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    Command::new(bin)
        .args(startup_options)
        .arg("start")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW)
        .spawn()
        .map_err(|e| format!("failed to spawn {}: {e}", bin.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connect_errors_are_not_free_port_evidence() {
        use std::io::ErrorKind;

        for error in [
            ErrorKind::TimedOut,
            ErrorKind::PermissionDenied,
            ErrorKind::ConnectionReset,
            ErrorKind::Interrupted,
            ErrorKind::Other,
        ] {
            assert_eq!(classify_connect(Err(error)), None);
        }
        assert_eq!(classify_connect(Ok(())), Some(true));
        assert_eq!(
            classify_connect(Err(ErrorKind::ConnectionRefused)),
            Some(false)
        );
    }

    #[tokio::test]
    async fn port_presence_observes_owned_listener_and_non_listener() {
        let listener = tokio::net::TcpListener::bind((DAEMON_HOST, 0))
            .await
            .unwrap();
        assert_eq!(
            port_open(listener.local_addr().unwrap().port()).await,
            Some(true)
        );

        // Keep the endpoint bound but not listening, so no other fixture can
        // acquire it. The OS may refuse or time out; derive that observation
        // independently instead of assuming drop means immediate refusal.
        let closed = tokio::net::TcpSocket::new_v4().unwrap();
        closed
            .bind((std::net::Ipv4Addr::LOCALHOST, 0).into())
            .unwrap();
        let address = closed.local_addr().unwrap();
        let observed = std::net::TcpStream::connect_timeout(&address, Duration::from_secs(6));
        assert!(
            observed.is_err(),
            "a non-listening fixture accepted a connection"
        );
        assert_eq!(
            port_open(address.port()).await,
            match observed.unwrap_err().kind() {
                std::io::ErrorKind::ConnectionRefused => Some(false),
                _ => None,
            }
        );
    }

    #[cfg(unix)]
    fn owned_launcher(script: &str) -> tokio::process::Child {
        use std::process::Stdio;

        tokio::process::Command::new("/bin/sh")
            .args(["-c", script])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .unwrap()
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn launcher_success_requires_actual_exit() {
        let mut launcher = owned_launcher("exit 0");
        let result = settle_launcher(&mut launcher, Duration::from_secs(5)).await;
        let exited = launcher.try_wait().unwrap().is_some();
        if !exited {
            launcher.kill().await.unwrap();
        }
        assert!(result.is_ok(), "{result:?}");
        assert!(exited, "success cannot precede real child exit");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn launcher_failure_is_reaped_not_reported_as_ready() {
        let mut launcher = owned_launcher("exit 7");
        let result = settle_launcher(&mut launcher, Duration::from_secs(5)).await;
        let exited = launcher.try_wait().unwrap().is_some();
        if !exited {
            launcher.kill().await.unwrap();
        }
        assert!(result.is_err());
        assert!(exited, "failure must also settle its owned child");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn launcher_timeout_kills_and_reaps_before_returning() {
        let mut launcher = owned_launcher("exec /bin/sleep 30");
        let result = tokio::time::timeout(
            Duration::from_secs(5),
            settle_launcher(&mut launcher, Duration::from_millis(20)),
        )
        .await;
        let exited = launcher.try_wait().unwrap().is_some();
        // This owner also cleans up when the implementation is RED or cancelled.
        if !exited {
            launcher.kill().await.unwrap();
        }
        assert!(
            result.is_ok(),
            "launcher settlement exceeded the test bound"
        );
        assert!(result.unwrap().unwrap_err().contains("timed out"));
        assert!(exited, "timeout returned before the real child was reaped");
    }
}
