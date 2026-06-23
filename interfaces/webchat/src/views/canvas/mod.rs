mod edge_label;
mod galaxy_canvas;
pub mod gl;
mod graph_canvas;
#[cfg(target_arch = "wasm32")]
mod minimap_view;
pub mod node_card;
mod node_detail_panel;

pub use node_detail_panel::{NodeDetailPanel, NodeExcerpt};

use crate::canvas_engine::mini_map::GlobalMiniMap;
#[cfg(target_arch = "wasm32")]
use minimap_view::MiniMapOverlay;

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::api::graph::GraphApi;
use crate::canvas_engine::adapter::{
    populate_orphans, to_neighborhood, GraphNeighborsResponse, GraphQueryResponse,
    NoteDetailResponse, NoteNodeDto,
};
use crate::canvas_engine::interaction::CanvasEvent;
use crate::canvas_engine::markdown_excerpt::render_excerpt;
use crate::canvas_engine::navigation::{NavController, RETARGET_DURATION_MS};
use crate::canvas_engine::prefetch::{
    try_acquire_in_flight, InFlightSet, PrefetchCache, HOVER_DEBOUNCE_MS, MAX_IN_FLIGHT,
};
use gloo_timers::callback::Timeout;
use leptos::callback::Callback;

use crate::context::DashboardState;
use crate::state::memory::MemoryState;

#[allow(unused_imports)]
use graph_canvas::{GraphCanvas, GraphState};
use galaxy_canvas::GalaxyCanvas;

use crate::api::agents::AgentsApi;

#[component]
#[must_use]
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
    let mem = expect_context::<MemoryState>();

    // Derive agent_id from MemoryState so the sidebar's agent selector drives
    // the canvas. Local alias for readability in the Effects below.
    let agent_id = mem.agent_id;

    // Fetch the agent list once on mount, writing results into MemoryState so
    // MemorySidebar's <select> can render them. Uses a reusable closure for
    // retry-safe reconnect behaviour.
    let fetch_agents = move || {
        let state = state;
        spawn_local(async move {
            // Retry up to 3 times with 500ms delay to handle transient "Not connected"
            // errors that occur when the WebSocket is still initializing.
            let mut retries = 0;
            const MAX_RETRIES: u32 = 3;
            const RETRY_DELAY_MS: u32 = 500;

            loop {
                match AgentsApi::list(&state).await {
                    Ok(resp) => {
                        mem.agents.set(resp.agents);
                        let new_default = resp.default_id;
                        // Only override agent_id if it would actually change.
                        if mem.agent_id.get_untracked() != new_default {
                            mem.agent_id.set(new_default);
                        }
                        break;
                    }
                    Err(e) => {
                        if e.contains("Not connected") && retries < MAX_RETRIES {
                            retries += 1;
                            web_sys::console::log_1(
                                &format!(
                                    "Agents list not connected, retrying {retries}/{MAX_RETRIES} in {RETRY_DELAY_MS}ms..."
                                )
                                .into(),
                            );
                            gloo_timers::future::TimeoutFuture::new(RETRY_DELAY_MS).await;
                            continue;
                        }
                        web_sys::console::error_1(&format!("Agents list failed: {e}").into());
                        break;
                    }
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
    // selected_node, search_query, fold_threshold are sourced from MemoryState so
    // the sidebar and canvas share the same values.
    let selected_node = mem.selected_node;
    let set_selected_node = mem.selected_node;
    let search_query = mem.search_query;
    let fold_threshold = mem.fold_threshold;
    let set_fold_threshold = mem.fold_threshold;
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

    // 3D galaxy data built from the full graph.query response via force-layout seed.
    let galaxy_data: RwSignal<Option<gl::GraphData>> = RwSignal::new(None);

    // Non-reactive radial navigation state (Rc<RefCell<_>> — WASM single-thread safe)
    let nav = Rc::new(RefCell::new(NavController::new()));
    let prefetch = Rc::new(RefCell::new(PrefetchCache::<GraphNeighborsResponse>::new()));

    // Bounded in-flight tracker: dedup per-id + ≤MAX_IN_FLIGHT concurrent RPCs.
    // Without this, a hover-skim across 50 nodes spawned 50 parallel
    // `graph.neighbors` calls (one per pointer frame).
    let in_flight = Rc::new(RefCell::new(InFlightSet::new(MAX_IN_FLIGHT)));

    // Raw hover intent — written by the (Send+Sync) on_event callback on every
    // HoverNode transition. A separate Effect (below) reads this, manages a
    // 150ms gloo Timeout, and only writes `prefetch_request` once the dwell
    // threshold is met. Stored as RwSignal so on_event (write) and the Effect
    // (read) can both reach it without sharing a non-Send Rc.
    let hover_intent: RwSignal<Option<String>> = RwSignal::new(None);

    // Outstanding hover debounce timer. Cleared / replaced inside the hover
    // Effect; `None` means no pending dwell. Held in an Rc so the Effect
    // closure can cancel-on-replace across runs.
    let pending_hover_timer: Rc<RefCell<Option<Timeout>>> = Rc::new(RefCell::new(None));

    // Non-reactive detail-response cache; keyed by node id.
    let detail_cache: Rc<RefCell<PrefetchCache<NoteDetailResponse>>> =
        Rc::new(RefCell::new(PrefetchCache::<NoteDetailResponse>::new()));

    // Reactive map of node id → pre-rendered excerpt HTML.
    // Populated lazily when a card enters FULL mode (hover or select).
    let excerpt_by_id: RwSignal<HashMap<String, String>> = RwSignal::new(HashMap::new());

    // -----------------------------------------------------------------------
    // Effect: lazy-fetch note detail for cards that need a FULL-mode excerpt.
    //
    // BUG FIX (canvas hover markdown): this Effect previously resolved a single
    // `target = selected_node.or_else(prefetch_request)`. Once any node was
    // selected (the center stays selected after a click/navigation),
    // `selected_node` is `Some` indefinitely and permanently SHADOWED the
    // hover-driven `prefetch_request` — so hovering a neighbor while a node was
    // focused never fetched that neighbor's excerpt, and its FULL card rendered
    // title-only ("markdown often doesn't show on hover").
    //
    // Fix: subscribe to BOTH signals and fetch the *union* of {selected, hovered}
    // (de-duplicated). Each fetch is idempotent — guarded by the raw-response
    // cache and the rendered-excerpt map — so spawning per target is cheap.
    // -----------------------------------------------------------------------
    let detail_cache_eff = detail_cache.clone();
    Effect::new(move || {
        // Read both signals unconditionally so the Effect subscribes to each;
        // neither must shadow the other.
        let selected = selected_node.get();
        let hovered = prefetch_request.get();

        // De-duplicate: hovering the already-selected node must not fan out
        // into two identical RPCs (the async fetch hasn't populated the cache
        // yet, so the per-id guards below can't catch the same-frame dup).
        let mut targets: Vec<String> = Vec::with_capacity(2);
        if let Some(s) = selected {
            targets.push(s);
        }
        if let Some(h) = hovered {
            if !targets.contains(&h) {
                targets.push(h);
            }
        }

        let now = now_ms();
        for id in targets {
            // Already in raw-response cache — skip if content is fresh.
            if detail_cache_eff.borrow().has(&id, now) {
                continue;
            }
            // Already rendered — nothing to do.
            if excerpt_by_id.with(|m| m.contains_key(&id)) {
                continue;
            }

            let detail_cache_spawn = detail_cache_eff.clone();
            let agent = agent_id.get_untracked();
            let id_spawn = id.clone();

            spawn_local(async move {
                match GraphApi::node_detail(&state, &agent, &id_spawn).await {
                    Ok(detail) => {
                        let html = render_excerpt(&detail.content);
                        excerpt_by_id.update(|m| {
                            m.insert(id_spawn.clone(), html);
                        });
                        let now = now_ms();
                        detail_cache_spawn.borrow_mut().put(id_spawn, detail, now);
                    }
                    Err(e) => {
                        web_sys::console::warn_1(
                            &format!("canvas: note_detail fetch failed for {id_spawn}: {e}").into(),
                        );
                    }
                }
            });
        }
    });

    // Non-reactive 60fps canvas state
    let graph_state = Rc::new(RefCell::new(GraphState::new()));

    // Minimap state — rebuilt once on mount, repainted reactively on focus changes
    let minimap: Rc<RefCell<GlobalMiniMap>> = Rc::new(RefCell::new(GlobalMiniMap::empty(200.0)));
    let (focus_id, set_focus_id) = signal(None::<String>);
    let (focus_neighbors, set_focus_neighbors) = signal(Vec::<String>::new());
    // `view!` consumes these signals only when targeting WASM; silence the
    // unused-variable warning on the native (cargo check) target.
    #[cfg(not(target_arch = "wasm32"))]
    let _ = (focus_id, focus_neighbors);
    let (_visible_counts, set_visible_counts) = signal((0usize, 0usize));

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
    let detail_cache_reset = detail_cache;
    let in_flight_reset = in_flight.clone();
    let pending_hover_timer_reset = pending_hover_timer.clone();
    Effect::new(move |prev: Option<String>| {
        let current = agent_id.get();
        if let Some(p) = prev.as_ref() {
            if *p != current {
                // Reset reactive signals
                set_selected_node.set(None);
                search_query.set(String::new());
                set_fold_threshold.set(12);
                set_focus_id.set(None);
                set_focus_neighbors.set(Vec::new());
                set_visible_counts.set((0, 0));
                last_response.set(None);
                prefetch_request.set(None);
                all_dtos.set(Vec::new());
                excerpt_by_id.set(HashMap::new());

                // Reset non-reactive state
                *nav_reset.borrow_mut() = NavController::new();
                *prefetch_reset.borrow_mut() = PrefetchCache::<GraphNeighborsResponse>::new();
                *detail_cache_reset.borrow_mut() = PrefetchCache::<NoteDetailResponse>::new();
                *in_flight_reset.borrow_mut() = InFlightSet::new(MAX_IN_FLIGHT);
                // Cancel any pending hover-debounce timer on agent switch.
                *pending_hover_timer_reset.borrow_mut() = None;
                hover_intent.set(None);
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
                // Build deterministic 3D galaxy seed from full-graph topology.
                galaxy_data.set(Some(build_galaxy(r)));
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
                        + nbhd
                            .clusters
                            .iter()
                            .map(|c| c.member_ids.len())
                            .sum::<usize>();
                    let neighbor_ids: Vec<String> =
                        nbhd.one_hop.iter().map(|n| n.id.clone()).collect();
                    seed_graph_state(&gs_inner, &nbhd, Some(entry_id.clone()));
                    nav_inner
                        .borrow_mut()
                        .fulfilled(entry_id.clone(), name, nbhd);
                    prefetch_inner
                        .borrow_mut()
                        .put(entry_id.clone(), resp.clone(), now_ms);
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
        let Some(id) = active_request.get() else {
            return;
        };
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
                + nbhd
                    .clusters
                    .iter()
                    .map(|c| c.member_ids.len())
                    .sum::<usize>();
            let neighbor_ids: Vec<String> = nbhd.one_hop.iter().map(|n| n.id.clone()).collect();
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
                        + nbhd
                            .clusters
                            .iter()
                            .map(|c| c.member_ids.len())
                            .sum::<usize>();
                    let neighbor_ids: Vec<String> =
                        nbhd.one_hop.iter().map(|n| n.id.clone()).collect();
                    seed_graph_state(&gs_fetch, &nbhd, Some(id.clone()));
                    nav_fetch.borrow_mut().fulfilled(id.clone(), name, nbhd);
                    prefetch_fetch
                        .borrow_mut()
                        .put(id.clone(), resp.clone(), now_ms);
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

    // NOTE: a redundant "Effect 3" used to re-fetch `graph.node_detail` on every
    // `selected_node` change and discard the result (`Ok(_) => {}`) — a duplicate
    // RPC with no caching side-effect, since the lazy-fetch Effect above already
    // fetches AND caches the selected node's detail. Removed (entropy reduction).

    // -----------------------------------------------------------------------
    // Hover debounce Effect: HOVER_DEBOUNCE_MS dwell gate.
    //
    // `hover_intent` mirrors the raw `CanvasEvent::HoverNode` transitions
    // (edge-triggered: changes only when the hovered node changes). This
    // Effect translates "pointer landed on node X" into "pointer has rested on
    // node X for ≥HOVER_DEBOUNCE_MS, schedule a prefetch" by arming a
    // gloo_timers::Timeout and writing `prefetch_request` only when the timer
    // fires. Re-arms (= cancels previous, starts new) on each hover change, so
    // skimming across nodes never reaches the fire path.
    // -----------------------------------------------------------------------
    let pending_hover_timer_e = pending_hover_timer;
    Effect::new(move || {
        let target = hover_intent.get();
        // Drop any pending timer; replacing the Option cancels the underlying
        // gloo Timeout via Drop.
        *pending_hover_timer_e.borrow_mut() = None;
        let Some(id) = target else { return };
        let id_for_timer = id;
        let timer = Timeout::new(HOVER_DEBOUNCE_MS as u32, move || {
            prefetch_request.set(Some(id_for_timer));
        });
        *pending_hover_timer_e.borrow_mut() = Some(timer);
    });

    // -----------------------------------------------------------------------
    // Effect 4: hover prefetch — background-fetch when pointer dwells on a node
    //
    // Guarded by InFlightSet: at most MAX_IN_FLIGHT concurrent fetches, and a
    // single in-flight per id (no parallel RPC for the same target). The guard
    // is moved into the spawned future and freed by RAII on completion / error
    // / panic.
    //
    // TODO(memory-events): once the gateway publishes `memory.note.changed`
    // (server emits `MemoryEvent::NoteDeleted` but the topic is not yet
    // bridged), subscribe here and call `prefetch.invalidate(id)` so edits
    // bust the cache before the 90s TTL expires.
    // -----------------------------------------------------------------------
    let prefetch_e4 = prefetch;
    let in_flight_e4 = in_flight;
    Effect::new(move || {
        let Some(id) = prefetch_request.get() else {
            return;
        };

        let now = now_ms();
        // Skip if already cached and not stale
        if prefetch_e4.borrow().has(&id, now) {
            return;
        }

        // Concurrency cap + per-id de-dup. None means: already in flight, or
        // we'd exceed MAX_IN_FLIGHT. Both cases drop this prefetch silently.
        let Some(guard) = try_acquire_in_flight(&in_flight_e4, &id) else {
            return;
        };

        let agent = agent_id.get();
        let prefetch_inner = prefetch_e4.clone();
        spawn_local(async move {
            let _g = guard; // released on future completion
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
        let Some((cached_id, raw)) = last_response.get_untracked() else {
            return;
        };
        if active_request.get_untracked().as_ref() != Some(&cached_id) {
            return; // race: slider fired during a center transition
        }

        let now = now_ms();
        let mut nbhd = to_neighborhood(&raw, now, threshold);
        let dtos = all_dtos.get_untracked();
        populate_orphans(&mut nbhd, &dtos);

        let one_hop_len = nbhd.one_hop.len();
        let total_len = one_hop_len
            + nbhd
                .clusters
                .iter()
                .map(|c| c.member_ids.len())
                .sum::<usize>();
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
            // Drive the same fetch path via the intent signal
            active_request.set(Some(id));
        }
        CanvasEvent::HoverNode(hovered_id) => {
            // Edge-triggered: `HoverNode` only fires on transition (see
            // graph_canvas.rs hit-test). The dwell-threshold work happens in
            // the hover-debounce Effect below, which reads this signal and
            // schedules a 150ms `gloo_timers::callback::Timeout`.
            //
            // We only write `Copy` signals here because the `on_event`
            // callback is wrapped in `Callback::new` (Send + Sync). The
            // `Rc<RefCell<Option<Timeout>>>` lives outside this callback in
            // the Effect's capture set, where Send is not required.
            hover_intent.set(hovered_id);
        }
        _ => {}
    };

    // Search: on_search is driven by the sidebar's search field via mem.search_query.
    // Subscribe to search_query changes and trigger a graph navigation when non-empty.
    Effect::new(move || {
        let query = search_query.get();
        if query.is_empty() {
            return;
        }
        let agent = agent_id.get();
        spawn_local(async move {
            match GraphApi::search(&state, &agent, &query, 20).await {
                Ok(response) => {
                    if let Some(first) = response.results.first() {
                        let id = first.id.clone();
                        active_request.set(Some(id));
                    }
                }
                Err(e) => {
                    web_sys::console::error_1(&format!("Search failed: {e}").into());
                }
            }
        });
    });

    view! {
        <div class="relative w-full h-full bg-[#080818]">
            // GalaxyCanvas: 3D force-layout nebula (Phase 1).
            // GraphCanvas + GraphState imports retained for Phase 4 removal.
            <GalaxyCanvas
                graph=galaxy_data
                on_event=Callback::new(on_event)
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
                {  }
            }
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
    gs.selected_node = selected;
    // Recenter: world (0,0) → canvas center, reset zoom + drag
    gs.viewport.offset.x = gs.viewport.width / 2.0;
    gs.viewport.offset.y = gs.viewport.height / 2.0;
    gs.viewport.scale = 1.0;
    gs.drag_offset = (0.0, 0.0);
    // Auto-fit viewport to the freshly seeded layout so we don't lose nodes off-screen.
    // Safe when nodes is empty (fit_to_content early-returns) or when width/height are
    // still zero pre-mount (T10 will refit on resize).
    // Reborrow through `&mut *gs` so the borrow checker sees disjoint field borrows on
    // GraphState rather than two simultaneous borrows of the RefMut wrapper.
    let gs = &mut *gs;
    gs.viewport.fit_to_content(&gs.nodes, 0.10);
}

/// Build the initial 3D galaxy GraphData from a full-graph query response.
///
/// Node positions come from `ForceLayout::seed` (deterministic, hash-derived)
/// so the scene's starting positions match what the layout engine expects.
/// Scene::set_graph then builds a ForceLayout over these positions and animates
/// them to their settled state over up to MAX_SETTLE_STEPS frames.
fn build_galaxy(resp: &GraphQueryResponse) -> gl::GraphData {
    use gl::layout3d::ForceLayout;
    use gl::{GalaxyNode, GraphData};
    use crate::canvas_engine::category_color::category_rgb;

    let mut id_index = std::collections::HashMap::new();
    for (i, n) in resp.nodes.iter().enumerate() {
        id_index.insert(n.id.clone(), i as u32);
    }

    let edges: Vec<(u32, u32)> = resp.edges.iter().filter_map(|e| {
        Some((*id_index.get(&e.from)?, *id_index.get(&e.to)?))
    }).collect();

    let ids: Vec<String> = resp.nodes.iter().map(|n| n.id.clone()).collect();
    let layout = ForceLayout::new(ids.len(), &edges);
    let positions = layout.seed(&ids);

    let nodes: Vec<GalaxyNode> = resp.nodes.iter().zip(positions).map(|(n, pos)| {
        GalaxyNode {
            id: n.id.clone(),
            name: n.name.clone(),
            category: n.category.clone(),
            link_count: n.link_count as u32,
            pos,
            color: category_rgb(&n.category),
        }
    }).collect();

    GraphData { nodes, edges }
}

/// Refresh `GraphState`'s node/edge buffers from a freshly folded `Neighborhood`
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
