//! WebGL2 galaxy canvas host. Owns the `<canvas>`, rAF loop, pointer events.
//!
//! Architecture note (`canvas/mod.rs:48-54`): `Scene` holds web-sys GL handles
//! → it is NOT `Send`. Keep it in `Rc<RefCell<Option<Scene>>>` captured by the
//! rAF closure and Effects. The `on_event` `Callback` captures only `Copy`
//! reactive signals so it satisfies `Send + Sync`.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use leptos::callback::Callback;
use leptos::prelude::*;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;

use crate::canvas_engine::interaction::CanvasEvent;
use super::gl::scene::Scene;
use super::gl::GraphData;

#[component]
#[must_use]
pub fn GalaxyCanvas(
    graph: RwSignal<Option<GraphData>>,
    #[allow(unused)] on_event: Callback<CanvasEvent>,
) -> impl IntoView {
    let canvas_ref = NodeRef::<leptos::html::Canvas>::new();

    // Non-Send GL scene — lives in Rc<RefCell> never crossed to another thread.
    let scene: Rc<RefCell<Option<Scene>>> = Rc::new(RefCell::new(None));

    // Last pointer position for computing drag deltas (screen pixels).
    let last_ptr: Rc<Cell<(f32, f32)>> = Rc::new(Cell::new((0.0, 0.0)));
    let ptr_down: Rc<Cell<bool>> = Rc::new(Cell::new(false));

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

    // --- Pointer events ---
    // Drag delta → scene.on_drag; wheel → scene.on_wheel.
    // All captures are Rc (non-Send), safe for WASM single-thread.

    let _scene_pd = scene.clone();
    let last_ptr_pd = last_ptr.clone();
    let ptr_down_pd = ptr_down.clone();
    let on_pointerdown = move |ev: web_sys::PointerEvent| {
        // Capture so move/up fire even when pointer leaves canvas.
        if let Some(target) = ev.target() {
            if let Ok(el) = target.dyn_into::<web_sys::Element>() {
                let _ = el.set_pointer_capture(ev.pointer_id());
            }
        }
        ptr_down_pd.set(true);
        last_ptr_pd.set((ev.client_x() as f32, ev.client_y() as f32));
    };

    let scene_pm = scene.clone();
    let last_ptr_pm = last_ptr.clone();
    let ptr_down_pm = ptr_down.clone();
    let on_pointermove = move |ev: web_sys::PointerEvent| {
        if !ptr_down_pm.get() {
            return;
        }
        let (lx, ly) = last_ptr_pm.get();
        let cx = ev.client_x() as f32;
        let cy = ev.client_y() as f32;
        let dx = cx - lx;
        let dy = cy - ly;
        last_ptr_pm.set((cx, cy));
        let t_ms = perf_now();
        if let Some(s) = scene_pm.borrow_mut().as_mut() {
            s.on_drag(dx, dy, t_ms);
        }
    };

    let ptr_down_pu = ptr_down.clone();
    let on_pointerup = move |ev: web_sys::PointerEvent| {
        if let Some(target) = ev.target() {
            if let Ok(el) = target.dyn_into::<web_sys::Element>() {
                let _ = el.release_pointer_capture(ev.pointer_id());
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
