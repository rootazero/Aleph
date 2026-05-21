//! `aleph-server` daemon lifecycle.
//!
//! Locate the daemon binary, launch it detached when it is not already
//! running, and wait until its HTTP `/ready` probe turns green.
//!
//! The daemon is deliberately NOT a child of the shell: the shell may quit
//! while the daemon keeps serving (R5/R6). We launch it detached and never
//! reap or signal it — except the explicit "Quit & Stop" tray action.

use std::path::{Path, PathBuf};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

const DAEMON_HOST: &str = "127.0.0.1";
const DAEMON_PORT: u16 = 18790;
/// How long to wait for the daemon to report ready before giving up.
const READY_TIMEOUT: Duration = Duration::from_secs(40);
/// Interval between `/ready` polls.
const POLL_INTERVAL: Duration = Duration::from_millis(400);

/// Ensure the daemon is running and ready to serve the Panel.
pub async fn ensure_ready() -> Result<(), String> {
    if is_ready().await {
        return Ok(());
    }
    // If the port is already open the daemon is mid-boot (or a second start
    // would just hit the flock and exit 64) — wait rather than relaunch.
    if !port_open().await {
        launch_detached()?;
    }
    wait_until_ready().await
}

/// Best-effort `aleph-server stop` — used only by the explicit
/// "Quit & Stop Aleph" tray action.
pub fn stop_daemon() {
    let Some(bin) = resolve_daemon_binary() else {
        tracing::warn!("cannot stop daemon: aleph-server binary not found");
        return;
    };
    match std::process::Command::new(&bin).arg("stop").output() {
        Ok(out) => tracing::info!(
            "aleph-server stop: {}",
            String::from_utf8_lossy(&out.stdout).trim()
        ),
        Err(e) => tracing::warn!("aleph-server stop failed: {e}"),
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

/// True once the daemon's `/ready` probe reports 200 (boot complete).
async fn is_ready() -> bool {
    matches!(http_get_status("/ready").await, Some(200))
}

/// True if something is listening on the daemon's port.
async fn port_open() -> bool {
    TcpStream::connect((DAEMON_HOST, DAEMON_PORT)).await.is_ok()
}

/// Minimal HTTP/1.0 GET that returns just the numeric status code. Avoids
/// pulling a full HTTP client into the shell for a single localhost probe.
async fn http_get_status(path: &str) -> Option<u16> {
    let mut stream = TcpStream::connect((DAEMON_HOST, DAEMON_PORT)).await.ok()?;
    let request =
        format!("GET {path} HTTP/1.0\r\nHost: {DAEMON_HOST}\r\nConnection: close\r\n\r\n");
    stream.write_all(request.as_bytes()).await.ok()?;

    // The status line lives in the first packet; 512 bytes is ample.
    let mut chunk = [0u8; 512];
    let n = stream.read(&mut chunk).await.ok()?;
    let head = String::from_utf8_lossy(&chunk[..n]);
    // e.g. "HTTP/1.1 200 OK"
    head.split_whitespace().nth(1)?.parse().ok()
}

/// Launch `aleph-server` fully detached from the shell process.
fn launch_detached() -> Result<(), String> {
    let bin = resolve_daemon_binary()
        .ok_or_else(|| "could not locate the aleph-server binary".to_string())?;
    tracing::info!("launching daemon: {}", bin.display());
    spawn_detached(&bin)
}

/// Platform-specific daemon binary file name.
fn daemon_bin_name() -> &'static str {
    if cfg!(windows) {
        "aleph-server.exe"
    } else {
        "aleph-server"
    }
}

/// Resolve the `aleph-server` binary: bundled next to the shell, then a
/// separately installed copy, then anything on `PATH`.
fn resolve_daemon_binary() -> Option<PathBuf> {
    let name = daemon_bin_name();

    // 1. Bundled / dev build — a sibling of the shell executable.
    if let Ok(exe) = std::env::current_exe() {
        if let Some(candidate) = exe.parent().map(|d| d.join(name)) {
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    // 2. A separately installed daemon (install.sh / install.ps1 target).
    if let Some(home) = dirs::home_dir() {
        let candidate = home.join(".aleph").join("bin").join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    // 3. Anything on PATH.
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
    let _ = launcher.wait();
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
