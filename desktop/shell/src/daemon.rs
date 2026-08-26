//! `aleph-server` daemon lifecycle.
//!
//! Locate the daemon binary, launch it detached when it is not already
//! running, wait until its HTTP `/ready` probe turns green, and — for the
//! lifetime of the shell — relaunch it if it ever disappears.
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

/// Best-effort `aleph-server stop` — used only by the explicit
/// "Quit & Stop Aleph" tray action.
pub fn stop_daemon() {
    let Some(bin) = resolve_daemon_binary() else {
        tracing::warn!("cannot stop daemon: aleph-server binary not found");
        return;
    };
    match std::process::Command::new(&bin).arg("stop").output() {
        Ok(out) if out.status.success() => tracing::info!(
            "aleph-server stop: {}",
            String::from_utf8_lossy(&out.stdout).trim()
        ),
        Ok(out) => tracing::warn!(
            "aleph-server stop exited with status {}: stderr={}",
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        ),
        Err(e) => tracing::warn!("aleph-server stop failed: {e}"),
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

/// Force any previously running daemon offline so the `aleph-server` bundled
/// inside this app takes over. Runs once per app version — first launch and
/// after every update — tracked by a marker file.
///
/// The pre-app bash installers registered a keep-alive autostart service for
/// a separate `aleph-server`; left in place it would resurrect a stale daemon
/// and shadow the bundled one. This removes that legacy service and stops
/// whatever daemon is currently running; `ensure_ready` then launches the
/// bundled binary.
pub async fn reconcile_for_version(version: &str) {
    let Some(marker) = version_marker() else {
        return;
    };
    if tokio::fs::read_to_string(&marker)
        .await
        .is_ok_and(|v| v.trim() == version)
    {
        return; // already reconciled for this app version — fast path
    }
    tracing::info!("reconciling daemon for app version {version}");

    remove_legacy_autostart();
    stop_daemon();
    wait_until_port_closed().await;

    if let Some(parent) = marker.parent() {
        let _ = tokio::fs::create_dir_all(parent).await;
    }
    if let Err(e) = tokio::fs::write(&marker, version).await {
        tracing::warn!("could not record daemon-version marker: {e}");
    }
}

/// Marker file recording the app version the daemon was last reconciled for.
fn version_marker() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".aleph").join(".desktop-shell-daemon-version"))
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

/// Best-effort removal of the bash-installer-era launchd autostart service so
/// a stale `aleph-server` cannot resurrect itself. A no-op when none exists.
#[cfg(target_os = "macos")]
fn remove_legacy_autostart() {
    use std::process::Command;

    let Some(plist) =
        dirs::home_dir().map(|h| h.join("Library/LaunchAgents/com.aleph.server.plist"))
    else {
        return;
    };
    if !plist.exists() {
        return;
    }
    // `launchctl bootout` needs the GUI domain target; resolve the uid
    // without pulling in a libc dependency.
    let mut bootout_ok = false;
    if let Ok(out) = Command::new("id").arg("-u").output() {
        let uid = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if !uid.is_empty() {
            bootout_ok = Command::new("launchctl")
                .args(["bootout", &format!("gui/{uid}/com.aleph.server")])
                .output()
                .is_ok_and(|o| o.status.success());
        }
    }
    if !bootout_ok {
        tracing::warn!("failed to unload legacy launchd service");
    }
    let _ = std::fs::remove_file(&plist);
    tracing::info!("removed legacy launchd autostart service");
}

/// Best-effort removal of the bash-installer-era systemd autostart service.
#[cfg(target_os = "linux")]
fn remove_legacy_autostart() {
    use std::process::Command;

    let Some(unit) = dirs::home_dir().map(|h| h.join(".config/systemd/user/aleph.service")) else {
        return;
    };
    if !unit.exists() {
        return;
    }
    let _ = Command::new("systemctl")
        .args(["--user", "disable", "--now", "aleph"])
        .output();
    let _ = std::fs::remove_file(&unit);
    tracing::info!("removed legacy systemd autostart service");
}

/// Best-effort removal of the PowerShell-installer-era Task Scheduler entry.
#[cfg(target_os = "windows")]
fn remove_legacy_autostart() {
    if !schtasks(&["/Query", "/TN", "AlephServer"]).is_ok_and(|o| o.status.success()) {
        return; // no legacy task registered
    }
    let _ = schtasks(&["/End", "/TN", "AlephServer"]);
    let _ = schtasks(&["/Delete", "/TN", "AlephServer", "/F"]);
    tracing::info!("removed legacy Task Scheduler autostart entry");
}

/// Run `schtasks` without flashing a console window.
#[cfg(target_os = "windows")]
fn schtasks(args: &[&str]) -> std::io::Result<std::process::Output> {
    use std::os::windows::process::CommandExt;

    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    std::process::Command::new("schtasks")
        .args(args)
        .creation_flags(CREATE_NO_WINDOW)
        .output()
}

/// Other platforms ship no legacy installer, so there is nothing to remove.
#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
fn remove_legacy_autostart() {}

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
    tokio::time::timeout(PROBE_TIMEOUT, http_get_status_inner(path))
        .await
        .ok()?
}

/// The unbounded body of [`http_get_status`]; always call it through the
/// timeout wrapper above so a stalled peer cannot hang the probe.
async fn http_get_status_inner(path: &str) -> Option<u16> {
    let mut stream = TcpStream::connect((DAEMON_HOST, DAEMON_PORT)).await.ok()?;
    let request =
        format!("GET {path} HTTP/1.0\r\nHost: {DAEMON_HOST}\r\nConnection: close\r\n\r\n");
    stream.write_all(request.as_bytes()).await.ok()?;

    // The status line lives in the first packet; 512 bytes is ample.
    let mut chunk = [0u8; 512];
    let n = stream.read(&mut chunk).await.ok()?;
    let head = String::from_utf8_lossy(chunk.get(..n)?);
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
fn resolve_daemon_binary() -> Option<PathBuf> {
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
}
