use leptos::prelude::*;
use crate::components::ui::*;
use crate::context::DashboardState;
use crate::api::{MemoryApi, RawMemory, CompressedFact, MemoryStats};

#[component]
pub fn Memory() -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let is_disabled = Signal::derive(move || !state.is_connected.get());

    // Memory stats
    let stats = RwSignal::new(None::<MemoryStats>);

    // Active tab: "facts" or "raw"
    let active_tab = RwSignal::new("facts".to_string());

    // Facts data
    let facts_list = RwSignal::new(Vec::<CompressedFact>::new());
    let facts_loaded = RwSignal::new(false);

    // Raw memories data
    let search_query = RwSignal::new(String::new());
    let raw_memories = RwSignal::new(Vec::<RawMemory>::new());
    let is_searching = RwSignal::new(false);
    let raw_loaded = RwSignal::new(false);

    // Fetch stats + facts + raw memories when connected
    Effect::new(move || {
        if state.is_connected.get() {
            let state = state.clone();
            leptos::task::spawn_local(async move {
                if let Ok(s) = MemoryApi::stats(&state).await {
                    stats.set(Some(s));
                }
                if let Ok(facts) = MemoryApi::list_facts(&state, Some(50)).await {
                    facts_list.set(facts);
                }
                facts_loaded.set(true);

                if let Ok(results) = MemoryApi::search(&state, String::new(), Some(20)).await {
                    raw_memories.set(results);
                }
                raw_loaded.set(true);
            });
        } else {
            stats.set(None);
            facts_list.set(Vec::new());
            raw_memories.set(Vec::new());
            facts_loaded.set(false);
            raw_loaded.set(false);
        }
    });

    // Search handler for raw memories
    let do_search = move || {
        let query = search_query.get();
        let state = state.clone();
        leptos::task::spawn_local(async move {
            is_searching.set(true);
            if let Ok(results) = MemoryApi::search(&state, query, Some(20)).await {
                raw_memories.set(results);
            }
            is_searching.set(false);
        });
    };

    // Delete handler for raw memories
    let on_delete = move |memory_id: String| {
        let state = state.clone();
        leptos::task::spawn_local(async move {
            if MemoryApi::delete(&state, memory_id).await.is_ok() {
                if let Ok(s) = MemoryApi::stats(&state).await {
                    stats.set(Some(s));
                }
                let q = search_query.get_untracked();
                if let Ok(results) = MemoryApi::search(&state, q, Some(20)).await {
                    raw_memories.set(results);
                }
                if let Ok(facts) = MemoryApi::list_facts(&state, Some(50)).await {
                    facts_list.set(facts);
                }
            }
        });
    };

    view! {
        <div class="p-8 max-w-7xl mx-auto space-y-8">
            <header class="flex items-center justify-between">
                <div>
                    <h2 class="text-3xl font-bold tracking-tight mb-2 flex items-center gap-3 text-text-primary">
                        <svg width="32" height="32" attr:class="w-8 h-8 text-primary" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                            <ellipse cx="12" cy="5" rx="9" ry="3" />
                            <path d="M21 12c0 1.66-4 3-9 3s-9-1.34-9-3" />
                            <path d="M3 5v14c0 1.66 4 3 9 3s9-1.34 9-3V5" />
                        </svg>
                        "Memory Vault"
                    </h2>
                    <p class="text-text-secondary">"Browse and manage long-term memory: compressed facts and raw conversation records."</p>
                </div>
            </header>

            // Connection status warning
            {move || {
                if !state.is_connected.get() {
                    view! {
                        <div class="bg-warning-subtle border border-warning/20 rounded-xl p-6 flex items-start gap-4">
                            <svg width="24" height="24" attr:class="w-6 h-6 text-warning flex-shrink-0 mt-0.5" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                                <path d="M10.29 3.86L1.82 18a2 2 0 0 0 1.71 3h16.94a2 2 0 0 0 1.71-3L13.71 3.86a2 2 0 0 0-3.42 0z" />
                                <line x1="12" y1="9" x2="12" y2="13" />
                                <line x1="12" y1="17" x2="12.01" y2="17" />
                            </svg>
                            <div>
                                <h3 class="text-warning font-semibold mb-1">"Gateway Connection Required"</h3>
                                <p class="text-sm text-text-secondary">"Please connect to the Aleph Gateway from the System Status page to access memory data."</p>
                            </div>
                        </div>
                    }.into_any()
                } else {
                    view! { <div></div> }.into_any()
                }
            }}

            // Memory Stats
            <div class="grid grid-cols-1 md:grid-cols-3 gap-6">
                 <Card class="bg-primary-subtle border-primary/10 p-6 flex flex-col items-start".to_string()>
                    <span class="text-[10px] font-bold text-primary uppercase tracking-widest mb-1.5">"Compressed Facts"</span>
                    <span class="text-3xl font-bold font-mono">
                        {move || {
                            stats.get()
                                .map(|s| s.total_facts.to_string())
                                .unwrap_or_else(|| "\u{2014}".to_string())
                        }}
                    </span>
                 </Card>
                 <Card class="bg-success-subtle border-success/10 p-6 flex flex-col items-start".to_string()>
                    <span class="text-[10px] font-bold text-success uppercase tracking-widest mb-1.5">"Raw Memories"</span>
                    <span class="text-3xl font-bold font-mono">
                        {move || {
                            stats.get()
                                .map(|s| s.total_memories.to_string())
                                .unwrap_or_else(|| "\u{2014}".to_string())
                        }}
                    </span>
                 </Card>
                 <Card class="bg-primary-subtle border-primary/10 p-6 flex flex-col items-start".to_string()>
                    <span class="text-[10px] font-bold text-primary uppercase tracking-widest mb-1.5">"Graph Nodes"</span>
                    <span class="text-3xl font-bold font-mono">
                        {move || {
                            stats.get()
                                .map(|s| s.total_graph_nodes.to_string())
                                .unwrap_or_else(|| "\u{2014}".to_string())
                        }}
                    </span>
                 </Card>
            </div>

            // Tab switcher
            <div class="flex items-center gap-1 border-b border-border">
                <button
                    class=move || if active_tab.get() == "facts" {
                        "px-4 py-2 text-sm font-medium text-primary border-b-2 border-primary -mb-px"
                    } else {
                        "px-4 py-2 text-sm font-medium text-text-tertiary hover:text-text-secondary -mb-px"
                    }
                    on:click=move |_| active_tab.set("facts".to_string())
                >
                    "Compressed Facts"
                </button>
                <button
                    class=move || if active_tab.get() == "raw" {
                        "px-4 py-2 text-sm font-medium text-primary border-b-2 border-primary -mb-px"
                    } else {
                        "px-4 py-2 text-sm font-medium text-text-tertiary hover:text-text-secondary -mb-px"
                    }
                    on:click=move |_| active_tab.set("raw".to_string())
                >
                    "Raw Memories"
                </button>

                // Search bar (only for raw memories tab)
                {move || {
                    if active_tab.get() == "raw" {
                        view! {
                            <div class="ml-auto pb-1">
                                <div class="relative group">
                                    <svg width="16" height="16" attr:class="absolute left-3 top-1/2 -translate-y-1/2 w-4 h-4 text-text-tertiary group-focus-within:text-primary transition-colors" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                                        <circle cx="11" cy="11" r="8" />
                                        <line x1="21" y1="21" x2="16.65" y2="16.65" />
                                    </svg>
                                    <input
                                        type="text"
                                        placeholder="Search raw memories..."
                                        class="pl-10 pr-4 py-1.5 bg-surface-raised border border-border rounded-lg focus:outline-none focus:border-primary/50 focus:ring-4 focus:ring-primary/10 w-56 transition-all text-sm text-text-primary placeholder:text-text-tertiary"
                                        disabled=is_disabled
                                        on:input=move |ev| search_query.set(event_target_value(&ev))
                                        on:keydown=move |ev| {
                                            if ev.key() == "Enter" { do_search(); }
                                        }
                                    />
                                </div>
                            </div>
                        }.into_any()
                    } else {
                        view! { <div></div> }.into_any()
                    }
                }}
            </div>

            // Tab content
            {move || {
                if active_tab.get() == "facts" {
                    view! { <FactsTable facts=facts_list loaded=facts_loaded connected=state.is_connected /> }.into_any()
                } else {
                    let on_delete = on_delete.clone();
                    view! { <RawMemoriesTable memories=raw_memories loaded=raw_loaded searching=is_searching connected=state.is_connected on_delete=on_delete /> }.into_any()
                }
            }}
        </div>
    }
}

// ─── Facts Table ────────────────────────────────────────────────────────────

#[component]
fn FactsTable(
    facts: RwSignal<Vec<CompressedFact>>,
    loaded: RwSignal<bool>,
    connected: RwSignal<bool>,
) -> impl IntoView {
    view! {
        <Card class="overflow-hidden".to_string()>
            <table class="w-full text-left border-collapse">
                <thead>
                    <tr class="bg-surface-sunken text-[10px] font-bold text-text-tertiary uppercase tracking-widest">
                        <th class="p-4 pl-8">"Content"</th>
                        <th class="p-4">"Agent"</th>
                        <th class="p-4">"Type"</th>
                        <th class="p-4">"Confidence"</th>
                        <th class="p-4 pr-8">"Date"</th>
                    </tr>
                </thead>
                <tbody class="divide-y divide-border-subtle">
                    {move || {
                        if !connected.get() {
                            view! {
                                <tr><td colspan="5" class="p-8 text-center text-text-tertiary">"Connect to Gateway to view facts"</td></tr>
                            }.into_any()
                        } else if !loaded.get() {
                            view! {
                                <tr><td colspan="5" class="p-8 text-center text-text-tertiary">"Loading..."</td></tr>
                            }.into_any()
                        } else if facts.get().is_empty() {
                            view! {
                                <tr><td colspan="5" class="p-8 text-center text-text-tertiary">"No compressed facts yet. Facts are extracted from raw memories by the compression service."</td></tr>
                            }.into_any()
                        } else {
                            view! {
                                <For
                                    each=move || facts.get()
                                    key=|fact| fact.id.clone()
                                    children=move |fact| {
                                        let badge_variant = match fact.fact_type.as_str() {
                                            "Preference" => BadgeVariant::Indigo,
                                            "Learning" => BadgeVariant::Emerald,
                                            "Personal" => BadgeVariant::Amber,
                                            _ => BadgeVariant::Slate,
                                        };
                                        let agent_id = fact.agent_id.clone();
                                        let confidence_pct = format!("{:.0}%", fact.confidence * 100.0);
                                        let date = format_ts(fact.created_at);
                                        view! {
                                            <tr class="group hover:bg-surface-sunken transition-colors">
                                                <td class="p-4 pl-8">
                                                    <div class="text-sm font-medium text-text-primary line-clamp-2 group-hover:line-clamp-none transition-all">{fact.content}</div>
                                                    <div class="text-xs text-text-tertiary mt-0.5 font-mono">{fact.path.clone()}</div>
                                                </td>
                                                <td class="p-4">
                                                    <Badge variant=BadgeVariant::Indigo>{agent_id}</Badge>
                                                </td>
                                                <td class="p-4">
                                                    <Badge variant=badge_variant>{fact.fact_type}</Badge>
                                                </td>
                                                <td class="p-4">
                                                    <span class="text-sm font-mono text-text-secondary">{confidence_pct}</span>
                                                </td>
                                                <td class="p-4 pr-8">
                                                    <div class="flex items-center gap-2 text-xs text-text-tertiary font-mono">
                                                        <svg width="12" height="12" attr:class="w-3 h-3" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                                                            <rect x="3" y="4" width="18" height="18" rx="2" ry="2" />
                                                            <line x1="16" y1="2" x2="16" y2="6" />
                                                            <line x1="8" y1="2" x2="8" y2="6" />
                                                            <line x1="3" y1="10" x2="21" y2="10" />
                                                        </svg>
                                                        {date}
                                                    </div>
                                                </td>
                                            </tr>
                                        }
                                    }
                                />
                            }.into_any()
                        }
                    }}
                </tbody>
            </table>
        </Card>
    }
}

// ─── Raw Memories Table ─────────────────────────────────────────────────────

#[component]
fn RawMemoriesTable(
    memories: RwSignal<Vec<RawMemory>>,
    loaded: RwSignal<bool>,
    searching: RwSignal<bool>,
    connected: RwSignal<bool>,
    on_delete: impl Fn(String) + Clone + Send + 'static,
) -> impl IntoView {
    view! {
        <Card class="overflow-hidden".to_string()>
            <table class="w-full text-left border-collapse">
                <thead>
                    <tr class="bg-surface-sunken text-[10px] font-bold text-text-tertiary uppercase tracking-widest">
                        <th class="p-4 pl-8">"Content"</th>
                        <th class="p-4">"Agent"</th>
                        <th class="p-4">"Date"</th>
                        <th class="p-4 pr-8 text-right">"Actions"</th>
                    </tr>
                </thead>
                <tbody class="divide-y divide-border-subtle">
                    {move || {
                        if !connected.get() {
                            view! {
                                <tr><td colspan="4" class="p-8 text-center text-text-tertiary">"Connect to Gateway to view raw memories"</td></tr>
                            }.into_any()
                        } else if searching.get() {
                            view! {
                                <tr><td colspan="4" class="p-8 text-center text-text-tertiary">"Searching..."</td></tr>
                            }.into_any()
                        } else if !loaded.get() {
                            view! {
                                <tr><td colspan="4" class="p-8 text-center text-text-tertiary">"Loading..."</td></tr>
                            }.into_any()
                        } else if memories.get().is_empty() {
                            view! {
                                <tr><td colspan="4" class="p-8 text-center text-text-tertiary">"No raw memories stored yet. Chat with Aleph to start building memory."</td></tr>
                            }.into_any()
                        } else {
                            let on_delete = on_delete.clone();
                            view! {
                                <For
                                    each=move || memories.get()
                                    key=|m| m.id.clone()
                                    children=move |entry| {
                                        let created_at = entry.created_at.clone().unwrap_or_else(|| "Unknown".to_string());
                                        let agent_id = entry.agent_id.clone();
                                        let entry_id = entry.id.clone();
                                        let on_delete = on_delete.clone();
                                        view! {
                                            <MemoryRow
                                                content=entry.content
                                                agent_id=agent_id
                                                date=created_at
                                                on_delete=move |_| on_delete(entry_id.clone())
                                            />
                                        }
                                    }
                                />
                            }.into_any()
                        }
                    }}
                </tbody>
            </table>
        </Card>
    }
}

#[component]
fn MemoryRow(
    content: String,
    agent_id: String,
    date: String,
    on_delete: impl Fn(()) + 'static,
) -> impl IntoView {
    view! {
        <tr class="group hover:bg-surface-sunken transition-colors">
            <td class="p-4 pl-8">
                <div class="text-sm font-medium text-text-primary line-clamp-1 group-hover:line-clamp-none transition-all">{content}</div>
            </td>
            <td class="p-4">
                <Badge variant=BadgeVariant::Indigo>{agent_id}</Badge>
            </td>
            <td class="p-4">
                <div class="flex items-center gap-2 text-xs text-text-tertiary font-mono">
                    <svg width="12" height="12" attr:class="w-3 h-3" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                        <rect x="3" y="4" width="18" height="18" rx="2" ry="2" />
                        <line x1="16" y1="2" x2="16" y2="6" />
                        <line x1="8" y1="2" x2="8" y2="6" />
                        <line x1="3" y1="10" x2="21" y2="10" />
                    </svg>
                    {date}
                </div>
            </td>
            <td class="p-4 pr-8 text-right">
                <div class="flex items-center justify-end gap-2 opacity-0 group-hover:opacity-100 transition-opacity">
                    <Button variant=ButtonVariant::Destructive size=ButtonSize::Sm class="p-1.5 h-auto".to_string() on:click=move |_| on_delete(())>
                        <svg width="16" height="16" attr:class="w-4 h-4" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                            <polyline points="3 6 5 6 21 6" />
                            <path d="M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6m3 0V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2" />
                        </svg>
                    </Button>
                </div>
            </td>
        </tr>
    }
}

/// Format unix timestamp (seconds) to display string
fn format_ts(ts: i64) -> String {
    if ts <= 0 {
        return "—".to_string();
    }
    let date = js_sys::Date::new(&wasm_bindgen::JsValue::from_f64((ts * 1000) as f64));
    let year = date.get_full_year();
    let month = date.get_month() + 1;
    let day = date.get_date();
    let hour = date.get_hours();
    let min = date.get_minutes();
    format!("{:04}-{:02}-{:02} {:02}:{:02}", year, month, day, hour, min)
}
