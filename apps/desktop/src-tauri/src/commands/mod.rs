use tauri::{AppHandle, Emitter, Manager, Runtime};

use crate::error::{AlephError, Result};
use crate::settings::{self, AlephPaths, Settings, WindowPosition};

/// Application version information
#[derive(Debug, serde::Serialize)]
pub struct AppVersion {
    pub version: String,
    pub build: String,
}

/// Cursor position
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct Position {
    pub x: i32,
    pub y: i32,
}

/// Get application version
#[tauri::command]
pub fn get_app_version() -> AppVersion {
    AppVersion {
        version: env!("CARGO_PKG_VERSION").to_string(),
        build: "1".to_string(),
    }
}

/// Get cursor position
#[tauri::command]
pub fn get_cursor_position() -> Result<Position> {
    #[cfg(target_os = "windows")]
    {
        use windows::Win32::Foundation::POINT;
        use windows::Win32::UI::WindowsAndMessaging::GetCursorPos;

        let mut point = POINT::default();
        unsafe {
            GetCursorPos(&mut point).map_err(|e| AlephError::Unknown(e.to_string()))?;
        }
        Ok(Position {
            x: point.x,
            y: point.y,
        })
    }

    #[cfg(target_os = "linux")]
    {
        // TODO: Implement for Linux using X11 or Wayland
        Ok(Position { x: 0, y: 0 })
    }
}

/// Show halo window at fixed screen position (center horizontally, 30% from bottom)
#[tauri::command]
pub async fn show_halo_window<R: Runtime>(app: AppHandle<R>) -> Result<()> {
    if let Some(window) = app.get_webview_window("halo") {
        // Get window size
        let window_size = window
            .outer_size()
            .map_err(|e| AlephError::Window(e.to_string()))?;

        // Calculate fixed position: center horizontally, 30% from bottom
        let (final_x, final_y) = calculate_fixed_position(
            &app,
            window_size.width as i32,
            window_size.height as i32,
        );

        window
            .set_position(tauri::Position::Physical(tauri::PhysicalPosition {
                x: final_x,
                y: final_y,
            }))
            .map_err(|e| AlephError::Window(e.to_string()))?;

        window
            .show()
            .map_err(|e| AlephError::Window(e.to_string()))?;

        window
            .emit("halo:activate", ())
            .map_err(|e: tauri::Error| AlephError::Window(e.to_string()))?;

        tracing::debug!("Halo window shown at ({}, {})", final_x, final_y);
    }

    Ok(())
}

/// Calculate fixed window position: center horizontally, 30% from bottom vertically
fn calculate_fixed_position<R: Runtime>(
    app: &AppHandle<R>,
    window_width: i32,
    window_height: i32,
) -> (i32, i32) {
    if let Some(monitor) = app
        .primary_monitor()
        .ok()
        .flatten()
        .or_else(|| app.available_monitors().ok().and_then(|m| m.into_iter().next()))
    {
        let monitor_pos = monitor.position();
        let monitor_size = monitor.size();

        let screen_width = monitor_size.width as i32;
        let screen_height = monitor_size.height as i32;

        // Center horizontally
        let x = monitor_pos.x + (screen_width - window_width) / 2;

        // 30% from bottom = 70% from top
        let y = monitor_pos.y + (screen_height as f32 * 0.7) as i32 - window_height / 2;

        (x, y)
    } else {
        // Fallback: center of a default 1920x1080 screen
        let x = (1920 - window_width) / 2;
        let y = (1080.0 * 0.7) as i32 - window_height / 2;
        (x, y)
    }
}

/// Hide halo window
#[tauri::command]
pub async fn hide_halo_window<R: Runtime>(app: AppHandle<R>) -> Result<()> {
    if let Some(window) = app.get_webview_window("halo") {
        window
            .hide()
            .map_err(|e| AlephError::Window(e.to_string()))?;
        tracing::debug!("Halo window hidden");
    }

    Ok(())
}

/// Open settings window
#[tauri::command]
pub async fn open_settings_window<R: Runtime>(app: AppHandle<R>) -> Result<()> {
    if let Some(window) = app.get_webview_window("settings") {
        // Try to restore window position
        if let Ok(state) = settings::load_window_state() {
            if let Some(pos) = state.settings {
                let _ = window.set_position(tauri::Position::Physical(tauri::PhysicalPosition {
                    x: pos.x,
                    y: pos.y,
                }));
                let _ = window.set_size(tauri::Size::Physical(tauri::PhysicalSize {
                    width: pos.width,
                    height: pos.height,
                }));
            }
        }

        window
            .show()
            .map_err(|e| AlephError::Window(e.to_string()))?;
        window
            .set_focus()
            .map_err(|e| AlephError::Window(e.to_string()))?;
        tracing::debug!("Settings window opened");
    }

    Ok(())
}

/// Get current settings
#[tauri::command]
pub async fn get_settings() -> Result<Settings> {
    settings::load_settings()
}

/// Save settings
#[tauri::command]
pub async fn save_settings(new_settings: Settings) -> Result<()> {
    tracing::info!("Saving settings");
    settings::save_settings(&new_settings)?;

    // Handle launch at login change
    #[cfg(any(target_os = "windows", target_os = "linux"))]
    {
        // The actual autostart management is handled by tauri-plugin-autostart
        // We just log the intention here; the frontend should use the plugin directly
        tracing::info!(
            "Launch at login: {}",
            new_settings.general.launch_at_login
        );
    }

    Ok(())
}

/// Save window position (called when window is moved/resized)
#[tauri::command]
pub async fn save_window_position<R: Runtime>(
    app: AppHandle<R>,
    window_name: String,
) -> Result<()> {
    if let Some(window) = app.get_webview_window(&window_name) {
        let position = window
            .outer_position()
            .map_err(|e| AlephError::Window(e.to_string()))?;
        let size = window
            .outer_size()
            .map_err(|e| AlephError::Window(e.to_string()))?;

        let mut state = settings::load_window_state().unwrap_or_default();

        let window_pos = WindowPosition {
            x: position.x,
            y: position.y,
            width: size.width,
            height: size.height,
        };

        if window_name.as_str() == "settings" {
            state.settings = Some(window_pos);
        }

        settings::save_window_state(&state)?;
        tracing::debug!("Window position saved for {}", window_name);
    }

    Ok(())
}

/// Get window position
#[tauri::command]
pub async fn get_window_position(window_name: String) -> Result<Option<WindowPosition>> {
    let state = settings::load_window_state()?;

    let pos = match window_name.as_str() {
        "settings" => state.settings,
        _ => None,
    };

    Ok(pos)
}

/// Send notification
#[tauri::command]
pub async fn send_notification<R: Runtime>(
    app: AppHandle<R>,
    title: String,
    body: String,
) -> Result<()> {
    use tauri_plugin_notification::NotificationExt;

    app.notification()
        .builder()
        .title(&title)
        .body(&body)
        .show()
        .map_err(|e| AlephError::Unknown(e.to_string()))?;

    Ok(())
}

/// Get autostart status
#[tauri::command]
pub async fn get_autostart_enabled<R: Runtime>(app: AppHandle<R>) -> Result<bool> {
    use tauri_plugin_autostart::ManagerExt;

    let manager = app.autolaunch();
    manager
        .is_enabled()
        .map_err(|e| AlephError::Config(e.to_string()))
}

/// Set autostart status
#[tauri::command]
pub async fn set_autostart_enabled<R: Runtime>(app: AppHandle<R>, enabled: bool) -> Result<()> {
    use tauri_plugin_autostart::ManagerExt;

    let manager = app.autolaunch();

    if enabled {
        manager
            .enable()
            .map_err(|e| AlephError::Config(e.to_string()))?;
        tracing::info!("Autostart enabled");
    } else {
        manager
            .disable()
            .map_err(|e| AlephError::Config(e.to_string()))?;
        tracing::info!("Autostart disabled");
    }

    Ok(())
}

/// Get all Aleph paths (~/.config/aleph/*)
#[tauri::command]
pub async fn get_aleph_paths() -> Result<AlephPaths> {
    settings::get_aleph_paths()
}
