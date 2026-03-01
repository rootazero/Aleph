use leptos::prelude::*;
use crate::components::ui::*;
use crate::context::DashboardState;
use crate::api::{MemoryApi, MemoryFact, MemoryStats};

#[component]
pub fn Memory() -> impl IntoView {
    // Get dashboard state from context
    let state = expect_context::<DashboardState>();

    // Create a signal for disabled state
    let is_disabled = Signal::derive(move || !state.is_connected.get());

    // Memory stats
    let stats = RwSignal::new(None::<MemoryStats>);

    // Search results
    let search_query = RwSignal::new(String::new());
    let search_results = RwSignal::new(Vec::<MemoryFact>::new());
    let is_searching = RwSignal::new(false);

    // Fetch stats when connected
    Effect::new(move || {
        if state.is_connected.get() {
            let state = state.clone();
            leptos::task::spawn_local(async move {
                match MemoryApi::stats(&state).await {
                    Ok(s) => {
                        stats.set(Some(s));
                    }
                    Err(e) => {
                        web_sys::console::error_1(&format!("Failed to fetch memory stats: {}", e).into());
                    }
                }
            });
        } else {
            stats.set(None);
        }
    });

    // Search handler
    let handle_search = move |_| {
        let query = search_query.get();
        if query.is_empty() {
            return;
        }

        let state = state.clone();
        leptos::task::spawn_local(async move {
            is_searching.set(true);

            match MemoryApi::search(&state, query, Some(20)).await {
                Ok(results) => {
                    search_results.set(results);
                }
                Err(e) => {
                    web_sys::console::error_1(&format!("Search failed: {}", e).into());
                }
            }

            is_searching.set(false);
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
                    <p class="text-text-secondary">"Browse and manage Agent's long-term memory and facts."</p>
                </div>

                <div class="flex items-center gap-3">
                    <div class="relative group">
                        <svg width="16" height="16" attr:class="absolute left-3 top-1/2 -translate-y-1/2 w-4 h-4 text-text-tertiary group-focus-within:text-primary transition-colors" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                            <circle cx="11" cy="11" r="8" />
                            <line x1="21" y1="21" x2="16.65" y2="16.65" />
                        </svg>
                        <input
                            type="text"
                            placeholder="Search facts..."
                            class="pl-10 pr-4 py-2 bg-surface-raised border border-border rounded-xl focus:outline-none focus:border-primary/50 focus:ring-4 focus:ring-primary/10 w-64 transition-all text-sm text-text-primary placeholder:text-text-tertiary"
                            disabled=is_disabled
                            on:input=move |ev| {
                                search_query.set(event_target_value(&ev));
                            }
                            on:keydown=move |ev| {
                                if ev.key() == "Enter" {
                                    handle_search(());
                                }
                            }
                        />
                    </div>
                    <Button variant=ButtonVariant::Secondary size=ButtonSize::Sm class="p-2 h-auto rounded-xl".to_string() disabled=is_disabled>
                        <svg width="20" height="20" attr:class="w-5 h-5 text-text-secondary" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                            <polygon points="22 3 2 3 10 12.46 10 19 14 21 14 12.46 22 3" />
                        </svg>
                    </Button>
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
                    <span class="text-[10px] font-bold text-primary uppercase tracking-widest mb-1.5">"Total Facts"</span>
                    <span class="text-3xl font-bold font-mono">
                        {move || {
                            stats.get()
                                .map(|s| s.total_facts.to_string())
                                .unwrap_or_else(|| "—".to_string())
                        }}
                    </span>
                 </Card>
                 <Card class="bg-success-subtle border-success/10 p-6 flex flex-col items-start".to_string()>
                    <span class="text-[10px] font-bold text-success uppercase tracking-widest mb-1.5">"Raw Memories"</span>
                    <span class="text-3xl font-bold font-mono">
                        {move || {
                            stats.get()
                                .map(|s| s.total_memories.to_string())
                                .unwrap_or_else(|| "—".to_string())
                        }}
                    </span>
                 </Card>
                 <Card class="bg-primary-subtle border-primary/10 p-6 flex flex-col items-start".to_string()>
                    <span class="text-[10px] font-bold text-primary uppercase tracking-widest mb-1.5">"Graph Nodes"</span>
                    <span class="text-3xl font-bold font-mono">
                        {move || {
                            stats.get()
                                .map(|s| s.total_graph_nodes.to_string())
                                .unwrap_or_else(|| "—".to_string())
                        }}
                    </span>
                 </Card>
            </div>

            // Facts List
            <Card class="overflow-hidden".to_string()>
                <table class="w-full text-left border-collapse">
                    <thead>
                        <tr class="bg-surface-sunken text-[10px] font-bold text-text-tertiary uppercase tracking-widest">
                            <th class="p-4 pl-8">"Fact Content"</th>
                            <th class="p-4">"Source"</th>
                            <th class="p-4">"Date"</th>
                            <th class="p-4 pr-8 text-right">"Actions"</th>
                        </tr>
                    </thead>
                    <tbody class="divide-y divide-border-subtle">
                        {move || {
                            if !state.is_connected.get() {
                                view! {
                                    <tr>
                                        <td colspan="4" class="p-8 text-center text-text-tertiary">
                                            "Connect to Gateway to view memory facts"
                                        </td>
                                    </tr>
                                }.into_any()
                            } else if is_searching.get() {
                                view! {
                                    <tr>
                                        <td colspan="4" class="p-8 text-center text-text-tertiary">
                                            "Searching..."
                                        </td>
                                    </tr>
                                }.into_any()
                            } else if search_results.get().is_empty() {
                                view! {
                                    <tr>
                                        <td colspan="4" class="p-8 text-center text-text-tertiary">
                                            "No facts found. Try searching for something."
                                        </td>
                                    </tr>
                                }.into_any()
                            } else {
                                view! {
                                    <For
                                        each=move || search_results.get()
                                        key=|fact| fact.id.clone()
                                        children=move |fact| {
                                            let created_at = fact.created_at.clone().unwrap_or_else(|| "Unknown".to_string());
                                            let source = fact.source.clone().unwrap_or_else(|| "Memory".to_string());
                                            view! {
                                                <MemoryRow
                                                    content=fact.content
                                                    source=source
                                                    date=created_at
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
        </div>
    }
}

#[component]
fn MemoryRow(
    content: String,
    source: String,
    date: String,
) -> impl IntoView {
    view! {
        <tr class="group hover:bg-surface-sunken transition-colors">
            <td class="p-4 pl-8">
                <div class="text-sm font-medium text-text-primary line-clamp-1 group-hover:line-clamp-none transition-all">{content}</div>
            </td>
            <td class="p-4">
                <Badge variant=BadgeVariant::Slate>
                    {source}
                </Badge>
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
                    <Button variant=ButtonVariant::Ghost size=ButtonSize::Sm class="p-1.5 h-auto".to_string()>
                        <svg width="16" height="16" attr:class="w-4 h-4" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                            <path d="M18 13v6a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h6" />
                            <polyline points="15 3 21 3 21 9" />
                            <line x1="10" y1="14" x2="21" y2="3" />
                        </svg>
                    </Button>
                    <Button variant=ButtonVariant::Destructive size=ButtonSize::Sm class="p-1.5 h-auto".to_string()>
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