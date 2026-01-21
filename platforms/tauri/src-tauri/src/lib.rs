mod commands;
mod error;
mod shortcuts;
mod tray;

use tauri::Manager;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

pub fn run() {
    // Initialize logging
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "aether_tauri=debug,tauri=info".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    tracing::info!("Starting Aether Tauri application");

    tauri::Builder::default()
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_notification::init())
        .setup(|app| {
            // Create system tray
            let _tray = tray::create_tray(app.handle())?;

            // Register global shortcuts
            if let Err(e) = shortcuts::register_shortcuts(app.handle()) {
                tracing::error!("Failed to register shortcuts: {:?}", e);
            }

            // Get windows
            let halo_window = app.get_webview_window("halo");
            let settings_window = app.get_webview_window("settings");

            tracing::info!(
                "Windows initialized - halo: {}, settings: {}",
                halo_window.is_some(),
                settings_window.is_some()
            );

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_app_version,
            commands::get_cursor_position,
            commands::show_halo_window,
            commands::hide_halo_window,
            commands::open_settings_window,
            commands::get_settings,
            commands::save_settings,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
