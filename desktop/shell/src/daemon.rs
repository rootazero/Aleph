//! Old-shell explicit stop and legacy version reconciliation only.
//! Shared startup/readiness lives in aleph-desktop-host; NSH must not
//! inherit the destructive reconciliation below.
pub use aleph_desktop_host::local_server::{
    ensure_ready, is_ready, relaunch_if_down, resolve_daemon_binary, wait_until_port_closed,
};
use std::path::PathBuf;

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

    let _ = tokio::task::spawn_blocking(|| {
        remove_legacy_autostart();
        stop_daemon();
    })
    .await;
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
