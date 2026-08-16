use leptos::leptos_dom::helpers::set_timeout_with_handle;
use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::hooks::use_location;

use crate::api::agents::AgentsApi;
use crate::api::graph::GraphApi;
use crate::api::{CompressedFact, MemoryApi, MemoryStats};
use crate::context::{strip_params, DashboardState};
use crate::i18n::{t, t_string, use_i18n};
use crate::state::memory::{MemoryState, MemoryView};

mod batch_bar;
mod cards;
pub mod data;
mod drawer;
mod facets;
pub mod galaxy;
mod loader;
mod pager;
mod provenance;
mod selection;
mod stats;
mod toast;

use batch_bar::BatchBar;
use cards::{CardListShell, NoteCard, RawCard};
use data::{
    bucket_counts, facet_slice, filter_notes, locate_note, notes_to_markdown, notes_truncated,
    page_slice, raws_to_markdown, stage_raw_export, Loadable, MemoryFacet, NoteExport, RawExport,
    EXPORT_MAX, NOTE_WINDOW, PAGE_SIZE,
};
use drawer::{DetailDrawer, DrawerTarget};
use facets::FacetBar;
use loader::{load_notes, load_raw, load_search_hits, load_stats, NotesWindow, RawWindow};
use pager::Pager;
use selection::AgentSelection;
use stats::{MemoryHeader, StatCards};
use toast::{push_toast, ToastHost, ToastKind, ToastMsg};

/// Debounce before the search box filters the loaded window. Long enough that a
/// fast typist does not re-filter on every keystroke, short enough to feel live.
const SEARCH_DEBOUNCE_MS: u64 = 200;

/// Cap on server-side full-text hits. Surfaced in the UI (`search_hits_capped`)
/// because a silent cap reads as "the store only has this much".
const SEARCH_HITS_LIMIT: usize = 100;

/// Cap on `memory.trace` evidence items per fetch. The server accepts an
/// unbounded walk (`max_results: Option<usize>`); this panel-side cap keeps
/// one evidence-chain fetch bounded, and `provenance_capped` says so in the
/// UI whenever the returned list actually hits it (no silent caps).
const TRACE_MAX_RESULTS: usize = 20;

/// Memory Vault console at `/memory?view=table`.
///
/// Layered over the note store (All / Facts / Feedback / Lessons), the raw
/// conversation log, and server-side full-text hits. Search is dual-track: the
/// box filters the loaded window locally as you type, and Enter runs a real
/// `graph.search` whose hits land in their own layer. Pure I/O — every mutation
/// is a JSON-RPC call (R4).
#[component]
#[must_use]
pub fn Memory() -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let mem = expect_context::<MemoryState>();
    let i18n = use_i18n();

    // ── Load slots ──────────────────────────────────────────────────────────
    let stats = RwSignal::new(Loadable::<MemoryStats>::Loading);
    let notes = RwSignal::new(Loadable::<NotesWindow>::Loading);
    let raws = RwSignal::new(Loadable::<RawWindow>::Loading);
    let hits = RwSignal::new(Loadable::<Vec<CompressedFact>>::Loading);

    // ── View state ──────────────────────────────────────────────────────────
    let facet = RwSignal::new(MemoryFacet::AllNotes);
    let page = RwSignal::new(0u32);
    let page_size = RwSignal::new(PAGE_SIZE);
    let raw_query = RwSignal::new(String::new());
    let local_query = RwSignal::new(String::new());
    let search_live = RwSignal::new(false);
    let selected = RwSignal::new(AgentSelection::default());
    let drawer_target = RwSignal::new(None::<DrawerTarget>);
    let highlight_id = RwSignal::new(None::<String>);
    let exporting = RwSignal::new(None::<(usize, usize)>);
    let toast_slot = RwSignal::new(None::<ToastMsg>);
    let refresh_nonce = RwSignal::new(0u32);

    // ── Agent bootstrap (only if the canvas hasn't populated it yet) ─────────
    Effect::new(move || {
        if !state.is_connected.get() || !mem.agents.get().is_empty() {
            return;
        }
        spawn_local(async move {
            if let Ok(resp) = AgentsApi::list(&state).await {
                mem.agents.set(resp.agents);
                // Post-`.await`: see `crate::disposed_reads`.
                let Some(current) = mem.agent_id.try_get_untracked() else {
                    return;
                };
                if current != resp.default_id {
                    mem.agent_id.set(resp.default_id);
                }
            }
        });
    });

    // ── Fetches ─────────────────────────────────────────────────────────────
    Effect::new(move || {
        refresh_nonce.get();
        if !state.is_connected.get() {
            stats.set(Loadable::Loading);
            notes.set(Loadable::Loading);
            return;
        }
        let agent = mem.agent_id.get();
        load_stats(state, agent.clone(), stats);
        load_notes(state, agent, NOTE_WINDOW, notes);
        page.set(0);
    });

    Effect::new(move || {
        refresh_nonce.get();
        if !state.is_connected.get() {
            raws.set(Loadable::Loading);
            return;
        }
        let (p, size, q, agent) = (
            page.get(),
            page_size.get(),
            raw_query.get(),
            mem.agent_id.get(),
        );
        load_raw(state, agent, q, size, p * size, raws);
    });

    // ── Cross-agent hygiene on switch (C1) ──────────────────────────────────
    // `selected`, `drawer_target`, and `highlight_id` all key off note *paths*
    // — `category/filename` over a small, fixed category set, so a collision
    // like `preference/coding-style` across two agents is entirely plausible.
    //
    // Safety no longer lives here. `AgentSelection` and `DrawerTarget::Note`
    // both carry the agent they were built under, and every consumer reads
    // through that stamp, so a stale selection or a drawer left open across a
    // switch cannot reach the new agent's store even if this Effect never ran.
    // What remains is housekeeping: clear the boxes and close the drawer so
    // the view matches the agent on screen. `highlight_id` is the one piece
    // with no stamp of its own — it only tints a row, and a tinted row under
    // the wrong agent is cosmetic, not destructive.
    //
    // Fires only when the agent actually changes, never on an unrelated
    // refresh, which must leave an in-progress selection alone.
    // `agent_switched` is the pure "did it actually change" decision; see its
    // unit tests below.
    let last_agent = StoredValue::new(mem.agent_id.get_untracked());
    Effect::new(move || {
        let agent = mem.agent_id.get();
        if !agent_switched(&last_agent.get_value(), &agent) {
            return;
        }
        last_agent.set_value(agent);
        selected.update(AgentSelection::clear);
        drawer_target.set(None);
        highlight_id.set(None);
    });

    // ── Dual-track search ───────────────────────────────────────────────────
    // Track 1: every keystroke debounce-filters the LOADED window. `filter_notes`
    // has existed (and been unit-tested) since the phone list shipped; the
    // desktop view never called it, so typing here used to do nothing to the
    // note layers.
    let debounce_handle = StoredValue::new(None::<leptos::leptos_dom::helpers::TimeoutHandle>);
    Effect::new(move || {
        let q = mem.search_query.get();
        if let Some(h) = debounce_handle.get_value() {
            h.clear();
        }
        let handle = set_timeout_with_handle(
            move || {
                local_query.set(q.clone());
                page.set(0);
            },
            std::time::Duration::from_millis(SEARCH_DEBOUNCE_MS),
        )
        .ok();
        debounce_handle.set_value(handle);
    });
    on_cleanup(move || {
        if let Some(h) = debounce_handle.get_value() {
            h.clear();
        }
    });

    // Track 2: Enter runs a real server-side search. Hits get their own layer;
    // they are NOT poured into the raw table (they are notes).
    Effect::new(move || {
        mem.search_nonce.get();
        let q = mem.search_query.get_untracked();
        let agent = mem.agent_id.get_untracked();
        page.set(0);
        if q.trim().is_empty() {
            search_live.set(false);
            if facet.get_untracked() == MemoryFacet::SearchHits {
                facet.set(MemoryFacet::AllNotes);
            }
            return;
        }
        search_live.set(true);
        facet.set(MemoryFacet::SearchHits);
        load_search_hits(state, agent, q, SEARCH_HITS_LIMIT, hits);
    });

    // `hits`'s own invalidation. `stats`/`notes`/`raws` all refetch on
    // `refresh_nonce` (header Refresh, and after every delete); `hits` used to
    // sit outside that cycle entirely, subscribed only to `search_nonce`. A
    // note deleted or exported while `SearchHits` was the active facet bumped
    // `refresh_nonce`, correctly refreshed everything else, and left the
    // just-deleted note sitting in the hit list with a stale chip count until
    // the user re-ran the search by hand. This effect closes that gap without
    // touching `facet`/`page`: unlike the Track 2 effect above (which owns
    // "should Enter switch us into SearchHits"), a background refresh must
    // never yank the user back into a facet they've since navigated away from.
    //
    // `mem.agent_id` is read (tracked) unconditionally, before the
    // `search_live` early return, for the same reason: an agent switch bumps
    // neither `refresh_nonce` nor `search_nonce`, so this effect must
    // resubscribe to the agent every run or it will never notice a switch
    // that happens while `search_live` is false and then becomes true again.
    Effect::new(move || {
        refresh_nonce.get();
        let agent = mem.agent_id.get();
        if !search_live.get_untracked() {
            return;
        }
        let q = mem.search_query.get_untracked();
        load_search_hits(state, agent, q, SEARCH_HITS_LIMIT, hits);
    });

    // The raw layer filters server-side (LIKE over content), so it needs the
    // committed query rather than the local one.
    Effect::new(move || {
        let q = local_query.get();
        if facet.get_untracked() == MemoryFacet::Raw {
            raw_query.set(q);
        }
    });
    Effect::new(move || {
        if facet.get() == MemoryFacet::Raw {
            raw_query.set(local_query.get_untracked());
        }
    });

    // ── Deep link: ?note=<path> ─────────────────────────────────────────────
    // `?view=` stays owned by memory_hub; this Effect owns `?note=` only, so the
    // two never overwrite each other.
    //
    // `note_param_consumed` guards a one-shot: `scrub_note_param` below rewrites
    // the address bar with raw `web_sys` `history.replaceState`, which does NOT
    // touch leptos_router's own `BrowserUrl` signal — only `navigate()` /
    // `complete_navigation()` or a `popstate` event update that, and browsers
    // never fire `popstate` for `replaceState`. So `location.search` would keep
    // reporting the same stale `note=<path>` for the rest of the session, and
    // since this Effect also has to track `notes.get()` (to wait for the window
    // on first load), it would silently reopen the same note on every Refresh
    // click, every delete, and every agent switch — each one bumps `notes`.
    // Reading the URL untracked and gating on this flag makes the whole thing
    // fire at most once per mount, which is all a deep link ever needs: by the
    // time a *different* `?note=` should apply, the browser has done a real
    // navigation and freshly mounted a new `Memory` instance anyway.
    let location = use_location();
    let note_param_consumed = RwSignal::new(false);
    Effect::new(move || {
        notes.track(); // re-run when the window lands; the value itself is read untracked below
        if note_param_consumed.get_untracked() {
            // Short-circuit before touching `notes` again: past this point the
            // window can hold up to `NOTE_WINDOW` (1000) facts, and every future
            // `notes` update (every delete, refresh, agent switch) would
            // otherwise clone the whole thing forever for a decision that's
            // already been made.
            return;
        }
        let search = location.search.get_untracked();
        let path = parse_note_param(&search);
        let window = notes.get_untracked().as_ready().cloned();
        let (should_process, mark_consumed) = note_param_decision(path.is_some(), window.is_some());
        if mark_consumed {
            note_param_consumed.set(true);
        }
        if !should_process {
            return;
        }
        let path = path.expect("note_param_decision only processes when has_path was true");
        let window = window.expect("note_param_decision only processes when window_ready was true");
        // The window was loaded for this agent, so it is the agent the opened
        // note belongs to — stamp it rather than letting the drawer re-resolve.
        let agent = mem.agent_id.get_untracked();
        match locate_note(&window.facts, &path, page_size.get_untracked()) {
            Some((f, pg)) => {
                facet.set(f);
                page.set(pg);
                highlight_id.set(Some(path.clone()));
                if let Some(found) = window.facts.iter().find(|x| x.path == path) {
                    drawer_target.set(Some(DrawerTarget::Note {
                        agent,
                        fact: found.clone(),
                    }));
                }
            }
            None => {
                // Outside the loaded window: open it directly rather than
                // shrugging with "not in the current window" like the old view.
                drawer_target.set(Some(DrawerTarget::Note {
                    agent,
                    fact: CompressedFact::stub_from_path(&path),
                }));
            }
        }
        // Still worth scrubbing the DOM: a genuine F5 reload reads the real
        // window.location fresh (no router signal involved), so this is what
        // stops a reload from reopening the drawer.
        scrub_note_param();
    });

    // Reverse link from the galaxy's node detail panel.
    Effect::new(move || {
        let Some(path) = mem.highlight_note_id.get() else {
            return;
        };
        let Some(window) = notes.get().as_ready().cloned() else {
            return;
        };
        let agent = mem.agent_id.get_untracked();
        if let Some((f, pg)) = locate_note(&window.facts, &path, page_size.get()) {
            facet.set(f);
            page.set(pg);
            highlight_id.set(Some(path.clone()));
            if let Some(found) = window.facts.iter().find(|x| x.path == path) {
                drawer_target.set(Some(DrawerTarget::Note {
                    agent,
                    fact: found.clone(),
                }));
            }
        } else {
            drawer_target.set(Some(DrawerTarget::Note {
                agent,
                fact: CompressedFact::stub_from_path(&path),
            }));
        }
        mem.highlight_note_id.set(None);
    });

    // ── Derived rows ────────────────────────────────────────────────────────
    // `SearchHits` rows come from `hits`, not the loaded window: `facet_slice`
    // deliberately returns empty for this facet (its rows arrive on their own
    // signal), so reading it here is what gives the hit list an actual body
    // instead of an always-empty "no notes" card.
    let note_rows = Signal::derive(move || {
        let f = facet.get();
        if f == MemoryFacet::SearchHits {
            return hits.get().as_ready().cloned().unwrap_or_default();
        }
        let Some(window) = notes.get().as_ready().cloned() else {
            return Vec::new();
        };
        filter_notes(&facet_slice(&window.facts, f), &local_query.get())
    });
    let counts = Signal::derive(move || {
        notes
            .get()
            .as_ready()
            .map(|w| bucket_counts(&w.facts))
            .unwrap_or([0; 4])
    });
    let is_notes = Memo::new(move |_| facet.get().is_notes());

    // Note rows are paginated client-side over the loaded window; raw rows are
    // server-paginated, so their pager reads the scoped stats total.
    let note_page_rows =
        Signal::derive(move || page_slice(&note_rows.get(), page.get(), page_size.get()));
    let raw_rows = Signal::derive(move || {
        raws.get()
            .as_ready()
            .map(|w| w.raws.clone())
            .unwrap_or_default()
    });

    view! {
        <div class="px-8 pb-8 aleph-content-top max-w-7xl mx-auto space-y-6">
            <MemoryHeader on_refresh=move || refresh_nonce.update(|n| *n += 1) />
            <StatCards stats=stats.into() />

            <div class="flex items-center justify-between gap-3 flex-wrap border-b border-border pb-2">
                <FacetBar
                    active=facet
                    counts=counts
                    raw_count=Signal::derive(move || stats.get().as_ready().map(|s| s.total_memories))
                    hits_count=Signal::derive(move || {
                        search_live
                            .get()
                            .then(|| hits.get().as_ready().map(Vec::len).unwrap_or(0))
                    })
                    on_select=Callback::new(move |f: MemoryFacet| {
                        facet.set(f);
                        page.set(0);
                        selected.update(AgentSelection::clear);
                    })
                />
            </div>

            <BatchBar
                selected=selected
                agent=Signal::derive(move || mem.agent_id.get())
                page_ids=Signal::derive(move || {
                    if is_notes.get() {
                        note_page_rows.get().into_iter().map(|f| f.path).collect()
                    } else {
                        raw_rows.get().into_iter().map(|r| r.id).collect()
                    }
                })
                exporting=exporting
                on_copy_md=move || {
                    // Resolve the agent first and read the selection *through*
                    // it: ids ticked under a different agent yield an empty set
                    // here, which is the same no-op as ticking nothing.
                    let agent = mem.agent_id.get_untracked();
                    let sel = selected.get_untracked().ids_for(&agent);
                    if sel.is_empty() || sel.len() > EXPORT_MAX {
                        return;
                    }
                    if is_notes.get_untracked() {
                        let rows: Vec<CompressedFact> = note_rows
                            .get_untracked()
                            .into_iter()
                            .filter(|f| sel.contains(&f.path))
                            .collect();
                        let total = rows.len();
                        exporting.set(Some((0, total)));
                        spawn_local(async move {
                            let mut staged = Vec::with_capacity(total);
                            let mut failures = 0usize;
                            for (i, f) in rows.into_iter().enumerate() {
                                let body = GraphApi::node_detail(&state, &agent, &f.path)
                                    .await
                                    .map(|d| d.content);
                                if body.is_err() {
                                    failures += 1;
                                }
                                staged.push(NoteExport {
                                    title: f.content.clone(),
                                    path: f.path.clone(),
                                    body,
                                });
                                exporting.set(Some((i + 1, total)));
                            }
                            let copied = write_clipboard(&notes_to_markdown(&staged)).await;
                            exporting.set(None);
                            // A green success tone under-signals a partial failure at a
                            // glance even with accurate text next to it, so the tone
                            // itself carries the "something didn't fully work" signal. A
                            // rejected clipboard write overrides that: nothing reached
                            // the clipboard at all, regardless of how many bodies loaded.
                            let (msg, kind) = if !copied {
                                (
                                    t_string!(i18n, memory.toast_copy_failed).to_string(),
                                    ToastKind::Error,
                                )
                            } else if failures > 0 {
                                (
                                    format!(
                                        "{} — {}",
                                        t_string!(i18n, memory.toast_copied),
                                        t_string!(i18n, memory.batch_export_partial)
                                    ),
                                    ToastKind::Error,
                                )
                            } else {
                                (
                                    t_string!(i18n, memory.toast_copied).to_string(),
                                    ToastKind::Success,
                                )
                            };
                            push_toast(toast_slot, msg, kind);
                        });
                    } else {
                        // Raw rows are server-paginated and `sel` is never cleared on
                        // page change, so a selection built across two pages can
                        // outlive the page it was made on — `stage_raw_export` reports
                        // how many selected ids are not on the currently loaded page
                        // rather than silently exporting only what's on screen.
                        let (staged_raws, dropped) =
                            stage_raw_export(&sel, &raw_rows.get_untracked());
                        let staged: Vec<RawExport> = staged_raws
                            .into_iter()
                            .map(|r| RawExport {
                                id: r.id,
                                agent_id: r.agent_id,
                                session_id: r.session_id,
                                created_at: r.created_at.unwrap_or_default(),
                                user_input: r.user_input,
                                ai_output: r.ai_output,
                            })
                            .collect();
                        spawn_local(async move {
                            let copied = write_clipboard(&raws_to_markdown(&staged)).await;
                            let (msg, kind) = if !copied {
                                (
                                    t_string!(i18n, memory.toast_copy_failed).to_string(),
                                    ToastKind::Error,
                                )
                            } else if dropped > 0 {
                                (
                                    format!(
                                        "{} — {}",
                                        t_string!(i18n, memory.toast_copied),
                                        t_string!(i18n, memory.batch_export_partial)
                                    ),
                                    ToastKind::Error,
                                )
                            } else {
                                (
                                    t_string!(i18n, memory.toast_copied).to_string(),
                                    ToastKind::Success,
                                )
                            };
                            push_toast(toast_slot, msg, kind);
                        });
                    }
                }
                on_delete=move || {
                    // Same order as `on_copy_md`, and for a stronger reason:
                    // reading the selection through the live agent is what
                    // makes it impossible to delete agent B's notes with boxes
                    // ticked under agent A.
                    let agent = mem.agent_id.get_untracked();
                    let ids: Vec<String> =
                        selected.get_untracked().ids_for(&agent).into_iter().collect();
                    if ids.is_empty() {
                        return;
                    }
                    // Two stores, two verbs. Notes are curated files
                    // (graph.delete_note); raw rows live in raw_memories
                    // (memory.delete). Sending a note path to memory.delete is
                    // exactly the mix-up that produced undeletable ghost rows.
                    let notes_layer = is_notes.get_untracked();
                    spawn_local(async move {
                        let mut failed = 0usize;
                        for id in ids {
                            let res = if notes_layer {
                                GraphApi::delete_note(&state, &agent, &id).await
                            } else {
                                MemoryApi::delete(&state, id).await
                            };
                            if res.is_err() {
                                failed += 1;
                            }
                        }
                        selected.update(AgentSelection::clear);
                        refresh_nonce.update(|n| *n += 1);
                        if failed > 0 {
                            push_toast(toast_slot, format!("{} ({failed})", t_string!(i18n, memory.toast_delete_failed)), ToastKind::Error);
                        } else {
                            push_toast(toast_slot, t_string!(i18n, memory.toast_deleted).to_string(), ToastKind::Success);
                        }
                    });
                }
            />

            {move || if is_notes.get() {
                view! {
                    <CardListShell
                        state=Signal::derive(move || {
                            if facet.get() == MemoryFacet::SearchHits {
                                match hits.get() {
                                    Loadable::Loading => Loadable::Loading,
                                    Loadable::Failed(e) => Loadable::Failed(e),
                                    Loadable::Ready(_) => Loadable::Ready(note_rows.get().len()),
                                }
                            } else {
                                match notes.get() {
                                    Loadable::Loading => Loadable::Loading,
                                    Loadable::Failed(e) => Loadable::Failed(e),
                                    Loadable::Ready(_) => Loadable::Ready(note_rows.get().len()),
                                }
                            }
                        })
                        empty_label=Signal::derive(move || {
                            if facet.get() == MemoryFacet::SearchHits {
                                t_string!(i18n, memory.no_search_hits).to_string()
                            } else {
                                t_string!(i18n, memory.no_facts).to_string()
                            }
                        })
                        on_retry=move || refresh_nonce.update(|n| *n += 1)
                    >
                        <For
                            each=move || note_page_rows.get()
                            key=|f| f.path.clone()
                            children=move |fact| {
                                let path = fact.path.clone();
                                let p_sel = path.clone();
                                let p_tog = path.clone();
                                let p_hl = path.clone();
                                let p_loc = path.clone();
                                let p_link = path.clone();
                                let fact_open = fact.clone();
                                view! {
                                    <NoteCard
                                        fact=fact
                                        checked=Signal::derive(move || selected.get().contains(&mem.agent_id.get(), &p_sel))
                                        highlighted=Signal::derive(move || highlight_id.get().as_deref() == Some(p_hl.as_str()))
                                        on_toggle=move || selected.update(|s| s.toggle(&mem.agent_id.get_untracked(), &p_tog))
                                        on_open=move || drawer_target.set(Some(DrawerTarget::Note {
                                            agent: mem.agent_id.get_untracked(),
                                            fact: fact_open.clone(),
                                        }))
                                        on_locate=move || {
                                            mem.selected_node.set(Some(p_loc.clone()));
                                            mem.memory_view.set(MemoryView::Graph);
                                        }
                                        on_copy_link=move || {
                                            let path = p_link.clone();
                                            spawn_local(async move {
                                                if copy_note_link(&path).await {
                                                    push_toast(toast_slot, t_string!(i18n, memory.toast_link_copied).to_string(), ToastKind::Success);
                                                } else {
                                                    push_toast(toast_slot, t_string!(i18n, memory.toast_copy_failed).to_string(), ToastKind::Error);
                                                }
                                            });
                                        }
                                    />
                                }
                            }
                        />
                    </CardListShell>
                    // No silent caps: say when the window or the hit list is capped.
                    {move || (facet.get() == MemoryFacet::SearchHits
                        && hits.get().as_ready().map(|h| h.len() >= SEARCH_HITS_LIMIT).unwrap_or(false))
                        .then(|| view! {
                            <p class="text-xs text-text-tertiary italic pt-1">{t!(i18n, memory.search_hits_capped)}</p>
                        })}
                    {move || notes.get().as_ready()
                        .map(|w| notes_truncated(w.total, w.facts.len()))
                        .unwrap_or(false)
                        .then(|| view! {
                            <p class="text-xs text-text-tertiary italic pt-1">{t!(i18n, memory.notes_truncated)}</p>
                        })}
                    <Pager
                        page=page
                        page_size=page_size
                        total=Signal::derive(move || Some(note_rows.get().len() as u64))
                        current_len=Signal::derive(move || note_page_rows.get().len())
                    />
                }.into_any()
            } else {
                view! {
                    <CardListShell
                        state=Signal::derive(move || match raws.get() {
                            Loadable::Loading => Loadable::Loading,
                            Loadable::Failed(e) => Loadable::Failed(e),
                            Loadable::Ready(w) => Loadable::Ready(w.raws.len()),
                        })
                        empty_label=Signal::derive(move || t_string!(i18n, memory.no_raw).to_string())
                        on_retry=move || refresh_nonce.update(|n| *n += 1)
                    >
                        <For
                            each=move || raw_rows.get()
                            key=|r| r.id.clone()
                            children=move |raw| {
                                let id = raw.id.clone();
                                let id_sel = id.clone();
                                let id_tog = id.clone();
                                let id_del = id.clone();
                                let raw_open = raw.clone();
                                view! {
                                    <RawCard
                                        raw=raw
                                        checked=Signal::derive(move || selected.get().contains(&mem.agent_id.get(), &id_sel))
                                        on_toggle=move || selected.update(|s| s.toggle(&mem.agent_id.get_untracked(), &id_tog))
                                        on_open=move || drawer_target.set(Some(DrawerTarget::Raw(raw_open.clone())))
                                        on_delete={
                                            let id_del = id_del.clone();
                                            move || {
                                                let id = id_del.clone();
                                                spawn_local(async move {
                                                    match MemoryApi::delete(&state, id).await {
                                                        Ok(()) => {
                                                            push_toast(toast_slot, t_string!(i18n, memory.toast_deleted).to_string(), ToastKind::Success);
                                                            refresh_nonce.update(|n| *n += 1);
                                                        }
                                                        // The old view checked is_ok() and dropped the
                                                        // reason, so a failed delete left the row in
                                                        // place with no explanation.
                                                        Err(e) => push_toast(toast_slot, format!("{}: {e}", t_string!(i18n, memory.toast_delete_failed)), ToastKind::Error),
                                                    }
                                                });
                                            }
                                        }
                                    />
                                }
                            }
                        />
                    </CardListShell>
                    <Pager
                        page=page
                        page_size=page_size
                        total=Signal::derive(move || raws.get().as_ready().and_then(|w| w.total))
                        current_len=Signal::derive(move || raw_rows.get().len())
                    />
                }.into_any()
            }}

            <ToastHost toast_slot=toast_slot />
            <DetailDrawer
                target=drawer_target
                toast_slot=toast_slot
                on_mutated=Callback::new(move |()| refresh_nonce.update(|n| *n += 1))
            />
        </div>
    }
}

/// Extract the raw (still percent-encoded) `note=` value from a URL query
/// string. Split from the percent-decoding step in [`parse_note_param`] so the
/// parsing logic is unit-testable without a browser runtime — mirrors
/// `loader.rs`'s split of "pure mapping" from the RPC call it feeds.
fn raw_note_param(search: &str) -> Option<&str> {
    let s = search.strip_prefix('?').unwrap_or(search);
    s.split('&')
        .find_map(|pair| pair.strip_prefix("note="))
        .filter(|v| !v.is_empty())
}

/// Read `note=<path>` out of a URL query string. `None` when absent, so the
/// Effect leaves the current selection alone.
#[must_use]
pub(crate) fn parse_note_param(search: &str) -> Option<String> {
    let raw = raw_note_param(search)?;
    let decoded: String = js_sys::decode_uri_component(raw).ok()?.into();
    (!decoded.is_empty()).then_some(decoded)
}

/// One-shot decision for the `?note=` deep-link Effect, for the case where it
/// has NOT already been consumed this mount (the Effect itself short-circuits
/// on that flag before ever calling this, so callers never pass a stale
/// decision back in). Split out so the consume-once state machine is
/// unit-testable without a browser `Location` — the Effect only ever reads
/// this through `bool`/`Option::is_some()` probes, never the values
/// themselves.
///
/// Returns `(should_process, mark_consumed)`:
/// - No `note=` param: nothing to do, and nothing will ever change that for
///   this mount (the URL only changes via a real navigation, which mounts a
///   fresh `Memory` with its own fresh flag) — mark consumed so future
///   `notes` updates skip the parse instead of repeating it forever.
/// - A `note=` param exists but the window isn't loaded yet: wait. Don't mark
///   consumed, so the next `notes` update (the window landing) retries.
/// - A `note=` param exists and the window is ready: process it now, exactly
///   once.
#[must_use]
fn note_param_decision(has_path: bool, window_ready: bool) -> (bool, bool) {
    if !has_path {
        return (false, true);
    }
    if !window_ready {
        return (false, false);
    }
    (true, true)
}

/// Whether the agent actually changed since the last time the cross-agent
/// selection-safety Effect ran (C1). Split out so the "did it change" decision
/// is unit-tested without a Leptos runtime, mirroring `note_param_decision`.
/// Deliberately not tied to any particular agent id shape (including the
/// empty-string starting value before the first agent list loads) — only
/// equality matters.
#[must_use]
fn agent_switched(previous: &str, current: &str) -> bool {
    previous != current
}

/// Drop `note=` from the address bar after consuming it, so a reload does not
/// force the drawer open again. Reuses `context::strip_params` (the same
/// rebuild-the-query-string logic already covers `?token=`/`?bt=` there) and
/// mirrors `context::scrub_credentials_from_url`'s use of `location().search()`
/// / `location().pathname()` rather than splitting `href` by hand, which would
/// mis-split a URL that also carries a `#fragment`.
fn scrub_note_param() {
    let Some(win) = web_sys::window() else { return };
    let Ok(search) = win.location().search() else {
        return;
    };
    if !search.contains("note=") {
        return;
    }
    let Ok(history) = win.history() else { return };
    let pathname = win
        .location()
        .pathname()
        .unwrap_or_else(|_| "/".to_string());
    let remaining = strip_params(&search, &["note="]);
    let next = if remaining.is_empty() {
        pathname
    } else {
        format!("{pathname}?{remaining}")
    };
    let _ = history.replace_state_with_url(&wasm_bindgen::JsValue::NULL, "", Some(&next));
}

/// Write `text` to the system clipboard, returning whether it succeeded.
///
/// `navigator.clipboard.write_text` returns a promise that can reject (lost
/// user activation after an awaited fetch loop, an insecure context, a denied
/// permission) — discarding it, as the old code did, means a rejected write
/// still falls through to a "Copied to clipboard" success toast.
async fn write_clipboard(text: &str) -> bool {
    let Some(win) = web_sys::window() else {
        return false;
    };
    let promise = win.navigator().clipboard().write_text(text);
    wasm_bindgen_futures::JsFuture::from(promise).await.is_ok()
}

/// Build a shareable deep link to one note and put it on the clipboard.
/// Returns whether the clipboard write succeeded.
async fn copy_note_link(path: &str) -> bool {
    let Some(win) = web_sys::window() else {
        return false;
    };
    let origin = win.location().origin().unwrap_or_default();
    let encoded: String = js_sys::encode_uri_component(path).into();
    write_clipboard(&format!("{origin}/memory?view=table&note={encoded}")).await
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── raw_note_param ───────────────────────────────────────────────────────

    #[test]
    fn raw_note_param_extracts_the_still_encoded_value() {
        assert_eq!(raw_note_param("?note=facts%2Fx"), Some("facts%2Fx"));
        assert_eq!(raw_note_param("note=facts%2Fx"), Some("facts%2Fx"));
    }

    #[test]
    fn raw_note_param_finds_it_alongside_other_params_in_either_order() {
        assert_eq!(
            raw_note_param("?view=table&note=facts%2Fx"),
            Some("facts%2Fx")
        );
        assert_eq!(
            raw_note_param("?note=facts%2Fx&view=table"),
            Some("facts%2Fx")
        );
    }

    #[test]
    fn raw_note_param_absent_is_none() {
        assert_eq!(raw_note_param("?view=table"), None);
        assert_eq!(raw_note_param(""), None);
    }

    #[test]
    fn raw_note_param_empty_value_is_none() {
        // An empty `note=` must not open the drawer on a stub path.
        assert_eq!(raw_note_param("?note="), None);
        assert_eq!(raw_note_param("?note=&view=table"), None);
    }

    // ── note_param_decision ──────────────────────────────────────────────────
    //
    // The "already consumed" case is NOT exercised through this function: the
    // Effect short-circuits on that flag before ever calling it (see the
    // comment at the call site), so there is no `already_consumed` input left
    // for this function to branch on.

    #[test]
    fn no_path_marks_consumed_with_nothing_to_process() {
        // Nothing will ever make a `note=`-less URL grow one on this mount, so
        // there is no reason to keep re-parsing on every future `notes` update.
        assert_eq!(note_param_decision(false, true), (false, true));
        assert_eq!(note_param_decision(false, false), (false, true));
    }

    #[test]
    fn path_present_but_window_not_ready_waits_without_consuming() {
        // Must NOT mark consumed here -- the whole reason this Effect tracks
        // `notes` is to retry once the window finishes loading.
        assert_eq!(note_param_decision(true, false), (false, false));
    }

    #[test]
    fn path_present_and_window_ready_processes_exactly_once() {
        assert_eq!(note_param_decision(true, true), (true, true));
    }

    // ── agent_switched (C1) ──────────────────────────────────────────────────

    #[test]
    fn agent_switched_true_when_the_id_differs() {
        assert!(agent_switched("agent-a", "agent-b"));
    }

    #[test]
    fn agent_switched_false_when_the_id_is_unchanged() {
        assert!(!agent_switched("agent-a", "agent-a"));
        assert!(!agent_switched("", ""));
    }
}
