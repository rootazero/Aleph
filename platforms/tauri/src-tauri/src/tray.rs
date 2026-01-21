use tauri::{
    menu::{Menu, MenuItem, PredefinedMenuItem, Submenu},
    tray::{TrayIcon, TrayIconBuilder},
    AppHandle, Manager, Runtime,
};

use crate::error::Result;

pub fn create_tray<R: Runtime>(app: &AppHandle<R>) -> Result<TrayIcon<R>> {
    // Create provider submenu
    let provider_submenu = Submenu::with_items(
        app,
        "Default Provider",
        true,
        &[
            &MenuItem::with_id(app, "provider_openai", "OpenAI", true, None::<&str>)?,
            &MenuItem::with_id(app, "provider_claude", "Claude", true, None::<&str>)?,
            &MenuItem::with_id(app, "provider_gemini", "Gemini", true, None::<&str>)?,
            &MenuItem::with_id(app, "provider_ollama", "Ollama", true, None::<&str>)?,
        ],
    )?;

    // Create main menu
    let menu = Menu::with_items(
        app,
        &[
            &MenuItem::with_id(app, "about", "About Aether", true, None::<&str>)?,
            &PredefinedMenuItem::separator(app)?,
            &provider_submenu,
            &PredefinedMenuItem::separator(app)?,
            &MenuItem::with_id(app, "settings", "Settings...", true, Some("CmdOrCtrl+,"))?,
            &PredefinedMenuItem::separator(app)?,
            &MenuItem::with_id(app, "quit", "Quit Aether", true, Some("CmdOrCtrl+Q"))?,
        ],
    )?;

    let tray = TrayIconBuilder::new()
        .icon(app.default_window_icon().unwrap().clone())
        .menu(&menu)
        .show_menu_on_left_click(true)
        .on_menu_event(move |app, event| {
            tracing::debug!("Tray menu event: {:?}", event.id);

            match event.id.as_ref() {
                "about" => {
                    tracing::info!("About menu clicked");
                    // TODO: Show about dialog
                }
                "settings" => {
                    tracing::info!("Settings menu clicked");
                    if let Some(window) = app.get_webview_window("settings") {
                        let _ = window.show();
                        let _ = window.set_focus();
                    }
                }
                "quit" => {
                    tracing::info!("Quit menu clicked");
                    app.exit(0);
                }
                id if id.starts_with("provider_") => {
                    let provider = id.strip_prefix("provider_").unwrap_or("unknown");
                    tracing::info!("Provider selected: {}", provider);
                    // TODO: Set default provider
                }
                _ => {}
            }
        })
        .build(app)?;

    tracing::info!("System tray created successfully");

    Ok(tray)
}
