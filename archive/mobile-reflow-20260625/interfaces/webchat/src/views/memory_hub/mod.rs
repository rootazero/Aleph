//! Memory Hub — single host that unifies the Canvas graph and the Vault table
//! behind one toolbar and a CSS-`display` view toggle (keep-alive: neither
//! view re-mounts on switch). Shared state lives in `MemoryState`. Pure I/O (R4).

use leptos::prelude::*;
use leptos_router::hooks::use_location;

use crate::state::memory::{parse_view_param, MemoryState, MemoryView};
use crate::state::viewport::ViewportState;
use crate::views::canvas::CanvasView;
use crate::views::memory::Memory;

mod sidebar;
pub use sidebar::MemorySidebar;

#[component]
#[must_use]
pub fn MemoryHub() -> impl IntoView {
    let mem = expect_context::<MemoryState>();
    let viewport = expect_context::<ViewportState>();
    let location = use_location();
    let search = location.search;

    // Honor `?view=` when the URL query changes — e.g. the /dashboard/memory
    // redirect lands on /memory?view=table. Manual toolbar toggles change
    // `memory_view` WITHOUT touching the URL, so this Effect never fights them
    // (it only re-runs on an actual query-string change, and ignores absence).
    Effect::new(move |_| {
        if let Some(v) = parse_view_param(&search.get()) {
            mem.memory_view.set(v);
        }
    });

    // Mobile default: open the Vault list, not the WebGL galaxy (heavy + not
    // touch-friendly on a phone). One-shot — untracked reads give it no
    // reactive deps so it runs once on mount — and only when the URL doesn't
    // pin a view; manual toolbar/drawer toggles afterward are respected.
    Effect::new(move |_| {
        if viewport.is_mobile.get_untracked() && parse_view_param(&search.get_untracked()).is_none()
        {
            mem.memory_view.set(MemoryView::Table);
        }
    });

    let is_graph = Memo::new(move |_| mem.memory_view.get() == MemoryView::Graph);
    let galaxy_unsupported = mem.galaxy_unsupported;
    let i18n = crate::i18n::use_i18n();

    view! {
        <div class="h-full min-h-0 relative">
            <div
                class="absolute inset-0"
                style:display=move || if is_graph.get() { "block" } else { "none" }
            >
                <CanvasView />
            </div>
            <div
                class="absolute inset-0 overflow-y-auto"
                style:display=move || if is_graph.get() { "none" } else { "block" }
            >
                {move || galaxy_unsupported.get().then(|| view! {
                    <div class="px-3 py-2 text-xs text-amber-200 bg-amber-900/30
                                border-b border-amber-700/40">
                        {crate::i18n::t!(i18n, memory.galaxy_unsupported)}
                    </div>
                })}
                <Memory />
            </div>
        </div>
    }
}
