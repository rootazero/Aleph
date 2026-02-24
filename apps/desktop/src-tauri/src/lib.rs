mod bridge;
mod commands;
mod error;
mod settings;
mod shortcuts;
mod tray;

use tauri::Manager;
use tauri_plugin_autostart::MacosLauncher;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

pub fn run() {
    let _ = tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "aleph_tauri=debug,tauri=info".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .try_init();

    tracing::info!("Starting Aleph Tauri application");

    tauri::Builder::default()
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_autostart::init(
            MacosLauncher::LaunchAgent,
            Some(vec!["--minimized"]),
        ))
        .plugin(tauri_plugin_store::Builder::new().build())
        .setup(|app| {
            let _tray = tray::create_tray(app.handle())?;

            if let Err(e) = shortcuts::register_shortcuts(app.handle()) {
                tracing::error!("Failed to register shortcuts: {:?}", e);
            }

            // Start Desktop Bridge UDS server
            tauri::async_runtime::spawn(async {
                bridge::start_bridge_server().await;
            });

            let halo_window = app.get_webview_window("halo");
            let settings_window = app.get_webview_window("settings");

            tracing::info!(
                "Windows initialized - halo: {}, settings: {}",
                halo_window.is_some(),
                settings_window.is_some()
            );

            if let Some(settings) = settings_window {
                let app_handle = app.handle().clone();
                settings.on_window_event(move |event| {
                    if let tauri::WindowEvent::CloseRequested { .. } = event {
                        let handle = app_handle.clone();
                        tauri::async_runtime::spawn(async move {
                            let _ = commands::save_window_position(handle, "settings".to_string()).await;
                        });
                    }
                });
            }

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
            commands::save_window_position,
            commands::get_window_position,
            commands::send_notification,
            commands::get_autostart_enabled,
            commands::set_autostart_enabled,
            commands::get_aleph_paths,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
