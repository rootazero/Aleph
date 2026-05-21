//! System tray — the resident, always-available entry point to the shell.
//!
//! Pure UI plumbing: it shows/hides the window and quits the app. It holds
//! no state and makes no decisions of its own.

use tauri::{
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Manager,
};

/// Build the tray icon and its menu, wiring menu/click events.
pub fn build(app: &AppHandle) -> tauri::Result<()> {
    let show = MenuItem::with_id(app, "show", "Show Aleph", true, None::<&str>)?;
    let separator = PredefinedMenuItem::separator(app)?;
    let quit = MenuItem::with_id(
        app,
        "quit",
        "Quit (Aleph keeps running)",
        true,
        None::<&str>,
    )?;
    let quit_stop =
        MenuItem::with_id(app, "quit_stop", "Quit & Stop Aleph", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show, &separator, &quit, &quit_stop])?;

    let mut builder = TrayIconBuilder::with_id("aleph-tray")
        .tooltip("Aleph")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "show" => reveal(app),
            "quit" => app.exit(0),
            "quit_stop" => {
                crate::daemon::stop_daemon();
                app.exit(0);
            }
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            // Left-click the tray icon toggles the window forward.
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                reveal(tray.app_handle());
            }
        });

    // The icon comes from the bundle config; skip gracefully if absent.
    if let Some(icon) = app.default_window_icon().cloned() {
        builder = builder.icon(icon);
    }
    builder.build(app)?;
    Ok(())
}

/// Show, un-minimise and focus the main window.
fn reveal(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}
