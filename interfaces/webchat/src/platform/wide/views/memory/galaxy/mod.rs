mod galaxy_build;
mod galaxy_canvas;
pub mod gl;
mod interaction;
mod node_detail_panel;
mod overlay;
mod viewport_controls;

pub use node_detail_panel::{NodeDetailPanel, NodeExcerpt};

use aleph_protocol::memory_graph::NoteAddress;
use leptos::prelude::*;
use leptos::task::spawn_local;
use std::collections::HashMap;

use crate::api::graph::GraphApi;
use galaxy_build::{build_galaxy, compute_highlight_set, fold_to_lod};
use interaction::CanvasEvent;
use leptos::callback::Callback;

use crate::context::DashboardState;
use crate::i18n::{t_string, use_i18n};
use crate::state::memory::{MemoryState, MemoryView, DEFAULT_FOLD};

use galaxy_canvas::GalaxyCanvas;
use overlay::{CanvasOverlay, CanvasStatus};
use viewport_controls::ViewportControls;

use crate::api::agents::AgentsApi;

#[component]
#[must_use]
pub fn GalaxyView() -> impl IntoView {
    view! { <GalaxyCanvasView /> }
}

/// 3D WebGL galaxy canvas host.
///
/// Architecture note: `Callback::new` in Leptos 0.8 requires `Send + Sync + 'static`, but WASM
/// is single-threaded. The `on_event` closure captures only `Copy` reactive signals so it can
/// satisfy the `Send + Sync` bound.
#[component]
fn GalaxyCanvasView() -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let mem = expect_context::<MemoryState>();
    let i18n = use_i18n();

    // Derive agent_id from MemoryState so the sidebar's agent selector drives
    // the canvas. Local alias for readability in the Effects below.
    let agent_id = mem.agent_id;

    // Fetch the agent list once on mount, writing results into MemoryState so
    // MemorySidebar's <select> can render them.
    //
    // This used to retry 3×500 ms on an error whose text contained "Not
    // connected" — one of five hand-rolled answers in the Panel to "the socket
    // may still be connecting". It was doubly redundant: the Effect below
    // already gates the call on `is_connected`, and `rpc_call` now parks the
    // request until the handshake completes. Matching on error *text* to decide
    // whether to retry was also the fragile half — the transport's readiness is
    // not something a caller should be re-deriving from a string.
    let fetch_agents = move || {
        let state = state;
        spawn_local(async move {
            match AgentsApi::list(&state).await {
                Ok(resp) => {
                    mem.agents.set(resp.agents);
                    let new_default = resp.default_id;
                    // Only override agent_id if it would actually change.
                    // Post-`.await` — see `crate::disposed_reads`.
                    let Some(current) = mem.agent_id.try_get_untracked() else {
                        return;
                    };
                    if current != new_default {
                        mem.agent_id.set(new_default);
                    }
                }
                Err(e) => {
                    web_sys::console::error_1(&format!("Agents list failed: {e}").into());
                }
            }
        });
    };

    // Agent-list bootstrap — gated on the WebSocket connection so AgentsApi::list
    // doesn't fire before the panel is connected, and gated on an EMPTY agent list
    // so a reconnect (laptop sleep, server restart) does not re-run it: the fetch
    // resets `agent_id` to the server default, which would silently throw away the
    // user's agent selection, search, density and camera with no interaction on
    // their part.
    Effect::new(move || {
        if !state.is_connected.get() || !mem.agents.with_untracked(|a| a.is_empty()) {
            return;
        }
        fetch_agents();
    });

    // Reactive signals (all Copy — safe to capture in Callback::new closures)
    // selected_node, search_query, fold_threshold are sourced from MemoryState so
    // the sidebar and canvas share the same values.
    let selected_node = RwSignal::<Option<String>>::new(None);
    let addresses = RwSignal::new(HashMap::<String, NoteAddress>::new());
    let graph_version = RwSignal::new(0u32);
    let search_version = RwSignal::new(0u32);
    let search_query = mem.search_query;
    let fold_threshold = mem.fold_threshold;
    let set_fold_threshold = mem.fold_threshold;

    // 3D galaxy data built from the full graph.query response via force-layout seed.
    let galaxy_data: RwSignal<Option<gl::GraphData>> = RwSignal::new(None);

    // (shown_count, total) when graph.query hit the node cap; None otherwise.
    let truncation: RwSignal<Option<(usize, usize)>> = RwSignal::new(None);

    // Load state of the galaxy fetch. `loading` starts true: on mount the round-trip
    // is pending (or waiting on the WebSocket), which is not the same thing as an
    // empty graph. `load_error` holds the last graph.query failure.
    let loading: RwSignal<bool> = RwSignal::new(true);
    let load_error: RwSignal<Option<String>> = RwSignal::new(None);
    // Written by GalaxyCanvas on a WebGL2 init failure or a lost context.
    let gl_error: RwSignal<Option<String>> = RwSignal::new(None);
    // Transient one-liner (no search match, note outside the loaded galaxy).
    let notice: RwSignal<Option<String>> = RwSignal::new(None);

    // Re-fetch pulses for the galaxy-build Effect: `reload_nonce` is the user's
    // Retry button; `graph_nonce` is bumped by NodeDetailPanel after a note is
    // edited / renamed / deleted, so the topology on screen matches the store.
    let reload_nonce: RwSignal<u32> = RwSignal::new(0);
    let graph_nonce: RwSignal<u32> = RwSignal::new(0);

    // Intent channels: host → GalaxyCanvas → Scene (non-Send bridge via signals).
    // `focus_request` triggers fly_to_node; `highlight_request` triggers set_highlight.
    // `lod_request` controls edge density (0 = all edges, 1 = backbone only).
    // `viewport_cmd` carries ViewportControls' zoom / fit / reset / focus commands.
    // All are one-shot pulses consumed (reset to None) by GalaxyCanvas.
    let focus_request: RwSignal<Option<String>> = RwSignal::new(None);
    let highlight_request: RwSignal<Option<std::collections::HashSet<u32>>> = RwSignal::new(None);
    let lod_request: RwSignal<f32> = RwSignal::new(0.0);
    let highlight_edges_request: RwSignal<Option<std::collections::HashSet<(u32, u32)>>> =
        RwSignal::new(None);
    let viewport_cmd: RwSignal<Option<gl::scene::ViewportCmd>> = RwSignal::new(None);

    // Per-node excerpt cache for NodeDetailPanel.
    let detail_panel_excerpts = RwSignal::new(HashMap::<NoteAddress, NodeExcerpt>::new());

    // Raw hover intent — written by the (Send+Sync) on_event callback on every
    // HoverNode transition. Stored as RwSignal so on_event (write) and Effects
    // (read) can both reach it without sharing a non-Send Rc.
    let hover_intent: RwSignal<Option<String>> = RwSignal::new(None);

    // Fly the camera to `id` and highlight it plus its topological neighbors.
    // Captures only Copy signals, so it stays Copy + Send + Sync and can be
    // reused from the on_event Callback and from the Effects below.
    let focus_and_highlight = move |id: &str| {
        focus_request.set(Some(id.to_string()));
        if let Some(data) = galaxy_data.get_untracked() {
            highlight_request.set(Some(compute_highlight_set(&data, id)));
            highlight_edges_request.set(Some(gl::compute_highlight_edges(&data, id)));
        }
    };

    // -----------------------------------------------------------------------
    // Agent-switch reset Effect.
    // Subscribes to `agent_id`; on a real change (prev != current), wipes all
    // canvas view state so the new agent's galaxy renders from a clean slate.
    // The galaxy-build Effect also subscribes to `agent_id` and re-fires
    // automatically — this Effect's only job is the reset.
    //
    // The closure returns the current `agent_id`, so the next invocation sees
    // it as `prev`. On first mount `prev == None` and the reset body is skipped
    // (avoids clearing empty state before the galaxy-build Effect's first fetch).
    // -----------------------------------------------------------------------
    Effect::new(move |prev: Option<(String, u64)>| {
        let current = (agent_id.get(), state.connection_epoch.get());
        if let Some(p) = prev.as_ref() {
            if *p != current {
                // Reset reactive signals
                mem.selected_node.set(None);
                selected_node.set(None);
                search_query.set(String::new());
                set_fold_threshold.set(DEFAULT_FOLD);
                // Clear 3D galaxy signals so the new agent's galaxy rebuilds from scratch.
                // The galaxy-build Effect repopulates galaxy_data when it re-runs.
                galaxy_data.set(None);
                addresses.update(HashMap::clear);
                detail_panel_excerpts.update(HashMap::clear);
                truncation.set(None);
                focus_request.set(None);
                highlight_request.set(None);
                highlight_edges_request.set(None);
                hover_intent.set(None);
                load_error.set(None);
                notice.set(None);
            }
        }
        current
    });

    // -----------------------------------------------------------------------
    // Galaxy-build Effect: on mount, on agent switch, on a note mutation
    // (`graph_nonce`) and on Retry (`reload_nonce`), fetch the full graph and
    // build the deterministic 3D galaxy seed from its topology.
    //
    // A failure is surfaced (`load_error` → the overlay's error card), never
    // swallowed: an unexplained black rectangle is indistinguishable from an
    // empty agent.
    // -----------------------------------------------------------------------
    Effect::new(move || {
        graph_nonce.get();
        reload_nonce.get();
        let agent = agent_id.get();
        let connected = state.is_connected.get();
        let epoch = state.connection_epoch.get();
        let version = graph_version.get_untracked().wrapping_add(1);
        graph_version.set(version);
        loading.set(true);
        load_error.set(None);
        if !connected {
            mem.selected_node.set(None);
            detail_panel_excerpts.update(HashMap::clear);
            addresses.update(HashMap::clear);
            galaxy_data.set(None);
            truncation.set(None);
            return;
        }
        spawn_local(async move {
            let response = GraphApi::query(&state, &agent, 500).await;
            if graph_version.try_get_untracked() != Some(version)
                || state.connection_epoch.try_get_untracked() != Some(epoch)
                || agent_id.try_get_untracked().as_deref() != Some(agent.as_str())
                || state.is_connected.try_get_untracked() != Some(true)
            {
                return;
            }
            match response {
                Ok(r) => {
                    load_error.set(None);
                    notice.set(None);
                    // Build deterministic 3D galaxy seed from full-graph topology.
                    addresses.set(
                        r.nodes
                            .iter()
                            .filter_map(|node| {
                                node.address().map(|address| (node.id.clone(), address))
                            })
                            .collect(),
                    );
                    galaxy_data.set(Some(build_galaxy(&r)));
                    // Surface a "showing top N of M" badge when the query truncated.
                    truncation.set(
                        r.total
                            .filter(|&t| t > r.nodes.len())
                            .map(|t| (r.nodes.len(), t)),
                    );
                }
                Err(e) => {
                    web_sys::console::error_1(&format!("graph.query failed: {e}").into());
                    load_error.set(Some(
                        crate::components::admin_refusal::settings_write_error(i18n, &e, |e| {
                            e.to_string()
                        }),
                    ));
                }
            }
            loading.set(false);
        });
    });

    // Canvas status, in priority order: a GL failure beats a load failure beats a
    // pending round-trip beats an empty graph.
    let status = Memo::new(move |_| {
        // GL failures are FATAL, not retryable: the Scene is built once in the
        // mount Effect, so no amount of re-fetching brings the context back.
        if let Some(e) = gl_error.get() {
            return CanvasStatus::Fatal(e);
        }
        if let Some(e) = load_error.get() {
            return CanvasStatus::Error(e);
        }
        if loading.get() {
            return CanvasStatus::Loading;
        }
        if galaxy_data.with(|d| d.as_ref().is_some_and(|g| !g.nodes.is_empty())) {
            CanvasStatus::Ready
        } else {
            CanvasStatus::Empty
        }
    });
    let has_nodes = Signal::derive(move || {
        galaxy_data.with(|d| d.as_ref().is_some_and(|g| !g.nodes.is_empty()))
    });
    let has_selection = Signal::derive(move || selected_node.get().is_some());

    // -----------------------------------------------------------------------
    // Canvas event handler — captures only Copy signals, safe for Callback::new
    // -----------------------------------------------------------------------
    let on_event = move |event: CanvasEvent| match event {
        CanvasEvent::SelectNode(id) => {
            let address = addresses.with_untracked(|items| items.get(&id).cloned());
            mem.selected_node.set(address.clone());
            if let Some(address) = address {
                mem.push_recent(address);
            }
            selected_node.set(Some(id.clone()));
            focus_and_highlight(&id);
        }
        CanvasEvent::DeselectNode => {
            mem.selected_node.set(None);
            selected_node.set(None);
            // Clear highlight when deselecting.
            highlight_request.set(None);
            highlight_edges_request.set(None);
        }
        CanvasEvent::HoverNode(hovered_id) => {
            // Edge-triggered: `HoverNode` only fires on transition (see
            // galaxy_canvas.rs hit-test). `hover_intent` is passed directly
            // to `GalaxyCanvas` as `hovered_node` to drive the label overlay.
            hover_intent.set(hovered_id);
        }
    };
    let on_event = Callback::new(on_event);

    // Search: driven by the hub toolbar's Enter-submit pulse (`mem.search_nonce`).
    // The toolbar writes `search_query` live on every keystroke but only bumps
    // `search_nonce` on Enter (same pattern as views/memory/mod.rs:149-157).
    // Subscribing to `search_nonce` here prevents a graph.search RPC + camera
    // fly-to on every keystroke; the query is read untracked to avoid a second
    // subscription.
    //
    // On a match, drive the 3D galaxy's intent channels: fly-to + highlight + open
    // panel. Zero matches and RPC failures used to be entirely invisible — both
    // now land in the notice strip.
    Effect::new(move |previous: Option<u32>| {
        let submit = mem.search_nonce.get();
        let agent = agent_id.get();
        let connected = state.is_connected.get();
        let epoch = state.connection_epoch.get();
        let version = search_version.get_untracked().wrapping_add(1);
        search_version.set(version);
        let query = search_query.get_untracked();
        if !connected || previous == Some(submit) || submit == 0 || query.is_empty() {
            return submit;
        }
        // Translate before the round-trip: the reactive read belongs in the Effect.
        let no_match = t_string!(i18n, memory.search_no_match).to_string();
        let failed = t_string!(i18n, memory.graph_error).to_string();
        notice.set(None);
        spawn_local(async move {
            let response = GraphApi::search(&state, &agent, &query, 20).await;
            if search_version.try_get_untracked() != Some(version)
                || state.connection_epoch.try_get_untracked() != Some(epoch)
                || agent_id.try_get_untracked().as_deref() != Some(agent.as_str())
                || state.is_connected.try_get_untracked() != Some(true)
                || search_query.try_get_untracked().as_deref() != Some(query.as_str())
            {
                return;
            }
            match response {
                Ok(response) => match response
                    .results
                    .first()
                    .filter(|row| !row.agent_id.is_empty())
                {
                    Some(first) => mem.selected_node.set(Some(NoteAddress {
                        agent_id: first.agent_id.clone(),
                        node_id: first.id.clone(),
                    })),
                    None => notice.set(Some(no_match)),
                },
                Err(e) => {
                    web_sys::console::error_1(&format!("Search failed: {e}").into());
                    notice.set(Some(
                        crate::components::admin_refusal::settings_write_error(i18n, &e, |e| {
                            format!("{failed} {e}")
                        }),
                    ));
                }
            }
        });
        submit
    });

    // -----------------------------------------------------------------------
    // Reverse-link Effect: list → graph cross-link.
    //
    // When the Memory table's "view in graph" button is clicked, `on_locate`
    // stores the exact note address and flips `mem.memory_view` to Graph
    // (see views/memory/mod.rs on_locate callback). This Effect detects that
    // and drives the 3D galaxy intent channels to fly to and highlight the node.
    //
    // `mem.memory_view` is read with `get_untracked()` — the Effect only
    // subscribes to `mem.selected_node` changes, not to memory_view.
    //
    // It ALSO subscribes to `galaxy_data`, and it must: on a fresh mount carrying
    // a pre-existing selection, this Effect runs before the graph has landed.
    // Re-check membership when the data arrives, including the partition.
    //
    // A note that is not among the loaded (capped) galaxy nodes used to be a
    // total silent no-op; it now says so, and does not fire dead intent channels.
    // -----------------------------------------------------------------------
    Effect::new(move || {
        galaxy_data.track();
        let Some(address) = mem.selected_node.get() else {
            selected_node.set(None);
            focus_request.set(None);
            highlight_request.set(None);
            highlight_edges_request.set(None);
            return;
        };
        // Only act when the memory hub is showing the Graph view; list-originated
        // locates always flip to Graph first (see on_locate in memory/mod.rs).
        if mem.memory_view.get_untracked() != MemoryView::Graph {
            return;
        }
        if galaxy_data.with_untracked(Option::is_none) {
            return;
        }
        if !addresses.with(|items| items.get(&address.node_id) == Some(&address)) {
            selected_node.set(None);
            highlight_request.set(None);
            highlight_edges_request.set(None);
            notice.set(Some(t_string!(i18n, memory.node_not_in_graph).to_string()));
            return;
        }
        notice.set(None);
        selected_node.set(Some(address.node_id.clone()));
        focus_and_highlight(&address.node_id);
    });

    // -----------------------------------------------------------------------
    // Density slider → LOD mapping Effect: fold_threshold (0..=10) → lod (0..1)
    // via `fold_to_lod`. Higher slider = denser graph.
    // -----------------------------------------------------------------------
    Effect::new(move || {
        lod_request.set(fold_to_lod(fold_threshold.get()));
    });

    view! {
        <div class="relative w-full h-full bg-[#080818]">
            // GalaxyCanvas: 3D force-layout nebula.
            <GalaxyCanvas
                graph=galaxy_data
                on_event=on_event
                focus_request=focus_request
                highlight_request=highlight_request
                lod_request=lod_request
                selected_node=selected_node
                hovered_node=hover_intent
                highlight_edges_request=highlight_edges_request
                viewport_cmd=viewport_cmd
                gl_error=gl_error
            />
            // Viewport cluster (zoom / fit / reset / focus) + graph-scoped hotkeys.
            <ViewportControls
                viewport_cmd=viewport_cmd
                on_event=on_event
                has_nodes=has_nodes
                has_selection=has_selection
            />
            // Loading / empty / error card + the transient notice strip.
            <CanvasOverlay
                status=status.into()
                notice=notice
                on_retry=Callback::new(move |()| reload_nonce.update(|n| *n += 1))
            />
            // Truncation badge: shown when graph.query returned fewer nodes than the agent has.
            {move || truncation.get().map(|(shown, total)| view! {
                <div class="absolute top-2 right-2 pointer-events-none text-[11px] text-white/70
                            bg-black/40 rounded px-2 py-0.5 select-none">
                    {t_string!(i18n, memory.graph_truncated)
                        .replace("{shown}", &shown.to_string())
                        .replace("{total}", &total.to_string())}
                </div>
            })}
            // NodeDetailPanel: always mounted — with no selection it shows the
            // "recently visited" list and the click-a-node hint, which is also
            // what the empty galaxy needs.
            <div class="absolute bottom-0 right-0 w-72 max-h-[60%] overflow-y-auto
                        bg-[#0d1120cc] border border-[#2a3060] rounded-tl-lg shadow-xl
                        backdrop-blur-sm">
                <NodeDetailPanel excerpts=detail_panel_excerpts graph_nonce=graph_nonce />
            </div>
        </div>
    }
}

// Pure graph→galaxy transforms (`build_galaxy`, `fold_to_lod`,
// `compute_highlight_set`, …) live in `galaxy_build.rs`, the status card in
// `overlay.rs` — this file holds only the component's reactive wiring.
