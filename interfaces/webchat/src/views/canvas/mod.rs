mod agent_selector;
mod breadcrumb;
mod detail_panel;
mod graph_canvas;
#[cfg(target_arch = "wasm32")]
mod minimap_view;
mod toolbar;

use crate::canvas_engine::mini_map::GlobalMiniMap;
#[cfg(target_arch = "wasm32")]
use minimap_view::MiniMapOverlay;

use std::cell::RefCell;
use std::rc::Rc;

use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::api::graph::GraphApi;
use crate::canvas_engine::adapter::{
    populate_orphans, to_neighborhood, GraphNeighborsResponse, GraphQueryResponse,
    NoteDetailResponse, NoteNodeDto,
};
use crate::canvas_engine::interaction::CanvasEvent;
use crate::canvas_engine::navigation::{NavController, RETARGET_DURATION_MS};
use crate::canvas_engine::prefetch::PrefetchCache;
use crate::canvas_engine::types::BreadcrumbEntry;
use detail_panel::DetailContent;
use leptos::callback::Callback;

use crate::context::DashboardState;

use breadcrumb::Breadcrumb;
use detail_panel::DetailPanel;
use graph_canvas::{GraphCanvas, GraphState};
use toolbar::CanvasToolbar;

use crate::api::agents::{AgentSummary, AgentsApi};
use agent_selector::AgentSelectorBar;

#[component]
pub fn CanvasView() -> impl IntoView {
    view! { <RadialCanvasView /> }
}

/// New radial navigation canvas — wired in T22.
///
/// Architecture note: `Callback::new` in Leptos 0.8 requires `Send + Sync + 'static`, but WASM
/// is single-threaded and `Rc<RefCell<_>>` is not `Send`. To work around this, the `on_event`
/// closure captures only `Copy` reactive signals. Nav/prefetch mutations are driven by a
/// signal-based intent channel: `on_event` writes to `active_request` (a signal), and a
/// separate `Effect` reads it and performs the actual fetch + nav update using `Rc<RefCell<_>>`.
#[component]
fn RadialCanvasView() -> impl IntoView {
    let state = expect_context::<DashboardState>();

    // -----------------------------------------------------------------------
    // Agent selection signals
    // -----------------------------------------------------------------------
    // Placeholder is the literal "main" — must match server's DEFAULT_AGENT_ID.
    // If they ever diverge, the worst case is one extra graph fetch on mount,
    // because AgentsApi::list().default_id overrides this once it resolves.
    let agent_id = RwSignal::new("main".to_string());
    let agents = RwSignal::new(Vec::<AgentSummary>::new());
    let default_agent_id = RwSignal::new("main".to_string());
    let agents_loading = RwSignal::new(false);
    let agents_error = RwSignal::new(None::<String>);

    // Fetch the agent list once on mount, plus a reusable refresh callback.
    let fetch_agents = move || {
        agents_loading.set(true);
        agents_error.set(None);
        spawn_local(async move {
            match AgentsApi::list(&state).await {
                Ok(resp) => {
                    agents.set(resp.agents);
                    let new_default = resp.default_id;
                    default_agent_id.set(new_default.clone());
                    // Only override agent_id if it would actually change.
                    if agent_id.get_untracked() != new_default {
                        agent_id.set(new_default);
                    }
                    agents_loading.set(false);
                }
                Err(e) => {
                    agents_error.set(Some(e));
                    agents_loading.set(false);
                }
            }
        });
    };

    // Initial fetch — gated on WebSocket connection so AgentsApi::list
    // doesn't fire before the panel is connected. Re-runs when is_connected
    // flips to true (Leptos subscribes via state.is_connected.get()).
    Effect::new(move || {
        if !state.is_connected.get() {
            return;
        }
        fetch_agents();
    });

    // Reactive signals (all Copy — safe to capture in Callback::new closures)
    let (selected_node, set_selected_node) = signal(None::<String>);
    let (node_detail, set_node_detail) = signal(None::<NoteDetailResponse>);
    let (detail_content, set_detail_content) = signal(DetailContent::Closed);
    let (breadcrumb_entries, set_breadcrumb) = signal(Vec::<BreadcrumbEntry>::new());
    let search_query = RwSignal::new(String::new());
    let (fold_threshold, set_fold_threshold) = signal(12usize);

    // Raw-response snapshot for the current center. Set after a successful Effect-fetch
    // (or prefetch hit), cleared at the start of every Effect-fetch invocation.
    // Effect-refold reads this to perform local re-fold without a network round-trip.
    let last_response: RwSignal<Option<(String, GraphNeighborsResponse)>> = RwSignal::new(None);

    // Intent channel: on_event writes an id here, Effect picks it up and fetches.
    // Using RwSignal so both on_event (write) and Effect (read) can access it.
    let active_request: RwSignal<Option<String>> = RwSignal::new(None);

    // Hover prefetch intent channel — same pattern as active_request.
    // HoverNode event writes here; Effect 4 reads and fires the background fetch.
    let prefetch_request: RwSignal<Option<String>> = RwSignal::new(None);

    // Full-graph node cache — populated once on mount, used to compute the
    // ghost-dot ring of orphans (nodes outside the current connected component).
    let all_dtos: RwSignal<Vec<NoteNodeDto>> = RwSignal::new(Vec::new());

    // Non-reactive radial navigation state (Rc<RefCell<_>> — WASM single-thread safe)
    let nav = Rc::new(RefCell::new(NavController::new()));
    let prefetch = Rc::new(RefCell::new(PrefetchCache::new()));

    // Non-reactive 60fps canvas state
    let graph_state = Rc::new(RefCell::new(GraphState::new()));

    // Minimap state — rebuilt once on mount, repainted reactively on focus changes
    let minimap: Rc<RefCell<GlobalMiniMap>> = Rc::new(RefCell::new(GlobalMiniMap::empty(200.0)));
    let (focus_id, set_focus_id) = signal(None::<String>);
    let (focus_neighbors, set_focus_neighbors) = signal(Vec::<String>::new());
    let (visible_counts, set_visible_counts) = signal((0usize, 0usize));

    // -----------------------------------------------------------------------
    // Agent-switch reset Effect.
    // Subscribes to `agent_id`; on a real change (prev != current), wipes all
    // canvas view state so the new agent's graph renders from a clean slate.
    // The four graph-fetch Effects also subscribe to `agent_id` and re-fire
    // automatically — this Effect's only job is the reset.
    //
    // The closure returns the current `agent_id`, so the next invocation sees
    // it as `prev`. On first mount `prev == None` and the reset body is skipped
    // (avoids clearing empty state before Effect 1's initial fetch).
    // -----------------------------------------------------------------------
    let nav_reset = nav.clone();
    let gs_reset = graph_state.clone();
    let prefetch_reset = prefetch.clone();
    Effect::new(move |prev: Option<String>| {
        let current = agent_id.get();
        if let Some(p) = prev.as_ref() {
            if *p != current {
                // Reset reactive signals
                set_selected_node.set(None);
                set_node_detail.set(None);
                set_detail_content.set(DetailContent::Closed);
                set_breadcrumb.set(Vec::new());
                search_query.set(String::new());
                set_fold_threshold.set(12);
                set_focus_id.set(None);
                set_focus_neighbors.set(Vec::new());
                set_visible_counts.set((0, 0));
                last_response.set(None);
                prefetch_request.set(None);
                all_dtos.set(Vec::new());

                // Reset non-reactive state
                *nav_reset.borrow_mut() = NavController::new();
                *prefetch_reset.borrow_mut() = PrefetchCache::new();
                {
                    let mut gs = gs_reset.borrow_mut();
                    gs.nodes.clear();
                    gs.edges.clear();
                    gs.selected_node = None;
                    gs.viewport.offset.x = gs.viewport.width / 2.0;
                    gs.viewport.offset.y = gs.viewport.height / 2.0;
                    gs.viewport.scale = 1.0;
                    gs.drag_offset = (0.0, 0.0);
                }

                // Defensive clear: Effect-fetch (which subscribes to active_request)
                // would otherwise re-fire with the old agent's center id. Effect 1
                // re-runs automatically (it subscribes to agent_id) and will set
                // active_request to the new entry node.
                active_request.set(None);
            }
        }
        current
    });

    // -----------------------------------------------------------------------
    // Effect 1: initial mount — pick entry point and fetch first neighborhood
    // -----------------------------------------------------------------------
    let nav_init = nav.clone();
    let gs_init = graph_state.clone();
    let minimap_init = minimap.clone();
    let prefetch_init = prefetch.clone();
    Effect::new(move || {
        if !state.is_connected.get() {
            return;
        }
        let agent = agent_id.get();
        let nav_inner = nav_init.clone();
        let gs_inner = gs_init.clone();
        let minimap_inner = minimap_init.clone();
        let prefetch_inner = prefetch_init.clone();

        spawn_local(async move {
            let now_ms = now_ms();

            // Always fetch the full graph: needed for entry pick fallback AND
            // for the ghost-dot ring (orphans = all nodes - in-view nodes).
            let query_result = GraphApi::query(&state, &agent, 500, vec![]).await.ok();
            if let Some(ref r) = query_result {
                all_dtos.set(r.nodes.clone());
                let mm = GlobalMiniMap::build(&r.nodes, &r.edges, 200.0);
                *minimap_inner.borrow_mut() = mm;
            }

            // Entry point: localStorage "canvas_entry" → highest-degree node
            let entry_id: Option<String> = web_sys::window()
                .and_then(|w| w.local_storage().ok().flatten())
                .and_then(|ls| ls.get_item("canvas_entry").ok().flatten())
                .filter(|s| !s.is_empty());

            let entry_id = match entry_id {
                Some(id) => Some(id),
                None => query_result.as_ref().and_then(pick_highest_degree),
            };

            let Some(entry_id) = entry_id else { return };

            nav_inner.borrow_mut().enter(entry_id.clone(), now_ms);

            let threshold = fold_threshold.get_untracked();
            match GraphApi::neighbors(&state, &agent, &entry_id, 3, 200).await {
                Ok(resp) => {
                    let mut nbhd = to_neighborhood(&resp, now_ms, threshold);
                    let dtos = all_dtos.get_untracked();
                    populate_orphans(&mut nbhd, &dtos);
                    let name = nbhd.center.name.clone();
                    let one_hop_len = nbhd.one_hop.len();
                    let total_len = one_hop_len
                        + nbhd.clusters.iter().map(|c| c.member_ids.len()).sum::<usize>();
                    let neighbor_ids: Vec<String> =
                        nbhd.one_hop.iter().map(|n| n.id.clone()).collect();
                    seed_graph_state(&gs_inner, &nbhd, Some(entry_id.clone()));
                    nav_inner.borrow_mut().fulfilled(entry_id.clone(), name, nbhd);
                    prefetch_inner.borrow_mut().put(entry_id.clone(), resp.clone(), now_ms);
                    last_response.set(Some((entry_id.clone(), resp)));
                    active_request.set(Some(entry_id.clone()));
                    set_focus_id.set(Some(entry_id));
                    set_focus_neighbors.set(neighbor_ids);
                    set_visible_counts.set((one_hop_len, total_len));
                }
                Err(e) => {
                    web_sys::console::error_1(
                        &format!("RadialCanvasView: entry fetch failed: {e}").into(),
                    );
                }
            }
        });
    });

    // -----------------------------------------------------------------------
    // Effect-fetch: subscribes to `active_request` only.
    // Fired on center change (node click / search / breadcrumb). Network fetch
    // path; transitions to Loading then Active. Slider re-folds use Effect-refold.
    // -----------------------------------------------------------------------
    let nav_req = nav.clone();
    let prefetch_req = prefetch.clone();
    let gs_req = graph_state.clone();
    Effect::new(move || {
        let Some(id) = active_request.get() else { return };
        let now_ms = now_ms();
        let agent = agent_id.get();

        // Sync prelude — invalidate stale snapshot, enter Loading
        last_response.set(None);
        nav_req.borrow_mut().enter(id.clone(), now_ms);

        // Prefetch cache hit → fold + apply locally, no network
        let cached = prefetch_req.borrow().get(&id, now_ms).cloned();
        if let Some(raw) = cached {
            let threshold = fold_threshold.get_untracked();
            let mut nbhd = to_neighborhood(&raw, now_ms, threshold);
            let dtos = all_dtos.get_untracked();
            populate_orphans(&mut nbhd, &dtos);
            let name = nbhd.center.name.clone();
            let one_hop_len = nbhd.one_hop.len();
            let total_len = one_hop_len
                + nbhd.clusters.iter().map(|c| c.member_ids.len()).sum::<usize>();
            let neighbor_ids: Vec<String> =
                nbhd.one_hop.iter().map(|n| n.id.clone()).collect();
            seed_graph_state(&gs_req, &nbhd, Some(id.clone()));
            nav_req.borrow_mut().fulfilled(id.clone(), name, nbhd);
            last_response.set(Some((id.clone(), raw)));
            set_focus_id.set(Some(id));
            set_focus_neighbors.set(neighbor_ids);
            set_visible_counts.set((one_hop_len, total_len));
            return;
        }

        // Cache miss — fetch from network
        let nav_fetch = nav_req.clone();
        let gs_fetch = gs_req.clone();
        let prefetch_fetch = prefetch_req.clone();
        spawn_local(async move {
            match GraphApi::neighbors(&state, &agent, &id, 3, 200).await {
                Ok(resp) => {
                    let threshold = fold_threshold.get_untracked();
                    let mut nbhd = to_neighborhood(&resp, now_ms, threshold);
                    let dtos = all_dtos.get_untracked();
                    populate_orphans(&mut nbhd, &dtos);
                    let name = nbhd.center.name.clone();
                    let one_hop_len = nbhd.one_hop.len();
                    let total_len = one_hop_len
                        + nbhd.clusters.iter().map(|c| c.member_ids.len()).sum::<usize>();
                    let neighbor_ids: Vec<String> =
                        nbhd.one_hop.iter().map(|n| n.id.clone()).collect();
                    seed_graph_state(&gs_fetch, &nbhd, Some(id.clone()));
                    nav_fetch.borrow_mut().fulfilled(id.clone(), name, nbhd);
                    prefetch_fetch.borrow_mut().put(id.clone(), resp.clone(), now_ms);
                    last_response.set(Some((id.clone(), resp)));
                    set_focus_id.set(Some(id));
                    set_focus_neighbors.set(neighbor_ids);
                    set_visible_counts.set((one_hop_len, total_len));
                }
                Err(e) => {
                    nav_fetch.borrow_mut().fail(id.clone(), e.clone());
                    web_sys::console::error_1(
                        &format!("RadialCanvasView: neighbor fetch failed: {e}").into(),
                    );
                }
            }
        });
    });

    // -----------------------------------------------------------------------
    // Effect 3: node detail — fetch wiki/backlinks when selected_node changes
    // -----------------------------------------------------------------------
    Effect::new(move || {
        let node_id = selected_node.get();
        match node_id {
            Some(id) => {
                let agent = agent_id.get();
                spawn_local(async move {
                    match GraphApi::node_detail(&state, &agent, &id).await {
                        Ok(detail) => {
                            set_detail_content
                                .set(DetailContent::Node { detail: detail.clone() });
                            set_node_detail.set(Some(detail));
                        }
                        Err(e) => {
                            web_sys::console::error_1(
                                &format!("Failed to load node detail: {e}").into(),
                            );
                            set_node_detail.set(None);
                            set_detail_content.set(DetailContent::Closed);
                        }
                    }
                });
            }
            None => {
                set_node_detail.set(None);
                set_detail_content.set(DetailContent::Closed);
            }
        }
    });

    // -----------------------------------------------------------------------
    // Effect 4: hover prefetch — background-fetch when pointer dwells on a node
    // -----------------------------------------------------------------------
    let prefetch_e4 = prefetch.clone();
    Effect::new(move || {
        let Some(id) = prefetch_request.get() else { return };

        let now = now_ms();
        // Skip if already cached and not stale
        if prefetch_e4.borrow().has(&id, now) {
            return;
        }
        let agent = agent_id.get();

        let prefetch_inner = prefetch_e4.clone();
        spawn_local(async move {
            match GraphApi::neighbors(&state, &agent, &id, 3, 200).await {
                Ok(resp) => {
                    prefetch_inner.borrow_mut().put(id, resp, now);
                }
                Err(_) => {
                    // Prefetch failures are silently ignored — they will retry on next dwell
                }
            }
        });
    });

    // -----------------------------------------------------------------------
    // Effect-refold: subscribes to `fold_threshold` only.
    // Fired on slider drag. Locally re-folds the cached raw response and drives
    // an interruptible NavController.retarget tween. No network, no Loading frame.
    // -----------------------------------------------------------------------
    let nav_refold = nav.clone();
    let gs_refold = graph_state.clone();
    Effect::new(move || {
        let threshold = fold_threshold.get().clamp(1, 1000);

        // Snapshot last_response and active id without subscribing to them.
        let Some((cached_id, raw)) = last_response.get_untracked() else { return };
        if active_request.get_untracked().as_ref() != Some(&cached_id) {
            return; // race: slider fired during a center transition
        }

        let now = now_ms();
        let mut nbhd = to_neighborhood(&raw, now, threshold);
        let dtos = all_dtos.get_untracked();
        populate_orphans(&mut nbhd, &dtos);

        let one_hop_len = nbhd.one_hop.len();
        let total_len =
            one_hop_len + nbhd.clusters.iter().map(|c| c.member_ids.len()).sum::<usize>();
        let neighbor_ids: Vec<String> = nbhd.one_hop.iter().map(|n| n.id.clone()).collect();

        update_graph_state_nodes_only(&gs_refold, &nbhd);
        nav_refold
            .borrow_mut()
            .retarget(nbhd, now, RETARGET_DURATION_MS);

        set_focus_neighbors.set(neighbor_ids);
        set_visible_counts.set((one_hop_len, total_len));
    });

    // -----------------------------------------------------------------------
    // Canvas event handler — captures only Copy signals, safe for Callback::new
    // -----------------------------------------------------------------------
    let on_event = move |event: CanvasEvent| match event {
        CanvasEvent::SelectNode(id) => {
            set_selected_node.set(Some(id.clone()));
            active_request.set(Some(id));
        }
        CanvasEvent::DeselectNode => {
            set_selected_node.set(None);
        }
        CanvasEvent::EnterLocalView(id) => {
            set_breadcrumb.update(|entries| {
                entries.push(BreadcrumbEntry {
                    node_id: id.clone(),
                    node_name: id.clone(),
                });
            });
            // Drive the same fetch path via the intent signal
            active_request.set(Some(id));
        }
        CanvasEvent::HoverNode(hovered_id) => {
            // Hover prefetch: debounce then kick off a background fetch into the prefetch cache.
            // The debouncer lives inside CanvasInteractionState; we drive it here via now_ms().
            // on_pointer_move returns Some(PrefetchNeighbor(id)) when the threshold is met.
            let now = now_ms();
            let intent = {
                // We can't capture `interaction` (Rc<RefCell<_>>) in a Callback::new (Send+Sync),
                // so we keep a local RwSignal<Option<String>> as the intent channel for prefetch.
                // Writes here; an Effect below reads and fires the async fetch.
                prefetch_request.set(hovered_id)
            };
            let _ = intent;
            let _ = now;
        }
        _ => {}
    };

    let on_search = move |query: String| {
        if query.is_empty() {
            return;
        }
        let agent = agent_id.get();
        spawn_local(async move {
            match GraphApi::search(&state, &agent, &query, 20).await {
                Ok(response) => {
                    if let Some(first) = response.results.first() {
                        let id = first.id.clone();
                        let name = first.name.clone();
                        set_breadcrumb.set(vec![BreadcrumbEntry {
                            node_id: id.clone(),
                            node_name: name,
                        }]);
                        // Trigger neighborhood fetch via the intent channel.
                        active_request.set(Some(id));
                    }
                }
                Err(e) => {
                    web_sys::console::error_1(&format!("Search failed: {e}").into());
                }
            }
        });
    };

    let on_breadcrumb_navigate = move |target: Option<String>| match target {
        None => {
            set_breadcrumb.set(vec![]);
            set_selected_node.set(None);
        }
        Some(id) => {
            set_breadcrumb.update(|entries| {
                if let Some(pos) = entries.iter().position(|e| e.node_id == id) {
                    entries.truncate(pos + 1);
                }
            });
            set_selected_node.set(None);
            // Re-center the radial neighborhood on the breadcrumb target.
            active_request.set(Some(id));
        }
    };

    let has_breadcrumb = move || !breadcrumb_entries.get().is_empty();
    let has_detail = move || node_detail.get().is_some();

    view! {
        <div class="flex flex-col h-full">
            <AgentSelectorBar
                agent_id=agent_id
                agents=agents
                default_agent_id=default_agent_id
                loading=agents_loading
                error=agents_error
                on_refresh=fetch_agents
            />

            <CanvasToolbar
                search_query=search_query
                on_search=on_search
                fold_threshold=fold_threshold
                set_fold_threshold=set_fold_threshold
                visible_counts=visible_counts
            />

            {move || has_breadcrumb().then(|| view! {
                <Breadcrumb
                    entries=breadcrumb_entries
                    on_navigate=on_breadcrumb_navigate
                />
            })}

            <div class="flex flex-1 overflow-hidden">
                <div class="flex-1 relative bg-[#0a0a0f]">
                    <GraphCanvas
                        graph_state=graph_state.clone()
                        on_event=Callback::new(on_event)
                        nav=nav.clone()
                    />
                    {
                        #[cfg(target_arch = "wasm32")]
                        {
                            view! {
                                <MiniMapOverlay
                                    minimap=minimap.clone()
                                    focus_id=focus_id
                                    focus_neighbor_ids=focus_neighbors
                                    on_pick=move |id: String| {
                                        set_selected_node.set(Some(id.clone()));
                                        active_request.set(Some(id));
                                    }
                                />
                            }
                        }
                        #[cfg(not(target_arch = "wasm32"))]
                        { () }
                    }
                </div>

                {move || has_detail().then(|| view! {
                    <DetailPanel
                        content=detail_content
                        on_jump_to=Callback::new(move |id: String| {
                            set_selected_node.set(Some(id));
                        })
                    />
                })}
            </div>
        </div>
    }
}

/// Pick the most-connected node from a `GraphQueryResponse` as the radial entry point.
///
/// Falls back to the first node if the response contains no edges, so an isolated
/// vault still renders at least one center node.
fn pick_highest_degree(resp: &GraphQueryResponse) -> Option<String> {
    use std::collections::HashMap;
    let mut degree: HashMap<&str, usize> = HashMap::new();
    for e in &resp.edges {
        *degree.entry(e.from.as_str()).or_insert(0) += 1;
        *degree.entry(e.to.as_str()).or_insert(0) += 1;
    }
    resp.nodes
        .iter()
        .max_by_key(|n| degree.get(n.id.as_str()).copied().unwrap_or(0))
        .map(|n| n.id.clone())
}

// ---------------------------------------------------------------------------
// Private helpers shared by RadialCanvasView Effects
// ---------------------------------------------------------------------------

/// Returns `performance.now()` in milliseconds, falling back to 0.0.
fn now_ms() -> f64 {
    web_sys::window()
        .and_then(|w| w.performance())
        .map(|p| p.now())
        .unwrap_or(0.0)
}

/// Flatten a `Neighborhood` into `GraphState.nodes/edges` and wake the physics engine.
///
/// Also recenters the viewport so the radial layout's world origin (where the
/// active center sits) maps to the canvas geometric center. Without this,
/// switching focus could leave the new center anywhere on screen depending on
/// previous pan/zoom state.
fn seed_graph_state(
    gs: &Rc<RefCell<GraphState>>,
    nbhd: &crate::canvas_engine::types::Neighborhood,
    selected: Option<String>,
) {
    let nodes: Vec<_> = std::iter::once(nbhd.center.clone())
        .chain(nbhd.one_hop.iter().cloned())
        .chain(nbhd.two_hop.iter().cloned())
        // Orphans land here too so the click hit-test (which iterates gs.nodes)
        // can resolve a ghost-dot click into a re-center request.
        .chain(nbhd.orphans.iter().cloned())
        .collect();
    let edges = nbhd.edges.clone();
    let mut gs = gs.borrow_mut();
    gs.nodes = nodes;
    gs.edges = edges;
    gs.layout.wake();
    gs.selected_node = selected;
    // Recenter: world (0,0) → canvas center, reset zoom + drag
    gs.viewport.offset.x = gs.viewport.width / 2.0;
    gs.viewport.offset.y = gs.viewport.height / 2.0;
    gs.viewport.scale = 1.0;
    gs.drag_offset = (0.0, 0.0);
}

/// Refresh GraphState's node/edge buffers from a freshly folded `Neighborhood`
/// without resetting viewport, scale, drag offset, selected node, or layout.
/// Used by the slider re-fold path so the user's pan/zoom/drag survives a slider tick.
fn update_graph_state_nodes_only(
    gs: &Rc<RefCell<GraphState>>,
    nbhd: &crate::canvas_engine::types::Neighborhood,
) {
    let nodes: Vec<_> = std::iter::once(nbhd.center.clone())
        .chain(nbhd.one_hop.iter().cloned())
        .chain(nbhd.two_hop.iter().cloned())
        .chain(nbhd.orphans.iter().cloned())
        .collect();
    let edges = nbhd.edges.clone();
    let mut gs = gs.borrow_mut();
    gs.nodes = nodes;
    gs.edges = edges;
    // Intentionally NOT modified: viewport.{offset,scale}, drag_offset,
    // selected_node, layout (no wake — radial uses target_positions, not physics).
}

