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
    // Agent picker popover visibility — closes on mouse-leave to mirror the chat
    // sidebar agent picker (replaces the native <select> dismissal).
    let agent_open = RwSignal::new(false);

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

            // Shared agent selector — custom popover that closes on mouse-leave,
            // mirroring the chat sidebar agent picker (replaces the native <select>
            // so its dismissal matches the rest of the app's pickers).
            <div class="relative min-w-[160px]">
                <button
                    type="button"
                    class="w-full flex items-center gap-2 px-3 py-1.5 rounded-lg bg-surface-raised \
                           border border-border text-sm text-text-primary hover:border-primary/60 \
                           focus:outline-none focus:ring-2 focus:ring-primary/30 transition-colors"
                    on:click=move |_| agent_open.update(|v| *v = !*v)
                >
                    <span class="flex-1 min-w-0 truncate text-left">
                        {move || {
                            let id = mem.agent_id.get();
                            mem.agents.get().iter().find(|a| a.id == id)
                                .map(|a| a.name.as_deref()
                                    .map(|n| if let Some(e) = a.emoji.as_deref() { format!("{e} {n}") } else { n.to_string() })
                                    .unwrap_or_else(|| a.id.clone()))
                                .unwrap_or(id)
                        }}
                    </span>
                    <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor"
                         stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"
                         class=move || if agent_open.get() {
                             "flex-shrink-0 text-text-tertiary rotate-180 transition-transform"
                         } else {
                             "flex-shrink-0 text-text-tertiary transition-transform"
                         }
                    >
                        <polyline points="18 15 12 9 6 15" />
                    </svg>
                </button>

                <Show when=move || agent_open.get()>
                    <div class="glass animate-pop-in absolute top-full left-0 right-0 mt-2 z-50 \
                                max-h-[60vh] overflow-y-auto rounded-xl border border-border \
                                bg-surface-overlay/85 shadow-xl p-1.5 space-y-0.5"
                        on:mouseleave=move |_| agent_open.set(false)>
                        {move || {
                            let cur = mem.agent_id.get();
                            let agents = mem.agents.get();
                            if agents.is_empty() {
                                return view! {
                                    <div class="px-3 py-2 text-sm text-text-tertiary truncate">{cur.clone()}</div>
                                }.into_any();
                            }
                            agents.into_iter().map(|a| {
                                let id = a.id.clone();
                                let id_for_click = id.clone();
                                let label = a.name.as_deref()
                                    .map(|n| if let Some(e) = a.emoji.as_deref() { format!("{e} {n}") } else { n.to_string() })
                                    .unwrap_or_else(|| a.id.clone());
                                let is_selected = id == cur;
                                view! {
                                    <button
                                        type="button"
                                        class=move || {
                                            let base = "w-full flex items-center gap-2 px-3 py-2 \
                                                        rounded-lg text-sm text-left";
                                            if is_selected { format!("{base} nav-tile-active") } else { format!("{base} nav-tile") }
                                        }
                                        on:click=move |_| { agent_open.set(false); mem.agent_id.set(id_for_click.clone()); }
                                    >
                                        <span class="flex-1 min-w-0 truncate">{label}</span>
                                        {is_selected.then(|| view! {
                                            <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor"
                                                 stroke-width="3" stroke-linecap="round" stroke-linejoin="round"
                                                 class="flex-shrink-0 text-primary">
                                                <polyline points="20 6 9 17 4 12" />
                                            </svg>
                                        })}
                                    </button>
                                }
                            }).collect_view().into_any()
                        }}
                    </div>
                </Show>
            </div>
        </div>
    }
}
