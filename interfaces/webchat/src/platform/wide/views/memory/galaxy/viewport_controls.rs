//! Viewport control cluster for the memory galaxy: fit / zoom / reset / focus,
//! plus the graph-scoped keyboard shortcuts that drive the same commands.
//!
//! Before this existed the only way to move the camera was drag / shift-drag /
//! wheel — so a trackpad-less or touch user had no zoom affordance at all, and
//! nobody had a way back to a known pose after orbiting into a degenerate
//! elevation. Every control here is pure I/O (R4): it writes a `ViewportCmd`
//! into the host→Scene intent channel and nothing else.
//!
//! Placement is bottom-LEFT. The canvas already carries the truncation badge at
//! top-right and the `NodeDetailPanel` overlay at bottom-right (w-72, up to 60%
//! of the height), so bottom-left is the only corner that stays clear at every
//! viewport size; the memory sidebar is a sibling of the canvas box, not an
//! overlay on it, so it does not compete for that corner either.

use leptos::callback::Callback;
use leptos::ev::keydown;
use leptos::prelude::*;

use super::gl::scene::ViewportCmd;
use super::interaction::CanvasEvent;
use crate::i18n::{t_string, use_i18n};
use crate::state::hotkey::focus_is_editable;
use crate::state::memory::{MemoryState, MemoryView};

/// Floating viewport cluster over the galaxy canvas.
///
/// Buttons are DISABLED, never hidden, when their command has no target — the
/// affordance stays visible (and its tooltip keeps teaching the shortcut) even
/// while the graph is empty. There is deliberately no zoom-% readout: an orbit
/// camera has no scale percentage.
#[component]
#[must_use]
pub fn ViewportControls(
    /// Host → Scene intent channel; every control writes a command here and
    /// `GalaxyCanvas` drains it into `Scene::apply_cmd`.
    viewport_cmd: RwSignal<Option<ViewportCmd>>,
    /// The canvas's existing event path — Escape deselects through it rather
    /// than reaching around it.
    on_event: Callback<CanvasEvent>,
    /// False while the galaxy is empty or still loading: Fit / Reset have
    /// nothing to frame.
    has_nodes: Signal<bool>,
    /// False when no node is selected: Focus has no target.
    has_selection: Signal<bool>,
) -> impl IntoView {
    let mem = expect_context::<MemoryState>();
    let i18n = use_i18n();

    install_hotkeys(viewport_cmd, on_event, mem, has_nodes, has_selection);

    // Styled like the truncation badge (bg-black/40, 11px, white/70) so the
    // canvas overlays read as one family.
    let btn = "px-1.5 py-0.5 rounded hover:bg-white/10 disabled:opacity-40 \
               disabled:hover:bg-transparent transition-colors";

    view! {
        <div class="absolute bottom-3 left-3 flex items-center gap-0.5 select-none \
                    bg-black/40 rounded px-1 py-0.5 text-[11px] text-white/70">
            <button
                type="button"
                class=btn
                title=move || format!("{}  −", t_string!(i18n, memory.zoom_out))
                on:click=move |_| viewport_cmd.set(Some(ViewportCmd::ZoomOut))
            >
                "−"
            </button>
            <button
                type="button"
                class=btn
                title=move || format!("{}  +", t_string!(i18n, memory.zoom_in))
                on:click=move |_| viewport_cmd.set(Some(ViewportCmd::ZoomIn))
            >
                "+"
            </button>
            <button
                type="button"
                class=btn
                disabled=move || !has_nodes.get()
                title=move || format!("{}  ⇧1", t_string!(i18n, memory.fit_view))
                on:click=move |_| viewport_cmd.set(Some(ViewportCmd::Fit))
            >
                {move || t_string!(i18n, memory.fit_view).to_string()}
            </button>
            <button
                type="button"
                class=btn
                disabled=move || !has_nodes.get()
                title=move || format!("{}  0", t_string!(i18n, memory.reset_view))
                on:click=move |_| viewport_cmd.set(Some(ViewportCmd::Reset))
            >
                {move || t_string!(i18n, memory.reset_view).to_string()}
            </button>
            <button
                type="button"
                class=btn
                disabled=move || !has_selection.get()
                title=move || format!("{}  ⇧2", t_string!(i18n, memory.focus_selected))
                on:click=move |_| viewport_cmd.set(Some(ViewportCmd::FocusSelection))
            >
                {move || t_string!(i18n, memory.focus_selected).to_string()}
            </button>
        </div>
    }
}

/// Install the graph-scoped keydown listener and tear it down with the
/// component (the canvas remounts across the phone/wide form-factor line, so a
/// leaked listener would stack).
///
/// Bindings (tldraw's canonical set, mapped to an orbit camera):
/// - **⇧1** fit to graph, **⇧2** focus the selected node
/// - **+ / =** zoom in, **-** zoom out, **0** reset the view
/// - **Esc** deselect
///
/// Two guards, both mandatory:
/// 1. `focus_is_editable()` — these are bare keys, so the sidebar search box
///    (and the detail panel's note editor) would otherwise swallow every `-`
///    the user types. Reuses `state::hotkey`'s guard, not a second copy.
/// 2. `memory_view == Graph` — MemoryHub keeps the canvas mounted under
///    `display:none` while the table view is showing, so an unguarded listener
///    would steal keys from a view the user cannot even see.
fn install_hotkeys(
    viewport_cmd: RwSignal<Option<ViewportCmd>>,
    on_event: Callback<CanvasEvent>,
    mem: MemoryState,
    has_nodes: Signal<bool>,
    has_selection: Signal<bool>,
) {
    let handle = window_event_listener(keydown, move |ev: web_sys::KeyboardEvent| {
        if focus_is_editable() || mem.memory_view.get_untracked() != MemoryView::Graph {
            return;
        }
        // Never shadow the browser's own chords (⌘+/- page zoom, ⌘0, …).
        if ev.meta_key() || ev.ctrl_key() || ev.alt_key() {
            return;
        }

        // ⇧1 / ⇧2 are read off `code()`: on a US layout `key()` for those
        // presses is "!" / "@", not "1" / "2".
        let cmd = if ev.shift_key() && ev.code() == "Digit1" {
            Some(ViewportCmd::Fit)
        } else if ev.shift_key() && ev.code() == "Digit2" {
            Some(ViewportCmd::FocusSelection)
        } else {
            match ev.key().as_str() {
                // "+" is shift+"=" on a US layout; "_" is shift+"-".
                "=" | "+" => Some(ViewportCmd::ZoomIn),
                "-" | "_" => Some(ViewportCmd::ZoomOut),
                "0" => Some(ViewportCmd::Reset),
                _ => None,
            }
        };

        // Same enablement rule as the buttons: a command with no target is not
        // dispatched (and does not swallow the keystroke).
        let enabled = match cmd {
            Some(ViewportCmd::Fit | ViewportCmd::Reset) => has_nodes.get_untracked(),
            Some(ViewportCmd::FocusSelection) => has_selection.get_untracked(),
            Some(_) => true,
            None => false,
        };
        if enabled {
            ev.prevent_default();
            viewport_cmd.set(cmd);
            return;
        }

        if ev.key() == "Escape" && has_selection.get_untracked() {
            on_event.run(CanvasEvent::DeselectNode);
        }
    });
    on_cleanup(move || handle.remove());
}
