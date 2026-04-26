mod breadcrumb;
mod detail_panel;
mod graph_canvas;
mod toolbar;

use std::cell::RefCell;
use std::rc::Rc;

use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::api::graph::GraphApi;
use crate::canvas_engine::adapter::{
    adapt_graph_response, populate_orphans, to_neighborhood, GraphNeighborsResponse,
    GraphQueryResponse, NoteDetailResponse, NoteNodeDto,
};
use crate::canvas_engine::interaction::CanvasEvent;
use crate::canvas_engine::mini_map::MiniMap;
use crate::canvas_engine::navigation::NavController;
use crate::canvas_engine::prefetch::PrefetchCache;
use crate::canvas_engine::types::{BreadcrumbEntry, NavState, ViewMode};
use detail_panel::DetailContent;
use leptos::callback::Callback;

use crate::context::DashboardState;

use breadcrumb::Breadcrumb;
use detail_panel::DetailPanel;
use graph_canvas::{GraphCanvas, GraphState};
use toolbar::CanvasToolbar;

/// Top-level dispatcher: reads the reactive feature flag from `DashboardState`
/// and routes to the appropriate canvas view. The flag is initialized from
/// localStorage in `DashboardState::new()` and mutated by the Settings panel.
#[component]
pub fn CanvasView() -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let radial_enabled = state.canvas_radial_navigation;

    view! {
        {move || if radial_enabled.get() {
            view! { <RadialCanvasView /> }.into_any()
        } else {
            view! { <LegacyCanvasView /> }.into_any()
        }}
    }
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

    // Reactive signals (all Copy — safe to capture in Callback::new closures)
    let (view_mode, set_view_mode) = signal(ViewMode::Global { top_k: 100 });
    let (selected_node, set_selected_node) = signal(None::<String>);
    let (node_detail, set_node_detail) = signal(None::<NoteDetailResponse>);
    let (detail_content, set_detail_content) = signal(DetailContent::Closed);
    let (breadcrumb_entries, set_breadcrumb) = signal(Vec::<BreadcrumbEntry>::new());
    let search_query = RwSignal::new(String::new());
    let (fold_threshold, set_fold_threshold) = signal(12usize);

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
    let _minimap = Rc::new(RefCell::new(MiniMap::empty()));

    // Non-reactive 60fps canvas state
    let graph_state = Rc::new(RefCell::new(GraphState::new()));

    // -----------------------------------------------------------------------
    // Effect 1: initial mount — pick entry point and fetch first neighborhood
    // -----------------------------------------------------------------------
    let nav_init = nav.clone();
    let gs_init = graph_state.clone();
    Effect::new(move || {
        if !state.is_connected.get() {
            return;
        }
        let nav_inner = nav_init.clone();
        let gs_inner = gs_init.clone();

        spawn_local(async move {
            let now_ms = now_ms();

            // Always fetch the full graph: needed for entry pick fallback AND
            // for the ghost-dot ring (orphans = all nodes - in-view nodes).
            let query_result = GraphApi::query(&state, 500, vec![]).await.ok();
            if let Some(ref r) = query_result {
                all_dtos.set(r.nodes.clone());
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
            match GraphApi::neighbors(&state, &entry_id, 3, 200).await {
                Ok(resp) => {
                    let mut nbhd = to_neighborhood(&resp, now_ms, threshold);
                    let dtos = all_dtos.get_untracked();
                    populate_orphans(&mut nbhd, &dtos);
                    let name = nbhd.center.name.clone();
                    seed_graph_state(&gs_inner, &nbhd, Some(entry_id.clone()));
                    nav_inner.borrow_mut().fulfilled(entry_id, name, nbhd);
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
    // Effect 2: active_request — fetch neighborhood when user clicks a node
    // -----------------------------------------------------------------------
    let nav_req = nav.clone();
    let prefetch_req = prefetch.clone();
    let gs_req = graph_state.clone();
    Effect::new(move || {
        let Some(id) = active_request.get() else { return };
        // Subscribe to fold_threshold so the slider re-fires this Effect.
        let threshold = fold_threshold.get();

        let now_ms = now_ms();

        // Prefetch cache hit → apply immediately without fetch
        let cached = prefetch_req.borrow().get(&id, threshold, now_ms).cloned();
        if let Some(nbhd) = cached {
            let name = nbhd.center.name.clone();
            seed_graph_state(&gs_req, &nbhd, Some(id.clone()));
            nav_req.borrow_mut().fulfilled(id, name, nbhd);
            return;
        }

        // Cache miss — transition to Loading, then fetch
        nav_req.borrow_mut().enter(id.clone(), now_ms);

        let nav_fetch = nav_req.clone();
        let gs_fetch = gs_req.clone();
        spawn_local(async move {
            match GraphApi::neighbors(&state, &id, 3, 200).await {
                Ok(resp) => {
                    let mut nbhd = to_neighborhood(&resp, now_ms, threshold);
                    let dtos = all_dtos.get_untracked();
                    populate_orphans(&mut nbhd, &dtos);
                    let name = nbhd.center.name.clone();
                    seed_graph_state(&gs_fetch, &nbhd, Some(id.clone()));
                    nav_fetch.borrow_mut().fulfilled(id, name, nbhd);
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
                spawn_local(async move {
                    match GraphApi::node_detail(&state, &id).await {
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
        let threshold = fold_threshold.get_untracked();
        // Skip if already cached and not stale
        if prefetch_e4.borrow().get(&id, threshold, now).is_some() {
            return;
        }

        let prefetch_inner = prefetch_e4.clone();
        spawn_local(async move {
            match GraphApi::neighbors(&state, &id, 3, 200).await {
                Ok(resp) => {
                    let mut nbhd = to_neighborhood(&resp, now, threshold);
                    let dtos = all_dtos.get_untracked();
                    populate_orphans(&mut nbhd, &dtos);
                    prefetch_inner.borrow_mut().put(id, threshold, nbhd);
                }
                Err(_) => {
                    // Prefetch failures are silently ignored — they will retry on next dwell
                }
            }
        });
    });

    // -----------------------------------------------------------------------
    // Effect 5: fold_threshold change → bust cache, re-issue active_request
    // so the current neighborhood is refetched and re-folded with the new value.
    // -----------------------------------------------------------------------
    let nav_thresh = nav.clone();
    Effect::new(move || {
        // Subscribe to the slider — must be the first reactive read in this Effect.
        let _threshold = fold_threshold.get();

        // Identify the node currently in focus. Idle/Loading/Error states have
        // no center yet, so we just no-op until something is on screen.
        let center_id = match &nav_thresh.borrow().state {
            NavState::Active { node_id, .. } => Some(node_id.clone()),
            NavState::Animating { to_id, .. } => Some(to_id.clone()),
            _ => None,
        };
        let Some(id) = center_id else { return };

        // (cache key now includes threshold; Effect 5 itself will be removed in Task 8)

        // Force Effect 2 to re-fire even if active_request already holds `id`:
        // RwSignal dedupes by equality, so we round-trip via None.
        active_request.set(None);
        active_request.set(Some(id));
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
            set_view_mode.set(ViewMode::Local {
                center_node_id: id.clone(),
                depth: 2,
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

    let on_toggle_mode = move || match view_mode.get() {
        ViewMode::Global { .. } => {}
        ViewMode::Local { .. } => {
            set_view_mode.set(ViewMode::Global { top_k: 100 });
            set_breadcrumb.set(vec![]);
            set_selected_node.set(None);
        }
    };

    let on_search = move |query: String| {
        if query.is_empty() {
            return;
        }
        spawn_local(async move {
            match GraphApi::search(&state, &query, 20).await {
                Ok(response) => {
                    if let Some(first) = response.results.first() {
                        let id = first.id.clone();
                        let name = first.name.clone();
                        set_breadcrumb.set(vec![BreadcrumbEntry {
                            node_id: id.clone(),
                            node_name: name,
                        }]);
                        set_view_mode.set(ViewMode::Local {
                            center_node_id: id.clone(),
                            depth: 2,
                        });
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
            set_view_mode.set(ViewMode::Global { top_k: 100 });
            set_breadcrumb.set(vec![]);
            set_selected_node.set(None);
        }
        Some(id) => {
            set_breadcrumb.update(|entries| {
                if let Some(pos) = entries.iter().position(|e| e.node_id == id) {
                    entries.truncate(pos + 1);
                }
            });
            set_view_mode.set(ViewMode::Local {
                center_node_id: id.clone(),
                depth: 2,
            });
            set_selected_node.set(None);
            // Re-center the radial neighborhood on the breadcrumb target.
            active_request.set(Some(id));
        }
    };

    let is_local = move || matches!(view_mode.get(), ViewMode::Local { .. });
    let has_detail = move || node_detail.get().is_some();

    view! {
        <div class="flex flex-col h-full">
            <CanvasToolbar
                view_mode=view_mode
                search_query=search_query
                on_toggle_mode=on_toggle_mode
                on_search=on_search
                fold_threshold=fold_threshold
                set_fold_threshold=set_fold_threshold
                is_radial=true
            />

            {move || is_local().then(|| view! {
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

/// Legacy canvas view — unchanged from pre-T22, kept as fallback when feature flag is off.
#[component]
fn LegacyCanvasView() -> impl IntoView {
    let state = expect_context::<DashboardState>();

    // Reactive signals
    let (view_mode, set_view_mode) = signal(ViewMode::Global { top_k: 100 });
    let (selected_node, set_selected_node) = signal(None::<String>);
    let (node_detail, set_node_detail) = signal(None::<NoteDetailResponse>);
    let (detail_content, set_detail_content) = signal(DetailContent::Closed);
    let (breadcrumb_entries, set_breadcrumb) = signal(Vec::<BreadcrumbEntry>::new());
    let search_query = RwSignal::new(String::new());
    // fold_threshold controls cluster folding granularity (6..=20); wired fully in T22
    let (fold_threshold, set_fold_threshold) = signal(12usize);

    // Non-reactive 60fps state
    let graph_state = Rc::new(RefCell::new(GraphState::new()));

    // Load data when connected or view mode changes
    let gs_load = graph_state.clone();
    Effect::new(move || {
        if !state.is_connected.get() {
            return;
        }
        let mode = view_mode.get();
        let gs = gs_load.clone();

        spawn_local(async move {
            // Fetch graph data; normalize GraphNeighborsResponse into GraphQueryResponse
            // so the existing adapt_graph_response path works until Task 11 replaces it.
            let result: Result<GraphQueryResponse, String> = match &mode {
                ViewMode::Global { top_k } => GraphApi::query(&state, *top_k, vec![]).await,
                ViewMode::Local {
                    center_node_id,
                    depth,
                } => GraphApi::neighbors(&state, center_node_id, *depth, 200)
                    .await
                    .map(|r: GraphNeighborsResponse| {
                        // Merge center into nodes so the flat adapter sees all nodes.
                        let mut nodes = vec![r.center];
                        nodes.extend(r.nodes);
                        GraphQueryResponse { nodes, edges: r.edges }
                    }),
            };

            match result {
                Ok(response) => {
                    let (nodes, edges) = adapt_graph_response(&response);
                    let mut gs = gs.borrow_mut();
                    gs.nodes = nodes;
                    gs.edges = edges;
                    gs.layout.wake();
                    gs.selected_node = None;
                    gs.hovered_node = None;
                }
                Err(e) => {
                    web_sys::console::error_1(&format!("Failed to load graph: {}", e).into());
                }
            }
        });
    });

    // Fetch node detail when selection changes
    Effect::new(move || {
        let node_id = selected_node.get();
        match node_id {
            Some(id) => {
                spawn_local(async move {
                    match GraphApi::node_detail(&state, &id).await {
                        Ok(detail) => {
                            set_detail_content.set(DetailContent::Node { detail: detail.clone() });
                            set_node_detail.set(Some(detail));
                        }
                        Err(e) => {
                            web_sys::console::error_1(
                                &format!("Failed to load node detail: {}", e).into(),
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

    // Callbacks — must be Send+Sync for Callback::new, so no Rc<RefCell<_>> capture.
    let on_event = move |event: CanvasEvent| match event {
        CanvasEvent::SelectNode(id) => {
            set_selected_node.set(Some(id));
        }
        CanvasEvent::DeselectNode => {
            set_selected_node.set(None);
        }
        CanvasEvent::EnterLocalView(id) => {
            // Push breadcrumb (use id as display name; detail fetch will supply the real name)
            set_breadcrumb.update(|entries| {
                entries.push(BreadcrumbEntry {
                    node_id: id.clone(),
                    node_name: id.clone(),
                });
            });
            set_view_mode.set(ViewMode::Local {
                center_node_id: id,
                depth: 2,
            });
            set_selected_node.set(None);
        }
        CanvasEvent::HoverNode(_) => {
            // Hover is handled in GraphState directly
        }
        _ => {}
    };

    let on_toggle_mode = move || {
        match view_mode.get() {
            ViewMode::Global { .. } => {
                // To enter Local view we need a center. Use the currently selected
                // node if there is one; otherwise leave the view alone — the user
                // has nothing to focus on.
                if let Some(id) = selected_node.get() {
                    set_breadcrumb.set(vec![BreadcrumbEntry {
                        node_id: id.clone(),
                        node_name: id.clone(),
                    }]);
                    set_view_mode.set(ViewMode::Local {
                        center_node_id: id,
                        depth: 2,
                    });
                } else {
                    web_sys::console::warn_1(
                        &"Click a node first to enter Local view".into(),
                    );
                }
            }
            ViewMode::Local { .. } => {
                set_view_mode.set(ViewMode::Global { top_k: 100 });
                set_breadcrumb.set(vec![]);
                set_selected_node.set(None);
            }
        }
    };

    let on_search = move |query: String| {
        if query.is_empty() {
            return;
        }
        spawn_local(async move {
            match GraphApi::search(&state, &query, 20).await {
                Ok(response) => {
                    if let Some(first) = response.results.first() {
                        let id = first.id.clone();
                        let name = first.name.clone();
                        set_breadcrumb.set(vec![BreadcrumbEntry {
                            node_id: id.clone(),
                            node_name: name,
                        }]);
                        set_view_mode.set(ViewMode::Local {
                            center_node_id: id,
                            depth: 2,
                        });
                    }
                }
                Err(e) => {
                    web_sys::console::error_1(&format!("Search failed: {}", e).into());
                }
            }
        });
    };

    let on_breadcrumb_navigate = move |target: Option<String>| match target {
        None => {
            // Go back to global
            set_view_mode.set(ViewMode::Global { top_k: 100 });
            set_breadcrumb.set(vec![]);
            set_selected_node.set(None);
        }
        Some(id) => {
            // Truncate breadcrumb to the navigated node
            set_breadcrumb.update(|entries| {
                if let Some(pos) = entries.iter().position(|e| e.node_id == id) {
                    entries.truncate(pos + 1);
                }
            });
            set_view_mode.set(ViewMode::Local {
                center_node_id: id,
                depth: 2,
            });
            set_selected_node.set(None);
        }
    };

    let is_local = move || matches!(view_mode.get(), ViewMode::Local { .. });
    let has_detail = move || node_detail.get().is_some();

    view! {
        <div class="flex flex-col h-full">
            <CanvasToolbar
                view_mode=view_mode
                search_query=search_query
                on_toggle_mode=on_toggle_mode
                on_search=on_search
                fold_threshold=fold_threshold
                set_fold_threshold=set_fold_threshold
            />

            {move || is_local().then(|| view! {
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
                    />
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
