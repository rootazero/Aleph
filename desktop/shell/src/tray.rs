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
    // Dual-purpose: a manual update check until an update is found, then the
    // button to apply it — the auto-updater relabels it (see `update.rs`).
    let update_item = MenuItem::with_id(app, "update", "Check for Updates…", true, None::<&str>)?;
    // Let the auto-updater relabel this item once an update is staged.
    app.state::<crate::update::Updater>()
        .attach_update_item(update_item.clone());
    // Connection-target entry points: switch the shell between the bundled
    // local daemon and a remote Gateway. "Connect to Remote…" opens the
    // bundled connection page; "Back to Local" resets to the local daemon.
    let connect_remote = MenuItem::with_id(
        app,
        "connect_remote",
        "Connect to Remote…",
        true,
        None::<&str>,
    )?;
    let connect_local =
        MenuItem::with_id(app, "connect_local", "Back to Local", true, None::<&str>)?;
    let separator = PredefinedMenuItem::separator(app)?;
    let quit = MenuItem::with_id(
        app,
        "quit",
        "Quit (Aleph keeps running)",
        true,
        None::<&str>,
    )?;
    let quit_stop = MenuItem::with_id(app, "quit_stop", "Quit & Stop Aleph", true, None::<&str>)?;
    let menu = Menu::with_items(
        app,
        &[
            &show,
            &update_item,
            &connect_remote,
            &connect_local,
            &separator,
            &quit,
            &quit_stop,
        ],
    )?;

    let mut builder = TrayIconBuilder::with_id("aleph-tray")
        .tooltip("Aleph")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "show" => crate::focus_window(app),
            // Apply a staged update, or trigger a fresh check if none.
            "update" => {
                if crate::update::has_staged_update(app) {
                    crate::update::apply_staged_update(app);
                } else {
                    crate::update::check_now(app);
                }
            }
            // Navigate to the bundled connection page so the user can enter a
            // remote Gateway address.
            "connect_remote" => {
                if let Some(w) = app.get_webview_window("main") {
                    if let Ok(url) = tauri::Url::parse("tauri://localhost/connect.html") {
                        let _ = w.navigate(url);
                    }
                    crate::focus_window(app);
                }
            }
            // Reset to the local daemon (launch + supervise it again).
            "connect_local" => {
                let _ = crate::connection::clear_connection_target(app.clone());
            }
            "quit" => app.exit(0),
            "quit_stop" => {
                // Full app only: stop the bundled daemon before quitting. The
                // panel-only shell owns no local daemon, so it just exits.
                #[cfg(feature = "embedded-core")]
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
                crate::focus_window(tray.app_handle());
            }
        });

    // The icon comes from the bundle config; skip gracefully if absent.
    if let Some(icon) = app.default_window_icon().cloned() {
        builder = builder.icon(icon);
    }
    builder.build(app)?;
    Ok(())
}
