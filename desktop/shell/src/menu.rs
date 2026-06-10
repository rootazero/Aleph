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
    let app_menu = Submenu::with_items(
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
            &MenuItem::with_id(
                app,
                ID_CHECK_UPDATE,
                "Check for Updates…",
                true,
                None::<&str>,
            )?,
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
    )?;

    // Predefined Edit items so the webview gets native text editing.
    let edit_menu = Submenu::with_items(
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
    )?;

    // View — Panel-level actions exposed via menu so they're discoverable
    // without dropping to the tray. DevTools only ships in debug builds:
    // production users never need it, and a release menu item that
    // dead-ends would confuse them.
    let view_menu = {
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
            Submenu::with_items(app, "View", true, &[&reload, &devtools])?
        }
        #[cfg(not(debug_assertions))]
        {
            Submenu::with_items(app, "View", true, &[&reload])?
        }
    };

    let window_menu = Submenu::with_items(
        app,
        "Window",
        true,
        &[
            &PredefinedMenuItem::minimize(app, None)?,
            &PredefinedMenuItem::fullscreen(app, None)?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::close_window(app, None)?,
        ],
    )?;

    // Help — external resources. Opens via the system browser (no in-app
    // navigation away from the Panel), reusing daemon::open_url_in_browser.
    let help_menu = Submenu::with_items(
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
    )?;

    Menu::with_items(
        app,
        &[&app_menu, &edit_menu, &view_menu, &window_menu, &help_menu],
    )
}

/// Route a macOS menu click to its action. Ids the shell does not own
/// (predefined items, the tray menu) are ignored.
pub fn on_event(app: &AppHandle, id: &str) {
    match id {
        ID_SHOW => crate::focus_window(app),
        ID_OPEN_BROWSER => {
            tauri::async_runtime::spawn(async {
                crate::daemon::open_in_system_browser().await;
            });
        }
        ID_CONNECT_REMOTE => {
            if let Some(window) = app.get_webview_window("main") {
                if let Ok(url) = tauri::Url::parse("tauri://localhost/connect.html") {
                    let _ = window.navigate(url);
                }
                crate::focus_window(app);
            }
        }
        ID_CONNECT_LOCAL => {
            let _ = crate::connection::clear_connection_target(app.clone());
        }
        ID_CHECK_UPDATE => crate::update::check_now(app),
        ID_QUIT => app.exit(0),
        ID_QUIT_STOP => {
            crate::daemon::stop_daemon();
            app.exit(0);
        }
        ID_RELOAD_PANEL => reload_panel(app),
        #[cfg(debug_assertions)]
        ID_OPEN_DEVTOOLS => open_devtools(app),
        ID_VISIT_REPO => crate::daemon::open_url_in_browser(REPO_URL),
        ID_REPORT_ISSUE => crate::daemon::open_url_in_browser(ISSUES_URL),
        _ => {}
    }
}

/// Re-fetch the Panel in the current main window. The Panel keeps its
/// `aleph_session` cookie across this reload — no fresh bootstrap nonce
/// is needed.
fn reload_panel(app: &AppHandle) {
    let Some(window) = app.get_webview_window("main") else {
        return;
    };
    if let Err(e) = window.eval("window.location.reload()") {
        tracing::warn!("could not reload Panel: {e}");
    }
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
