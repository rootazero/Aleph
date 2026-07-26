use crate::api::agents::AgentsApi;
use crate::api::{CompressedFact, MemoryApi, MemoryStats, RawMemory};
use crate::components::ui::{
    Badge, BadgeVariant, Button, ButtonSize, ButtonVariant, Card, ConfirmButton,
};
use crate::context::DashboardState;
use crate::i18n::{t, t_string, use_i18n};
use crate::state::memory::{MemoryState, MemoryView};
use leptos::prelude::*;
use std::collections::HashSet;

mod batch_bar;
mod cards;
pub mod data;
mod drawer;
mod facets;
mod loader;
mod pager;
mod toast;

use data::{
    bucket_counts, facet_slice, format_ts, locate_note, page_slice, MemoryFacet, NOTE_WINDOW,
    PAGE_SIZE,
};
use drawer::{DetailDrawer, DrawerTarget};
use facets::FacetBar;
use pager::Pager;

/// Total item count for a facet, read from the pre-computed bucket counts
/// (`[AllNotes, Facts, Feedback, Lessons]`). `None` for facets this window
/// cannot count: `Raw` is server-paginated via `MemoryApi::stats` instead
/// (its own `Pager` uses `raw_total`, not this one), and `SearchHits` rows
/// arrive on their own signal — `graph.search` returns a capped `results`
/// list with no total. Reporting `Some(0)` for either would make the pager
/// compute `page_count(0, ps) == 1` and silently hide prev/next behind a
/// false one-page claim even when many rows are loaded elsewhere; `None` lets
/// `Pager` degrade to its honest "unknown total" heuristic instead.
fn facet_total(counts: [usize; 4], facet: MemoryFacet) -> Option<u64> {
    let n = match facet {
        MemoryFacet::AllNotes => counts[0],
        MemoryFacet::Facts => counts[1],
        MemoryFacet::Feedback => counts[2],
        MemoryFacet::Lessons => counts[3],
        MemoryFacet::Raw | MemoryFacet::SearchHits => return None,
    };
    Some(n as u64)
}

/// Top-level memory management console at `/dashboard/memory`.
///
/// Faceted over the note layer (All / Facts / Feedback / Lessons) plus a Raw
/// conversation-log facet. Notes are pulled in one `list_facts` window per
/// agent and faceted/paginated client-side; Raw is server-paginated via
/// `memory.search`. Row click opens the shared `DetailDrawer`. Agent selection
/// is shared with the Canvas via `MemoryState`. Pure I/O — all persistence is
/// JSON-RPC (R4).
#[component]
#[must_use]
pub fn Memory() -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let mem = expect_context::<MemoryState>();
    let i18n = use_i18n();

    let stats = RwSignal::new(None::<MemoryStats>);
    let facet = RwSignal::new(MemoryFacet::AllNotes);
    // Shared by both tables' pagers; the selector resets its own `page` signal
    // to 0 on change (see `Pager`), so a stale offset never survives a resize.
    let page_size = RwSignal::new(PAGE_SIZE);

    // Note window (faceted + paginated client-side), reloaded per agent.
    let notes_window = RwSignal::new(Vec::<CompressedFact>::new());
    let notes_loaded = RwSignal::new(false);
    let notes_page = RwSignal::new(0u32);

    // Raw memories (server-paginated).
    let applied_query = RwSignal::new(String::new());
    let raw_memories = RwSignal::new(Vec::<RawMemory>::new());
    let is_searching = RwSignal::new(false);
    let raw_loaded = RwSignal::new(false);
    let raw_page = RwSignal::new(0u32);

    // Raw batch selection + shared detail drawer.
    let selected_ids = RwSignal::new(HashSet::<String>::new());
    let drawer_target = RwSignal::new(None::<DrawerTarget>);

    // Agent bootstrap — only if the Canvas hasn't already populated the shared
    // list. Idempotent: re-runs until `agents` is non-empty.
    Effect::new(move || {
        if !state.is_connected.get() || !mem.agents.get().is_empty() {
            return;
        }
        let state = state;
        leptos::task::spawn_local(async move {
            if let Ok(resp) = AgentsApi::list(&state).await {
                mem.agents.set(resp.agents);
                if mem.agent_id.get_untracked() != resp.default_id {
                    mem.agent_id.set(resp.default_id);
                }
            }
        });
    });

    // Stats — refreshed after deletes via `refresh_raw`.
    Effect::new(move || {
        if state.is_connected.get() {
            let state = state;
            let agent = mem.agent_id.get();
            leptos::task::spawn_local(async move {
                if let Ok(s) = MemoryApi::stats(&state, &agent).await {
                    stats.set(Some(s));
                }
            });
        } else {
            stats.set(None);
        }
    });

    // Notes window loader — reruns on connection or agent change.
    Effect::new(move || {
        if state.is_connected.get() {
            let state = state;
            let agent = mem.agent_id.get();
            leptos::task::spawn_local(async move {
                notes_loaded.set(false);
                if let Ok((facts, _total)) =
                    MemoryApi::list_facts(&state, &agent, NOTE_WINDOW, 0).await
                {
                    notes_window.set(facts);
                }
                notes_loaded.set(true);
                notes_page.set(0);
            });
        } else {
            notes_window.set(Vec::new());
            notes_loaded.set(false);
        }
    });

    // Raw loader — reruns on page / applied query / agent / connection change.
    Effect::new(move || {
        if state.is_connected.get() {
            let page = raw_page.get();
            let ps = page_size.get();
            let query = applied_query.get();
            let agent = mem.agent_id.get();
            let state = state;
            leptos::task::spawn_local(async move {
                is_searching.set(true);
                raw_loaded.set(false);
                if let Ok((results, _total)) =
                    MemoryApi::browse_raw(&state, &agent, query, ps, page * ps).await
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

    // Shared search box (in the hub toolbar) writes `mem.search_query` live and
    // bumps `mem.search_nonce` on Enter. Commit that into the Raw search here;
    // a non-empty query also switches to the Raw facet so results are visible.
    Effect::new(move || {
        mem.search_nonce.get(); // subscribe to submit pulses
        let q = mem.search_query.get_untracked();
        applied_query.set(q.clone());
        raw_page.set(0);
        if !q.is_empty() {
            facet.set(MemoryFacet::Raw);
        }
    });

    // Reverse link (graph → list): the node detail panel sets
    // `mem.highlight_note_id`; jump to the note's facet+page, highlight the row,
    // and open its drawer. Outside the loaded window → inline notice.
    let highlight_id = RwSignal::new(None::<String>);
    let highlight_missing = RwSignal::new(false);
    Effect::new(move || {
        let Some(path) = mem.highlight_note_id.get() else {
            return;
        };
        if !notes_loaded.get() {
            return; // wait for the window; re-runs when it loads
        }
        let window = notes_window.get();
        match locate_note(&window, &path, page_size.get()) {
            Some((f, pg)) => {
                facet.set(f);
                notes_page.set(pg);
                highlight_id.set(Some(path.clone()));
                highlight_missing.set(false);
                if let Some(found) = window.into_iter().find(|x| x.path == path) {
                    drawer_target.set(Some(DrawerTarget::Note(found)));
                }
            }
            None => {
                highlight_missing.set(true);
                highlight_id.set(None);
            }
        }
        mem.highlight_note_id.set(None); // consume so re-selecting re-triggers
    });

    // Reusable raw refresh after a delete — keeps stats + current page coherent
    // and steps back off a now-empty trailing page.
    let refresh_raw = move || {
        let state = state;
        leptos::task::spawn_local(async move {
            let agent = mem.agent_id.get_untracked();
            if let Ok(s) = MemoryApi::stats(&state, &agent).await {
                stats.set(Some(s));
            }
            let page = raw_page.get_untracked();
            let ps = page_size.get_untracked();
            let query = applied_query.get_untracked();
            if let Ok((results, _total)) =
                MemoryApi::browse_raw(&state, &agent, query, ps, page * ps).await
            {
                if results.is_empty() && page > 0 {
                    raw_page.set(page - 1);
                } else {
                    raw_memories.set(results);
                }
            }
        });
    };

    let on_delete_one = move |memory_id: String| {
        let state = state;
        leptos::task::spawn_local(async move {
            if MemoryApi::delete(&state, memory_id).await.is_ok() {
                refresh_raw();
            }
        });
    };

    let on_batch_delete = move || {
        let ids: Vec<String> = selected_ids.get_untracked().into_iter().collect();
        if ids.is_empty() {
            return;
        }
        let state = state;
        leptos::task::spawn_local(async move {
            for id in ids {
                let _ = MemoryApi::delete(&state, id).await;
            }
            selected_ids.set(HashSet::new());
            refresh_raw();
        });
    };

    // Derived signals.
    let counts = Signal::derive(move || bucket_counts(&notes_window.get()));
    let raw_total = Signal::derive(move || stats.get().map(|s| s.total_memories));
    let is_search_active = Signal::derive(move || !applied_query.get().is_empty());
    // Memo so switching among note facets updates the table in place rather
    // than remounting it; only a notes<->raw flip changes the active table.
    let is_notes = Memo::new(move |_| facet.get().is_notes());
    let notes_total = Signal::derive(move || facet_total(counts.get(), facet.get()));
    let connected = state.is_connected;

    view! {
        <div class="px-8 pb-8 aleph-content-top max-w-7xl mx-auto space-y-6">
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

            {move || {
                if !connected.get() {
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

            // Memory stats
            <div class="grid grid-cols-2 md:grid-cols-4 gap-6">
                <Card class="bg-primary-subtle border-primary/10 p-6 flex flex-col items-start".to_string()>
                    <span class="text-[10px] font-bold text-primary uppercase tracking-widest mb-1.5">{t!(i18n, memory.compressed_facts)}</span>
                    <span class="text-3xl font-bold font-mono">
                        {move || stats.get().map(|s| s.total_facts.to_string()).unwrap_or_else(|| "\u{2014}".to_string())}
                    </span>
                </Card>
                <Card class="bg-success-subtle border-success/10 p-6 flex flex-col items-start".to_string()>
                    <span class="text-[10px] font-bold text-success uppercase tracking-widest mb-1.5">{t!(i18n, memory.raw_memories)}</span>
                    <span class="text-3xl font-bold font-mono">
                        {move || stats.get().map(|s| s.total_memories.to_string()).unwrap_or_else(|| "\u{2014}".to_string())}
                    </span>
                </Card>
                <Card class="bg-primary-subtle border-primary/10 p-6 flex flex-col items-start".to_string()>
                    <span class="text-[10px] font-bold text-primary uppercase tracking-widest mb-1.5">{t!(i18n, memory.graph_nodes)}</span>
                    <span class="text-3xl font-bold font-mono">
                        {move || stats.get().and_then(|s| s.total_graph_nodes).map(|n| n.to_string()).unwrap_or_else(|| "\u{2014}".to_string())}
                    </span>
                </Card>
                <Card class="bg-success-subtle border-success/10 p-6 flex flex-col items-start".to_string()>
                    <span class="text-[10px] font-bold text-success uppercase tracking-widest mb-1.5">{t!(i18n, memory.graph_edges)}</span>
                    <span class="text-3xl font-bold font-mono">
                        {move || stats.get().and_then(|s| s.total_graph_edges).map(|n| n.to_string()).unwrap_or_else(|| "\u{2014}".to_string())}
                    </span>
                </Card>
            </div>

            // Facet bar (search lives in the hub toolbar).
            <div class="flex items-center justify-between gap-3 flex-wrap border-b border-border pb-2">
                <FacetBar
                    active=facet
                    counts=counts
                    raw_count=raw_total
                    // TODO(Task 18): this is only half-wired. Two things need
                    // a real producer, not just `hits_count`:
                    // 1. `hits_count` itself — `None` keeps the chip hidden
                    //    until `load_search_hits` is threaded through here.
                    // 2. The `Pager` below (in the `is_notes` branch) is fed
                    //    `current_len=Signal::derive(|| 0usize)` — hardcoded.
                    //    Once `hits_count` goes `Some` the chip becomes
                    //    selectable and `notes_total` is `None` for
                    //    `SearchHits` (see `facet_total`), so `Pager` falls
                    //    back to its unknown-total heuristic
                    //    `current_len >= page_size`. Against a hardcoded `0`
                    //    that is always false, so prev/indicator/next would
                    //    silently vanish on any multi-page hit set. Replace
                    //    `current_len` with the real loaded-hits length in
                    //    the same change that wires `hits_count`.
                    hits_count=Signal::derive(|| None)
                    on_select=Callback::new(move |f: MemoryFacet| { facet.set(f); notes_page.set(0); })
                />
            </div>

            // Reverse-link notice when the highlighted note is outside the window.
            {move || highlight_missing.get().then(|| view! {
                <p class="text-xs text-warning italic">{t!(i18n, memory.highlight_not_in_window)}</p>
            })}

            // Content — switches only between notes-vs-raw so paging/loads don't
            // remount the active table.
            {move || {
                if is_notes.get() {
                    view! {
                        <NotesTable
                            window=notes_window
                            facet=facet
                            page=notes_page
                            page_size=page_size
                            loaded=notes_loaded
                            connected=connected
                            highlight=Signal::derive(move || highlight_id.get())
                            on_open=move |fact| drawer_target.set(Some(DrawerTarget::Note(fact)))
                            on_locate=move |path: String| {
                                mem.selected_node.set(Some(path));
                                mem.memory_view.set(MemoryView::Graph);
                            }
                        />
                        {move || (notes_window.get().len() >= NOTE_WINDOW).then(|| view! {
                            <p class="text-xs text-text-tertiary italic pt-1">{t!(i18n, memory.notes_truncated)}</p>
                        })}
                        // `current_len` is hardcoded to 0 — see the
                        // TODO(Task 18) at the `FacetBar` call above; it only
                        // matters once `SearchHits` is reachable and
                        // `notes_total` goes `None` for it.
                        <Pager page=notes_page page_size=page_size total=notes_total current_len=Signal::derive(|| 0usize) />
                    }.into_any()
                } else {
                    let on_delete = on_delete_one;
                    let on_batch = on_batch_delete;
                    view! {
                        <RawTable
                            memories=raw_memories
                            loaded=raw_loaded
                            searching=is_searching
                            connected=connected
                            selected=selected_ids
                            on_open=move |raw| drawer_target.set(Some(DrawerTarget::Raw(raw)))
                            on_delete=on_delete
                            on_batch_delete=on_batch
                        />
                        {move || if is_search_active.get() {
                            view! { <p class="text-xs text-text-tertiary italic pt-1">{t!(i18n, memory.search_no_pagination)}</p> }.into_any()
                        } else {
                            view! { <Pager page=raw_page page_size=page_size total=raw_total current_len=Signal::derive(move || raw_memories.get().len()) /> }.into_any()
                        }}
                    }.into_any()
                }
            }}

            <DetailDrawer target=drawer_target />
        </div>
    }
}

// ─── Notes Table ────────────────────────────────────────────────────────────

#[component]
fn NotesTable(
    window: RwSignal<Vec<CompressedFact>>,
    facet: RwSignal<MemoryFacet>,
    page: RwSignal<u32>,
    page_size: RwSignal<u32>,
    loaded: RwSignal<bool>,
    connected: RwSignal<bool>,
    highlight: Signal<Option<String>>,
    on_open: impl Fn(CompressedFact) + Clone + Send + 'static,
    on_locate: impl Fn(String) + Clone + Send + 'static,
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
                            return view! { <tr><td colspan="4" class="p-8 text-center text-text-tertiary">{t!(i18n, memory.connect_to_view_facts)}</td></tr> }.into_any();
                        }
                        if !loaded.get() {
                            return view! { <tr><td colspan="4" class="p-8 text-center text-text-tertiary">{t!(i18n, common.loading)}</td></tr> }.into_any();
                        }
                        let current = facet_slice(&window.get(), facet.get());
                        let rows = page_slice(&current, page.get(), page_size.get());
                        if rows.is_empty() {
                            return view! { <tr><td colspan="4" class="p-8 text-center text-text-tertiary">{t!(i18n, memory.no_facts)}</td></tr> }.into_any();
                        }
                        let on_open = on_open.clone();
                        rows.into_iter().map(|fact| {
                            let badge_variant = match fact.fact_type.as_str() {
                                "preference" | "personal" => BadgeVariant::Indigo,
                                "learning" | "lesson" | "skill" => BadgeVariant::Emerald,
                                "plan" | "project" => BadgeVariant::Amber,
                                "feedback" => BadgeVariant::Red,
                                _ => BadgeVariant::Slate,
                            };
                            let agent_id = fact.agent_id.clone();
                            let fact_type = fact.fact_type.clone();
                            let path = fact.path.clone();
                            let content = fact.content.clone();
                            let date = format_ts(fact.created_at);
                            let on_open = on_open.clone();
                            let on_locate = on_locate.clone();
                            let fact_for_click = fact.clone();
                            let path_for_locate = path.clone();
                            let path_for_highlight = path.clone();
                            let i18n_row = i18n;
                            let is_hl = Signal::derive(move || highlight.get().as_deref() == Some(path_for_highlight.as_str()));
                            view! {
                                <tr
                                    class=move || if is_hl.get() {
                                        "group bg-primary-subtle ring-1 ring-primary/40 transition-colors cursor-pointer"
                                    } else {
                                        "group hover:bg-surface-sunken transition-colors cursor-pointer"
                                    }
                                    on:click=move |_| on_open(fact_for_click.clone())
                                >
                                    <td class="p-4 pl-8">
                                        <div class="flex items-start justify-between gap-2">
                                            <div class="min-w-0">
                                                <div class="text-sm font-medium text-text-primary line-clamp-2">{content}</div>
                                                <div class="text-xs text-text-tertiary mt-0.5 font-mono">{path}</div>
                                            </div>
                                            <button
                                                class="flex-shrink-0 p-1 rounded text-text-tertiary opacity-0 group-hover:opacity-100 hover:text-primary transition-all"
                                                title=t_string!(i18n_row, memory.view_in_graph)
                                                on:click=move |ev| { ev.stop_propagation(); on_locate(path_for_locate.clone()); }
                                            >
                                                <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                                                    <circle cx="5" cy="6" r="2" /><circle cx="19" cy="6" r="2" /><circle cx="12" cy="18" r="2" />
                                                    <line x1="6.6" y1="7.4" x2="10.6" y2="16.4" /><line x1="17.4" y1="7.4" x2="13.4" y2="16.4" /><line x1="7" y1="6" x2="17" y2="6" />
                                                </svg>
                                            </button>
                                        </div>
                                    </td>
                                    <td class="p-4"><Badge variant=BadgeVariant::Indigo>{agent_id}</Badge></td>
                                    <td class="p-4"><Badge variant=badge_variant>{fact_type}</Badge></td>
                                    <td class="p-4 pr-8">
                                        <div class="flex items-center gap-2 text-xs text-text-tertiary font-mono">
                                            <svg width="12" height="12" attr:class="w-3 h-3" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                                                <rect x="3" y="4" width="18" height="18" rx="2" ry="2" /><line x1="16" y1="2" x2="16" y2="6" /><line x1="8" y1="2" x2="8" y2="6" /><line x1="3" y1="10" x2="21" y2="10" />
                                            </svg>
                                            {date}
                                        </div>
                                    </td>
                                </tr>
                            }
                        }).collect_view().into_any()
                    }}
                </tbody>
            </table>
        </Card>
    }
}

// ─── Raw Memories Table ─────────────────────────────────────────────────────

#[component]
fn RawTable(
    memories: RwSignal<Vec<RawMemory>>,
    loaded: RwSignal<bool>,
    searching: RwSignal<bool>,
    connected: RwSignal<bool>,
    selected: RwSignal<HashSet<String>>,
    on_open: impl Fn(RawMemory) + Clone + Send + 'static,
    on_delete: impl Fn(String) + Clone + Send + 'static,
    on_batch_delete: impl Fn() + Clone + Send + 'static,
) -> impl IntoView {
    let i18n = use_i18n();
    let confirm_batch = RwSignal::new(false);
    let selected_count = Signal::derive(move || selected.get().len());

    view! {
        <div class="space-y-2">
            {move || {
                if selected_count.get() == 0 {
                    return view! { <div></div> }.into_any();
                }
                let on_batch_delete = on_batch_delete.clone();
                view! {
                    <div class="flex items-center justify-between bg-surface-sunken border border-border rounded-lg px-4 py-2">
                        <span class="text-sm text-text-secondary">
                            {move || selected_count.get().to_string()} " " {t!(i18n, memory.batch_selected)}
                        </span>
                        <div class="flex items-center gap-2">
                            {move || if confirm_batch.get() {
                                view! { <ConfirmButton confirming=confirm_batch on_confirm=on_batch_delete.clone() size_class="px-3 py-1 text-xs" /> }.into_any()
                            } else {
                                view! {
                                    <Button variant=ButtonVariant::Destructive size=ButtonSize::Sm class="px-3 py-1 text-xs".to_string() on:click=move |_| confirm_batch.set(true)>
                                        {t!(i18n, memory.batch_delete)}
                                    </Button>
                                }.into_any()
                            }}
                            <button class="text-xs text-text-tertiary hover:text-text-secondary" on:click=move |_| { selected.set(HashSet::new()); confirm_batch.set(false); }>
                                {t!(i18n, memory.batch_clear)}
                            </button>
                        </div>
                    </div>
                }.into_any()
            }}

            <Card class="overflow-hidden".to_string()>
                <table class="w-full text-left border-collapse">
                    <thead>
                        <tr class="bg-surface-sunken text-[10px] font-bold text-text-tertiary uppercase tracking-widest">
                            <th class="p-4 pl-8 w-10"></th>
                            <th class="p-4">{t!(i18n, memory.col_content)}</th>
                            <th class="p-4">{t!(i18n, memory.col_agent)}</th>
                            <th class="p-4">{t!(i18n, memory.col_date)}</th>
                            <th class="p-4 pr-8 text-right">{t!(i18n, memory.col_actions)}</th>
                        </tr>
                    </thead>
                    <tbody class="divide-y divide-border-subtle">
                        {move || {
                            if !connected.get() {
                                return view! { <tr><td colspan="5" class="p-8 text-center text-text-tertiary">{t!(i18n, memory.connect_to_view_raw)}</td></tr> }.into_any();
                            }
                            if searching.get() {
                                return view! { <tr><td colspan="5" class="p-8 text-center text-text-tertiary">{t!(i18n, memory.searching)}</td></tr> }.into_any();
                            }
                            if !loaded.get() {
                                return view! { <tr><td colspan="5" class="p-8 text-center text-text-tertiary">{t!(i18n, common.loading)}</td></tr> }.into_any();
                            }
                            if memories.get().is_empty() {
                                return view! { <tr><td colspan="5" class="p-8 text-center text-text-tertiary">{t!(i18n, memory.no_raw)}</td></tr> }.into_any();
                            }
                            let on_open = on_open.clone();
                            let on_delete = on_delete.clone();
                            view! {
                                <For
                                    each=move || memories.get()
                                    key=|m| m.id.clone()
                                    children=move |entry| {
                                        let created_at = entry.created_at.clone().unwrap_or_else(|| t_string!(i18n, memory.unknown).to_string());
                                        let entry_id = entry.id.clone();
                                        let entry_id_sel = entry.id.clone();
                                        let entry_id_toggle = entry.id.clone();
                                        let agent_id = entry.agent_id.clone();
                                        let content = entry.display_text();
                                        let entry_for_open = entry.clone();
                                        let on_open = on_open.clone();
                                        let on_delete = on_delete.clone();
                                        let is_checked = Signal::derive(move || selected.get().contains(&entry_id_sel));
                                        view! {
                                            <RawRow
                                                content=content
                                                agent_id=agent_id
                                                date=created_at
                                                checked=is_checked
                                                on_toggle=move || selected.update(|s| { if !s.remove(&entry_id_toggle) { s.insert(entry_id_toggle.clone()); } })
                                                on_open=move || on_open(entry_for_open.clone())
                                                on_delete=move || on_delete(entry_id.clone())
                                            />
                                        }
                                    }
                                />
                            }.into_any()
                        }}
                    </tbody>
                </table>
            </Card>
        </div>
    }
}

#[component]
fn RawRow(
    content: String,
    agent_id: String,
    date: String,
    checked: Signal<bool>,
    on_toggle: impl Fn() + Clone + Send + 'static,
    on_open: impl Fn() + Clone + Send + 'static,
    on_delete: impl Fn() + Clone + Send + 'static,
) -> impl IntoView {
    let confirm = RwSignal::new(false);
    let on_confirm_delete = move || on_delete();

    view! {
        <tr class="group hover:bg-surface-sunken transition-colors">
            <td class="p-4 pl-8">
                <input
                    type="checkbox"
                    class="cursor-pointer"
                    prop:checked=move || checked.get()
                    on:change=move |_| on_toggle()
                />
            </td>
            <td class="p-4 cursor-pointer" on:click=move |_| on_open()>
                <div class="text-sm font-medium text-text-primary line-clamp-1">{content}</div>
            </td>
            <td class="p-4"><Badge variant=BadgeVariant::Indigo>{agent_id}</Badge></td>
            <td class="p-4">
                <div class="flex items-center gap-2 text-xs text-text-tertiary font-mono">
                    <svg width="12" height="12" attr:class="w-3 h-3" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                        <rect x="3" y="4" width="18" height="18" rx="2" ry="2" /><line x1="16" y1="2" x2="16" y2="6" /><line x1="8" y1="2" x2="8" y2="6" /><line x1="3" y1="10" x2="21" y2="10" />
                    </svg>
                    {date}
                </div>
            </td>
            <td class="p-4 pr-8 text-right">
                <div class="flex items-center justify-end gap-2">
                    {move || if confirm.get() {
                        view! { <ConfirmButton confirming=confirm on_confirm=on_confirm_delete.clone() size_class="px-2.5 py-1 text-xs" /> }.into_any()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn facet_total_reports_the_note_bucket_it_names() {
        let counts = [10, 4, 3, 3];
        assert_eq!(facet_total(counts, MemoryFacet::AllNotes), Some(10));
        assert_eq!(facet_total(counts, MemoryFacet::Facts), Some(4));
        assert_eq!(facet_total(counts, MemoryFacet::Feedback), Some(3));
        assert_eq!(facet_total(counts, MemoryFacet::Lessons), Some(3));
    }

    #[test]
    fn facet_total_is_honestly_unknown_for_search_hits_not_a_lying_zero() {
        // SearchHits rows arrive on their own signal, not the note window
        // `counts` is built from — there is nothing here resembling a real
        // count. Reporting `Some(0)` (what this used to return) would make
        // the pager compute `page_count(0, ps) == 1` and silently hide
        // prev/next even while many real hits are loaded elsewhere. `None`
        // is the honest "unknown" `Pager` already knows how to degrade from.
        let counts = [10, 4, 3, 3]; // any note-window counts; must not leak in
        assert_eq!(facet_total(counts, MemoryFacet::SearchHits), None);
    }

    #[test]
    fn facet_total_is_none_for_raw_too_its_own_pager_uses_stats_total() {
        // Raw is server-paginated via `raw_total` (agent stats), not this
        // note-window total; `None` here is inert (its `Pager` never reads
        // this value) but must not be a lying zero either.
        let counts = [10, 4, 3, 3];
        assert_eq!(facet_total(counts, MemoryFacet::Raw), None);
    }
}
