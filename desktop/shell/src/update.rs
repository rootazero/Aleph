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
const CHECK_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);

/// Shared update state, managed by Tauri so the background checker, the
/// tray, and the macOS menu agree on whether an update is waiting.
#[derive(Default)]
pub struct Updater {
    /// The version of a found-but-not-yet-applied update, if any.
    staged: Mutex<Option<String>>,
    /// The tray's update menu item, registered by `tray.rs` so the checker
    /// can relabel it once an update is staged.
    tray_item: Mutex<Option<MenuItem<Wry>>>,
}

impl Updater {
    /// Hand the background checker the tray item it should relabel.
    pub fn attach_tray_item(&self, item: MenuItem<Wry>) {
        *self.tray_item.lock().unwrap_or_else(|e| e.into_inner()) = Some(item);
    }
}

/// True once an update has been found and is waiting to be applied.
pub fn has_staged_update(app: &AppHandle) -> bool {
    app.state::<Updater>()
        .staged
        .lock()
        .unwrap_or_else(|e| e.into_inner())
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
        match update.download_and_install(|_, _| {}, || {}).await {
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
            notify(
                app,
                "Update available",
                &format!(
                    "Aleph v{} is ready — choose \"Restart to update\" from the tray \
                     or the Aleph menu.",
                    update.version
                ),
            );
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

/// Record a staged update and relabel the tray's update item.
fn stage(app: &AppHandle, version: &str) {
    *app.state::<Updater>()
        .staged
        .lock()
        .unwrap_or_else(|e| e.into_inner()) = Some(version.to_string());
    set_tray_update_label(app, staged_tray_label(version));
}

/// Relabel the tray's update item on the main thread — macOS menu mutation
/// must not happen off the UI thread.
fn set_tray_update_label(app: &AppHandle, label: String) {
    let app = app.clone();
    let dispatch = app.run_on_main_thread({
        let app = app.clone();
        move || {
            if let Some(item) = app
                .state::<Updater>()
                .tray_item
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .as_ref()
            {
                let _ = item.set_text(&label);
            }
        }
    });
    if let Err(e) = dispatch {
        tracing::debug!("could not relabel the tray update item: {e}");
    }
}

/// The tray item's label once an update is staged.
fn staged_tray_label(version: &str) -> String {
    format!("Restart to update to v{version}")
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
    fn staged_tray_label_names_the_version() {
        assert_eq!(
            staged_tray_label("26.5.30"),
            "Restart to update to v26.5.30"
        );
    }
}
