use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, Runtime};

// Re-export error types
use crate::error::{AetherError, Result};

/// Application version information
#[derive(Debug, Serialize)]
pub struct AppVersion {
    pub version: String,
    pub build: String,
}

/// Cursor position
#[derive(Debug, Serialize, Deserialize)]
pub struct Position {
    pub x: i32,
    pub y: i32,
}

/// Settings structure (simplified for Phase 1)
#[derive(Debug, Serialize, Deserialize, Default)]
pub struct Settings {
    pub general: GeneralSettings,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct GeneralSettings {
    pub sound_enabled: bool,
    pub launch_at_login: bool,
    pub language: String,
}

impl Default for GeneralSettings {
    fn default() -> Self {
        Self {
            sound_enabled: true,
            launch_at_login: false,
            language: "system".to_string(),
        }
    }
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
    // Platform-specific cursor position retrieval
    #[cfg(target_os = "windows")]
    {
        use windows::Win32::Foundation::POINT;
        use windows::Win32::UI::WindowsAndMessaging::GetCursorPos;

        let mut point = POINT::default();
        unsafe {
            GetCursorPos(&mut point).map_err(|e| AetherError::Unknown(e.to_string()))?;
        }
        Ok(Position {
            x: point.x,
            y: point.y,
        })
    }

    #[cfg(target_os = "macos")]
    {
        use cocoa::appkit::NSEvent;
        use cocoa::base::nil;
        use cocoa::foundation::NSPoint;

        let location: NSPoint = unsafe { NSEvent::mouseLocation(nil) };
        // Note: macOS uses bottom-left origin, may need conversion
        Ok(Position {
            x: location.x as i32,
            y: location.y as i32,
        })
    }

    #[cfg(target_os = "linux")]
    {
        // TODO: Implement for Linux using X11 or Wayland
        Ok(Position { x: 0, y: 0 })
    }
}

/// Show halo window at cursor position
#[tauri::command]
pub async fn show_halo_window<R: Runtime>(app: AppHandle<R>) -> Result<()> {
    let position = get_cursor_position()?;

    if let Some(window) = app.get_webview_window("halo") {
        // Position window at cursor
        window
            .set_position(tauri::Position::Physical(tauri::PhysicalPosition {
                x: position.x,
                y: position.y,
            }))
            .map_err(|e| AetherError::Window(e.to_string()))?;

        window
            .show()
            .map_err(|e| AetherError::Window(e.to_string()))?;

        // Emit activation event to frontend
        window
            .emit("halo:activate", ())
            .map_err(|e: tauri::Error| AetherError::Window(e.to_string()))?;

        tracing::debug!("Halo window shown at ({}, {})", position.x, position.y);
    }

    Ok(())
}

/// Hide halo window
#[tauri::command]
pub async fn hide_halo_window<R: Runtime>(app: AppHandle<R>) -> Result<()> {
    if let Some(window) = app.get_webview_window("halo") {
        window
            .hide()
            .map_err(|e| AetherError::Window(e.to_string()))?;
        tracing::debug!("Halo window hidden");
    }

    Ok(())
}

/// Open settings window
#[tauri::command]
pub async fn open_settings_window<R: Runtime>(app: AppHandle<R>) -> Result<()> {
    if let Some(window) = app.get_webview_window("settings") {
        window
            .show()
            .map_err(|e| AetherError::Window(e.to_string()))?;
        window
            .set_focus()
            .map_err(|e| AetherError::Window(e.to_string()))?;
        tracing::debug!("Settings window opened");
    }

    Ok(())
}

/// Get current settings
#[tauri::command]
pub async fn get_settings() -> Result<Settings> {
    // TODO: Load from aether-core config
    Ok(Settings::default())
}

/// Save settings
#[tauri::command]
pub async fn save_settings(settings: Settings) -> Result<()> {
    tracing::info!("Saving settings: {:?}", settings);
    // TODO: Save to aether-core config
    Ok(())
}
