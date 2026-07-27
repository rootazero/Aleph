//! Memory-mode left sidebar controls — agent selector, graph/list view toggle,
//! search box, and the edge-density slider, stacked top-to-bottom. Pure
//! I/O: reads/writes `MemoryState` only (R4). Replaces both the former
//! `MemoryToolbar` (which sat atop the canvas) and the old `NodeDetailPanel`
//! sidebar instance, leaving the canvas overlay as the single node-detail surface.

use leptos::prelude::*;

use crate::i18n::{t_string, use_i18n};
use crate::state::memory::{MemoryState, MemoryView};

#[component]
#[must_use]
pub fn MemorySidebar() -> impl IntoView {
    let mem = expect_context::<MemoryState>();
    let i18n = use_i18n();
    let is_graph = Memo::new(move |_| mem.memory_view.get() == MemoryView::Graph);
    // Agent picker popover visibility — closes on mouse-leave, mirroring the
    // chat sidebar / former toolbar picker.
    let agent_open = RwSignal::new(false);

    view! {
        <div class="flex flex-col h-full">
            // ── Agent selector (popover drops downward; selector sits at top) ──
            <div class="px-3 pt-3 pb-2">
                <div class="relative">
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
                            <polyline points="6 9 12 15 18 9" />
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

            // ── Graph / List view toggle — two vertical nav tiles ──
            <div class="px-3 py-1 space-y-0.5">
                <button
                    class=move || if is_graph.get() {
                        "nav-tile-active w-full flex items-center gap-3 px-3 py-2 rounded-lg text-sm"
                    } else {
                        "nav-tile w-full flex items-center gap-3 px-3 py-2 rounded-lg text-sm"
                    }
                    on:click=move |_| mem.memory_view.set(MemoryView::Graph)
                >
                    <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor"
                         stroke-width="2" stroke-linecap="round" stroke-linejoin="round"
                         class=move || if is_graph.get() { "text-sidebar-accent flex-shrink-0" } else { "text-text-tertiary flex-shrink-0" }
                    >
                        <circle cx="5" cy="6" r="2" /><circle cx="19" cy="6" r="2" /><circle cx="12" cy="18" r="2" />
                        <line x1="6.6" y1="7.4" x2="10.6" y2="16.4" /><line x1="17.4" y1="7.4" x2="13.4" y2="16.4" /><line x1="7" y1="6" x2="17" y2="6" />
                    </svg>
                    <span>{move || t_string!(i18n, memory.hub_view_graph).to_string()}</span>
                </button>
                <button
                    class=move || if is_graph.get() {
                        "nav-tile w-full flex items-center gap-3 px-3 py-2 rounded-lg text-sm"
                    } else {
                        "nav-tile-active w-full flex items-center gap-3 px-3 py-2 rounded-lg text-sm"
                    }
                    on:click=move |_| mem.memory_view.set(MemoryView::Table)
                >
                    <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor"
                         stroke-width="2" stroke-linecap="round" stroke-linejoin="round"
                         class=move || if is_graph.get() { "text-text-tertiary flex-shrink-0" } else { "text-sidebar-accent flex-shrink-0" }
                    >
                        <line x1="8" y1="6" x2="21" y2="6" /><line x1="8" y1="12" x2="21" y2="12" /><line x1="8" y1="18" x2="21" y2="18" />
                        <line x1="3" y1="6" x2="3.01" y2="6" /><line x1="3" y1="12" x2="3.01" y2="12" /><line x1="3" y1="18" x2="3.01" y2="18" />
                    </svg>
                    <span>{move || t_string!(i18n, memory.hub_view_table).to_string()}</span>
                </button>
            </div>

            // ── Search — live writes search_query; Enter bumps search_nonce ──
            <div class="px-3 py-2">
                <div class="relative">
                    <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor"
                         stroke-width="2" stroke-linecap="round" stroke-linejoin="round"
                         class="absolute left-2.5 top-1/2 -translate-y-1/2 text-text-tertiary"
                    >
                        <circle cx="11" cy="11" r="8" />
                        <path d="m21 21-4.35-4.35" />
                    </svg>
                    <input
                        type="search"
                        placeholder=t_string!(i18n, memory.search_placeholder)
                        class="w-full pl-8 pr-2 py-1.5 rounded-md bg-surface-sunken border border-border text-sm text-text-primary placeholder:text-text-tertiary focus:outline-none focus:border-primary/60 focus:ring-1 focus:ring-primary/30"
                        prop:value=move || mem.search_query.get()
                        on:input=move |ev| mem.search_query.set(event_target_value(&ev))
                        on:keydown=move |ev| { if ev.key() == "Enter" { mem.search_nonce.update(|n| *n += 1); } }
                    />
                    <Show when=move || !mem.search_query.get().trim().is_empty()>
                        <p class="mt-1 text-[10px] leading-snug text-text-tertiary">
                            {move || t_string!(i18n, memory.search_hint_local).to_string()}
                        </p>
                    </Show>
                </div>
            </div>

            // ── Edge-density slider (drives the canvas LOD; see `fold_to_lod`) ──
            <div class="px-3 pt-2 pb-3">
                <label style="font-size:9.5px;color:var(--color-text-secondary);text-transform:uppercase;letter-spacing:0.05em">
                    {move || t_string!(i18n, memory.edge_density).to_string()}
                </label>
                <input
                    type="range" min="0" max="10" step="1"
                    class="w-full mt-1 accent-[#a78bfa]"
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
    }
}
