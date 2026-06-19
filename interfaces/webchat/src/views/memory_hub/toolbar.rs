//! Shared Memory Hub toolbar: view toggle (graph⇄table), one search box bound
//! to the shared `search_query`, and the shared agent selector. Pure I/O — it
//! only reads/writes `MemoryState` (R4).

use leptos::prelude::*;

use crate::i18n::{t_string, use_i18n};
use crate::state::memory::{MemoryState, MemoryView};

#[component]
#[must_use]
pub fn MemoryToolbar() -> impl IntoView {
    let mem = expect_context::<MemoryState>();
    let i18n = use_i18n();
    let is_graph = Memo::new(move |_| mem.memory_view.get() == MemoryView::Graph);

    view! {
        <div class="flex items-center gap-3 px-6 py-3 border-b border-border flex-wrap aleph-content-top">
            // View toggle
            <div class="inline-flex rounded-lg border border-border overflow-hidden">
                <button
                    class=move || if is_graph.get() {
                        "px-3 py-1.5 text-sm bg-primary-subtle text-primary"
                    } else {
                        "px-3 py-1.5 text-sm text-text-tertiary hover:text-text-secondary transition-colors"
                    }
                    on:click=move |_| mem.memory_view.set(MemoryView::Graph)
                >
                    {move || t_string!(i18n, memory.hub_view_graph).to_string()}
                </button>
                <button
                    class=move || if is_graph.get() {
                        "px-3 py-1.5 text-sm text-text-tertiary hover:text-text-secondary transition-colors"
                    } else {
                        "px-3 py-1.5 text-sm bg-primary-subtle text-primary"
                    }
                    on:click=move |_| mem.memory_view.set(MemoryView::Table)
                >
                    {move || t_string!(i18n, memory.hub_view_table).to_string()}
                </button>
            </div>

            // Shared search — Enter bumps `search_nonce` so the table commits its
            // server search; the graph reads `search_query` live for highlight.
            <div class="relative flex-1 min-w-[180px] max-w-md">
                <input
                    type="search"
                    placeholder=t_string!(i18n, memory.search_placeholder)
                    class="w-full px-3 py-1.5 bg-surface-raised border border-border rounded-lg text-sm text-text-primary placeholder:text-text-tertiary focus:outline-none focus:border-primary/50"
                    prop:value=move || mem.search_query.get()
                    on:input=move |ev| mem.search_query.set(event_target_value(&ev))
                    on:keydown=move |ev| { if ev.key() == "Enter" { mem.search_nonce.update(|n| *n += 1); } }
                />
            </div>

            // Shared agent selector
            <select
                class="px-3 py-1.5 bg-surface-raised border border-border rounded-lg text-sm text-text-primary focus:outline-none focus:border-primary/50"
                prop:value=move || mem.agent_id.get()
                on:change=move |ev| mem.agent_id.set(event_target_value(&ev))
            >
                {move || {
                    let current = mem.agent_id.get();
                    let agents = mem.agents.get();
                    if agents.is_empty() {
                        view! { <option value=current.clone()>{current}</option> }.into_any()
                    } else {
                        agents.into_iter().map(|a| {
                            let id = a.id.clone();
                            let label = a.name.as_deref()
                                .map(|n| if let Some(e) = a.emoji.as_deref() { format!("{e} {n}") } else { n.to_string() })
                                .unwrap_or_else(|| a.id.clone());
                            let selected = id == mem.agent_id.get_untracked();
                            view! { <option value=id prop:selected=selected>{label}</option> }
                        }).collect_view().into_any()
                    }
                }}
            </select>
        </div>
    }
}
