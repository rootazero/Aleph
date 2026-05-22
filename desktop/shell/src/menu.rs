//! macOS application menu.
//!
//! macOS shows an app menu in the system menu bar whatever the window's
//! chrome. A tailored menu lets the shell wire items to its own actions
//! while keeping the predefined Edit items the webview needs for native
//! text editing (Cmd+C / V / X / A). macOS-only — Windows and Linux keep
//! their chromeless look with no menu.

use tauri::menu::{Menu, MenuItem, PredefinedMenuItem, Submenu};
use tauri::{AppHandle, Wry};

const ID_SHOW: &str = "menu_show";
const ID_CHECK_UPDATE: &str = "menu_check_update";
const ID_QUIT: &str = "menu_quit";
const ID_QUIT_STOP: &str = "menu_quit_stop";

/// Build the macOS app menu: Aleph / Edit / Window.
pub fn build(app: &AppHandle) -> tauri::Result<Menu<Wry>> {
    let app_menu = Submenu::with_items(
        app,
        "Aleph",
        true,
        &[
            &PredefinedMenuItem::about(app, Some("About Aleph"), None)?,
            &PredefinedMenuItem::separator(app)?,
            &MenuItem::with_id(app, ID_SHOW, "Show Aleph", true, None::<&str>)?,
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

    Menu::with_items(app, &[&app_menu, &edit_menu, &window_menu])
}

/// Route a macOS menu click to its action. Ids the shell does not own
/// (predefined items, the tray menu) are ignored.
pub fn on_event(app: &AppHandle, id: &str) {
    match id {
        ID_SHOW => crate::focus_window(app),
        ID_CHECK_UPDATE => crate::update::check_now(app),
        ID_QUIT => app.exit(0),
        ID_QUIT_STOP => {
            crate::daemon::stop_daemon();
            app.exit(0);
        }
        _ => {}
    }
}
