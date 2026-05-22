//! Shell-level global hotkey.
//!
//! One system-wide shortcut summons Aleph — show and focus the window from
//! anywhere, or dismiss it if it is already in front. The purest form of
//! R5 ("AI comes to you"): the assistant is one keystroke away.
//!
//! The shortcut defaults to a sensible combination and can be overridden
//! with `ALEPH_SHELL_HOTKEY` (e.g. `ALEPH_SHELL_HOTKEY="Alt+Space"`). It is
//! a shell concern with no IPC surface to the Panel (R4).

use tauri::{AppHandle, Manager};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};

/// The default summon shortcut — `A` for Aleph.
const DEFAULT_HOTKEY: &str = "CmdOrCtrl+Shift+A";

/// Register the global summon hotkey. Best-effort: a combination already
/// claimed by another app is logged and skipped — the tray and the window
/// remain the other ways in.
pub fn setup(app: &AppHandle) {
    let spec = resolve_hotkey(std::env::var("ALEPH_SHELL_HOTKEY").ok());
    let result = app
        .global_shortcut()
        .on_shortcut(spec.as_str(), |app, _shortcut, event| {
            // Each press fires Pressed then Released; act once.
            if event.state == ShortcutState::Pressed {
                toggle_window(app);
            }
        });
    match result {
        Ok(()) => tracing::info!("global summon hotkey registered: {spec}"),
        Err(e) => tracing::warn!("could not register global hotkey '{spec}': {e}"),
    }
}

/// Resolve the hotkey spec: a non-empty `ALEPH_SHELL_HOTKEY` wins, else the
/// built-in default.
fn resolve_hotkey(env_value: Option<String>) -> String {
    env_value
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| DEFAULT_HOTKEY.to_string())
}

/// Summon or dismiss the main window: visible-and-focused → hide; otherwise
/// bring it forward.
fn toggle_window(app: &AppHandle) {
    let Some(window) = app.get_webview_window("main") else {
        return;
    };
    let in_front = window.is_visible().unwrap_or(false) && window.is_focused().unwrap_or(false);
    if in_front {
        let _ = window.hide();
    } else {
        crate::focus_window(app);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_hotkey_prefers_a_non_empty_override() {
        assert_eq!(resolve_hotkey(Some("Alt+Space".into())), "Alt+Space");
        assert_eq!(resolve_hotkey(Some("  ".into())), DEFAULT_HOTKEY);
        assert_eq!(resolve_hotkey(None), DEFAULT_HOTKEY);
    }
}
