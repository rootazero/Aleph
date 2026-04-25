mod breadcrumb;
mod detail_panel;
mod graph_canvas;
mod toolbar;

use std::cell::RefCell;
use std::rc::Rc;

use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::api::graph::GraphApi;
use crate::api::settings::UserPrefs;
use crate::canvas_engine::adapter::{
    adapt_graph_response, to_neighborhood, GraphNeighborsResponse, GraphQueryResponse,
    NoteDetailResponse,
};
use crate::canvas_engine::interaction::CanvasEvent;
use crate::canvas_engine::mini_map::MiniMap;
use crate::canvas_engine::navigation::NavController;
use crate::canvas_engine::prefetch::PrefetchCache;
use crate::canvas_engine::types::{BreadcrumbEntry, ViewMode};
use detail_panel::DetailContent;
use leptos::callback::Callback;

use crate::context::DashboardState;

use breadcrumb::Breadcrumb;
use detail_panel::DetailPanel;
use graph_canvas::{GraphCanvas, GraphState};
use toolbar::CanvasToolbar;

/// Top-level dispatcher: reads the feature flag and routes to the appropriate canvas view.
#[component]
pub fn CanvasView() -> impl IntoView {
    // Read feature flag from UserPrefs (defaults to false until loaded from server).
    // T21 added the struct; we initialize from Default for now.
    let prefs = UserPrefs::default();
    if prefs.canvas_radial_navigation {
        view! { <RadialCanvasView /> }.into_any()
    } else {
        view! { <LegacyCanvasView /> }.into_any()
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

            // Entry point: localStorage "canvas_entry" → graph.query top-1
            let entry_id: Option<String> = web_sys::window()
                .and_then(|w| w.local_storage().ok().flatten())
                .and_then(|ls| ls.get_item("canvas_entry").ok().flatten())
                .filter(|s| !s.is_empty());

            let entry_id = match entry_id {
                Some(id) => Some(id),
                None => GraphApi::query(&state, 1, vec![])
                    .await
                    .ok()
                    .and_then(|r| r.nodes.into_iter().next())
                    .map(|n| n.id),
            };

            let Some(entry_id) = entry_id else { return };

            nav_inner.borrow_mut().enter(entry_id.clone(), now_ms);

            match GraphApi::neighbors(&state, &entry_id, 2, 50).await {
                Ok(resp) => {
                    let nbhd = to_neighborhood(&resp, now_ms);
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

        let now_ms = now_ms();

        // Prefetch cache hit → apply immediately without fetch
        let cached = prefetch_req.borrow().get(&id, now_ms).cloned();
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
            match GraphApi::neighbors(&state, &id, 2, 50).await {
                Ok(resp) => {
                    let nbhd = to_neighborhood(&resp, now_ms);
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
        // Skip if already cached and not stale
        if prefetch_e4.borrow().get(&id, now).is_some() {
            return;
        }

        let prefetch_inner = prefetch_e4.clone();
        spawn_local(async move {
            match GraphApi::neighbors(&state, &id, 2, 50).await {
                Ok(resp) => {
                    let nbhd = to_neighborhood(&resp, now);
                    prefetch_inner.borrow_mut().put(id, nbhd);
                }
                Err(_) => {
                    // Prefetch failures are silently ignored — they will retry on next dwell
                }
            }
        });
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
                            center_node_id: id,
                            depth: 2,
                        });
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
fn seed_graph_state(
    gs: &Rc<RefCell<GraphState>>,
    nbhd: &crate::canvas_engine::types::Neighborhood,
    selected: Option<String>,
) {
    let nodes: Vec<_> = std::iter::once(nbhd.center.clone())
        .chain(nbhd.one_hop.iter().cloned())
        .chain(nbhd.two_hop.iter().cloned())
        .collect();
    let edges = nbhd.edges.clone();
    let mut gs = gs.borrow_mut();
    gs.nodes = nodes;
    gs.edges = edges;
    gs.layout.wake();
    gs.selected_node = selected;
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
        let current = view_mode.get();
        match current {
            ViewMode::Global { .. } => {
                // Stay in global (no-op unless there's a selected node)
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
