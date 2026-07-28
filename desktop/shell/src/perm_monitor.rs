//! macOS permission change monitor.
//!
//! Watches TCC permissions that require a daemon restart to take effect
//! (`input_monitoring`, `screen_recording`). When the app returns to foreground
//! and a permission that was previously denied is now granted, triggers a
//! daemon restart so the user never has to manually "restart aleph-server".
//!
//! Everything here is macOS-only (TCC is an Apple concept); on other platforms
//! `start_monitor` is a no-op and none of the implementation is compiled.

/// Start a background task that polls permission status and restarts the
/// daemon when a watched permission transitions from denied → granted.
///
/// No-op on non-macOS platforms: there are no TCC permissions that require a
/// daemon restart.
#[cfg(not(target_os = "macos"))]
pub fn start_monitor(handle: tauri::AppHandle) {
    let _ = handle;
}

#[cfg(target_os = "macos")]
pub use macos::start_monitor;

#[cfg(target_os = "macos")]
mod macos {
    use std::time::Duration;

    use tauri::AppHandle;

    use crate::daemon;

    /// Permissions that require a daemon restart when granted.
    const RESTART_PERMISSIONS: &[PermissionKind] = &[
        PermissionKind::InputMonitoring,
        PermissionKind::ScreenRecording,
    ];

    /// How often to poll permission status while the app is in foreground.
    const POLL_INTERVAL: Duration = Duration::from_secs(3);

    /// Permission kinds that matter for daemon restart.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum PermissionKind {
        InputMonitoring,
        ScreenRecording,
    }

    /// Tracks the last-known state of restart-requiring permissions.
    #[derive(Debug, Default)]
    struct PermissionState {
        input_monitoring: Option<bool>,
        screen_recording: Option<bool>,
    }

    impl PermissionState {
        const fn get(&self, kind: PermissionKind) -> Option<bool> {
            match kind {
                PermissionKind::InputMonitoring => self.input_monitoring,
                PermissionKind::ScreenRecording => self.screen_recording,
            }
        }

        const fn set(&mut self, kind: PermissionKind, granted: bool) {
            match kind {
                PermissionKind::InputMonitoring => self.input_monitoring = Some(granted),
                PermissionKind::ScreenRecording => self.screen_recording = Some(granted),
            }
        }
    }

    /// Start a background task that polls permission status and restarts the
    /// daemon when a watched permission transitions from denied → granted.
    pub fn start_monitor(handle: AppHandle) {
        // Use Tauri's managed runtime: `start_monitor` is called from the setup
        // closure on the main thread, which is not inside a Tokio reactor, so a
        // bare `tokio::spawn` would panic ("no reactor running"). This mirrors
        // the spawn idiom used everywhere else in the shell crate.
        tauri::async_runtime::spawn(async move {
            let mut state = PermissionState::default();

            loop {
                tokio::time::sleep(POLL_INTERVAL).await;

                for &kind in RESTART_PERMISSIONS {
                    let granted = check_permission(kind);
                    let previous = state.get(kind);

                    // First poll: just record the baseline.
                    if previous.is_none() {
                        state.set(kind, granted);
                        continue;
                    }

                    // Transition: denied → granted → restart daemon.
                    if previous == Some(false) && granted {
                        tracing::info!(
                            "permission {:?} granted — restarting daemon to pick it up",
                            kind
                        );
                        restart_daemon(&handle).await;
                        state.set(kind, granted);
                    } else if previous != Some(granted) {
                        // Any other change (granted → denied, etc.) just record.
                        state.set(kind, granted);
                    }
                }
            }
        });
    }

    /// Query a single permission via the Swift bridge.
    ///
    /// This spawns a short-lived `aleph-bridge` process in CLI mode to check
    /// the permission, avoiding coupling to the long-lived RPC bridge.
    fn check_permission(kind: PermissionKind) -> bool {
        use std::process::Command;

        let kind_str = match kind {
            PermissionKind::InputMonitoring => "input_monitoring",
            PermissionKind::ScreenRecording => "screen_recording",
        };

        // Find the aleph-bridge binary next to the shell executable.
        // The macOS bundle ships the helper as CamelCase `AlephBridge` (the
        // bundle's own executable). Look for that first, then fall back to
        // lowercase for non-bundled installs (PATH layout).
        let bridge_bin = match std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(|p| p.join("AlephBridge")))
        {
            Some(p) if p.exists() => p,
            _ => match std::env::current_exe()
                .ok()
                .and_then(|exe| exe.parent().map(|p| p.join("aleph-bridge")))
            {
                Some(p) if p.exists() => p,
                _ => {
                    if let Some(p) =
                        find_in_path("AlephBridge").or_else(|| find_in_path("aleph-bridge"))
                    {
                        p
                    } else {
                        tracing::debug!("aleph-bridge not found — skipping permission check");
                        return false;
                    }
                }
            },
        };

        match Command::new(&bridge_bin)
            .args(["perm-check", kind_str])
            .output()
        {
            Ok(out) if out.status.success() => {
                let stdout = String::from_utf8_lossy(&out.stdout);
                stdout.trim() == "granted"
            }
            Ok(out) => {
                tracing::warn!(
                    "perm-check failed: {}",
                    String::from_utf8_lossy(&out.stderr)
                );
                false
            }
            Err(e) => {
                tracing::warn!("failed to spawn aleph-bridge for perm-check: {e}");
                false
            }
        }
    }

    /// Restart the daemon: stop, wait, then ensure it's back up.
    async fn restart_daemon(handle: &AppHandle) {
        daemon::stop_daemon();
        daemon::wait_until_port_closed().await;
        match daemon::ensure_ready().await {
            Ok(()) => {
                tracing::info!("daemon restarted successfully after permission grant");
                // Hard-reload the Panel so it picks up the restarted daemon's new
                // capabilities. A plain location.reload() is a *soft* reload that
                // reuses in-session css/js/wasm (menu::reload_panel documents why);
                // reuse that shared hard-reload path instead of the soft reload.
                crate::menu::reload_panel(handle);
            }
            Err(e) => {
                tracing::error!("daemon failed to restart after permission grant: {e}");
            }
        }
    }

    fn find_in_path(name: &str) -> Option<std::path::PathBuf> {
        std::env::var_os("PATH").and_then(|path| {
            std::env::split_paths(&path)
                .map(|dir| dir.join(name))
                .find(|p| p.is_file())
        })
    }
}
