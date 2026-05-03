use std::cell::{Cell, RefCell};
use std::rc::Rc;

use leptos::prelude::*;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;

use leptos::callback::Callback;

use crate::canvas_engine::drag::{DragState, ReleaseOutcome};
use crate::canvas_engine::interaction::{CanvasEvent, InteractionState};
use crate::canvas_engine::navigation::NavController;
use crate::canvas_engine::renderer::draw_neighborhood;
use crate::canvas_engine::tween::build_interpolated_neighborhood;
use crate::canvas_engine::types::*;
use crate::canvas_engine::viewport::Viewport;

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
) -> impl IntoView {
    let canvas_ref = NodeRef::<leptos::html::Canvas>::new();
    let raf_handle: Rc<RefCell<Option<i32>>> = Rc::new(RefCell::new(None));
    let raf_closure: RafClosure = Rc::new(RefCell::new(None));

    // Drag controller (Task 7): owns the elastic node-drag state machine.
    // Pointer events feed it; the rAF loop ticks it; the renderer reads its overlay.
    let drag_state: Rc<RefCell<DragState>> = Rc::new(RefCell::new(DragState::new()));
    let last_frame_ms: Rc<Cell<f64>> = Rc::new(Cell::new(0.0));

    // Clone handles once for each event closure that needs them, since `nav`,
    // `drag_state`, and `last_frame_ms` are all moved into the rAF Effect closure
    // below. (Effect::new takes `move ||`, which consumes its captures.)
    let nav_for_md = nav.clone();
    let nav_for_mu = nav.clone();
    let drag_state_for_md = drag_state.clone();
    let drag_state_for_mm = drag_state.clone();
    let drag_state_for_mu = drag_state.clone();
    let drag_state_for_leave = drag_state.clone();

    // Start render loop after mount
    let gs = graph_state.clone();
    let raf_h = raf_handle.clone();
    let raf_c = raf_closure.clone();

    Effect::new(move || {
        let Some(canvas_el) = canvas_ref.get() else {
            return;
        };

        let canvas: web_sys::HtmlCanvasElement = canvas_el.into();

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

        let canvas_for_resize = canvas.clone();
        let closure: Closure<dyn FnMut()> = Closure::new(move || {
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
                // Release the borrow before calling draw functions
                drop(state);

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

    // Mouse event handlers
    let gs_down = graph_state.clone();
    let on_mousedown = move |ev: web_sys::MouseEvent| {
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
            state.nodes[idx].pinned = true;
        } else {
            state.interaction.is_panning = true;
        }
    };

    let gs_move = graph_state.clone();
    let on_mousemove = move |ev: web_sys::MouseEvent| {
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
    let on_mouseup = move |ev: web_sys::MouseEvent| {
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

        if state.interaction.is_dragging_node {
            if let Some(idx) = state.interaction.dragged_node_idx {
                if let Some(node) = state.nodes.get_mut(idx) {
                    node.pinned = false;
                }
            }
        }

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
        let delta = -ev.delta_y() * 0.001;
        state.viewport.zoom_at(screen, delta);
    };

    // The rAF render loop self-terminates via the `is_running` flag.
    // When the parent CanvasView is hidden (display:none), the canvas element
    // is still alive but the loop checks `is_running` every frame.
    // Explicit cleanup is not needed because Leptos on_cleanup requires Send
    // which Rc<RefCell<_>> does not satisfy on multi-threaded targets.

    let on_mouseleave = move |_ev: web_sys::MouseEvent| {
        // Hard-cancel any in-flight drag without spring-back animation (spec §6.3).
        drag_state_for_leave.borrow_mut().cancel();
    };

    view! {
        <canvas
            node_ref=canvas_ref
            class="w-full h-full block"
            style="cursor: grab;"
            on:mousedown=on_mousedown
            on:mousemove=on_mousemove
            on:mouseup=on_mouseup
            on:mouseleave=on_mouseleave
            on:wheel=on_wheel
        />
    }
}
