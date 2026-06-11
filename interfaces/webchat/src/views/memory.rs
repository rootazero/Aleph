use crate::api::{CompressedFact, MemoryApi, MemoryStats, RawMemory};
use crate::components::ui::*;
use crate::context::DashboardState;
use crate::i18n::*;
use leptos::prelude::*;

/// Fixed number of entries shown per page in both memory tabs.
const PAGE_SIZE: u32 = 50;

#[component]
#[must_use]
pub fn Memory() -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let i18n = use_i18n();
    let is_disabled = Signal::derive(move || !state.is_connected.get());

    // Memory stats
    let stats = RwSignal::new(None::<MemoryStats>);

    // Active tab: "facts" or "raw"
    let active_tab = RwSignal::new("facts".to_string());

    // Facts data + page (0-indexed)
    let facts_list = RwSignal::new(Vec::<CompressedFact>::new());
    let facts_loaded = RwSignal::new(false);
    let facts_page = RwSignal::new(0u32);

    // Raw memories data + page (0-indexed)
    let search_query = RwSignal::new(String::new());
    // `applied_query` is the query actually in effect (set on Enter); browse
    // mode is `applied_query == ""`. Kept separate from the input buffer so
    // pagination reloads use the committed query, not every keystroke.
    let applied_query = RwSignal::new(String::new());
    let raw_memories = RwSignal::new(Vec::<RawMemory>::new());
    let is_searching = RwSignal::new(false);
    let raw_loaded = RwSignal::new(false);
    let raw_page = RwSignal::new(0u32);

    // Stats fetch — only depends on connection (refreshed after deletes).
    Effect::new(move || {
        if state.is_connected.get() {
            let state = state;
            leptos::task::spawn_local(async move {
                if let Ok(s) = MemoryApi::stats(&state).await {
                    stats.set(Some(s));
                }
            });
        } else {
            stats.set(None);
        }
    });

    // Facts page loader — reruns when the page changes or connection flips.
    Effect::new(move || {
        if state.is_connected.get() {
            let page = facts_page.get();
            let state = state;
            leptos::task::spawn_local(async move {
                facts_loaded.set(false);
                let offset = (page * PAGE_SIZE) as usize;
                if let Ok(facts) =
                    MemoryApi::list_facts(&state, Some(PAGE_SIZE as usize), offset).await
                {
                    facts_list.set(facts);
                }
                facts_loaded.set(true);
            });
        } else {
            facts_list.set(Vec::new());
            facts_loaded.set(false);
        }
    });

    // Raw memories page loader — reruns on page change, applied query change,
    // or connection flip.
    Effect::new(move || {
        if state.is_connected.get() {
            let page = raw_page.get();
            let query = applied_query.get();
            let state = state;
            leptos::task::spawn_local(async move {
                is_searching.set(true);
                raw_loaded.set(false);
                if let Ok(results) =
                    MemoryApi::search(&state, query, Some(PAGE_SIZE), page * PAGE_SIZE).await
                {
                    raw_memories.set(results);
                }
                is_searching.set(false);
                raw_loaded.set(true);
            });
        } else {
            raw_memories.set(Vec::new());
            raw_loaded.set(false);
        }
    });

    // Search handler — commits the query and jumps back to the first page.
    // The raw loader Effect reacts to both signals and performs the fetch.
    let do_search = move || {
        applied_query.set(search_query.get());
        raw_page.set(0);
    };

    // Delete handler for raw memories — refreshes stats + the current page.
    let on_delete = move |memory_id: String| {
        let state = state;
        leptos::task::spawn_local(async move {
            if MemoryApi::delete(&state, memory_id).await.is_ok() {
                if let Ok(s) = MemoryApi::stats(&state).await {
                    stats.set(Some(s));
                }
                let page = raw_page.get_untracked();
                let query = applied_query.get_untracked();
                if let Ok(results) =
                    MemoryApi::search(&state, query, Some(PAGE_SIZE), page * PAGE_SIZE).await
                {
                    // Deleting the last row on a non-first page: step back so the
                    // user doesn't land on an empty page (the loader Effect then
                    // refetches the previous page).
                    if results.is_empty() && page > 0 {
                        raw_page.set(page - 1);
                    } else {
                        raw_memories.set(results);
                    }
                }
            }
        });
    };

    view! {
        <div class="px-8 pb-8 aleph-content-top max-w-7xl mx-auto space-y-8">
            <header class="flex items-center justify-between">
                <div>
                    <h2 class="text-3xl font-bold tracking-tight mb-2 flex items-center gap-3 text-text-primary">
                        <svg width="32" height="32" attr:class="w-8 h-8 text-primary" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                            <ellipse cx="12" cy="5" rx="9" ry="3" />
                            <path d="M21 12c0 1.66-4 3-9 3s-9-1.34-9-3" />
                            <path d="M3 5v14c0 1.66 4 3 9 3s9-1.34 9-3V5" />
                        </svg>
                        {t!(i18n, memory.title)}
                    </h2>
                    <p class="text-text-secondary">{t!(i18n, memory.description)}</p>
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
                                <h3 class="text-warning font-semibold mb-1">{t!(i18n, dashboard.gateway_required)}</h3>
                                <p class="text-sm text-text-secondary">{t!(i18n, memory.gateway_required_desc)}</p>
                            </div>
                        </div>
                    }.into_any()
                } else {
                    view! { <div></div> }.into_any()
                }
            }}

            // Memory Stats
            <div class="grid grid-cols-2 md:grid-cols-4 gap-6">
                 <Card class="bg-primary-subtle border-primary/10 p-6 flex flex-col items-start".to_string()>
                    <span class="text-[10px] font-bold text-primary uppercase tracking-widest mb-1.5">{t!(i18n, memory.compressed_facts)}</span>
                    <span class="text-3xl font-bold font-mono">
                        {move || {
                            stats.get()
                                .map(|s| s.total_facts.to_string())
                                .unwrap_or_else(|| "\u{2014}".to_string())
                        }}
                    </span>
                 </Card>
                 <Card class="bg-success-subtle border-success/10 p-6 flex flex-col items-start".to_string()>
                    <span class="text-[10px] font-bold text-success uppercase tracking-widest mb-1.5">{t!(i18n, memory.raw_memories)}</span>
                    <span class="text-3xl font-bold font-mono">
                        {move || {
                            stats.get()
                                .map(|s| s.total_memories.to_string())
                                .unwrap_or_else(|| "\u{2014}".to_string())
                        }}
                    </span>
                 </Card>
                 <Card class="bg-primary-subtle border-primary/10 p-6 flex flex-col items-start".to_string()>
                    <span class="text-[10px] font-bold text-primary uppercase tracking-widest mb-1.5">{t!(i18n, memory.graph_nodes)}</span>
                    <span class="text-3xl font-bold font-mono">
                        {move || {
                            stats.get()
                                .map(|s| s.total_graph_nodes.to_string())
                                .unwrap_or_else(|| "\u{2014}".to_string())
                        }}
                    </span>
                 </Card>
                 <Card class="bg-success-subtle border-success/10 p-6 flex flex-col items-start".to_string()>
                    <span class="text-[10px] font-bold text-success uppercase tracking-widest mb-1.5">{t!(i18n, memory.graph_edges)}</span>
                    <span class="text-3xl font-bold font-mono">
                        {move || {
                            stats.get()
                                .map(|s| s.total_graph_edges.to_string())
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
                    {t!(i18n, memory.compressed_facts)}
                </button>
                <button
                    class=move || if active_tab.get() == "raw" {
                        "px-4 py-2 text-sm font-medium text-primary border-b-2 border-primary -mb-px"
                    } else {
                        "px-4 py-2 text-sm font-medium text-text-tertiary hover:text-text-secondary -mb-px"
                    }
                    on:click=move |_| active_tab.set("raw".to_string())
                >
                    {t!(i18n, memory.raw_memories)}
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
                                        placeholder=t_string!(i18n, memory.search_placeholder)
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
                    let facts_total = Signal::derive(move || stats.get().map(|s| s.total_facts));
                    let facts_len = Signal::derive(move || facts_list.get().len());
                    view! {
                        <FactsTable facts=facts_list loaded=facts_loaded connected=state.is_connected />
                        <Pager page=facts_page total=facts_total current_len=facts_len />
                    }.into_any()
                } else {
                    let on_delete = on_delete;
                    // In browse mode the total is the stats count; while a search
                    // query is applied the match count is unknown, so the pager
                    // falls back to "has a full page" heuristics.
                    let raw_total = Signal::derive(move || {
                        if applied_query.get().is_empty() {
                            stats.get().map(|s| s.total_memories)
                        } else {
                            None
                        }
                    });
                    let raw_len = Signal::derive(move || raw_memories.get().len());
                    view! {
                        <RawMemoriesTable memories=raw_memories loaded=raw_loaded searching=is_searching connected=state.is_connected on_delete=on_delete />
                        <Pager page=raw_page total=raw_total current_len=raw_len />
                    }.into_any()
                }
            }}
        </div>
    }
}

// ─── Pagination ─────────────────────────────────────────────────────────────

/// Prev / page-indicator / Next pager shared by both memory tabs.
///
/// `total` is the full item count when known (browse mode); when `None`
/// (an active search whose match count is unknown) the pager shows only the
/// current page number and enables "Next" while the current page is full.
#[component]
fn Pager(
    /// 0-indexed current page.
    page: RwSignal<u32>,
    /// Total item count, or `None` when unknown.
    total: Signal<Option<u64>>,
    /// Number of items rendered on the current page.
    current_len: Signal<usize>,
) -> impl IntoView {
    let i18n = use_i18n();

    let total_pages = Signal::derive(move || {
        total
            .get()
            .map(|t| t.div_ceil(PAGE_SIZE as u64).max(1) as u32)
    });
    let has_prev = Signal::derive(move || page.get() > 0);
    let has_next = Signal::derive(move || match total_pages.get() {
        Some(tp) => page.get() + 1 < tp,
        None => current_len.get() as u32 >= PAGE_SIZE,
    });

    move || {
        // Hide the pager entirely when there's only a single page.
        if !has_prev.get() && !has_next.get() {
            return view! { <div></div> }.into_any();
        }

        let indicator = match total_pages.get() {
            Some(tp) => format!("{} / {}", page.get() + 1, tp),
            None => format!("{}", page.get() + 1),
        };

        view! {
            <div class="flex items-center justify-end gap-2 pt-1">
                <button
                    class="px-3 py-1.5 text-sm rounded-lg border border-border bg-surface-raised text-text-secondary hover:text-text-primary hover:bg-surface-sunken disabled:opacity-40 disabled:cursor-not-allowed transition-colors"
                    prop:disabled=move || !has_prev.get()
                    on:click=move |_| {
                        let p = page.get();
                        if p > 0 {
                            page.set(p - 1);
                        }
                    }
                >
                    {t!(i18n, memory.prev_page)}
                </button>
                <span class="px-2 text-sm font-mono text-text-secondary tabular-nums">{indicator}</span>
                <button
                    class="px-3 py-1.5 text-sm rounded-lg border border-border bg-surface-raised text-text-secondary hover:text-text-primary hover:bg-surface-sunken disabled:opacity-40 disabled:cursor-not-allowed transition-colors"
                    prop:disabled=move || !has_next.get()
                    on:click=move |_| {
                        if has_next.get() {
                            page.set(page.get() + 1);
                        }
                    }
                >
                    {t!(i18n, memory.next_page)}
                </button>
            </div>
        }
        .into_any()
    }
}

// ─── Facts Table ────────────────────────────────────────────────────────────

#[component]
fn FactsTable(
    facts: RwSignal<Vec<CompressedFact>>,
    loaded: RwSignal<bool>,
    connected: RwSignal<bool>,
) -> impl IntoView {
    let i18n = use_i18n();
    view! {
        <Card class="overflow-hidden".to_string()>
            <table class="w-full text-left border-collapse">
                <thead>
                    <tr class="bg-surface-sunken text-[10px] font-bold text-text-tertiary uppercase tracking-widest">
                        <th class="p-4 pl-8">{t!(i18n, memory.col_title)}</th>
                        <th class="p-4">{t!(i18n, memory.col_agent)}</th>
                        <th class="p-4">{t!(i18n, memory.col_type)}</th>
                        <th class="p-4 pr-8">{t!(i18n, memory.col_date)}</th>
                    </tr>
                </thead>
                <tbody class="divide-y divide-border-subtle">
                    {move || {
                        if !connected.get() {
                            view! {
                                <tr><td colspan="4" class="p-8 text-center text-text-tertiary">{t!(i18n, memory.connect_to_view_facts)}</td></tr>
                            }.into_any()
                        } else if !loaded.get() {
                            view! {
                                <tr><td colspan="4" class="p-8 text-center text-text-tertiary">{t!(i18n, common.loading)}</td></tr>
                            }.into_any()
                        } else if facts.get().is_empty() {
                            view! {
                                <tr><td colspan="4" class="p-8 text-center text-text-tertiary">{t!(i18n, memory.no_facts)}</td></tr>
                            }.into_any()
                        } else {
                            view! {
                                <For
                                    each=move || facts.get()
                                    key=|fact| fact.id.clone()
                                    children=move |fact| {
                                        // Backend note categories are lowercase
                                        // (src/memory/context/enums.rs NoteType::as_str).
                                        let badge_variant = match fact.fact_type.as_str() {
                                            "preference" | "personal" => BadgeVariant::Indigo,
                                            "learning" | "lesson" | "skill" => BadgeVariant::Emerald,
                                            "plan" | "project" => BadgeVariant::Amber,
                                            "feedback" => BadgeVariant::Red,
                                            _ => BadgeVariant::Slate,
                                        };
                                        let agent_id = fact.agent_id.clone();
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
    let i18n = use_i18n();
    view! {
        <Card class="overflow-hidden".to_string()>
            <table class="w-full text-left border-collapse">
                <thead>
                    <tr class="bg-surface-sunken text-[10px] font-bold text-text-tertiary uppercase tracking-widest">
                        <th class="p-4 pl-8">{t!(i18n, memory.col_content)}</th>
                        <th class="p-4">{t!(i18n, memory.col_agent)}</th>
                        <th class="p-4">{t!(i18n, memory.col_date)}</th>
                        <th class="p-4 pr-8 text-right">{t!(i18n, memory.col_actions)}</th>
                    </tr>
                </thead>
                <tbody class="divide-y divide-border-subtle">
                    {move || {
                        if !connected.get() {
                            view! {
                                <tr><td colspan="4" class="p-8 text-center text-text-tertiary">{t!(i18n, memory.connect_to_view_raw)}</td></tr>
                            }.into_any()
                        } else if searching.get() {
                            view! {
                                <tr><td colspan="4" class="p-8 text-center text-text-tertiary">{t!(i18n, memory.searching)}</td></tr>
                            }.into_any()
                        } else if !loaded.get() {
                            view! {
                                <tr><td colspan="4" class="p-8 text-center text-text-tertiary">{t!(i18n, common.loading)}</td></tr>
                            }.into_any()
                        } else if memories.get().is_empty() {
                            view! {
                                <tr><td colspan="4" class="p-8 text-center text-text-tertiary">{t!(i18n, memory.no_raw)}</td></tr>
                            }.into_any()
                        } else {
                            let on_delete = on_delete.clone();
                            view! {
                                <For
                                    each=move || memories.get()
                                    key=|m| m.id.clone()
                                    children=move |entry| {
                                        let created_at = entry.created_at.clone().unwrap_or_else(|| t_string!(i18n, memory.unknown).to_string());
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
    on_delete: impl Fn(()) + Clone + Send + 'static,
) -> impl IntoView {
    // Two-step delete confirmation: the trash icon arms an inline "确认删除？"
    // button (shared ConfirmButton); clicking elsewhere reverts it.
    let confirm = RwSignal::new(false);
    let on_confirm_delete = move || on_delete(());

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
                // Confirm mode keeps the button visible (not hover-gated) so the
                // armed state is always readable; idle mode reveals on row hover.
                <div class=move || if confirm.get() {
                    "flex items-center justify-end gap-2 transition-opacity"
                } else {
                    "flex items-center justify-end gap-2 opacity-0 group-hover:opacity-100 transition-opacity"
                }>
                    {move || if confirm.get() {
                        view! {
                            <ConfirmButton confirming=confirm on_confirm=on_confirm_delete.clone() size_class="px-2.5 py-1 text-xs" />
                        }.into_any()
                    } else {
                        view! {
                            <Button variant=ButtonVariant::Destructive size=ButtonSize::Sm class="p-1.5 h-auto".to_string() on:click=move |_| confirm.set(true)>
                                <svg width="16" height="16" attr:class="w-4 h-4" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                                    <polyline points="3 6 5 6 21 6" />
                                    <path d="M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6m3 0V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2" />
                                </svg>
                            </Button>
                        }.into_any()
                    }}
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
    format!("{year:04}-{month:02}-{day:02} {hour:02}:{min:02}")
}
