//! WebGL2 galaxy canvas host. Owns the `<canvas>`, rAF loop, pointer events.
//!
//! Architecture note (`canvas/mod.rs:48-54`): `Scene` holds web-sys GL handles
//! → it is NOT `Send`. Keep it in `Rc<RefCell<Option<Scene>>>` captured by the
//! rAF closure and Effects. The `on_event` `Callback` captures only `Copy`
//! reactive signals so it satisfies `Send + Sync`.
//!
//! Intent channels (`focus_request`, `highlight_request`) let the host drive
//! the scene without violating the non-Send constraint: the host writes to a
//! `RwSignal`, an Effect inside `GalaxyCanvas` reads it and calls the Scene.

use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::rc::Rc;

use leptos::callback::Callback;
use leptos::prelude::*;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;

use crate::canvas_engine::interaction::CanvasEvent;
use super::gl::scene::Scene;
use super::gl::GraphData;

/// Click threshold in CSS pixels. A pointer-up within this distance of
/// pointer-down counts as a click; larger = drag (no selection).
const CLICK_THRESHOLD_PX: f32 = 5.0;

#[component]
#[must_use]
pub fn GalaxyCanvas(
    graph: RwSignal<Option<GraphData>>,
    on_event: Callback<CanvasEvent>,
    /// Intent channel: when `Some(id)`, fly the camera to that node.
    focus_request: RwSignal<Option<String>>,
    /// Intent channel: when `Some(indices)`, highlight those node indices.
    highlight_request: RwSignal<Option<HashSet<u32>>>,
) -> impl IntoView {
    let canvas_ref = NodeRef::<leptos::html::Canvas>::new();

    // Non-Send GL scene — lives in Rc<RefCell> never crossed to another thread.
    let scene: Rc<RefCell<Option<Scene>>> = Rc::new(RefCell::new(None));

    // Last pointer position for computing drag deltas (screen pixels).
    let last_ptr: Rc<Cell<(f32, f32)>> = Rc::new(Cell::new((0.0, 0.0)));
    let ptr_down: Rc<Cell<bool>> = Rc::new(Cell::new(false));

    // Pointer-down position (client pixels) for click-vs-drag detection.
    let down_pos: Rc<Cell<(f32, f32)>> = Rc::new(Cell::new((0.0, 0.0)));

    // Last hovered node id — used to emit HoverNode only on transition.
    let last_hover: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));

    // Canvas element bounding-rect origin (updated on pointerdown for perf).
    // Used to convert client coords → canvas-local coords for picking.
    let canvas_origin: Rc<Cell<(f32, f32)>> = Rc::new(Cell::new((0.0, 0.0)));

    // --- Init Effect: mount scene once the <canvas> is in the DOM ---
    let scene_init = scene.clone();
    Effect::new(move |_| {
        let Some(canvas) = canvas_ref.get() else { return };
        let el: web_sys::HtmlCanvasElement = canvas.unchecked_into();

        // Size canvas to its CSS layout box.
        let (cw, ch) = el
            .parent_element()
            .map(|p| {
                let r = p.get_bounding_client_rect();
                (r.width().max(1.0) as u32, r.height().max(1.0) as u32)
            })
            .unwrap_or((800, 600));
        el.set_width(cw);
        el.set_height(ch);

        match Scene::new(&el) {
            Ok(s) => *scene_init.borrow_mut() = Some(s),
            Err(e) => {
                web_sys::console::error_1(&format!("GalaxyCanvas GL init failed: {e}").into());
                return;
            }
        }

        // ResizeObserver: keep canvas dimensions in sync with its CSS container.
        let scene_resize = scene_init.clone();
        let resize_cb: Closure<dyn FnMut(js_sys::Array)> =
            Closure::new(move |entries: js_sys::Array| {
                if let Ok(entry) = entries.get(0).dyn_into::<web_sys::ResizeObserverEntry>() {
                    let rect = entry.content_rect();
                    let w = rect.width().max(1.0) as i32;
                    let h = rect.height().max(1.0) as i32;
                    // Also resize the canvas backing store.
                    if let Some(target) = entry.target().dyn_into::<web_sys::HtmlCanvasElement>().ok() {
                        target.set_width(w as u32);
                        target.set_height(h as u32);
                    }
                    if let Some(s) = scene_resize.borrow_mut().as_mut() {
                        s.resize(w, h);
                    }
                }
            });
        if let Ok(obs) = web_sys::ResizeObserver::new(resize_cb.as_ref().unchecked_ref()) {
            obs.observe(&el);
        }
        // Leak for panel lifetime — parent uses display:none keep-alive, never unmounts.
        resize_cb.forget();

        start_raf_loop(scene_init.clone());
    });

    // --- Data Effect: push GraphData into scene when it changes ---
    let scene_data = scene.clone();
    Effect::new(move |_| {
        if let Some(data) = graph.get() {
            if let Some(s) = scene_data.borrow_mut().as_mut() {
                s.set_graph(data);
            }
        }
    });

    // --- Intent channel Effect: fly-to ---
    // Reads `focus_request`; applies to the owned scene (non-Send, safe here).
    let scene_focus = scene.clone();
    Effect::new(move |_| {
        let Some(id) = focus_request.get() else { return };
        let t_ms = perf_now();
        if let Some(s) = scene_focus.borrow_mut().as_mut() {
            s.fly_to_node(&id, t_ms);
        }
    });

    // --- Intent channel Effect: highlight ---
    let scene_hl = scene.clone();
    Effect::new(move |_| {
        let hl = highlight_request.get();
        if let Some(s) = scene_hl.borrow_mut().as_mut() {
            s.set_highlight(hl);
        }
    });

    // --- Pointer events ---
    // Drag delta → scene.on_drag; click → pick → on_event; hover → pick → on_event.

    let last_ptr_pd = last_ptr.clone();
    let ptr_down_pd = ptr_down.clone();
    let down_pos_pd = down_pos.clone();
    let canvas_origin_pd = canvas_origin.clone();
    let on_pointerdown = move |ev: web_sys::PointerEvent| {
        // Capture so move/up fire even when pointer leaves canvas.
        if let Some(target) = ev.target() {
            if let Ok(el) = target.dyn_into::<web_sys::Element>() {
                // Snapshot canvas origin for this gesture.
                let rect = el.get_bounding_client_rect();
                canvas_origin_pd.set((rect.left() as f32, rect.top() as f32));
                let _ = el.set_pointer_capture(ev.pointer_id());
            }
        }
        ptr_down_pd.set(true);
        let pos = (ev.client_x() as f32, ev.client_y() as f32);
        last_ptr_pd.set(pos);
        down_pos_pd.set(pos);
    };

    let scene_pm = scene.clone();
    let last_ptr_pm = last_ptr.clone();
    let ptr_down_pm = ptr_down.clone();
    let canvas_origin_pm = canvas_origin.clone();
    let last_hover_pm = last_hover.clone();
    let on_event_pm = on_event;
    let on_pointermove = move |ev: web_sys::PointerEvent| {
        let cx = ev.client_x() as f32;
        let cy = ev.client_y() as f32;

        if ptr_down_pm.get() {
            // Drag: update orbit camera.
            let (lx, ly) = last_ptr_pm.get();
            let dx = cx - lx;
            let dy = cy - ly;
            last_ptr_pm.set((cx, cy));
            let t_ms = perf_now();
            if let Some(s) = scene_pm.borrow_mut().as_mut() {
                s.on_drag(dx, dy, t_ms);
            }
        } else {
            // Hover (no button down): pick and emit HoverNode on transition.
            // Refresh canvas origin from the event target so hover picks work
            // before the first pointerdown.
            if let Some(target) = ev.target() {
                if let Ok(el) = target.dyn_into::<web_sys::Element>() {
                    let rect = el.get_bounding_client_rect();
                    canvas_origin_pm.set((rect.left() as f32, rect.top() as f32));
                }
            }
            let (ox, oy) = canvas_origin_pm.get();
            let local = (cx - ox, cy - oy);
            let hit = scene_pm.borrow().as_ref().and_then(|s| s.pick(local));
            let mut lh = last_hover_pm.borrow_mut();
            if *lh != hit {
                *lh = hit.clone();
                on_event_pm.run(CanvasEvent::HoverNode(hit));
            }
        }
    };

    let scene_pu = scene.clone();
    let ptr_down_pu = ptr_down.clone();
    let down_pos_pu = down_pos.clone();
    let canvas_origin_pu = canvas_origin.clone();
    let on_event_pu = on_event;
    let on_pointerup = move |ev: web_sys::PointerEvent| {
        if let Some(target) = ev.target() {
            if let Ok(el) = target.dyn_into::<web_sys::Element>() {
                let _ = el.release_pointer_capture(ev.pointer_id());
            }
        }
        if ptr_down_pu.get() {
            let cx = ev.client_x() as f32;
            let cy = ev.client_y() as f32;
            let (dx_start, dy_start) = down_pos_pu.get();
            let dist = ((cx - dx_start).powi(2) + (cy - dy_start).powi(2)).sqrt();

            // Click (not drag): pick the node under the cursor.
            if dist < CLICK_THRESHOLD_PX {
                let (ox, oy) = canvas_origin_pu.get();
                let local = (cx - ox, cy - oy);
                let hit = scene_pu.borrow().as_ref().and_then(|s| s.pick(local));
                match hit {
                    Some(id) => on_event_pu.run(CanvasEvent::SelectNode(id)),
                    None => on_event_pu.run(CanvasEvent::DeselectNode),
                }
            }
        }
        ptr_down_pu.set(false);
    };

    let ptr_down_pc = ptr_down.clone();
    let on_pointercancel = move |_ev: web_sys::PointerEvent| {
        ptr_down_pc.set(false);
    };

    let scene_wh = scene.clone();
    let on_wheel = move |ev: web_sys::WheelEvent| {
        ev.prevent_default();
        // Positive deltaY → scroll down → zoom out (negative delta for scene).
        let delta = -(ev.delta_y() as f32);
        let t_ms = perf_now();
        if let Some(s) = scene_wh.borrow_mut().as_mut() {
            s.on_wheel(delta, t_ms);
        }
    };

    view! {
        <canvas
            node_ref=canvas_ref
            class="w-full h-full block"
            style="touch-action: none; cursor: grab;"
            on:pointerdown=on_pointerdown
            on:pointermove=on_pointermove
            on:pointerup=on_pointerup
            on:pointercancel=on_pointercancel
            on:wheel=on_wheel
        />
    }
}

/// Start the `requestAnimationFrame` recursive loop.
/// The closure holds a strong reference to itself through the `Rc<RefCell<Option<…>>>` trick.
fn start_raf_loop(scene: Rc<RefCell<Option<Scene>>>) {
    // `cb` holds the closure; `cb2` is the clone captured inside the closure.
    let cb: Rc<RefCell<Option<Closure<dyn FnMut(f64)>>>> = Rc::new(RefCell::new(None));
    let cb2 = cb.clone();

    *cb.borrow_mut() = Some(Closure::wrap(Box::new(move |t: f64| {
        if let Some(s) = scene.borrow_mut().as_mut() {
            s.render(t);
        }
        request_af(cb2.borrow().as_ref().unwrap());
    }) as Box<dyn FnMut(f64)>));

    request_af(cb.borrow().as_ref().unwrap());
    // Leak `cb`: it must outlive the closure it contains (self-referential).
    // The rAF chain lives for the panel's lifetime (keep-alive routing).
    std::mem::forget(cb);
}

fn request_af(cb: &Closure<dyn FnMut(f64)>) {
    if let Some(window) = web_sys::window() {
        let _ = window.request_animation_frame(cb.as_ref().unchecked_ref());
    }
}

fn perf_now() -> f64 {
    web_sys::window()
        .and_then(|w| w.performance())
        .map(|p| p.now())
        .unwrap_or(0.0)
}
