//! Memory Hub — single host that unifies the Canvas graph and the Vault table
//! behind one toolbar and a CSS-`display` view toggle (keep-alive: neither
//! view re-mounts on switch). Shared state lives in `MemoryState`. Pure I/O (R4).

use leptos::prelude::*;
use leptos_router::hooks::use_location;

use crate::state::memory::{parse_view_param, MemoryState, MemoryView};
use crate::views::memory::galaxy::GalaxyView;
use crate::views::memory::Memory;

mod sidebar;
pub use sidebar::MemorySidebar;

#[component]
#[must_use]
pub fn MemoryHub() -> impl IntoView {
    let mem = expect_context::<MemoryState>();
    let location = use_location();

    // Honor `?view=` when the URL query changes — e.g. the /dashboard/memory
    // redirect lands on /memory?view=table. Manual toolbar toggles change
    // `memory_view` WITHOUT touching the URL, so this Effect never fights them
    // (it only re-runs on an actual query-string change, and ignores absence).
    Effect::new(move |_| {
        let search = location.search.get();
        if let Some(v) = parse_view_param(&search) {
            mem.memory_view.set(v);
        }
    });

    let is_graph = Memo::new(move |_| mem.memory_view.get() == MemoryView::Graph);

    view! {
        <div class="h-full min-h-0 relative">
            <div
                class="absolute inset-0"
                style:display=move || if is_graph.get() { "block" } else { "none" }
            >
                <GalaxyView />
            </div>
            <div
                class="absolute inset-0 overflow-y-auto"
                style:display=move || if is_graph.get() { "none" } else { "block" }
            >
                <Memory />
            </div>
        </div>
    }
}
