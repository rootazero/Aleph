mod galaxy_canvas;
pub mod gl;
pub mod node_card;
mod node_detail_panel;

pub use node_detail_panel::{NodeDetailPanel, NodeExcerpt};

use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::api::graph::GraphApi;
use crate::canvas_engine::adapter::{GraphQueryResponse, NoteNodeDto};
use crate::canvas_engine::interaction::CanvasEvent;
use leptos::callback::Callback;

use crate::context::DashboardState;
use crate::state::memory::{MemoryState, MemoryView, DEFAULT_FOLD};

use galaxy_canvas::GalaxyCanvas;

use crate::api::agents::AgentsApi;

#[component]
#[must_use]
pub fn CanvasView() -> impl IntoView {
    view! { <RadialCanvasView /> }
}

/// 3D WebGL galaxy canvas host.
///
/// Architecture note: `Callback::new` in Leptos 0.8 requires `Send + Sync + 'static`, but WASM
/// is single-threaded. The `on_event` closure captures only `Copy` reactive signals so it can
/// satisfy the `Send + Sync` bound.
#[component]
fn RadialCanvasView() -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let mem = expect_context::<MemoryState>();
    let viewport = expect_context::<crate::state::viewport::ViewportState>();
    let is_mobile = viewport.is_mobile;

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

    // Full-graph node cache — populated once on mount, used to compute the
    // ghost-dot ring of orphans (nodes outside the current connected component).
    let all_dtos: RwSignal<Vec<NoteNodeDto>> = RwSignal::new(Vec::new());

    // 3D galaxy data built from the full graph.query response via force-layout seed.
    let galaxy_data: RwSignal<Option<gl::GraphData>> = RwSignal::new(None);

    // Intent channels: host → GalaxyCanvas → Scene (non-Send bridge via signals).
    // `focus_request` triggers fly_to_node; `highlight_request` triggers set_highlight.
    // `lod_request` controls edge density (0 = all edges, 1 = backbone only).
    let focus_request: RwSignal<Option<String>> = RwSignal::new(None);
    let highlight_request: RwSignal<Option<std::collections::HashSet<u32>>> = RwSignal::new(None);
    let lod_request: RwSignal<f32> = RwSignal::new(0.0);
    let highlight_edges_request: RwSignal<Option<std::collections::HashSet<(u32, u32)>>> =
        RwSignal::new(None);

    // WebGL2-fallback signal (§11 P-⑥): GalaxyCanvas sets this true when GL init
    // fails on mount; the watch Effect below switches Memory to the Table view.
    let canvas_fallback = mem.galaxy_unsupported;

    // Per-node excerpt cache for NodeDetailPanel.
    let detail_panel_excerpts: RwSignal<std::collections::HashMap<String, NodeExcerpt>> =
        RwSignal::new(std::collections::HashMap::new());

    // Raw hover intent — written by the (Send+Sync) on_event callback on every
    // HoverNode transition. Stored as RwSignal so on_event (write) and Effects
    // (read) can both reach it without sharing a non-Send Rc.
    let hover_intent: RwSignal<Option<String>> = RwSignal::new(None);

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
    Effect::new(move |prev: Option<String>| {
        let current = agent_id.get();
        if let Some(p) = prev.as_ref() {
            if *p != current {
                // Reset reactive signals
                set_selected_node.set(None);
                search_query.set(String::new());
                set_fold_threshold.set(DEFAULT_FOLD);
                all_dtos.set(Vec::new());
                // Clear 3D galaxy signals so the new agent's galaxy rebuilds from scratch.
                // The galaxy-build Effect repopulates galaxy_data when it re-runs.
                galaxy_data.set(None);
                focus_request.set(None);
                highlight_request.set(None);
                highlight_edges_request.set(None);
                lod_request.set(0.0);
                hover_intent.set(None);
            }
        }
        current
    });

    // -----------------------------------------------------------------------
    // Galaxy-build Effect: on mount (and agent switch) fetch the full graph and
    // build the deterministic 3D galaxy seed from its topology.
    // -----------------------------------------------------------------------
    Effect::new(move || {
        if !state.is_connected.get() {
            return;
        }
        let agent = agent_id.get();

        spawn_local(async move {
            // Fetch the full graph and build the 3D galaxy seed from its topology.
            let query_result = GraphApi::query(&state, &agent, 500, vec![]).await.ok();
            if let Some(ref r) = query_result {
                all_dtos.set(r.nodes.clone());
                // Build deterministic 3D galaxy seed from full-graph topology.
                galaxy_data.set(Some(build_galaxy(r)));
            }
        });
    });

    // -----------------------------------------------------------------------
    // Canvas event handler — captures only Copy signals, safe for Callback::new
    // -----------------------------------------------------------------------
    let on_event = move |event: CanvasEvent| match event {
        CanvasEvent::SelectNode(id) => {
            set_selected_node.set(Some(id.clone()));
            // Drive the scene via intent channels:
            // 1. Fly camera to selected node.
            focus_request.set(Some(id.clone()));
            // 2. Highlight selected node + topological neighbors.
            //    Read galaxy_data (untracked — we don't want re-runs on data changes).
            if let Some(data) = galaxy_data.get_untracked() {
                let hl = compute_highlight_set(&data, &id);
                highlight_request.set(Some(hl));
                highlight_edges_request.set(Some(
                    crate::views::canvas::gl::compute_highlight_edges(&data, &id),
                ));
            }
        }
        CanvasEvent::DeselectNode => {
            set_selected_node.set(None);
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

    // Search: driven by the hub toolbar's Enter-submit pulse (`mem.search_nonce`).
    // The toolbar writes `search_query` live on every keystroke but only bumps
    // `search_nonce` on Enter (same pattern as views/memory/mod.rs:149-157).
    // Subscribing to `search_nonce` here prevents a graph.search RPC + camera
    // fly-to on every keystroke; the query is read untracked to avoid a second
    // subscription.
    //
    // On a match, drive the 3D galaxy's intent channels: fly-to + highlight + open panel.
    // active_request is NOT set here — it drove the retired radial-fetch path; leaving
    // it disconnected from search prevents stale graph.neighbors fetches in the console.
    Effect::new(move || {
        mem.search_nonce.get(); // subscribe to Enter-submit pulses only
        let query = search_query.get_untracked();
        if query.is_empty() {
            return;
        }
        let agent = agent_id.get_untracked();
        spawn_local(async move {
            match GraphApi::search(&state, &agent, &query, 20).await {
                Ok(response) => {
                    if let Some(first) = response.results.first() {
                        let id = first.id.clone();
                        // Drive 3D galaxy: fly camera to matched node.
                        focus_request.set(Some(id.clone()));
                        // Highlight matched node + its topological neighbors.
                        if let Some(data) = galaxy_data.get_untracked() {
                            let hl = compute_highlight_set(&data, &id);
                            highlight_request.set(Some(hl));
                            highlight_edges_request.set(Some(
                                crate::views::canvas::gl::compute_highlight_edges(&data, &id),
                            ));
                        }
                        // Open the node detail panel by selecting the node.
                        mem.selected_node.set(Some(id));
                    }
                }
                Err(e) => {
                    web_sys::console::error_1(&format!("Search failed: {e}").into());
                }
            }
        });
    });

    // -----------------------------------------------------------------------
    // Reverse-link Effect: list → graph cross-link.
    //
    // When the Memory table's "view in graph" button is clicked, `on_locate`
    // sets `mem.selected_node` and flips `mem.memory_view` to Graph
    // (see views/memory/mod.rs on_locate callback). This Effect detects that
    // and drives the 3D galaxy intent channels to fly to and highlight the node.
    //
    // Feedback-loop avoidance:
    // 1. `mem.memory_view` is read with `get_untracked()` — the Effect only
    //    subscribes to `mem.selected_node` changes, not to memory_view.
    // 2. In-canvas clicks (on_event::SelectNode) and the search Effect BOTH
    //    write `focus_request` BEFORE (synchronously, in the same handler as)
    //    writing `mem.selected_node`. By the time this async Effect runs,
    //    `focus_request` already holds the id they initiated. The dedupe guard
    //    below detects that and returns early, so the fly-to animation is not
    //    restarted mid-flight (no visible stutter on galaxy clicks).
    //    List-originated locates (on_locate) do NOT pre-set `focus_request`,
    //    so the guard passes and a fresh fly-to is triggered — the intended path.
    // -----------------------------------------------------------------------
    Effect::new(move || {
        let Some(node_id) = mem.selected_node.get() else {
            return;
        };
        // Only act when the memory hub is showing the Graph view; list-originated
        // locates always flip to Graph first (see on_locate in memory/mod.rs).
        if mem.memory_view.get_untracked() != MemoryView::Graph {
            return;
        }
        // Dedupe guard: if focus_request already holds this id, the fly-to was
        // initiated by an in-canvas click or the search Effect (which both set
        // focus_request synchronously before setting selected_node). Skip to
        // avoid restarting the camera animation mid-flight.
        if focus_request.get_untracked().as_deref() == Some(node_id.as_str()) {
            return;
        }
        // Drive 3D galaxy: fly camera to the located node.
        focus_request.set(Some(node_id.clone()));
        // Highlight it and its topological neighbors.
        if let Some(data) = galaxy_data.get_untracked() {
            let hl = compute_highlight_set(&data, &node_id);
            highlight_request.set(Some(hl));
            highlight_edges_request.set(Some(crate::views::canvas::gl::compute_highlight_edges(
                &data, &node_id,
            )));
        }
    });

    // -----------------------------------------------------------------------
    // Fold slider → LOD mapping Effect: fold_threshold (0..=10) → lod (0..1)
    // via `fold_to_lod`. Higher slider = denser graph. The retired cluster-fold
    // semantics are reused purely as an edge-density knob; the slider's full
    // travel now spans the full LOD range (see `fold_to_lod`).
    // -----------------------------------------------------------------------
    Effect::new(move || {
        lod_request.set(fold_to_lod(fold_threshold.get()));
    });

    // Watch the WebGL2-fallback flag: on the first true, permanently switch the
    // Memory hub to the Table view (the galaxy can't render on this device).
    Effect::new(move || {
        if canvas_fallback.get() {
            mem.memory_view.set(MemoryView::Table);
        }
    });

    view! {
        <div class="relative w-full h-full bg-[#080818]">
            // GalaxyCanvas: 3D force-layout nebula.
            <GalaxyCanvas
                graph=galaxy_data
                on_event=Callback::new(on_event)
                focus_request=focus_request
                highlight_request=highlight_request
                lod_request=lod_request
                selected_node=selected_node
                hovered_node=hover_intent
                highlight_edges_request=highlight_edges_request
                is_mobile=is_mobile
                fallback=canvas_fallback
            />
            // NodeDetailPanel: overlay when a node is selected in the galaxy.
            // Desktop: docked bottom-right card. Mobile (max-sm): full-width
            // bottom sheet, notch-aware via safe-area-inset-bottom (R-20).
            {move || selected_node.get().map(|_| view! {
                <div class="absolute bottom-0 right-0 w-72 max-h-[60%] overflow-y-auto
                            bg-[#0d1120cc] border border-[#2a3060] rounded-tl-lg shadow-xl
                            backdrop-blur-sm
                            max-sm:left-0 max-sm:right-0 max-sm:w-full max-sm:max-h-[50%]
                            max-sm:rounded-tl-2xl max-sm:rounded-tr-2xl
                            max-sm:pb-[calc(env(safe-area-inset-bottom)+0.5rem)]">
                    <NodeDetailPanel excerpts=detail_panel_excerpts />
                </div>
            })}
        </div>
    }
}

// ---------------------------------------------------------------------------
// Private helpers shared by RadialCanvasView Effects
// ---------------------------------------------------------------------------

/// Build the initial 3D galaxy GraphData from a full-graph query response.
///
/// Node positions come from `ForceLayout::seed` (deterministic, hash-derived)
/// so the scene's starting positions match what the layout engine expects.
/// Scene::set_graph then builds a ForceLayout over these positions and animates
/// them to their settled state over up to MAX_SETTLE_STEPS frames.
fn build_galaxy(resp: &GraphQueryResponse) -> gl::GraphData {
    use crate::canvas_engine::category_color::category_rgb;
    use gl::layout3d::ForceLayout;
    use gl::{GalaxyNode, GraphData};

    let mut id_index = std::collections::HashMap::new();
    for (i, n) in resp.nodes.iter().enumerate() {
        id_index.insert(n.id.clone(), i as u32);
    }

    // Memory links are directed rows, but the galaxy is an undirected graph:
    // reciprocal wikilinks (A→B and B→A) and duplicate rows must collapse to a
    // single edge, or each pair draws two oppositely-bowed bézier arcs (the
    // "double arc" artifact). Also drops self-loops.
    let edges = dedup_undirected_edges(
        resp.edges
            .iter()
            .filter_map(|e| Some((*id_index.get(&e.from)?, *id_index.get(&e.to)?))),
    );

    let ids: Vec<String> = resp.nodes.iter().map(|n| n.id.clone()).collect();
    let layout = ForceLayout::new(ids.len(), &edges);
    let positions = layout.seed(&ids);

    let nodes: Vec<GalaxyNode> = resp
        .nodes
        .iter()
        .zip(positions)
        .map(|(n, pos)| GalaxyNode {
            id: n.id.clone(),
            name: n.name.clone(),
            category: n.category.clone(),
            link_count: n.link_count as u32,
            pos,
            color: category_rgb(&n.category),
        })
        .collect();

    GraphData { nodes, edges }
}

/// Collapse directed link rows into unique undirected edges.
///
/// Reciprocal links (`A→B` and `B→A`) and exact duplicates fold to one
/// `(min, max)` pair; self-loops (`A→A`) are dropped. First-appearance order is
/// preserved so the renderer's edge ordering stays deterministic across rebuilds.
/// Normalizing to `(min, max)` also matches the edge-highlight key normalization
/// in `gl::edges::EdgeRenderer::set_highlight`.
fn dedup_undirected_edges(directed: impl Iterator<Item = (u32, u32)>) -> Vec<(u32, u32)> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for (a, b) in directed {
        if a == b {
            continue; // degenerate self-loop
        }
        let key = (a.min(b), a.max(b));
        if seen.insert(key) {
            out.push(key);
        }
    }
    out
}

/// Map the Fold slider value (UI range 0..=10) to an edge-density LOD in [0,1]
/// for the galaxy renderer. Higher slider = denser graph: `fold=0` → lod 1.0
/// (only the ~90th-percentile backbone survives `Scene::recompute_filtered_edges`),
/// `fold=10` → lod 0.0 (all edges). The full slider travel spans the full LOD
/// range, replacing the old `1.0 - (ft-1)/999` map whose 0..10 input only
/// produced lod∈[0.991,1.0] (visibly no change).
fn fold_to_lod(fold: usize) -> f32 {
    let ft = fold.min(10) as f32;
    (1.0 - ft / 10.0).clamp(0.0, 1.0)
}

/// Compute the highlight set for a selected node: the selected node's index
/// plus all topologically adjacent node indices (one hop).
///
/// Returns a `HashSet<u32>` of node indices (matching `GraphData.nodes` order).
/// The scene's `set_highlight` will dim any node NOT in this set.
fn compute_highlight_set(
    data: &gl::GraphData,
    selected_id: &str,
) -> std::collections::HashSet<u32> {
    // Find the selected node's index.
    let Some(sel_idx) = data.nodes.iter().position(|n| n.id == selected_id) else {
        return std::collections::HashSet::new();
    };
    let sel_idx = sel_idx as u32;

    // Collect direct neighbors via edges.
    let mut hl = std::collections::HashSet::new();
    hl.insert(sel_idx);
    for &(a, b) in &data.edges {
        if a == sel_idx {
            hl.insert(b);
        } else if b == sel_idx {
            hl.insert(a);
        }
    }
    hl
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dedup_collapses_reciprocal_and_duplicate_edges() {
        // (0,1) and (1,0) are the same undirected edge; (2,3) appears twice.
        let directed = [(0u32, 1u32), (1, 0), (2, 3), (2, 3), (3, 4)];
        let out = dedup_undirected_edges(directed.into_iter());
        assert_eq!(out, vec![(0, 1), (2, 3), (3, 4)]);
    }

    #[test]
    fn dedup_drops_self_loops() {
        let directed = [(5u32, 5u32), (0, 1)];
        let out = dedup_undirected_edges(directed.into_iter());
        assert_eq!(out, vec![(0, 1)]);
    }

    #[test]
    fn fold_to_lod_spans_full_visible_range() {
        // Full slider travel (0..=10) must cover the full LOD range so the
        // control is visibly effective (the old 0..10→[0.991,1.0] map did not).
        assert_eq!(fold_to_lod(0), 1.0); // sparsest: backbone only
        assert_eq!(fold_to_lod(10), 0.0); // densest: all edges
        assert_eq!(fold_to_lod(5), 0.5); // midpoint
                                         // Monotonic decreasing: higher slider = denser graph (lower lod).
        assert!(fold_to_lod(2) > fold_to_lod(8));
        // Out-of-range slider values clamp instead of overflowing the LOD range.
        assert_eq!(fold_to_lod(99), 0.0);
    }
}
