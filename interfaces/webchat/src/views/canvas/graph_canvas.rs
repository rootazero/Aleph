use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use leptos::prelude::*;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;

use leptos::callback::Callback;

use crate::canvas_engine::drag::{DragState, ReleaseOutcome};
use crate::canvas_engine::edge_curve::{bezier_point, bezier_tangent, edge_control_point,
    DEFAULT_SAG};
use crate::canvas_engine::interaction::{CanvasEvent, InteractionState};
use crate::canvas_engine::navigation::NavController;
use crate::canvas_engine::renderer::draw_neighborhood;
use crate::canvas_engine::tween::build_interpolated_neighborhood;
use crate::canvas_engine::types::*;
use crate::canvas_engine::viewport::Viewport;
use crate::views::canvas::edge_label::EdgeLabel;
use crate::views::canvas::node_card::NodeCard;

/// Per-edge label state kept in the overlay signal map.
struct EdgeLabelEntry {
    /// The label text to display (never None — entries only exist when label.is_some()).
    label: String,
    /// Screen-space midpoint of the Bézier curve, updated each rAF frame.
    pos_sig: RwSignal<(f32, f32)>,
    /// Tangent angle in radians at t=0.5, clamped to [-π/4, π/4].
    tangent_sig: RwSignal<f32>,
    /// True iff zoom ≥ 0.7 AND the edge is adjacent to the hovered/selected node.
    visible_sig: RwSignal<bool>,
}

/// Shared mutable state for 60fps canvas rendering (not reactive).
pub struct GraphState {
    pub nodes: Vec<CanvasNode>,
    pub edges: Vec<CanvasEdge>,
    pub viewport: Viewport,
    pub interaction: InteractionState,
    pub selected_node: Option<String>,
    pub hovered_node: Option<String>,
    pub is_running: bool,
    /// Target position for smooth pan animation (world coordinates).
    pub pan_target: Option<Vec2>,
    /// Animation progress (0.0 → 1.0).
    pub pan_progress: f64,
    /// Drag offset for parallax (world-space delta since last pointer-down).
    pub drag_offset: (f32, f32),
}

impl GraphState {
    pub fn new() -> Self {
        Self {
            nodes: Vec::new(),
            edges: Vec::new(),
            viewport: Viewport::new(800.0, 600.0),
            interaction: InteractionState::new(),
            selected_node: None,
            hovered_node: None,
            is_running: false,
            pan_target: None,
            pan_progress: 0.0,
            drag_offset: (0.0, 0.0),
        }
    }
}

impl Default for GraphState {
    fn default() -> Self {
        Self::new()
    }
}

type RafClosure = Rc<RefCell<Option<Closure<dyn FnMut()>>>>;

/// Returns `performance.now()` in milliseconds.
fn now_ms() -> f64 {
    web_sys::window()
        .and_then(|w| w.performance())
        .map(|p| p.now())
        .unwrap_or(0.0)
}

/// Draw a simple placeholder when the nav is Idle, Loading, or in Error state.
fn draw_placeholder(ctx: &web_sys::CanvasRenderingContext2d, viewport: &Viewport, message: &str) {
    ctx.set_fill_style_str("#080818");
    ctx.fill_rect(0.0, 0.0, viewport.width, viewport.height);

    ctx.set_fill_style_str("rgba(148,163,184,0.5)");
    ctx.set_font("14px sans-serif");
    ctx.set_text_align("center");
    ctx.set_text_baseline("middle");
    let _ = ctx.fill_text(message, viewport.width / 2.0, viewport.height / 2.0);
}

#[component]
pub fn GraphCanvas(
    graph_state: Rc<RefCell<GraphState>>,
    on_event: Callback<CanvasEvent>,
    /// Optional NavController for the radial navigation path.
    /// When `Some`, the rAF loop uses NavState-aware rendering.
    /// When `None`, the rAF loop is a no-op (no flat-graph fallback).
    #[prop(optional)]
    nav: Option<Rc<RefCell<NavController>>>,
    /// Reactive map of node id → pre-rendered excerpt HTML.
    /// Populated lazily by the parent's detail-fetch Effect when a card
    /// enters FULL mode (hover or select). Empty string until the fetch
    /// resolves; the card re-renders automatically when a value arrives.
    #[prop(optional)]
    excerpt_by_id: Option<RwSignal<HashMap<String, String>>>,
) -> impl IntoView {
    let canvas_ref = NodeRef::<leptos::html::Canvas>::new();
    let raf_handle: Rc<RefCell<Option<i32>>> = Rc::new(RefCell::new(None));
    let raf_closure: RafClosure = Rc::new(RefCell::new(None));

    // Drag controller (Task 7): owns the elastic node-drag state machine.
    // Pointer events feed it; the rAF loop ticks it; the renderer reads its overlay.
    let drag_state: Rc<RefCell<DragState>> = Rc::new(RefCell::new(DragState::new()));
    let last_frame_ms: Rc<Cell<f64>> = Rc::new(Cell::new(0.0));

    // DOM overlay signals (Task 4.5): published each rAF tick so NodeCard overlays stay in sync.
    // node_screen_pos: maps node id → per-node screen position signal.
    // overlay_nodes:   flat ordered list of all visible nodes (center + 1-hop + 2-hop + orphans).
    // zoom_signal:     current viewport scale (f32) for NodeCard mode selection.
    // hovered_id_sig:  which node is currently hovered (from GraphState.hovered_node).
    // selected_id_sig: which node is currently selected (from GraphState.selected_node).
    let node_screen_pos: RwSignal<HashMap<String, RwSignal<(f32, f32)>>> =
        RwSignal::new(HashMap::new());
    let overlay_nodes: RwSignal<Vec<CanvasNode>> = RwSignal::new(Vec::new());
    let zoom_signal: RwSignal<f32> = RwSignal::new(1.0_f32);
    let hovered_id_sig: RwSignal<Option<String>> = RwSignal::new(None);
    let selected_id_sig: RwSignal<Option<String>> = RwSignal::new(None);

    // Edge-label overlay signals (Task 7.2): maps (from_id, to_id) → EdgeLabelEntry.
    // Only edges with label.is_some() get an entry. Entries are get-or-created lazily
    // in the rAF loop, mirroring the node_screen_pos pattern from Task 4.5.
    let edge_label_state: RwSignal<HashMap<(String, String), EdgeLabelEntry>> =
        RwSignal::new(HashMap::new());

    // Clone handles once for each event closure that needs them, since `nav`,
    // `drag_state`, and `last_frame_ms` are all moved into the rAF Effect closure
    // below. (Effect::new takes `move ||`, which consumes its captures.)
    let nav_for_md = nav.clone();
    let nav_for_mu = nav.clone();
    let drag_state_for_md = drag_state.clone();
    let drag_state_for_mm = drag_state.clone();
    let drag_state_for_mu = drag_state.clone();
    let drag_state_for_leave = drag_state.clone();

    // Visibility flag: drives the rAF closure's pause/resume decision.
    // Default true so the first frames render while IntersectionObserver
    // attaches. Updated asynchronously by the IntersectionObserver callback
    // when the canvas's bounding box appears/disappears (`display:none`
    // ancestor → empty box → not intersecting). Cell, not RefCell, because
    // the read in the rAF closure must not contend with the observer's
    // write.
    let is_visible: Rc<Cell<bool>> = Rc::new(Cell::new(true));

    // Start render loop after mount
    let gs = graph_state.clone();
    let raf_h = raf_handle.clone();
    let raf_c = raf_closure.clone();
    let is_visible_for_effect = is_visible.clone();

    Effect::new(move || {
        let Some(canvas_el) = canvas_ref.get() else {
            return;
        };

        let canvas: web_sys::HtmlCanvasElement = canvas_el.into();

        // Attach IntersectionObserver to flip `is_visible` without forcing
        // synchronous layout. `offset_width`/`getBoundingClientRect` would
        // each trigger updateLayoutIfDimensionsOutOfDate → resolveStyle
        // → updateCompositingLayers — at 60 Hz that costs more than the
        // render loop it was supposed to skip. IntersectionObserver
        // dispatches asynchronously off the layout pipeline.
        let is_visible_obs = is_visible_for_effect.clone();
        let observer_cb: Closure<dyn FnMut(js_sys::Array)> =
            Closure::new(move |entries: js_sys::Array| {
                if let Some(entry_val) = entries.get(0).dyn_into::<
                    web_sys::IntersectionObserverEntry,
                >()
                .ok()
                {
                    is_visible_obs.set(entry_val.is_intersecting());
                }
            });
        if let Ok(observer) = web_sys::IntersectionObserver::new(
            observer_cb.as_ref().unchecked_ref(),
        ) {
            observer.observe(&canvas);
        }
        // Leak the observer callback for the panel's lifetime — same
        // pattern as the rAF closure. Since the parent never unmounts us
        // (display:none keep-alive in MainContent), there is no cleanup
        // hook to disconnect cleanly.
        observer_cb.forget();

        // Initial canvas size — rAF loop will resize dynamically from parent
        let (w, h) = canvas
            .parent_element()
            .map(|p| {
                let r = p.get_bounding_client_rect();
                (r.width().max(1.0), r.height().max(1.0))
            })
            .unwrap_or((800.0, 600.0));
        let dpr = web_sys::window()
            .map(|w| w.device_pixel_ratio())
            .unwrap_or(1.0);

        canvas.set_width((w * dpr) as u32);
        canvas.set_height((h * dpr) as u32);

        let ctx = canvas
            .get_context("2d")
            .ok()
            .flatten()
            .and_then(|c| c.dyn_into::<web_sys::CanvasRenderingContext2d>().ok());

        let Some(ctx) = ctx else { return };
        let _ = ctx.scale(dpr, dpr);

        // Update viewport size
        {
            let mut gs = gs.borrow_mut();
            gs.viewport.width = w;
            gs.viewport.height = h;
            gs.is_running = true;
        }

        // Build the rAF loop
        let gs_render = gs.clone();
        let nav_render = nav.clone();
        let raf_h_inner = raf_h.clone();
        let raf_c_inner = raf_c.clone();
        let drag_state_inner = drag_state.clone();
        let last_frame_ms_inner = last_frame_ms.clone();
        let on_event_for_promote = on_event;
        // Overlay signals captured for position publishing each frame.
        let node_screen_pos_inner = node_screen_pos;
        let overlay_nodes_inner = overlay_nodes;
        let zoom_signal_inner = zoom_signal;
        let hovered_id_sig_inner = hovered_id_sig;
        let selected_id_sig_inner = selected_id_sig;
        let edge_label_state_inner = edge_label_state;

        let canvas_for_resize = canvas.clone();
        let is_visible_for_raf = is_visible.clone();
        let closure: Closure<dyn FnMut()> = Closure::new(move || {
            // Visibility pause: MainContent (app.rs) keeps every top-level
            // view mounted and toggles `display:none` for inactive tabs, so
            // `on_cleanup` never fires when the user navigates away from
            // Memory. Without this guard the rAF chain keeps ticking at
            // ~60 Hz on a hidden canvas (nav.tick + render + signal
            // publish), burning ~10% CPU/GPU. The flag is set
            // asynchronously by an IntersectionObserver attached above —
            // see the long comment there for why we don't poll layout
            // here.
            if !is_visible_for_raf.get() {
                if let Some(window) = web_sys::window() {
                    if let Some(closure) = raf_c_inner.borrow().as_ref() {
                        let id = window
                            .request_animation_frame(closure.as_ref().unchecked_ref())
                            .unwrap_or(0);
                        *raf_h_inner.borrow_mut() = Some(id);
                    }
                }
                return;
            }

            let mut state = gs_render.borrow_mut();
            if !state.is_running {
                return;
            }

            // Dynamic canvas resize: check parent size each frame
            if let Some(parent) = canvas_for_resize.parent_element() {
                let rect = parent.get_bounding_client_rect();
                let pw = rect.width();
                let ph = rect.height();
                if pw > 1.0 && ph > 1.0 {
                    let cur_w = canvas_for_resize.width() as f64 / dpr;
                    let cur_h = canvas_for_resize.height() as f64 / dpr;
                    if (pw - cur_w).abs() > 1.0 || (ph - cur_h).abs() > 1.0 {
                        canvas_for_resize.set_width((pw * dpr) as u32);
                        canvas_for_resize.set_height((ph * dpr) as u32);
                        let _ = ctx.set_transform(dpr, 0.0, 0.0, dpr, 0.0, 0.0);
                        state.viewport.width = pw;
                        state.viewport.height = ph;
                        state.viewport.offset.x = pw / 2.0;
                        state.viewport.offset.y = ph / 2.0;
                        // Refit content to the new canvas size. Only when nodes are loaded
                        // (otherwise nodes is empty and fit_to_content early-returns).
                        // Reborrow through `&mut *state` so the borrow checker sees disjoint
                        // field borrows on GraphState rather than two simultaneous borrows of
                        // the RefMut wrapper.
                        if !state.nodes.is_empty() {
                            let state = &mut *state;
                            state.viewport.fit_to_content(&state.nodes, 0.10);
                        }
                    }
                }
            }

            // ---------------------------------------------------------------
            // Radial nav branch: NavState-aware rendering
            // ---------------------------------------------------------------
            if let Some(ref nav_rc) = nav_render {
                let now = now_ms();

                // Advance animation state
                nav_rc.borrow_mut().tick(now);

                // Tick the drag controller. Compute frame dt with a cap to protect
                // against background-tab resume spikes (3 frames at 60fps = 0.05s).
                let prev_frame = last_frame_ms_inner.get();
                let dt_s: f64 = if prev_frame > 0.0 {
                    ((now - prev_frame) / 1000.0).min(0.05)
                } else {
                    0.016
                };
                last_frame_ms_inner.set(now);
                if let Some(promote_target) = drag_state_inner.borrow_mut().tick(dt_s) {
                    // Route promote completion through the existing SelectNode path —
                    // mod.rs already maps SelectNode → active_request.set(Some(id)),
                    // which drives the radial neighborhood re-fetch + animation.
                    on_event_for_promote.run(CanvasEvent::SelectNode(promote_target));
                }

                // Snapshot NavState to avoid holding borrow during draw
                let nav_state = nav_rc.borrow().state.clone();
                let drag = state.drag_offset;
                let selected = state.selected_node.as_deref().map(str::to_string);
                let hovered = state.hovered_node.as_deref().map(str::to_string);
                let viewport = state.viewport.clone();
                let current_zoom = viewport.scale as f32;
                // Release the borrow before calling draw functions
                drop(state);

                // Publish zoom, hover, selected to overlay signals.
                zoom_signal_inner.set(current_zoom);
                if hovered_id_sig_inner.get_untracked().as_deref() != hovered.as_deref() {
                    hovered_id_sig_inner.set(hovered.clone());
                }
                if selected_id_sig_inner.get_untracked().as_deref() != selected.as_deref() {
                    selected_id_sig_inner.set(selected.clone());
                }

                // Flatten all visible nodes for the overlay list and publish screen positions.
                // We do this for Active and Animating states (same neighborhood source).
                let flat_nodes: Vec<CanvasNode> = match &nav_state {
                    NavState::Active { neighborhood, .. } => {
                        std::iter::once(neighborhood.center.clone())
                            .chain(neighborhood.one_hop.iter().cloned())
                            .chain(neighborhood.two_hop.iter().cloned())
                            .chain(neighborhood.orphans.iter().cloned())
                            .collect()
                    }
                    NavState::Animating { to_neighborhood, .. } => {
                        std::iter::once(to_neighborhood.center.clone())
                            .chain(to_neighborhood.one_hop.iter().cloned())
                            .chain(to_neighborhood.two_hop.iter().cloned())
                            .chain(to_neighborhood.orphans.iter().cloned())
                            .collect()
                    }
                    _ => Vec::new(),
                };

                // Publish per-node screen positions (get-or-create inner signal per node).
                for n in &flat_nodes {
                    let screen = viewport.world_to_screen(n.position);
                    let sx = screen.x as f32;
                    let sy = screen.y as f32;
                    // Check if a signal already exists for this node id.
                    let existing = node_screen_pos_inner.with(|m| m.get(&n.id).copied());
                    if let Some(sig) = existing {
                        sig.set((sx, sy));
                    } else {
                        let sig: RwSignal<(f32, f32)> = RwSignal::new((sx, sy));
                        node_screen_pos_inner.update(|m| {
                            m.insert(n.id.clone(), sig);
                        });
                    }
                }

                // Update overlay_nodes when the set of visible nodes changes.
                // Compare by length+id to avoid spurious re-renders.
                let needs_update = overlay_nodes_inner.with(|prev| {
                    prev.len() != flat_nodes.len()
                        || prev.iter().zip(flat_nodes.iter()).any(|(a, b)| a.id != b.id)
                });
                if needs_update {
                    overlay_nodes_inner.set(flat_nodes);
                }

                // ---------------------------------------------------------------
                // Publish per-edge label screen positions (Task 7.2).
                // Only processes edges that carry a label (label.is_some()).
                // Node positions are read from the nav state to keep them in sync
                // with the same neighborhood that drives the canvas draw.
                // ---------------------------------------------------------------
                {
                    // Build a quick id→world_pos map from the current neighborhood
                    // (active or animating). We borrow only what we need.
                    let node_world: HashMap<&str, Vec2> = match &nav_state {
                        NavState::Active { neighborhood, .. } => {
                            std::iter::once(&neighborhood.center)
                                .chain(neighborhood.one_hop.iter())
                                .chain(neighborhood.two_hop.iter())
                                .map(|n| (n.id.as_str(), n.position))
                                .collect()
                        }
                        NavState::Animating { to_neighborhood, .. } => {
                            std::iter::once(&to_neighborhood.center)
                                .chain(to_neighborhood.one_hop.iter())
                                .chain(to_neighborhood.two_hop.iter())
                                .map(|n| (n.id.as_str(), n.position))
                                .collect()
                        }
                        _ => HashMap::new(),
                    };

                    // Highlight source: selected node wins, hovered is fallback.
                    let highlight: Option<&str> = selected.as_deref().or(hovered.as_deref());

                    // Iterate the GraphState edges snapshot (set by seed_graph_state).
                    // We need the edges vector — re-borrow state briefly for edges only.
                    // state was already dropped above; use gs_render for a fresh borrow.
                    let edges_snapshot: Vec<(String, String, String)> = gs_render
                        .borrow()
                        .edges
                        .iter()
                        .filter_map(|e| {
                            e.label.as_ref().map(|lbl| {
                                (e.from_id.clone(), e.to_id.clone(), lbl.clone())
                            })
                        })
                        .collect();

                    for (from_id, to_id, label_text) in edges_snapshot {
                        let Some(&from_world) = node_world.get(from_id.as_str()) else {
                            continue;
                        };
                        let Some(&to_world) = node_world.get(to_id.as_str()) else {
                            continue;
                        };

                        let cp = edge_control_point(from_world, to_world, DEFAULT_SAG);
                        let mid_world = bezier_point(from_world, cp, to_world, 0.5);
                        let mid_screen = viewport.world_to_screen(mid_world);

                        let tangent_clamped = bezier_tangent(from_world, cp, to_world, 0.5)
                            .clamp(
                                -(std::f64::consts::FRAC_PI_4),
                                std::f64::consts::FRAC_PI_4,
                            ) as f32;

                        let is_visible = current_zoom >= 0.7
                            && highlight
                                .map(|h| h == from_id || h == to_id)
                                .unwrap_or(false);

                        let key = (from_id.clone(), to_id.clone());
                        let existing = edge_label_state_inner
                            .with(|m| m.get(&key).map(|e| (e.pos_sig, e.tangent_sig, e.visible_sig)));

                        if let Some((pos_sig, tan_sig, vis_sig)) = existing {
                            pos_sig.set((mid_screen.x as f32, mid_screen.y as f32));
                            tan_sig.set(tangent_clamped);
                            vis_sig.set(is_visible);
                        } else {
                            let pos_sig = RwSignal::new((mid_screen.x as f32, mid_screen.y as f32));
                            let tangent_sig = RwSignal::new(tangent_clamped);
                            let visible_sig = RwSignal::new(is_visible);
                            edge_label_state_inner.update(|m| {
                                m.insert(key, EdgeLabelEntry {
                                    label: label_text,
                                    pos_sig,
                                    tangent_sig,
                                    visible_sig,
                                });
                            });
                        }
                    }
                }

                match nav_state {
                    NavState::Active { neighborhood, .. } => {
                        let center_radius = note_radius(neighborhood.center.edge_count);
                        let overlay = drag_state_inner
                            .borrow()
                            .overlay_snapshot(Vec2::zero(), center_radius);
                        draw_neighborhood(
                            &ctx,
                            &viewport,
                            &neighborhood,
                            drag,
                            selected.as_deref(),
                            hovered.as_deref(),
                            overlay.as_ref(),
                        );
                    }
                    NavState::Animating {
                        from_neighborhood,
                        to_neighborhood,
                        t,
                        ..
                    } => {
                        let center_radius = note_radius(to_neighborhood.center.edge_count);
                        let interp = build_interpolated_neighborhood(
                            &from_neighborhood,
                            &to_neighborhood,
                            t,
                        );
                        let overlay = drag_state_inner
                            .borrow()
                            .overlay_snapshot(Vec2::zero(), center_radius);
                        draw_neighborhood(
                            &ctx,
                            &viewport,
                            &interp,
                            drag,
                            selected.as_deref(),
                            hovered.as_deref(),
                            overlay.as_ref(),
                        );
                    }
                    NavState::Loading { .. } => {
                        draw_placeholder(&ctx, &viewport, "Loading…");
                    }
                    NavState::Error { ref reason, .. } => {
                        let msg = format!("Error: {reason}");
                        draw_placeholder(&ctx, &viewport, &msg);
                    }
                    NavState::Idle => {
                        draw_placeholder(&ctx, &viewport, "");
                    }
                }
            }

            // Schedule next frame
            if let Some(window) = web_sys::window() {
                let cb = raf_c_inner.borrow();
                if let Some(closure) = cb.as_ref() {
                    let id = window
                        .request_animation_frame(closure.as_ref().unchecked_ref())
                        .unwrap_or(0);
                    *raf_h_inner.borrow_mut() = Some(id);
                }
            }
        });

        // Store closure and kick off first frame
        if let Some(window) = web_sys::window() {
            let id = window
                .request_animation_frame(closure.as_ref().unchecked_ref())
                .unwrap_or(0);
            *raf_h.borrow_mut() = Some(id);
        }
        *raf_c.borrow_mut() = Some(closure);
    });

    // Pointer event handlers (mouse / pen / single-touch unified).
    // Using PointerEvent + setPointerCapture so drag survives when the
    // pointer leaves the canvas rect mid-gesture — a long-standing
    // bug under the legacy MouseEvent path where mouseup never fired
    // off-canvas, leaving the drag controller wedged in "pressed".
    let gs_down = graph_state.clone();
    let on_pointerdown = move |ev: web_sys::PointerEvent| {
        // Capture pointer so all subsequent move/up events route to this
        // element even if the pointer travels outside the canvas rect.
        // Best-effort: pointer capture not being granted is non-fatal.
        if let Some(target) = ev.target() {
            if let Ok(el) = target.dyn_into::<web_sys::Element>() {
                let _ = el.set_pointer_capture(ev.pointer_id());
            }
        }

        let mut state = gs_down.borrow_mut();
        let screen = Vec2::new(ev.offset_x() as f64, ev.offset_y() as f64);

        // Elastic-drag gating (Task 7): if the press lands on a 1-hop neighbor of
        // the current radial center, route through DragState instead of pan/legacy
        // node-drag. World-space contract: convert pointer screen → world before
        // calling DragState::press so overlay positions land in world coordinates.
        if let Some(ref nav_rc) = nav_for_md {
            // Block drag while a retarget tween is in flight: starting a new
            // drag mid-tween would race the in-progress neighborhood swap.
            if nav_rc.borrow().is_animating() {
                return;
            }
            let one_hop_owned: Vec<CanvasNode> = {
                let n = nav_rc.borrow();
                match &n.state {
                    NavState::Active { neighborhood, .. } => neighborhood.one_hop.clone(),
                    NavState::Animating {
                        to_neighborhood, ..
                    } => to_neighborhood.one_hop.clone(),
                    _ => Vec::new(),
                }
            };
            if let Some(hit_idx) = state.viewport.hit_test(screen, &one_hop_owned) {
                let world = state.viewport.screen_to_world(screen);
                let node_id = one_hop_owned[hit_idx].id.clone();
                drop(state);
                drag_state_for_md
                    .borrow_mut()
                    .press(node_id, world, now_ms());
                return;
            }
        }

        state.interaction.mouse_down_screen = screen;
        state.interaction.last_mouse_screen = screen;
        state.interaction.mouse_down_time = js_sys::Date::now();

        if let Some(idx) = state.viewport.hit_test(screen, &state.nodes) {
            state.interaction.is_dragging_node = true;
            state.interaction.dragged_node_idx = Some(idx);
        } else {
            state.interaction.is_panning = true;
        }
    };

    let gs_move = graph_state.clone();
    let on_pointermove = move |ev: web_sys::PointerEvent| {
        let mut state = gs_move.borrow_mut();
        let screen = Vec2::new(ev.offset_x() as f64, ev.offset_y() as f64);

        // Elastic drag in flight: forward world-space pointer to DragState and
        // skip pan/hover. World-space contract: convert before handing off.
        if drag_state_for_mm.borrow().is_active() {
            let world = state.viewport.screen_to_world(screen);
            drop(state);
            drag_state_for_mm.borrow_mut().pointer_move(world, now_ms());
            return;
        }

        let dx = screen.x - state.interaction.last_mouse_screen.x;
        let dy = screen.y - state.interaction.last_mouse_screen.y;
        state.interaction.last_mouse_screen = screen;

        if state.interaction.is_panning {
            state.viewport.pan(dx, dy);
            // Accumulate drag offset for parallax in the radial renderer
            state.drag_offset.0 += dx as f32;
            state.drag_offset.1 += dy as f32;
        } else if state.interaction.is_dragging_node {
            if let Some(idx) = state.interaction.dragged_node_idx {
                let world = state.viewport.screen_to_world(screen);
                if let Some(node) = state.nodes.get_mut(idx) {
                    node.position = world;
                }
            }
        } else {
            // Hover detection
            let hit = state.viewport.hit_test(screen, &state.nodes);
            let new_hovered = hit.and_then(|idx| state.nodes.get(idx).map(|n| n.id.clone()));
            if new_hovered != state.hovered_node {
                state.hovered_node = new_hovered.clone();
                drop(state);
                on_event.run(CanvasEvent::HoverNode(new_hovered));
            }
        }
    };

    let gs_up = graph_state.clone();
    let on_pointerup = move |ev: web_sys::PointerEvent| {
        // Release the pointer capture acquired in on_pointerdown. Safe
        // even if no capture was granted — releasePointerCapture is a
        // no-op for non-captured pointers per spec.
        if let Some(target) = ev.target() {
            if let Ok(el) = target.clone().dyn_into::<web_sys::Element>() {
                let _ = el.release_pointer_capture(ev.pointer_id());
            }
        }
        // Elastic-drag release: process through DragState if a drag is active.
        // Click  → SelectNode (re-uses existing intent path for breadcrumb/fetch).
        // Promote→ deferred until tick() emits target on tween completion.
        // SpringBack → tick() animates; nothing to do here.
        let active_node = drag_state_for_mu
            .borrow()
            .active_node_id()
            .map(|s| s.to_string());
        if let Some(node_id) = active_node {
            let center_radius = if let Some(ref nav_rc) = nav_for_mu {
                let n = nav_rc.borrow();
                match &n.state {
                    NavState::Active { neighborhood, .. } => {
                        note_radius(neighborhood.center.edge_count)
                    }
                    NavState::Animating {
                        to_neighborhood, ..
                    } => note_radius(to_neighborhood.center.edge_count),
                    _ => 24.0_f64,
                }
            } else {
                24.0_f64
            };
            let outcome = drag_state_for_mu.borrow_mut().release(
                Vec2::zero(),
                center_radius,
                &node_id,
                now_ms(),
            );
            match outcome {
                ReleaseOutcome::Click => {
                    on_event.run(CanvasEvent::SelectNode(node_id));
                }
                ReleaseOutcome::SpringBack | ReleaseOutcome::Promote { .. } => {
                    // tick() drives the rest; promote target emits via on_event in rAF.
                }
            }
            let _ = ev;
            return;
        }

        let mut state = gs_up.borrow_mut();
        let screen = Vec2::new(ev.offset_x() as f64, ev.offset_y() as f64);
        let now = js_sys::Date::now();

        if state.interaction.is_click(screen) {
            // Reset drag parallax offset on click (not a pan gesture)
            state.drag_offset = (0.0, 0.0);

            let hit = state.viewport.hit_test(screen, &state.nodes);
            if let Some(idx) = hit {
                let node_id = state.nodes[idx].id.clone();

                if state.interaction.is_double_click(now) {
                    // Double-click: enter local view
                    drop(state);
                    on_event.run(CanvasEvent::EnterLocalView(node_id));
                } else {
                    // Single click: select + pan to center
                    state.selected_node = Some(node_id.clone());
                    state.interaction.last_click_time = now;
                    if let Some(node) = state.nodes.get(idx) {
                        state.pan_target = Some(node.position);
                        state.pan_progress = 0.0;
                    }
                    drop(state);
                    on_event.run(CanvasEvent::SelectNode(node_id));
                }
            } else {
                state.selected_node = None;
                state.interaction.last_click_time = 0.0;
                drop(state);
                on_event.run(CanvasEvent::DeselectNode);
            }
        } else {
            // Pan end: reset drag offset
            state.drag_offset = (0.0, 0.0);
            // Reset drag/pan state only
            state.interaction.is_panning = false;
            state.interaction.is_dragging_node = false;
            state.interaction.dragged_node_idx = None;
            return;
        }

        // Always reset interaction flags
        let mut state = gs_up.borrow_mut();
        state.interaction.is_panning = false;
        state.interaction.is_dragging_node = false;
        state.interaction.dragged_node_idx = None;
    };

    let gs_wheel = graph_state.clone();
    let on_wheel = move |ev: web_sys::WheelEvent| {
        ev.prevent_default();
        let mut state = gs_wheel.borrow_mut();
        let screen = Vec2::new(ev.offset_x() as f64, ev.offset_y() as f64);

        // Normalize delta across deltaMode (0=pixel, 1=line, 2=page).
        // Pre-Pointer code assumed pixel mode and used a flat 0.001 factor;
        // Firefox/Chrome on Linux still emit DELTA_LINE for legacy mice,
        // so a fixed factor causes wildly different zoom speeds depending
        // on browser+OS combo.
        let line_height_px = 16.0_f64;
        let pixel_dy = match ev.delta_mode() {
            1 => ev.delta_y() * line_height_px,
            2 => ev.delta_y() * line_height_px * 16.0,
            _ => ev.delta_y(),
        };

        // macOS trackpad pinch-zoom synthesizes wheel events with
        // ctrlKey=true (a Webkit convention since adopted everywhere).
        // Amplify the factor so pinch-zoom feels responsive vs scroll-zoom.
        let factor = if ev.ctrl_key() { 0.01 } else { 0.0015 };
        let delta = -pixel_dy * factor;
        state.viewport.zoom_at(screen, delta);
    };

    // Keyboard zoom: Cmd/Ctrl + [+ | - | 0]. Canvas needs `tabindex="0"`
    // to receive focus and keydown events — set in the view! below.
    // Cmd+0 resets to scale=1 and recentres the offset, matching the
    // viewport's initial state from Viewport::new().
    let gs_key = graph_state.clone();
    let on_keydown = move |ev: web_sys::KeyboardEvent| {
        if !(ev.ctrl_key() || ev.meta_key()) {
            return;
        }
        let key = ev.key();
        let mut state = gs_key.borrow_mut();
        let center = Vec2::new(state.viewport.width / 2.0, state.viewport.height / 2.0);
        match key.as_str() {
            "=" | "+" => {
                ev.prevent_default();
                state.viewport.zoom_at(center, 0.2);
            }
            "-" | "_" => {
                ev.prevent_default();
                state.viewport.zoom_at(center, -0.2);
            }
            "0" => {
                ev.prevent_default();
                state.viewport.scale = 1.0;
                state.viewport.offset = center;
                state.drag_offset = (0.0, 0.0);
            }
            _ => {}
        }
    };

    // The rAF chain is leaked on purpose: the parent <MainContent> keeps
    // <CanvasView /> mounted (only toggles display:none) so `on_cleanup`
    // does not fire on route change. The closure's own visibility guard
    // (offset_width == 0 → return) does the heavy lifting: zero per-frame
    // work when hidden, instant resume when shown.
    //
    // NB: wasm32 is single-threaded so on_cleanup IS available — the
    // historical `Send` excuse was wrong. The real reason we skip it is
    // that unmount never happens with the current routing pattern.

    // pointercancel fires when the system aborts the gesture (e.g. another
    // gesture took over, scroll chaining kicked in, pen lifted on touchpad).
    // Treat it as a hard drag-cancel — same semantics as the prior
    // mouseleave path (spec §6.3) but driven by the proper Pointer event.
    let on_pointercancel = move |_ev: web_sys::PointerEvent| {
        drag_state_for_leave.borrow_mut().cancel();
    };

    view! {
        <div class="relative w-full h-full">
            <canvas
                node_ref=canvas_ref
                class="w-full h-full block focus:outline-none"
                style="cursor: grab; touch-action: none;"
                tabindex="0"
                on:pointerdown=on_pointerdown
                on:pointermove=on_pointermove
                on:pointerup=on_pointerup
                on:pointercancel=on_pointercancel
                on:wheel=on_wheel
                on:keydown=on_keydown
            />
            // DOM overlay: NodeCard components positioned over each visible node.
            // pointer-events-none on the wrapper so mouse events fall through to the canvas;
            // individual cards re-enable pointer events for click handling.
            <div class="absolute inset-0 pointer-events-none overflow-hidden">
                {move || {
                    let nodes = overlay_nodes.get();
                    nodes.into_iter().map(|n| {
                        let id_clone = n.id.clone();
                        // Get-or-create the per-node screen position signal.
                        let pos_sig: RwSignal<(f32, f32)> =
                            node_screen_pos.with(|m| m.get(&n.id).copied())
                                .unwrap_or_else(|| RwSignal::new((0.0_f32, 0.0_f32)));

                        let on_card_click = Callback::new(move |_id: String| {
                            selected_id_sig.set(Some(id_clone.clone()));
                        });

                        let id_lookup = n.id.clone();
                        let excerpt = excerpt_by_id
                            .as_ref()
                            .and_then(|sig| sig.with(|m| m.get(&id_lookup).cloned()))
                            .unwrap_or_default();

                        view! {
                            <NodeCard
                                id=n.id.clone()
                                name=n.name.clone()
                                category=n.category.clone()
                                tags=vec![]
                                excerpt_html=excerpt
                                hop=n.hop
                                screen_xy=pos_sig.read_only()
                                zoom=zoom_signal.read_only()
                                hovered_id=hovered_id_sig.read_only()
                                selected_id=selected_id_sig.read_only()
                                on_click=on_card_click
                            />
                        }
                    }).collect_view()
                }}
            </div>
            // Edge label overlay: EdgeLabel components positioned at Bézier midpoints.
            // All labels are pointer-events-none (set inside EdgeLabel's style).
            <div class="absolute inset-0 pointer-events-none overflow-hidden">
                {move || {
                    edge_label_state.with(|m| {
                        m.values().map(|el| {
                            let text = el.label.clone();
                            let pos = el.pos_sig.read_only();
                            let tan = el.tangent_sig.read_only();
                            let vis = el.visible_sig.read_only();
                            view! {
                                <EdgeLabel
                                    text=text
                                    screen_xy=pos
                                    tangent_rad=tan
                                    visible=vis
                                />
                            }
                        }).collect_view()
                    })
                }}
            </div>
        </div>
    }
}
