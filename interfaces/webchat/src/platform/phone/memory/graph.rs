//! Phone Graph screen (`/memory/graph`): full-screen WebGL galaxy — reuses the
//! desktop `CanvasView` — with a back button to the Memory menu and a floating
//! Fold (edge-density) slider. Pure presentation; `CanvasView` reads/writes
//! `MemoryState` (R4). The Fold slider writes `mem.fold_threshold`; CanvasView's
//! Fold→LOD Effect reacts (no extra wiring here).

use leptos::prelude::*;

use crate::platform::phone::shell::PhoneShell;
use crate::state::memory::MemoryState;
use crate::views::canvas::CanvasView;

#[component]
#[must_use]
pub fn PhoneMemoryGraph() -> impl IntoView {
    let mem = expect_context::<MemoryState>();
    view! {
        <PhoneShell title="Memory" back="/memory" back_label="Memory">
        // Single element child for PhoneShell (footgun: no bare dynamic block
        // as a direct PhoneShell child). A definite-height flex child so the
        // WebGL canvas (`w-full h-full`) resolves its height.
        <div style="position:relative; flex:1; min-height:0; border-radius:12px; overflow:hidden;">
            <CanvasView/>
            // Floating Fold (edge-density) control — mirrors the desktop sidebar
            // slider (sidebar.rs). Writes mem.fold_threshold; CanvasView reacts.
            <div
                class="glass"
                style="position:absolute; left:10px; right:10px; bottom:10px; display:flex; align-items:center; gap:10px; padding:8px 12px; border-radius:12px;"
            >
                <span style="font-size:9.5px; text-transform:uppercase; letter-spacing:0.05em; color:var(--color-text-secondary);">"Fold"</span>
                <input
                    type="range" min="0" max="10" step="1" style="flex:1;" class="accent-[#a78bfa]"
                    prop:value=move || mem.fold_threshold.get() as i32
                    on:input=move |ev| {
                        if let Ok(v) = event_target_value(&ev).parse::<usize>() {
                            mem.fold_threshold.set(v);
                        }
                    }
                />
            </div>
        </div>
        </PhoneShell>
    }
}
