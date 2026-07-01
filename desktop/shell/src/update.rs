//! Background auto-update.
//!
//! Checks GitHub Releases for a newer Aleph and — never stealing focus or
//! restarting under the user (R5) — surfaces an available update through
//! the tray and a desktop notification. Applying it is always the user's
//! explicit choice; doing so downloads, installs, and restarts the app
//! (and, with it, the bundled `aleph-server`).
//!
//! Best-effort: an unreachable or unconfigured update endpoint is logged
//! and the rest of the shell carries on unaffected.

use std::sync::Mutex;
use std::time::Duration;

use tauri::menu::MenuItem;
use tauri::{AppHandle, Manager, Wry};
use tauri_plugin_notification::NotificationExt;
use tauri_plugin_updater::UpdaterExt;

/// Delay before the first check — let the daemon boot and the Panel settle.
const FIRST_CHECK_DELAY: Duration = Duration::from_secs(90);
/// Interval between background checks for a long-running shell.
const CHECK_INTERVAL: Duration = Duration::from_hours(6);

/// Where to send users whose install can't self-update (Linux package
/// installs): the GitHub releases page.
const RELEASES_URL: &str = "https://github.com/rootazero/Aleph/releases/latest";

/// Shared update state, managed by Tauri so the background checker, the
/// tray, and the macOS menu agree on whether an update is waiting.
#[derive(Default)]
pub struct Updater {
    /// The version of a found-but-not-yet-applied update, if any.
    staged: Mutex<Option<String>>,
    /// The update menu items (tray, and the macOS app menu) registered by
    /// their builders so the checker can relabel them once an update is
    /// staged. Both surfaces stay in sync.
    update_items: Mutex<Vec<MenuItem<Wry>>>,
}

impl Updater {
    /// Register an update menu item to be relabeled once an update is staged.
    /// Called by both the tray and the macOS app menu.
    pub fn attach_update_item(&self, item: MenuItem<Wry>) {
        self.update_items
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(item);
    }
}

/// Whether Tauri's bundled updater can self-install on this platform/install.
///
/// Tauri's Linux updater only supports the AppImage format — it replaces the
/// running AppImage in place, keyed off the `APPIMAGE` env var the AppImage
/// runtime sets. A package-manager install (.deb / .rpm) has no `APPIMAGE`
/// and must be updated through the package manager instead. macOS and Windows
/// always self-install.
fn updater_can_self_install() -> bool {
    #[cfg(target_os = "linux")]
    {
        std::env::var_os("APPIMAGE").is_some()
    }
    #[cfg(not(target_os = "linux"))]
    {
        true
    }
}

/// True once an update has been found and is waiting to be applied.
pub fn has_staged_update(app: &AppHandle) -> bool {
    app.state::<Updater>()
        .staged
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .is_some()
}

/// The background update checker: one delayed first check, then a slow
/// poll for the lifetime of the shell. Silent unless it finds an update.
pub async fn run_update_checker(app: AppHandle) {
    tokio::time::sleep(FIRST_CHECK_DELAY).await;
    loop {
        if !has_staged_update(&app) {
            check(&app, Announce::OnlyOnUpdate).await;
        }
        tokio::time::sleep(CHECK_INTERVAL).await;
    }
}

/// Run a user-initiated check now (from the tray or the macOS menu). Unlike
/// the background checker it always reports the outcome.
pub fn check_now(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        check(&app, Announce::Always).await;
    });
}

/// Download, install, and restart into the staged update — the user has
/// explicitly asked for it, so the restart is expected.
pub fn apply_staged_update(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        // Package-manager installs (Linux .deb / .rpm) can't be self-installed
        // by Tauri's updater — point the user at the right path instead of
        // attempting a download_and_install that would fail.
        if !updater_can_self_install() {
            notify(
                &app,
                "Update via your package manager",
                &format!(
                    "Aleph was installed with your system package manager. Update \
                     with apt / dnf, or download the latest release from {RELEASES_URL}."
                ),
            );
            return;
        }
        notify(
            &app,
            "Updating Aleph",
            "Downloading the update — Aleph will restart shortly.",
        );
        let updater = match app.updater() {
            Ok(u) => u,
            Err(e) => {
                tracing::error!("updater unavailable: {e}");
                return;
            }
        };
        let update = match updater.check().await {
            Ok(Some(u)) => u,
            Ok(None) => {
                tracing::info!("nothing to apply — already up to date");
                return;
            }
            Err(e) => {
                tracing::error!("update re-check failed: {e}");
                notify(&app, "Update failed", "Could not reach the update server.");
                return;
            }
        };
        // Download first while the daemon is still running; the installer will
        // try to overwrite aleph-server.exe, which Windows locks while it is
        // executing. We stop the daemon only once the package is local so the
        // service interruption is as short as possible.
        let bytes = match update.download(|_, _| {}, || {}).await {
            Ok(b) => b,
            Err(e) => {
                tracing::error!("update download failed: {e}");
                notify(&app, "Update failed", "Could not download the update.");
                return;
            }
        };
        stop_daemon_for_update().await;
        match update.install(&bytes) {
            Ok(()) => {
                tracing::info!("update installed — restarting");
                app.restart();
            }
            Err(e) => {
                tracing::error!("update install failed: {e}");
                notify(&app, "Update failed", "Could not install the update.");
            }
        }
    });
}

/// Stop the bundled `aleph-server` so the installer can replace its binary.
#[cfg(feature = "embedded-core")]
async fn stop_daemon_for_update() {
    crate::daemon::stop_daemon();
    crate::daemon::wait_until_port_closed().await;
}

/// Panel-only shells have no bundled daemon to stop.
#[cfg(not(feature = "embedded-core"))]
async fn stop_daemon_for_update() {}

/// Whether a check announces a no-update / failure outcome to the user.
#[derive(Clone, Copy)]
enum Announce {
    /// Stay silent unless an update is found (background checks).
    OnlyOnUpdate,
    /// Always report the outcome (user-initiated checks).
    Always,
}

/// Probe the update endpoint once and fold the result into shared state.
async fn check(app: &AppHandle, announce: Announce) {
    let updater = match app.updater() {
        Ok(u) => u,
        Err(e) => {
            tracing::debug!("updater unavailable: {e}");
            if matches!(announce, Announce::Always) {
                notify(app, "Update check failed", "The updater is not configured.");
            }
            return;
        }
    };
    match updater.check().await {
        Ok(Some(update)) => {
            tracing::info!("update available: v{}", update.version);
            stage(app, &update.version);
            if updater_can_self_install() {
                notify(
                    app,
                    "Update available",
                    &format!(
                        "Aleph v{} is ready — choose \"Restart to update\" from the tray \
                         or the Aleph menu.",
                        update.version
                    ),
                );
            } else {
                notify(
                    app,
                    "Update available",
                    &format!(
                        "Aleph v{} is available. You installed Aleph via your package \
                         manager — update with apt / dnf, or download it from {RELEASES_URL}.",
                        update.version
                    ),
                );
            }
        }
        Ok(None) => {
            tracing::debug!("no update available");
            if matches!(announce, Announce::Always) {
                notify(
                    app,
                    "Aleph is up to date",
                    "You are running the latest version.",
                );
            }
        }
        Err(e) => {
            tracing::debug!("update check failed: {e}");
            if matches!(announce, Announce::Always) {
                notify(
                    app,
                    "Update check failed",
                    "Could not reach the update server.",
                );
            }
        }
    }
}

/// Record a staged update and relabel every registered update item.
fn stage(app: &AppHandle, version: &str) {
    *app.state::<Updater>()
        .staged
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(version.to_string());
    relabel_update_items(app, staged_label(version));
}

/// Relabel the registered update items (tray + macOS menu) on the main
/// thread — menu mutation must not happen off the UI thread.
fn relabel_update_items(app: &AppHandle, label: String) {
    let app = app.clone();
    let dispatch = app.run_on_main_thread({
        let app = app.clone();
        move || {
            let updater = app.state::<Updater>();
            let items = updater
                .update_items
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            for item in items.iter() {
                let _ = item.set_text(&label);
            }
        }
    });
    if let Err(e) = dispatch {
        tracing::debug!("could not relabel the update menu items: {e}");
    }
}

/// The update item's label once an update is staged. Installs that can
/// self-update offer the restart-to-apply action; package-manager installs
/// (which can't) are pointed at how to update instead.
fn staged_label(version: &str) -> String {
    if updater_can_self_install() {
        restart_label(version)
    } else {
        manual_update_label(version)
    }
}

/// Label for the apply-and-restart action (self-installing platforms).
fn restart_label(version: &str) -> String {
    format!("Restart to update to v{version}")
}

/// Label for installs that must update via their package manager.
fn manual_update_label(version: &str) -> String {
    format!("Update v{version} available — how to update")
}

/// Raise a desktop notification, best-effort.
fn notify(app: &AppHandle, title: &str, body: &str) {
    if let Err(e) = app.notification().builder().title(title).body(body).show() {
        tracing::debug!("failed to show update notification: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restart_label_names_the_version() {
        assert_eq!(restart_label("26.5.30"), "Restart to update to v26.5.30");
    }

    #[test]
    fn manual_update_label_names_the_version() {
        assert_eq!(
            manual_update_label("26.5.30"),
            "Update v26.5.30 available — how to update"
        );
    }
}
