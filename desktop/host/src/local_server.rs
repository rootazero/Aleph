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
    /// Nothing is listening — the port is free to use.
    Free,
    /// `aleph-server` is up and `/ready` reports 200.
    DaemonReady,
    /// `aleph-server` holds the port but is still booting (`/ready` → 503).
    DaemonBooting,
    /// Some other process holds the port — not `aleph-server`.
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
            launch_detached()?;
            wait_until_ready().await
        }
        // A non-daemon process holds the port. Waiting on `/ready` would
        // only burn the timeout, so fail fast with an actionable message.
        PortOccupant::Foreign => Err(format!(
            "port {DAEMON_PORT} is held by another process, not aleph-server; \
             free the port and relaunch Aleph"
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
            if let Err(e) = launch_detached() {
                tracing::error!("failed to relaunch daemon: {e}");
            }
        }
        PortOccupant::Foreign => tracing::warn!(
            "daemon port {DAEMON_PORT} held by a foreign process; \
             cannot relaunch aleph-server"
        ),
        PortOccupant::DaemonReady | PortOccupant::DaemonBooting => {}
    }
}

/// Poll until the daemon port is free, bounded so a stuck shutdown cannot
/// hang startup. A still-open port afterwards is harmless — `ensure_ready`
/// then treats it as a daemon mid-boot and waits on `/ready`.
pub async fn wait_until_port_closed() {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(6);
    while port_open().await {
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

/// Inspect the daemon port: free, ours, or taken by a stranger?
///
/// `aleph-server` serves `/ready` the moment it binds the port (200 when
/// ready, 503 while booting). Any other reply — a different status, or no
/// usable HTTP at all — means a foreign process holds the port, and the
/// shell must not wait on it as if it were a daemon mid-boot.
async fn probe_port() -> PortOccupant {
    if !port_open().await {
        return PortOccupant::Free;
    }
    classify_ready_status(http_get_status("/ready").await)
}

/// Map a `/ready` probe result to the kind of process holding the port.
const fn classify_ready_status(status: Option<u16>) -> PortOccupant {
    match status {
        Some(200) => PortOccupant::DaemonReady,
        Some(503) => PortOccupant::DaemonBooting,
        _ => PortOccupant::Foreign,
    }
}

/// True if something is listening on the daemon's port.
async fn port_open() -> bool {
    TcpStream::connect((DAEMON_HOST, DAEMON_PORT)).await.is_ok()
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
fn launch_detached() -> Result<(), String> {
    let bin = resolve_daemon_binary()
        .ok_or_else(|| "could not locate the aleph-server binary".to_string())?;
    tracing::info!("launching daemon: {}", bin.display());
    spawn_detached(&bin)
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

/// Spawn the daemon detached on Unix. `--daemon` makes `aleph-server`
/// double-fork and `setsid` itself, so the real daemon detaches; the
/// short-lived launcher we spawn is waited on only to reap it.
#[cfg(unix)]
fn spawn_detached(bin: &Path) -> Result<(), String> {
    use std::process::{Command, Stdio};

    let mut launcher = Command::new(bin)
        .arg("--daemon")
        .arg("start")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("failed to spawn {}: {e}", bin.display()))?;
    match launcher.wait() {
        Ok(status) if status.success() => {}
        Ok(status) => tracing::warn!("aleph-server launcher exited with {status}"),
        Err(e) => tracing::warn!("failed to wait for aleph-server launcher: {e}"),
    }
    Ok(())
}

/// Spawn the daemon detached on Windows. There is no fork-based
/// daemonization, so we detach the process and a new process group and
/// never wait on it, letting it outlive the shell.
#[cfg(windows)]
fn spawn_detached(bin: &Path) -> Result<(), String> {
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
}
