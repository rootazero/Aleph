//! Phone Graph screen (`/memory/graph`): full-screen WebGL galaxy — reuses the
//! desktop `GalaxyView` — with a back button to the Memory menu and a floating
//! edge-density slider. Pure presentation; `GalaxyView` reads/writes
//! `MemoryState` (R4). The slider writes `mem.fold_threshold`; GalaxyView's
//! Fold→LOD Effect reacts (no extra wiring here).

use leptos::prelude::*;

use crate::i18n::{t_string, use_i18n};
use crate::platform::phone::shell::PhoneShell;
use crate::state::memory::MemoryState;
use crate::views::memory::galaxy::GalaxyView;

#[component]
#[must_use]
pub fn PhoneMemoryGraph() -> impl IntoView {
    let mem = expect_context::<MemoryState>();
    let i18n = use_i18n();
    view! {
        <PhoneShell title="Memory" back="/memory" back_label="Memory">
        // Single element child for PhoneShell (footgun: no bare dynamic block
        // as a direct PhoneShell child). A definite-height flex child so the
        // WebGL canvas (`w-full h-full`) resolves its height.
        <div style="position:relative; flex:1; min-height:0; border-radius:12px; overflow:hidden;">
            <GalaxyView/>
            // Floating edge-density control — mirrors the desktop sidebar slider
            // (sidebar.rs). Writes mem.fold_threshold; GalaxyView reacts.
            // Docked to the TOP of the canvas box: GalaxyView now mounts the
            // viewport control cluster at bottom-left, and a full-width bottom
            // bar would sit on top of it.
            <div
                class="glass"
                style="position:absolute; left:10px; right:10px; top:10px; display:flex; align-items:center; gap:10px; padding:8px 12px; border-radius:12px;"
            >
                <span style="font-size:9.5px; text-transform:uppercase; letter-spacing:0.05em; color:var(--color-text-secondary);">
                    {move || t_string!(i18n, memory.edge_density).to_string()}
                </span>
                <input
                    type="range" min="0" max="10" step="1" style="flex:1;" class="accent-[#a78bfa]"
                    title=move || t_string!(i18n, memory.edge_density_hint).to_string()
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
