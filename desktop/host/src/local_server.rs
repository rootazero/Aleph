//! Shared local-server startup I/O, with no runtime or process at import.
//! HTTP readiness is not binary identity or permission to stop a service.
//! Legacy version takeover and login-service removal remain in the old shell.
use std::path::{Path, PathBuf};
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt};
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

/// What is currently answering on the daemon's port.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PortOccupant {
    /// TCP connection was refused; this is not proof a future bind will succeed.
    Free,
    /// A listener returned 200 on `/ready`, without binary identity proof.
    DaemonReady,
    /// A listener returned 503 on `/ready`, without binary identity proof.
    DaemonBooting,
    /// Unrecognized readiness or an unknown connection outcome.
    Foreign,
}

/// Ensure the daemon is running and ready to serve the Panel.
pub async fn ensure_ready() -> Result<(), String> {
    match probe_port().await {
        PortOccupant::DaemonReady => Ok(()),
        // The daemon is mid-boot (or a second `start` would just hit the
        // flock and exit 64) — wait rather than relaunch.
        PortOccupant::DaemonBooting => wait_until_ready().await,
        PortOccupant::Free => {
            launch_detached().await?;
            wait_until_ready().await
        }
        // Unrecognized or unknown readiness never authorizes another launch.
        PortOccupant::Foreign => Err(format!(
            "cannot establish aleph-server readiness on port {DAEMON_PORT}; \
             no new daemon was launched"
        )),
    }
}

/// Bring the daemon back if it has gone away: when the port is free,
/// relaunch it; otherwise leave whatever holds it untouched. Non-blocking —
/// it does not wait for the daemon to finish booting. Used by the shell's
/// health supervisor to recover from a mid-session daemon crash.
pub async fn relaunch_if_down() {
    match probe_port().await {
        PortOccupant::Free => {
            if let Err(e) = launch_detached().await {
                tracing::error!("failed to relaunch daemon: {e}");
            }
        }
        PortOccupant::Foreign => tracing::warn!(
            "local server readiness is unknown on port {DAEMON_PORT}; \
             leaving the listener and installed services untouched"
        ),
        PortOccupant::DaemonReady | PortOccupant::DaemonBooting => {}
    }
}

/// Legacy close waiting; a timeout is logged but its unit return is not a
/// stop acknowledgement. A new NSH lifecycle owner must not treat it as one.
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

/// Poll `/ready` until it returns 200 or the timeout elapses.
async fn wait_until_ready() -> Result<(), String> {
    let deadline = tokio::time::Instant::now() + READY_TIMEOUT;
    loop {
        if is_ready().await {
            return Ok(());
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(format!(
                "aleph-server did not report ready within {}s",
                READY_TIMEOUT.as_secs()
            ));
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// True once the daemon's `/ready` probe reports 200 (boot complete). Used
/// by `wait_until_ready` and by the shell's health supervisor.
pub async fn is_ready() -> bool {
    matches!(http_get_status("/ready").await, Some(200))
}

/// Classify listener readiness, not executable identity or service ownership.
/// `aleph-server` uses 200/503 on `/ready`; other replies and unknown connect
/// outcomes fail closed and do not authorize a launch.
async fn probe_port() -> PortOccupant {
    match port_open(DAEMON_PORT).await {
        Some(true) => classify_ready_status(http_get_status("/ready").await),
        Some(false) => PortOccupant::Free,
        None => PortOccupant::Foreign,
    }
}

/// Map a `/ready` probe result to the kind of process holding the port.
const fn classify_ready_status(status: Option<u16>) -> PortOccupant {
    match status {
        Some(200) => PortOccupant::DaemonReady,
        Some(503) => PortOccupant::DaemonBooting,
        _ => PortOccupant::Foreign,
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
    let result = tokio::time::timeout(PROBE_TIMEOUT, TcpStream::connect((DAEMON_HOST, port)))
        .await
        .ok()?;
    classify_connect(result.map(|_| ()).map_err(|error| error.kind()))
}

// `tcp_reachable` (bare TCP connect for a remote Gateway) lived here and is
// deliberately gone rather than kept "for future use" (R10). It answered a
// different question than its caller asked — a CDN edge or a port-forward to
// nothing completes the handshake and then closes, which it reported as
// healthy — and the remote supervisor now shares the lite shell's
// `gateway_probe::target_reachable`, which asks for `/ready` over the target's
// own scheme. `port_open` above stays: it is loopback-only and its caller
// (`probe_port`) immediately follows it with the HTTP status check.

/// Minimal HTTP/1.0 GET that returns just the numeric status code. Avoids
/// pulling a full HTTP client into the shell for a single localhost probe.
/// The whole exchange is bounded by [`PROBE_TIMEOUT`].
async fn http_get_status(path: &str) -> Option<u16> {
    tokio::time::timeout(PROBE_TIMEOUT, http_get_status_inner(DAEMON_PORT, path))
        .await
        .ok()?
}

/// The unbounded body of [`http_get_status`]; always call it through the
/// timeout wrapper above so a stalled peer cannot hang the probe.
async fn http_get_status_inner(port: u16, path: &str) -> Option<u16> {
    let mut stream = TcpStream::connect((DAEMON_HOST, port)).await.ok()?;
    let request =
        format!("GET {path} HTTP/1.0\r\nHost: {DAEMON_HOST}\r\nConnection: close\r\n\r\n");
    stream.write_all(request.as_bytes()).await.ok()?;

    // TCP reads are not HTTP message boundaries. Bound the complete status line,
    // including CRLF; the outer timeout also bounds peers that never finish it.
    let mut line = Vec::new();
    tokio::io::BufReader::with_capacity(512, stream.take(512))
        .read_until(b'\n', &mut line)
        .await
        .ok()?;
    if !line.ends_with(b"\r\n") {
        return None;
    }
    let mut fields = std::str::from_utf8(&line[..line.len() - 2])
        .ok()?
        .splitn(3, ' ');
    if !matches!(fields.next()?, "HTTP/1.0" | "HTTP/1.1") {
        return None;
    }
    let code = fields.next()?;
    if code.len() != 3 || !code.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    if fields
        .next()?
        .bytes()
        .any(|byte| (byte < 32 && byte != b'\t') || byte == 127)
    {
        return None;
    }
    let status = code.parse().ok()?;
    (100..=599).contains(&status).then_some(status)
}

/// Launch `aleph-server` fully detached from the shell process.
async fn launch_detached() -> Result<(), String> {
    let bin = resolve_daemon_binary()
        .ok_or_else(|| "could not locate the aleph-server binary".to_string())?;
    tracing::info!("launching daemon: {}", bin.display());
    spawn_detached(&bin).await
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
/// executable (Tauri `externalBin`, also where `cargo run` leaves it),
/// falling back to anything on `PATH`.
pub fn resolve_daemon_binary() -> Option<PathBuf> {
    let name = daemon_bin_name();

    // 1. Bundled inside the app, or a sibling of the dev build.
    if let Ok(exe) = std::env::current_exe() {
        if let Some(candidate) = exe.parent().map(|d| d.join(name)) {
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    // 2. Anything on `PATH` (covers unusual dev setups).
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(name))
        .find(|p| p.is_file())
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
async fn spawn_detached(bin: &Path) -> Result<(), String> {
    use std::process::Stdio;

    let mut launcher = tokio::process::Command::new(bin)
        .arg("--daemon")
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
async fn spawn_detached(bin: &Path) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    use std::process::{Command, Stdio};

    const DETACHED_PROCESS: u32 = 0x0000_0008;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    Command::new(bin)
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
    fn classify_ready_status_recognises_the_daemon() {
        assert_eq!(classify_ready_status(Some(200)), PortOccupant::DaemonReady);
        assert_eq!(
            classify_ready_status(Some(503)),
            PortOccupant::DaemonBooting
        );
    }

    #[test]
    fn classify_ready_status_treats_non_daemon_replies_as_foreign() {
        // A different HTTP service answering on the port.
        assert_eq!(classify_ready_status(Some(404)), PortOccupant::Foreign);
        assert_eq!(classify_ready_status(Some(500)), PortOccupant::Foreign);
        assert_eq!(classify_ready_status(Some(201)), PortOccupant::Foreign);
        // Connection accepted, but no usable HTTP reply at all.
        assert_eq!(classify_ready_status(None), PortOccupant::Foreign);
    }

    // Owned ephemeral listeners exercise the real parser, never an installed service.
    async fn scripted_status(parts: Vec<Vec<u8>>) -> Option<u16> {
        let listener = tokio::net::TcpListener::bind((DAEMON_HOST, 0))
            .await
            .unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = tokio::spawn(async move {
            let (mut peer, _) = listener.accept().await.unwrap();
            let mut request = [0; 1024];
            assert!(peer.read(&mut request).await.unwrap() > 0);
            for (index, part) in parts.into_iter().enumerate() {
                if index != 0 {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
                if peer.write_all(&part).await.is_err() {
                    break;
                }
            }
        });
        let result = tokio::time::timeout(PROBE_TIMEOUT, http_get_status_inner(port, "/ready"))
            .await
            .expect("owned readiness peer did not finish");
        server.await.unwrap();
        result
    }

    #[tokio::test]
    async fn readiness_status_line_can_cross_tcp_reads() {
        for (first, second, expected) in [
            (b"HTTP/1.1 2".as_slice(), b"00 OK\r\n".as_slice(), 200),
            (b"HTTP/1.0 50".as_slice(), b"3 Booting\r\n".as_slice(), 503),
        ] {
            assert_eq!(
                scripted_status(vec![first.to_vec(), second.to_vec()]).await,
                Some(expected)
            );
        }
    }

    #[tokio::test]
    async fn malformed_incomplete_or_oversized_status_lines_are_not_ready() {
        let mut oversized = b"HTTP/1.1 200 ".to_vec();
        oversized.extend(std::iter::repeat_n(b'x', 600));
        oversized.extend_from_slice(b"\r\n");
        for line in [
            b"not-http 200 OK\r\n".to_vec(),
            b"HTTP/1.1 200".to_vec(),
            b"HTTP/1.1 200\r\n".to_vec(),
            b"HTTP/1.1 0200 OK\r\n".to_vec(),
            b"HTTP/1.1 2x0 OK\r\n".to_vec(),
            b"HTTP/1.1 600 OK\r\n".to_vec(),
            b"HTTP/1.1 200 OK\0\r\n".to_vec(),
            oversized,
        ] {
            assert_eq!(scripted_status(vec![line]).await, None);
        }
    }

    #[tokio::test]
    async fn complete_http_statuses_keep_existing_classification() {
        for status in [200, 503, 404, 500] {
            let line = format!("HTTP/1.1 {status} fixture\r\n").into_bytes();
            assert_eq!(scripted_status(vec![line]).await, Some(status));
        }
    }

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
        let observed = std::net::TcpStream::connect_timeout(&address, Duration::from_millis(20));
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
