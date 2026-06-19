//! Top facet chips (layer/category switch + count badges) and the agent
//! filter dropdown for the memory console. Pure I/O — selection state lives in
//! a `MemoryFacet` signal and the shared `MemoryState` (R4).

use leptos::prelude::*;

use super::data::MemoryFacet;
use crate::i18n::{t_string, use_i18n};
use crate::state::memory::MemoryState;

/// A single facet chip with a count badge.
#[component]
fn FacetChip(
    facet: MemoryFacet,
    active: RwSignal<MemoryFacet>,
    label: String,
    badge: Signal<String>,
) -> impl IntoView {
    view! {
        <button
            class=move || if active.get() == facet {
                "px-3 py-1.5 text-sm font-medium rounded-lg bg-primary-subtle text-primary"
            } else {
                "px-3 py-1.5 text-sm font-medium rounded-lg text-text-tertiary hover:text-text-secondary transition-colors"
            }
            on:click=move |_| active.set(facet)
        >
            {label}
            <span class="ml-1.5 text-[10px] font-mono text-text-tertiary tabular-nums">
                {move || badge.get()}
            </span>
        </button>
    }
}

/// Facet bar. `counts` = `[AllNotes, Facts, Feedback, Lessons]` (note window);
/// `raw_count` = stats total memories (or `None` while unknown).
#[component]
pub fn FacetBar(
    active: RwSignal<MemoryFacet>,
    counts: Signal<[usize; 4]>,
    raw_count: Signal<Option<u64>>,
) -> impl IntoView {
    let i18n = use_i18n();
    view! {
        <div class="flex items-center gap-1 flex-wrap">
            <FacetChip
                facet=MemoryFacet::AllNotes active=active
                label=t_string!(i18n, memory.facet_all_notes).to_string()
                badge=Signal::derive(move || counts.get()[0].to_string())
            />
            <FacetChip
                facet=MemoryFacet::Facts active=active
                label=t_string!(i18n, memory.facet_facts).to_string()
                badge=Signal::derive(move || counts.get()[1].to_string())
            />
            <FacetChip
                facet=MemoryFacet::Feedback active=active
                label=t_string!(i18n, memory.facet_feedback).to_string()
                badge=Signal::derive(move || counts.get()[2].to_string())
            />
            <FacetChip
                facet=MemoryFacet::Lessons active=active
                label=t_string!(i18n, memory.facet_lessons).to_string()
                badge=Signal::derive(move || counts.get()[3].to_string())
            />
            <span class="mx-1 text-border select-none">"|"</span>
            <FacetChip
                facet=MemoryFacet::Raw active=active
                label=t_string!(i18n, memory.facet_raw).to_string()
                badge=Signal::derive(move || raw_count.get().map(|c| c.to_string()).unwrap_or_default())
            />
        </div>
    }
}

/// Agent dropdown — shares `MemoryState` with the Canvas so switching agent is
/// consistent across both memory surfaces. Displays agent name, falling back to
/// its id.
#[component]
pub fn AgentFilter() -> impl IntoView {
    let mem = expect_context::<MemoryState>();
    view! {
        <select
            class="px-3 py-1.5 bg-surface-raised border border-border rounded-lg text-sm text-text-primary focus:outline-none focus:border-primary/50"
            on:change=move |ev| mem.agent_id.set(event_target_value(&ev))
            prop:value=move || mem.agent_id.get()
        >
            {move || mem.agents.get().into_iter().map(|a| {
                let id = a.id.clone();
                let label = a.name.clone().filter(|n| !n.is_empty()).unwrap_or_else(|| a.id.clone());
                view! { <option value=id>{label}</option> }
            }).collect_view()}
        </select>
    }
}
