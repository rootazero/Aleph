use tauri::{
    menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu},
    tray::{TrayIcon, TrayIconBuilder},
    AppHandle, Emitter, Manager, Runtime,
};

use crate::error::Result;
use crate::settings;

pub fn create_tray<R: Runtime>(app: &AppHandle<R>) -> Result<TrayIcon<R>> {
    // Load current settings to get default provider
    let current_settings = settings::load_settings().unwrap_or_default();
    let default_provider = &current_settings.providers.default_provider_id;

    // Create provider submenu with checkmarks
    let provider_items: Vec<CheckMenuItem<R>> = vec![
        CheckMenuItem::with_id(
            app,
            "provider_openai",
            "OpenAI",
            true,
            default_provider == "openai",
            None::<&str>,
        )?,
        CheckMenuItem::with_id(
            app,
            "provider_anthropic",
            "Anthropic",
            true,
            default_provider == "anthropic",
            None::<&str>,
        )?,
        CheckMenuItem::with_id(
            app,
            "provider_gemini",
            "Gemini",
            true,
            default_provider == "gemini",
            None::<&str>,
        )?,
        CheckMenuItem::with_id(
            app,
            "provider_ollama",
            "Ollama",
            true,
            default_provider == "ollama",
            None::<&str>,
        )?,
    ];

    let provider_submenu = Submenu::with_items(
        app,
        "Default Provider",
        true,
        &[
            &provider_items[0],
            &provider_items[1],
            &provider_items[2],
            &provider_items[3],
        ],
    )?;

    // Create main menu
    let menu = Menu::with_items(
        app,
        &[
            &MenuItem::with_id(app, "about", "About Aether", true, None::<&str>)?,
            &MenuItem::with_id(app, "version", format!("Version {}", env!("CARGO_PKG_VERSION")).as_str(), false, None::<&str>)?,
            &PredefinedMenuItem::separator(app)?,
            &MenuItem::with_id(app, "show_halo", "Show Halo", true, Some("Ctrl+Alt+Space"))?,
            &PredefinedMenuItem::separator(app)?,
            &provider_submenu,
            &PredefinedMenuItem::separator(app)?,
            &MenuItem::with_id(app, "settings", "Settings...", true, Some("CmdOrCtrl+,"))?,
            &MenuItem::with_id(app, "check_updates", "Check for Updates...", true, None::<&str>)?,
            &PredefinedMenuItem::separator(app)?,
            &MenuItem::with_id(app, "quit", "Quit Aether", true, Some("CmdOrCtrl+Q"))?,
        ],
    )?;

    let tray = TrayIconBuilder::new()
        .icon(app.default_window_icon().unwrap().clone())
        .menu(&menu)
        .show_menu_on_left_click(true)
        .tooltip("Aether - AI Assistant")
        .on_menu_event(move |app, event| {
            tracing::debug!("Tray menu event: {:?}", event.id);

            match event.id.as_ref() {
                "about" => {
                    tracing::info!("About menu clicked");
                    // Show about dialog using notification for now
                    if let Some(window) = app.get_webview_window("settings") {
                        let _ = window.emit("navigate:about", ());
                        let _ = window.show();
                        let _ = window.set_focus();
                    }
                }
                "show_halo" => {
                    tracing::info!("Show Halo menu clicked");
                    let handle = app.clone();
                    tauri::async_runtime::spawn(async move {
                        if let Err(e) = crate::commands::show_halo_window(handle).await {
                            tracing::error!("Failed to show halo: {:?}", e);
                        }
                    });
                }
                "settings" => {
                    tracing::info!("Settings menu clicked");
                    if let Some(window) = app.get_webview_window("settings") {
                        let _ = window.show();
                        let _ = window.set_focus();
                    }
                }
                "check_updates" => {
                    tracing::info!("Check updates menu clicked");
                    // TODO: Implement update check
                }
                "quit" => {
                    tracing::info!("Quit menu clicked");
                    // Save window states before quitting
                    let handle = app.clone();
                    tauri::async_runtime::spawn(async move {
                        let _ = crate::commands::save_window_position(handle, "settings".to_string()).await;
                    });
                    // Give it a moment to save
                    std::thread::sleep(std::time::Duration::from_millis(100));
                    app.exit(0);
                }
                id if id.starts_with("provider_") => {
                    let provider = id.strip_prefix("provider_").unwrap_or("unknown");
                    tracing::info!("Provider selected: {}", provider);

                    // Update settings
                    if let Ok(mut settings) = settings::load_settings() {
                        settings.providers.default_provider_id = provider.to_string();
                        let _ = settings::save_settings(&settings);

                        // Notify frontend of provider change
                        if let Some(window) = app.get_webview_window("halo") {
                            let _ = window.emit("provider:changed", provider);
                        }
                    }
                }
                _ => {}
            }
        })
        .build(app)?;

    tracing::info!("System tray created successfully");

    Ok(tray)
}
