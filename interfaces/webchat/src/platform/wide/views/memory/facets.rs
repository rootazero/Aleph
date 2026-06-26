//! Top facet chips (layer/category switch + count badges) for the memory
//! console. Selection is reported via an `on_select` callback so the parent
//! co-locates page-reset with the click. Pure I/O (R4).

use leptos::prelude::*;

use super::data::MemoryFacet;
use crate::i18n::{t_string, use_i18n};

/// A single facet chip with a count badge.
#[component]
fn FacetChip(
    facet: MemoryFacet,
    active: RwSignal<MemoryFacet>,
    label: String,
    badge: Signal<String>,
    on_select: Callback<MemoryFacet>,
) -> impl IntoView {
    view! {
        <button
            class=move || if active.get() == facet {
                "px-3 py-1.5 text-sm font-medium rounded-lg bg-primary-subtle text-primary"
            } else {
                "px-3 py-1.5 text-sm font-medium rounded-lg text-text-tertiary hover:text-text-secondary transition-colors"
            }
            on:click=move |_| on_select.run(facet)
        >
            {label}
            <span class="ml-1.5 text-[10px] font-mono text-text-tertiary tabular-nums">
                {move || badge.get()}
            </span>
        </button>
    }
}

/// Facet bar. `counts` = `[AllNotes, Facts, Feedback, Lessons]` (note window);
/// `raw_count` = stats total memories (or `None` while unknown). `on_select`
/// fires on every chip click (parent resets the relevant page).
#[component]
pub fn FacetBar(
    active: RwSignal<MemoryFacet>,
    counts: Signal<[usize; 4]>,
    raw_count: Signal<Option<u64>>,
    on_select: Callback<MemoryFacet>,
) -> impl IntoView {
    let i18n = use_i18n();
    view! {
        <div class="flex items-center gap-1 flex-wrap">
            <FacetChip
                facet=MemoryFacet::AllNotes active=active on_select=on_select
                label=t_string!(i18n, memory.facet_all_notes).to_string()
                badge=Signal::derive(move || counts.get()[0].to_string())
            />
            <FacetChip
                facet=MemoryFacet::Facts active=active on_select=on_select
                label=t_string!(i18n, memory.facet_facts).to_string()
                badge=Signal::derive(move || counts.get()[1].to_string())
            />
            <FacetChip
                facet=MemoryFacet::Feedback active=active on_select=on_select
                label=t_string!(i18n, memory.facet_feedback).to_string()
                badge=Signal::derive(move || counts.get()[2].to_string())
            />
            <FacetChip
                facet=MemoryFacet::Lessons active=active on_select=on_select
                label=t_string!(i18n, memory.facet_lessons).to_string()
                badge=Signal::derive(move || counts.get()[3].to_string())
            />
            <span class="mx-1 text-border select-none">"|"</span>
            <FacetChip
                facet=MemoryFacet::Raw active=active on_select=on_select
                label=t_string!(i18n, memory.facet_raw).to_string()
                badge=Signal::derive(move || raw_count.get().map(|c| c.to_string()).unwrap_or_default())
            />
        </div>
    }
}
