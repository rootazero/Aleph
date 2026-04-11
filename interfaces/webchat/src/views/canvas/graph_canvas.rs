use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;

use leptos::prelude::*;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;

use leptos::callback::Callback;

use crate::canvas_engine::interaction::{CanvasEvent, InteractionState};
use crate::canvas_engine::layout::ForceLayout;
use crate::canvas_engine::renderer::Renderer;
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
        }
    }
}

impl Default for GraphState {
    fn default() -> Self {
        Self::new()
    }
}

type RafClosure = Rc<RefCell<Option<Closure<dyn FnMut()>>>>;

#[component]
pub fn GraphCanvas(
    graph_state: Rc<RefCell<GraphState>>,
    on_event: Callback<CanvasEvent>,
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

        // Set canvas size to container
        let rect = canvas.get_bounding_client_rect();
        let w = rect.width();
        let h = rect.height();
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
        let raf_h_inner = raf_h.clone();
        let raf_c_inner = raf_c.clone();

        let closure: Closure<dyn FnMut()> = Closure::new(move || {
            let mut state = gs_render.borrow_mut();
            if !state.is_running {
                return;
            }

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

            // Render
            Renderer::draw(
                &ctx,
                &state.viewport,
                &state.nodes,
                &state.edges,
                state.selected_node.as_deref(),
                state.hovered_node.as_deref(),
                &state.kind_filter,
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
            let hit = state.viewport.hit_test(screen, &state.nodes);
            if let Some(idx) = hit {
                let node_id = state.nodes[idx].id.clone();

                if state.interaction.is_double_click(now) {
                    // Double-click: enter local view
                    drop(state);
                    on_event.run(CanvasEvent::EnterLocalView(node_id));
                } else {
                    // Single click: select
                    state.selected_node = Some(node_id.clone());
                    state.interaction.last_click_time = now;
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
