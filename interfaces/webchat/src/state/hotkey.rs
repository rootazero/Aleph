//! Panel-wide hotkey state.
//!
//! Single source of truth for the in-app command palette open/close signal,
//! plus the global ⌘K / Ctrl+K listener that toggles it. Kept intentionally
//! tiny so that adding more hotkeys later is purely additive (push more
//! fields onto `HotkeyState`; install more listeners from `install()`).
//!
//! The desktop shell already owns a *global* summon hotkey (CmdOrCtrl+
//! Shift+A → show/hide the window, see `desktop/shell/src/hotkey.rs`). This
//! file is the in-panel sibling — short keystrokes that only fire while the
//! webview has focus.

use leptos::ev::keydown;
use leptos::prelude::*;

/// Process-wide UI hotkey state. Clone-cheap (signals are `Copy`).
#[derive(Clone, Copy, Default)]
pub struct HotkeyState {
    /// Whether the command palette is currently open.
    pub palette_open: RwSignal<bool>,
}

impl HotkeyState {
    #[must_use]
    pub fn new() -> Self {
        Self {
            palette_open: RwSignal::new(false),
        }
    }

    /// Toggle the palette without disturbing other state.
    pub fn toggle_palette(self) {
        self.palette_open.update(|v| *v = !*v);
    }

    /// Force-close (Esc handler).
    pub fn close_palette(self) {
        if self.palette_open.get_untracked() {
            self.palette_open.set(false);
        }
    }
}

/// Install the global keydown listener. Idempotent in practice because
/// callers only mount it once at App start; we don't guard against being
/// called twice (the listener is leaked on purpose so it lives as long as
/// the page).
///
/// Bindings:
/// - **⌘K / Ctrl+K** → toggle the command palette
///
/// Inputs while the palette itself is open are handled inside the palette
/// (Esc to close, ↑↓ to navigate, Enter to run) — the global listener
/// stays out of the way so it doesn't fight the inner input element.
pub fn install(state: HotkeyState) {
    window_event_listener(keydown, move |ev: web_sys::KeyboardEvent| {
        // ⌘K / Ctrl+K — match either modifier so the same key works on
        // every platform without branching on user-agent.
        let mod_pressed = ev.meta_key() || ev.ctrl_key();
        if mod_pressed && !ev.alt_key() && ev.key().eq_ignore_ascii_case("k") {
            // Don't fight legitimate browser shortcuts when the user is
            // typing into a contenteditable surface that explicitly wants
            // ⌘K (rare — none in Aleph today). The common case (input /
            // textarea) is fine to swallow since we want ⌘K to open the
            // palette even mid-typing.
            ev.prevent_default();
            state.toggle_palette();
            return;
        }

        // Esc — close the palette if it's open. Other Esc handlers (e.g.
        // sidebar uncollapse in `app.rs`) keep running; we only act when
        // we actually owned the keystroke.
        if ev.key() == "Escape" && state.palette_open.get_untracked() {
            // Don't prevent_default — let other handlers still see it.
            state.close_palette();
        }
    });
}
