mod bridge;
mod commands;
mod error;
mod settings;
mod shortcuts;
mod tray;

use std::sync::OnceLock;
use tauri::{AppHandle, Manager};
use tauri_plugin_autostart::MacosLauncher;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

/// Global AppHandle so the bridge module can access Tauri windows.
static APP_HANDLE: OnceLock<AppHandle> = OnceLock::new();

/// Retrieve the stored AppHandle (available after setup).
pub fn get_app_handle() -> Option<&'static AppHandle> {
    APP_HANDLE.get()
}

// ── Bridge-mode CLI configuration ──────────────────────────────────

/// Bridge mode configuration parsed from CLI args.
///
/// When the Aleph server spawns the Tauri app as a subprocess, it passes
/// `--bridge-mode --socket <path> --server-port <port>`. This struct
/// captures those values so the bridge knows it is running in managed mode.
#[derive(Debug, Clone)]
pub struct BridgeModeConfig {
    /// Override UDS socket path (instead of default ~/.aleph/bridge.sock)
    pub socket_path: Option<String>,
    /// Server port for WebView URLs (e.g. control-plane dashboard)
    pub server_port: Option<u16>,
}

/// Parse bridge-mode args from command line.
///
/// Returns `None` if `--bridge-mode` is not present, meaning the app
/// is running standalone (user launched it directly).
pub fn parse_bridge_args() -> Option<BridgeModeConfig> {
    let args: Vec<String> = std::env::args().collect();
    if !args.iter().any(|a| a == "--bridge-mode") {
        return None;
    }

    let socket_path = args
        .iter()
        .position(|a| a == "--socket")
        .and_then(|i| args.get(i + 1))
        .cloned();

    let server_port = args
        .iter()
        .position(|a| a == "--server-port")
        .and_then(|i| args.get(i + 1))
        .and_then(|s| s.parse().ok());

    Some(BridgeModeConfig {
        socket_path,
        server_port,
    })
}

// ── Application entry point ────────────────────────────────────────

pub fn run() {
    let _ = tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "aleph_tauri=debug,tauri=info".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .try_init();

    // Parse bridge-mode CLI flags before anything else
    let bridge_config = parse_bridge_args();
    let is_bridge_mode = bridge_config.is_some();

    if is_bridge_mode {
        tracing::info!("Starting Aleph Tauri in BRIDGE mode");
    } else {
        tracing::info!("Starting Aleph Tauri in STANDALONE mode");
    }

    // Apply bridge-mode environment overrides so downstream code
    // (bridge::start_bridge_server, WebView URL construction) picks them up.
    if let Some(ref config) = bridge_config {
        if let Some(ref path) = config.socket_path {
            // SAFETY: called before any threads are spawned
            unsafe { std::env::set_var("ALEPH_SOCKET_PATH", path) };
            tracing::info!(socket_path = %path, "Bridge socket path override");
        }
        if let Some(port) = config.server_port {
            // SAFETY: called before any threads are spawned
            unsafe { std::env::set_var("ALEPH_SERVER_PORT", port.to_string()) };
            tracing::info!(server_port = port, "Server port override");
        }
    }

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
            let _ = APP_HANDLE.set(app.handle().clone());

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
