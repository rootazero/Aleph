//! Panel-wide hotkey state.
//!
//! Single source of truth for the in-app command palette open/close signal,
//! plus the global ⌘K / Ctrl+K listener that toggles it. Kept intentionally
//! tiny so that adding more hotkeys later is purely additive (push more
//! fields onto `HotkeyState`; install more listeners from `install()`).
//!
//! The desktop shell already owns a *global* summon hotkey (`CmdOrCtrl`+
//! Shift+A → show/hide the window, see `desktop/shell/src/hotkey.rs`). This
//! file is the in-panel sibling — short keystrokes that only fire while the
//! webview has focus.

use leptos::ev::keydown;
use leptos::prelude::*;
use wasm_bindgen::JsCast;

use crate::views::chat::state::ChatState;
use crate::views::voice::VoiceMode;

/// Process-wide UI hotkey state. Clone-cheap (signals are `Copy`).
#[derive(Clone, Copy)]
pub struct HotkeyState {
    /// Whether the command palette is currently open.
    pub palette_open: RwSignal<bool>,
    /// Immersive voice-mode switch, threaded in so the global keydown listener
    /// can toggle it without `expect_context` (which would panic when invoked
    /// from a leaked, owner-less DOM-event closure). `pub(crate)` because
    /// `VoiceMode` is crate-private — only in-crate callers wire it.
    pub(crate) voice: VoiceMode,
    /// Chat state, threaded in for the same reason as `voice`: the global
    /// keydown closure is leaked/owner-less and cannot `expect_context`. The
    /// `f` binding bumps `chat.repair_pulse` to trigger the doctor + LLM
    /// repair flow (G1). `pub(crate)` since `ChatState` is crate-internal.
    pub(crate) chat: ChatState,
}

impl HotkeyState {
    #[must_use]
    pub(crate) fn new(voice: VoiceMode, chat: ChatState) -> Self {
        Self {
            palette_open: RwSignal::new(false),
            voice,
            chat,
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
/// - **⌘⇧V (macOS) / Ctrl+Alt+V (Win/Linux)** → toggle immersive voice mode
/// - **Esc** → close the voice overlay (if open), else close the palette
/// - **f** (bare, only when no editable field is focused) → run doctor + LLM
///   repair (G1); guarded so it never hijacks normal typing
///
/// Inputs while the palette itself is open are handled inside the palette
/// (Esc to close, ↑↓ to navigate, Enter to run) — the global listener
/// stays out of the way so it doesn't fight the inner input element.
pub fn install(state: HotkeyState) {
    // window-listener-permanent: `install` is called once at App start and the
    // listener is leaked on purpose (see the doc above) — it must outlive every
    // route and layout change.
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

        // Voice mode toggle — ⌘⇧V on macOS, Ctrl+Alt+V on Win/Linux. The PC
        // combo deliberately uses Alt to dodge Ctrl+Shift+V (paste-as-plain-
        // text in most browsers).
        let is_mac_combo = ev.meta_key() && ev.shift_key() && ev.key().eq_ignore_ascii_case("v");
        let is_pc_combo = ev.ctrl_key() && ev.alt_key() && ev.key().eq_ignore_ascii_case("v");
        if is_mac_combo || is_pc_combo {
            ev.prevent_default();
            state.voice.open.update(|o| *o = !*o);
            return;
        }

        // Esc while the voice overlay is up — close it and OWN the keystroke
        // (prevent_default + return) so the palette / sidebar Esc handlers
        // below and in `app.rs` don't also fire on the same press.
        if ev.key() == "Escape" && state.voice.open.get_untracked() {
            ev.prevent_default();
            state.voice.open.set(false);
            return;
        }

        // Esc — close the palette if it's open. Other Esc handlers (e.g.
        // sidebar uncollapse in `app.rs`) keep running; we only act when
        // we actually owned the keystroke.
        if ev.key() == "Escape" && state.palette_open.get_untracked() {
            // Don't prevent_default — let other handlers still see it.
            state.close_palette();
        }

        // Bare `f` → run doctor + LLM repair (G1). Unlike ⌘K / Esc above, `f`
        // is an ordinary character a user types constantly, so it is guarded
        // hard: ANY modifier, an open palette / voice overlay, or focus sitting
        // in an editable field (the composer textarea, any text input) all
        // suppress it — otherwise typing "f" anywhere would fire a diagnostic
        // run. When clear, it bumps `repair_pulse`; the composer owns the send.
        let bare = !ev.meta_key() && !ev.ctrl_key() && !ev.alt_key() && !ev.shift_key();
        if bare
            && ev.key().eq_ignore_ascii_case("f")
            && !state.palette_open.get_untracked()
            && !state.voice.open.get_untracked()
            && !focus_is_editable()
        {
            ev.prevent_default();
            state.chat.request_repair();
        }
    });
}

/// True when keyboard focus sits in a field that accepts text — a text
/// `<input>`, a `<textarea>`, or any contenteditable host. Bare-letter
/// hotkeys must defer to these so they don't hijack a user's keystrokes.
///
/// `pub(crate)` because the memory-canvas viewport hotkeys (`views/memory/galaxy/
/// viewport_controls.rs`) are bare keys too (`+` / `-` / `0` / `shift+1`) and
/// reuse this exact guard rather than forking a second copy of it.
pub(crate) fn focus_is_editable() -> bool {
    let Some(el) = web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.active_element())
    else {
        return false;
    };
    let tag = el.tag_name();
    if tag.eq_ignore_ascii_case("input") || tag.eq_ignore_ascii_case("textarea") {
        return true;
    }
    el.dyn_ref::<web_sys::HtmlElement>()
        .is_some_and(web_sys::HtmlElement::is_content_editable)
}
