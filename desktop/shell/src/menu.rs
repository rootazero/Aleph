//! macOS application menu.
//!
//! macOS shows an app menu in the system menu bar whatever the window's
//! chrome. A tailored menu lets the shell wire items to its own actions
//! while keeping the predefined Edit items the webview needs for native
//! text editing (Cmd+C / V / X / A). macOS-only — Windows and Linux keep
//! their chromeless look with no menu.

use tauri::menu::{Menu, MenuItem, PredefinedMenuItem, Submenu};
use tauri::{AppHandle, Manager, Wry};

const ID_SHOW: &str = "menu_show";
const ID_OPEN_BROWSER: &str = "menu_open_browser";
const ID_CONNECT_REMOTE: &str = "menu_connect_remote";
const ID_CONNECT_LOCAL: &str = "menu_connect_local";
const ID_CHECK_UPDATE: &str = "menu_check_update";
const ID_QUIT: &str = "menu_quit";
const ID_QUIT_STOP: &str = "menu_quit_stop";
const ID_RELOAD_PANEL: &str = "menu_reload_panel";
// Only referenced inside `#[cfg(debug_assertions)]` blocks (DevTools ships
// in debug builds only), so the constant must be gated the same way or it
// reads as dead code in release.
#[cfg(debug_assertions)]
const ID_OPEN_DEVTOOLS: &str = "menu_open_devtools";
const ID_VISIT_REPO: &str = "menu_visit_repo";
const ID_REPORT_ISSUE: &str = "menu_report_issue";

/// GitHub repository — the project's source of truth. Used by Help menu.
const REPO_URL: &str = "https://github.com/rootazero/Aleph";
const ISSUES_URL: &str = "https://github.com/rootazero/Aleph/issues/new";

/// Build the macOS app menu: Aleph / Edit / View / Window / Help.
pub fn build(app: &AppHandle) -> tauri::Result<Menu<Wry>> {
    // Dual-purpose, mirroring the tray: a manual update check until an update
    // is found, then the button to apply it. Registered with the updater so it
    // is relabeled (alongside the tray item) once an update is staged.
    let update_item = MenuItem::with_id(
        app,
        ID_CHECK_UPDATE,
        "Check for Updates…",
        true,
        None::<&str>,
    )?;
    app.state::<crate::update::Updater>()
        .attach_update_item(update_item.clone());

    let app_menu = build_app_menu(app, &update_item)?;
    let edit_menu = build_edit_menu(app)?;
    let view_menu = build_view_menu(app)?;
    let window_menu = build_window_menu(app)?;
    let help_menu = build_help_menu(app)?;

    Menu::with_items(
        app,
        &[&app_menu, &edit_menu, &view_menu, &window_menu, &help_menu],
    )
}

/// Aleph submenu: about, show / browser / connect, the shared update item, and
/// the app-owned quit variants. `update_item` is already registered with the
/// updater so its label stays in sync with the tray.
fn build_app_menu(
    app: &AppHandle,
    update_item: &MenuItem<Wry>,
) -> tauri::Result<Submenu<Wry>> {
    Submenu::with_items(
        app,
        "Aleph",
        true,
        &[
            &PredefinedMenuItem::about(app, Some("About Aleph"), None)?,
            &PredefinedMenuItem::separator(app)?,
            &MenuItem::with_id(app, ID_SHOW, "Show Aleph", true, None::<&str>)?,
            &MenuItem::with_id(app, ID_OPEN_BROWSER, "Open in Browser", true, None::<&str>)?,
            &MenuItem::with_id(
                app,
                ID_CONNECT_REMOTE,
                "Connect to Remote…",
                true,
                None::<&str>,
            )?,
            &MenuItem::with_id(app, ID_CONNECT_LOCAL, "Back to Local", true, None::<&str>)?,
            update_item,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::hide(app, None)?,
            &PredefinedMenuItem::hide_others(app, None)?,
            &PredefinedMenuItem::separator(app)?,
            // App-owned Quit items, not the predefined one: the predefined
            // macOS Quit calls NSApplication terminate, bypassing the
            // shell's close-to-tray lifecycle.
            &MenuItem::with_id(
                app,
                ID_QUIT,
                "Quit (Aleph keeps running)",
                true,
                Some("Cmd+Q"),
            )?,
            &MenuItem::with_id(app, ID_QUIT_STOP, "Quit & Stop Aleph", true, None::<&str>)?,
        ],
    )
}

/// Predefined Edit items so the webview gets native text editing.
fn build_edit_menu(app: &AppHandle) -> tauri::Result<Submenu<Wry>> {
    Submenu::with_items(
        app,
        "Edit",
        true,
        &[
            &PredefinedMenuItem::undo(app, None)?,
            &PredefinedMenuItem::redo(app, None)?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::cut(app, None)?,
            &PredefinedMenuItem::copy(app, None)?,
            &PredefinedMenuItem::paste(app, None)?,
            &PredefinedMenuItem::select_all(app, None)?,
        ],
    )
}

/// View — Panel-level actions exposed via menu so they're discoverable
/// without dropping to the tray. DevTools only ships in debug builds:
/// production users never need it, and a release menu item that
/// dead-ends would confuse them.
fn build_view_menu(app: &AppHandle) -> tauri::Result<Submenu<Wry>> {
    let reload = MenuItem::with_id(app, ID_RELOAD_PANEL, "Reload Panel", true, Some("Cmd+R"))?;
    #[cfg(debug_assertions)]
    {
        let devtools = MenuItem::with_id(
            app,
            ID_OPEN_DEVTOOLS,
            "Open DevTools",
            true,
            Some("Cmd+Alt+I"),
        )?;
        Submenu::with_items(app, "View", true, &[&reload, &devtools])
    }
    #[cfg(not(debug_assertions))]
    {
        Submenu::with_items(app, "View", true, &[&reload])
    }
}

/// Window submenu: the standard minimize / fullscreen / close predefined items.
fn build_window_menu(app: &AppHandle) -> tauri::Result<Submenu<Wry>> {
    Submenu::with_items(
        app,
        "Window",
        true,
        &[
            &PredefinedMenuItem::minimize(app, None)?,
            &PredefinedMenuItem::fullscreen(app, None)?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::close_window(app, None)?,
        ],
    )
}

/// Help — external resources. Opens via the system browser (no in-app
/// navigation away from the Panel), reusing external_link::open_url.
fn build_help_menu(app: &AppHandle) -> tauri::Result<Submenu<Wry>> {
    Submenu::with_items(
        app,
        "Help",
        true,
        &[
            &MenuItem::with_id(
                app,
                ID_VISIT_REPO,
                "Visit GitHub Repository",
                true,
                None::<&str>,
            )?,
            &MenuItem::with_id(app, ID_REPORT_ISSUE, "Report an Issue…", true, None::<&str>)?,
        ],
    )
}

/// Route a macOS menu click to its action. Ids the shell does not own
/// (predefined items, the tray menu) are ignored.
pub fn on_event(app: &AppHandle, id: &str) {
    match id {
        ID_SHOW => crate::focus_window(app),
        // Open the Panel in the system browser at the configured Gateway
        // origin. Nonce-free under LAN-trust: same-LAN browsers are trusted,
        // so there is no bootstrap handshake or credential handoff. Common to
        // both shell variants; the CLI sibling of this affordance is
        // `aleph open` (spec §4.1).
        ID_OPEN_BROWSER => {
            crate::external_link::open_url(&browser_origin(&crate::connection::load_target()));
        }
        ID_CONNECT_REMOTE => {
            if let Some(window) = app.get_webview_window("main") {
                if let Ok(url) = tauri::Url::parse(crate::connection::connect_page_url()) {
                    let _ = window.navigate(url);
                }
                crate::focus_window(app);
            }
        }
        ID_CONNECT_LOCAL => {
            let _ = crate::connection::clear_connection_target(app.clone());
        }
        // Apply a staged update, or trigger a fresh check if none — mirrors
        // the tray so the notification's "or the Aleph menu" is truthful.
        ID_CHECK_UPDATE => {
            if crate::update::has_staged_update(app) {
                crate::update::apply_staged_update(app);
            } else {
                crate::update::check_now(app);
            }
        }
        ID_QUIT => app.exit(0),
        ID_QUIT_STOP => {
            // Full app only: stop the bundled daemon before quitting. The
            // panel-only shell owns no local daemon, so it just exits.
            #[cfg(feature = "embedded-core")]
            crate::daemon::stop_daemon();
            app.exit(0);
        }
        ID_RELOAD_PANEL => reload_panel(app),
        #[cfg(debug_assertions)]
        ID_OPEN_DEVTOOLS => open_devtools(app),
        ID_VISIT_REPO => crate::external_link::open_url(REPO_URL),
        ID_REPORT_ISSUE => crate::external_link::open_url(ISSUES_URL),
        _ => {}
    }
}

/// Re-fetch the Panel from scratch: clear the WebKit data store, then
/// re-navigate to the active origin.
///
/// A plain `location.reload()` is a *soft* reload — WebKit reuses the
/// in-session memory copies of css/js/wasm even under `Cache-Control:
/// no-store`, so a freshly deployed Panel build would not show up (the classic
/// "I rebuilt the Panel but the App still shows the old UI"). Clearing the data
/// store (HTTP cache, memory resource cache, bfcache, localStorage) and then
/// doing a *fresh navigation* (not a reload) forces every subresource to be
/// re-fetched from the Gateway.
pub(crate) fn reload_panel(app: &AppHandle) {
    let Some(window) = app.get_webview_window("main") else {
        return;
    };
    if let Err(e) = window.clear_all_browsing_data() {
        tracing::warn!("could not clear browsing data before Panel reload: {e}");
    }
    crate::navigate_to_target(app, &crate::connection::load_target());
}

/// Open Tauri's `WebView` devtools for the main window. Only wired in
/// debug builds — the menu item itself is hidden in release.
#[cfg(debug_assertions)]
fn open_devtools(app: &AppHandle) {
    let Some(window) = app.get_webview_window("main") else {
        return;
    };
    window.open_devtools();
}

/// The origin the "Open in Browser" item hands to the system browser: the
/// loopback Gateway for Local, the configured origin for Remote. Pure so it
/// is unit-testable without a window or menu.
fn browser_origin(target: &crate::connection::ConnectionTarget) -> String {
    match target {
        crate::connection::ConnectionTarget::Local => crate::PANEL_URL.to_string(),
        crate::connection::ConnectionTarget::Remote(url) => url.as_str().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn browser_origin_local_is_the_loopback_panel() {
        assert_eq!(
            browser_origin(&crate::connection::ConnectionTarget::Local),
            "http://127.0.0.1:18790"
        );
    }

    #[test]
    fn browser_origin_remote_is_the_configured_origin() {
        let target = crate::connection::ConnectionTarget::parse("10.0.0.5").unwrap();
        assert_eq!(browser_origin(&target), "http://10.0.0.5:18790/");
    }
}
