mod commands;
mod core;
mod error;
mod settings;
mod shortcuts;
mod tray;

use tauri::Manager;
use tauri_plugin_autostart::MacosLauncher;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

pub fn run() {
    // Initialize logging (use try_init to avoid panic if already initialized by alephcore)
    let _ = tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "aleph_tauri=debug,tauri=info,alephcore=info".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .try_init();

    tracing::info!("Starting Aleph Tauri application");

    tauri::Builder::default()
        // Plugins
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_autostart::init(
            MacosLauncher::LaunchAgent,
            Some(vec!["--minimized"]),
        ))
        .plugin(tauri_plugin_store::Builder::new().build())
        // Manage CoreState for AI functionality
        .manage(core::CoreState::new())
        .setup(|app| {
            // Create system tray
            let _tray = tray::create_tray(app.handle())?;

            // Register global shortcuts
            if let Err(e) = shortcuts::register_shortcuts(app.handle()) {
                tracing::error!("Failed to register shortcuts: {:?}", e);
            }

            // Initialize Aleph core
            match core::init_aleph_core(app.handle()) {
                Ok(aleph_core) => {
                    let state = app.state::<core::CoreState>();
                    state.initialize(aleph_core);
                    tracing::info!("Aleph core initialized successfully");
                }
                Err(e) => {
                    tracing::error!("Failed to initialize Aleph core: {:?}", e);
                    // Continue without core - some features will be unavailable
                }
            }

            // Get windows
            let halo_window = app.get_webview_window("halo");
            let settings_window = app.get_webview_window("settings");

            tracing::info!(
                "Windows initialized - halo: {}, settings: {}",
                halo_window.is_some(),
                settings_window.is_some()
            );

            // Setup window close handlers for position saving
            if let Some(settings) = settings_window {
                let app_handle = app.handle().clone();
                settings.on_window_event(move |event| {
                    if let tauri::WindowEvent::CloseRequested { .. } = event {
                        // Save window position before closing
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
            // App commands
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
            // AI processing commands
            core::process_input,
            core::cancel_processing,
            core::is_processing_cancelled,
            core::generate_topic_title,
            core::extract_text_from_image,
            // Provider commands
            core::list_generation_providers,
            core::set_default_provider,
            core::reload_config,
            // Memory commands
            core::search_memory,
            core::get_memory_stats,
            core::clear_memory,
            // Tool commands
            core::list_tools,
            core::get_tool_count,
            // MCP commands
            core::list_mcp_servers,
            core::get_mcp_config,
            // Skills commands
            core::list_skills,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
