use tauri::{AppHandle, Runtime};
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};

use crate::commands::show_halo_window;
use crate::error::Result;

/// Register global shortcuts for the application
pub fn register_shortcuts<R: Runtime>(app: &AppHandle<R>) -> Result<()> {
    let manager = app.global_shortcut();

    // Register Ctrl+Alt+/ (or Cmd+Option+/ on macOS) to show Halo
    let show_halo_shortcut = Shortcut::new(
        Some(Modifiers::CONTROL | Modifiers::ALT),
        Code::Slash,
    );

    let app_handle = app.clone();
    manager
        .on_shortcut(show_halo_shortcut, move |_app, _shortcut, event| {
            if event.state == ShortcutState::Pressed {
                tracing::info!("Global shortcut triggered: Show Halo");
                let handle = app_handle.clone();
                tauri::async_runtime::spawn(async move {
                    if let Err(e) = show_halo_window(handle).await {
                        tracing::error!("Failed to show halo window: {:?}", e);
                    }
                });
            }
        })
        .map_err(|e| crate::error::AlephError::Unknown(e.to_string()))?;

    tracing::info!("Global shortcuts registered successfully");
    tracing::info!("  - Ctrl+Alt+/: Show Halo");

    Ok(())
}

/// Unregister all global shortcuts
#[allow(dead_code)]
pub fn unregister_shortcuts<R: Runtime>(app: &AppHandle<R>) -> Result<()> {
    let manager = app.global_shortcut();
    manager
        .unregister_all()
        .map_err(|e| crate::error::AlephError::Unknown(e.to_string()))?;

    tracing::info!("Global shortcuts unregistered");
    Ok(())
}
