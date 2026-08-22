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

/// Facet bar. `counts` = `[AllNotes, Facts, Feedback, Lessons]` over the loaded
/// window; `raw_count` = the agent's raw total; `hits_count` = number of
/// server-side search hits, or `None` when no search is active.
///
/// The hits chip appears only while a search is live. Leaving an empty
/// "Search results 0" chip behind after the box is cleared would imply the
/// store had been searched and found wanting.
#[component]
pub fn FacetBar(
    active: RwSignal<MemoryFacet>,
    counts: Signal<[usize; 4]>,
    raw_count: Signal<Option<u64>>,
    hits_count: Signal<Option<usize>>,
    /// Curated entry count, or `None` while that fetch is in flight / failed.
    /// A blank badge is honest there; a `0` would claim an empty hot tier.
    curated_count: Signal<Option<usize>>,
    on_select: Callback<MemoryFacet>,
) -> impl IntoView {
    let i18n = use_i18n();
    view! {
        <div class="flex items-center gap-1 flex-wrap">
            // Hot tier first: it is the block the model reads on every single
            // turn, so it leads the bar the way it leads the prompt.
            <FacetChip
                facet=MemoryFacet::Curated active=active on_select=on_select
                label=t_string!(i18n, memory.facet_curated).to_string()
                badge=Signal::derive(move || curated_count.get().map(|c| c.to_string()).unwrap_or_default())
            />
            <span class="mx-1 text-border select-none">"|"</span>
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
            <Show when=move || hits_count.get().is_some()>
                <span class="mx-1 text-border select-none">"|"</span>
                <FacetChip
                    facet=MemoryFacet::SearchHits active=active on_select=on_select
                    label=t_string!(i18n, memory.facet_search_hits).to_string()
                    badge=Signal::derive(move || hits_count.get().map(|c| c.to_string()).unwrap_or_default())
                />
            </Show>
        </div>
    }
}
