use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;

use leptos::prelude::*;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;

use leptos::callback::Callback;

use crate::canvas_engine::interaction::{CanvasEvent, InteractionState};
use crate::canvas_engine::layout::ForceLayout;
use crate::canvas_engine::navigation::NavController;
use crate::canvas_engine::renderer::{draw_neighborhood, Renderer};
use crate::canvas_engine::tween::lerp_node;
use crate::canvas_engine::types::*;
use crate::canvas_engine::viewport::Viewport;

/// Shared mutable state for 60fps canvas rendering (not reactive).
pub struct GraphState {
    pub nodes: Vec<CanvasNode>,
    pub edges: Vec<CanvasEdge>,
    pub viewport: Viewport,
    pub layout: ForceLayout,
    pub interaction: InteractionState,
    pub selected_node: Option<String>,
    pub hovered_node: Option<String>,
    pub kind_filter: HashSet<String>,
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
            layout: ForceLayout::new(),
            interaction: InteractionState::new(),
            selected_node: None,
            hovered_node: None,
            kind_filter: HashSet::new(),
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

/// Build an interpolated Neighborhood at tween parameter `t` between `from` and `to`.
/// The resulting neighborhood's `target_positions` map contains lerped Vec3 positions
/// for every node id that appears in either neighborhood.
fn build_interpolated_neighborhood(from: &Neighborhood, to: &Neighborhood, t: f32) -> Neighborhood {
    let mut all_ids: HashSet<String> = HashSet::new();
    all_ids.insert(from.center.id.clone());
    all_ids.insert(to.center.id.clone());
    all_ids.extend(from.one_hop.iter().map(|n| n.id.clone()));
    all_ids.extend(from.two_hop.iter().map(|n| n.id.clone()));
    all_ids.extend(to.one_hop.iter().map(|n| n.id.clone()));
    all_ids.extend(to.two_hop.iter().map(|n| n.id.clone()));

    let mut interp = to.clone();
    for id in all_ids {
        let r = lerp_node(&id, from, to, t);
        interp.target_positions.insert(id, r.pos);
    }
    interp
}

/// Draw a simple placeholder when the nav is Idle, Loading, or in Error state.
fn draw_placeholder(
    ctx: &web_sys::CanvasRenderingContext2d,
    viewport: &Viewport,
    message: &str,
) {
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
    /// When `Some`, the RAF loop uses NavState-aware rendering.
    /// When `None`, falls back to the legacy flat `Renderer::draw`.
    #[prop(optional)]
    nav: Option<Rc<RefCell<NavController>>>,
) -> impl IntoView {
    let canvas_ref = NodeRef::<leptos::html::Canvas>::new();
    let raf_handle: Rc<RefCell<Option<i32>>> = Rc::new(RefCell::new(None));
    let raf_closure: RafClosure = Rc::new(RefCell::new(None));

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
                        draw_neighborhood(
                            &ctx,
                            &viewport,
                            &neighborhood,
                            drag,
                            selected.as_deref(),
                            hovered.as_deref(),
                        );
                    }
                    NavState::Animating {
                        from_neighborhood,
                        to_neighborhood,
                        t,
                        ..
                    } => {
                        let interp =
                            build_interpolated_neighborhood(&from_neighborhood, &to_neighborhood, t);
                        draw_neighborhood(
                            &ctx,
                            &viewport,
                            &interp,
                            drag,
                            selected.as_deref(),
                            hovered.as_deref(),
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
                return;
            }

            // ---------------------------------------------------------------
            // Legacy flat-graph branch (no nav controller)
            // ---------------------------------------------------------------

            // Physics tick — use destructuring to get disjoint borrows
            if !state.layout.is_settled {
                let GraphState {
                    ref mut nodes,
                    ref edges,
                    ref mut layout,
                    ..
                } = *state;
                layout.tick(nodes, edges);
            }

            // Smooth pan-to-center animation
            if let Some(target) = state.pan_target {
                state.pan_progress += 0.08; // ~300ms at 60fps
                if state.pan_progress >= 1.0 {
                    // Snap to target
                    state.viewport.offset.x =
                        state.viewport.width / 2.0 - target.x * state.viewport.scale;
                    state.viewport.offset.y =
                        state.viewport.height / 2.0 - target.y * state.viewport.scale;
                    state.pan_target = None;
                    state.pan_progress = 0.0;
                } else {
                    // Ease-out interpolation
                    let t = 1.0 - (1.0 - state.pan_progress).powi(3);
                    let current_center_x = (state.viewport.width / 2.0 - state.viewport.offset.x)
                        / state.viewport.scale;
                    let current_center_y = (state.viewport.height / 2.0 - state.viewport.offset.y)
                        / state.viewport.scale;
                    let new_x = current_center_x + (target.x - current_center_x) * t;
                    let new_y = current_center_y + (target.y - current_center_y) * t;
                    state.viewport.offset.x =
                        state.viewport.width / 2.0 - new_x * state.viewport.scale;
                    state.viewport.offset.y =
                        state.viewport.height / 2.0 - new_y * state.viewport.scale;
                }
            }

            // Compute highlighted neighbors for hover dimming
            let highlighted_neighbors: HashSet<String> =
                if let Some(ref hov_id) = state.hovered_node {
                    state
                        .edges
                        .iter()
                        .filter_map(|e| {
                            let from = state.nodes.get(e.from_idx)?;
                            let to = state.nodes.get(e.to_idx)?;
                            if from.id == *hov_id {
                                Some(to.id.clone())
                            } else if to.id == *hov_id {
                                Some(from.id.clone())
                            } else {
                                None
                            }
                        })
                        .collect()
                } else {
                    HashSet::new()
                };

            // Render
            Renderer::draw(
                &ctx,
                &state.viewport,
                &state.nodes,
                &state.edges,
                state.selected_node.as_deref(),
                state.hovered_node.as_deref(),
                &state.kind_filter,
                &highlighted_neighbors,
            );

            drop(state);

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
                state.layout.wake();
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

    view! {
        <canvas
            node_ref=canvas_ref
            class="w-full h-full block"
            style="cursor: grab;"
            on:mousedown=on_mousedown
            on:mousemove=on_mousemove
            on:mouseup=on_mouseup
            on:wheel=on_wheel
        />
    }
}
