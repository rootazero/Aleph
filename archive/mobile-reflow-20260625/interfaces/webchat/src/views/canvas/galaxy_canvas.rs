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

use super::gl::scene::Scene;
use super::gl::GraphData;
use crate::canvas_engine::interaction::CanvasEvent;

/// Click threshold in CSS pixels. A pointer-up within this distance of
/// pointer-down counts as a click; larger = drag (no selection).
const CLICK_THRESHOLD_PX: f32 = 5.0;
/// Touch pick radius (CSS px) — WCAG 2.5.5 minimum 44px target. High-DPI is
/// handled by the browser's client-pixel coordinate space (no manual scaling).
const TOUCH_PICK_RADIUS_PX: f32 = 44.0;
/// Mouse pick radius — tight, matches the previous hardcoded value.
const MOUSE_PICK_RADIUS_PX: f32 = 18.0;
/// Minimum ms between hover picks on mobile (coarse touch movement).
const MOBILE_HOVER_PICK_MS: f64 = 75.0;

/// Label data: node name + canvas-local screen position.
#[derive(Clone, PartialEq)]
struct LabelInfo {
    name: String,
    x: f32,
    y: f32,
}

#[component]
#[must_use]
pub fn GalaxyCanvas(
    graph: RwSignal<Option<GraphData>>,
    on_event: Callback<CanvasEvent>,
    /// Intent channel: when `Some(id)`, fly the camera to that node.
    focus_request: RwSignal<Option<String>>,
    /// Intent channel: when `Some(indices)`, highlight those node indices.
    highlight_request: RwSignal<Option<HashSet<u32>>>,
    /// Intent channel: LOD level in [0, 1] controlling edge density.
    /// 0 = all edges; 1 = only high-degree backbone. Updated by the density slider.
    lod_request: RwSignal<f32>,
    /// Intent channel: currently selected node id (for label overlay).
    selected_node: RwSignal<Option<String>>,
    /// Intent channel: currently hovered node id (for label overlay).
    hovered_node: RwSignal<Option<String>>,
    /// Intent channel: edges incident to the selected node (normalized index pairs).
    highlight_edges_request: RwSignal<Option<std::collections::HashSet<(u32, u32)>>>,
    /// Mobile flag: widens the touch pick radius to ≈44px and throttles
    /// hover-picking to ~75ms (touch movement is coarse). Desktop stays at the
    /// tight 18px radius with per-move picking.
    is_mobile: RwSignal<bool>,
    /// WebGL2-unsupported flag (§11 P-⑥). Set to `true` when `Scene::new`
    /// (via `context::from_canvas`) errors on mount. `CanvasView` watches this
    /// and switches the Memory hub to the Table view permanently, with an
    /// inline banner explaining the galaxy is unavailable on this device.
    fallback: RwSignal<bool>,
) -> impl IntoView {
    let canvas_ref = NodeRef::<leptos::html::Canvas>::new();

    // Non-Send GL scene — lives in Rc<RefCell> never crossed to another thread.
    let scene: Rc<RefCell<Option<Scene>>> = Rc::new(RefCell::new(None));

    // Last pointer position for computing drag deltas (screen pixels).
    let last_ptr: Rc<Cell<(f32, f32)>> = Rc::new(Cell::new((0.0, 0.0)));
    let ptr_down: Rc<Cell<bool>> = Rc::new(Cell::new(false));

    // Pointer-down position (client pixels) for click-vs-drag detection.
    let down_pos: Rc<Cell<(f32, f32)>> = Rc::new(Cell::new((0.0, 0.0)));

    // Multi-pointer pinch tracking (§11 P-⑤). Each entry = (pointerId, x, y) in
    // client pixels. ≥2 entries → pinch gesture active. Pointer Events ONLY (no
    // TouchEvent): galaxy_canvas already uses them with pointer-capture.
    let active_ptrs: Rc<RefCell<Vec<(i32, f32, f32)>>> = Rc::new(RefCell::new(Vec::new()));
    // Baseline two-finger distance captured when the pinch began (or rebased
    // when a finger lifts mid-gesture). 0.0 = no active pinch.
    let pinch_base: Rc<Cell<f32>> = Rc::new(Cell::new(0.0));

    // Last hover-pick time (ms) for mobile throttle. Desktop ignores it.
    let last_hover_pick_ms: Rc<Cell<f64>> = Rc::new(Cell::new(0.0));

    // Last hovered node id — used to emit HoverNode only on transition.
    let last_hover: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));

    // Canvas element bounding-rect origin cached to avoid forced-reflow on every
    // pointermove (D: perf cleanup). Refreshed on pointerdown and on resize.
    let canvas_origin: Rc<Cell<(f32, f32)>> = Rc::new(Cell::new((0.0, 0.0)));

    // Label overlay signals: Option<LabelInfo> for hovered + selected node.
    // Written by the rAF loop each frame; read by the reactive view below.
    let hover_label: RwSignal<Option<LabelInfo>> = RwSignal::new(None);
    let select_label: RwSignal<Option<LabelInfo>> = RwSignal::new(None);

    // --- Init Effect: mount scene once the <canvas> is in the DOM ---
    let scene_init = scene.clone();
    let canvas_origin_init = canvas_origin.clone();
    Effect::new(move |_| {
        let Some(canvas) = canvas_ref.get() else {
            return;
        };
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

        // Snapshot the initial canvas origin for hover coordinate mapping.
        {
            let rect = el.get_bounding_client_rect();
            canvas_origin_init.set((rect.left() as f32, rect.top() as f32));
        }

        // §11 P-⑦: mobile perf guardrails — fewer settle steps, bloom off.
        let mobile = is_mobile.get_untracked();
        let settle_cap = if mobile {
            super::gl::scene::MAX_SETTLE_STEPS_MOBILE
        } else {
            super::gl::scene::MAX_SETTLE_STEPS_DESKTOP
        };
        let bloom_level = if mobile { 0.0 } else { 1.0 };

        match Scene::new(&el, settle_cap, bloom_level) {
            Ok(s) => *scene_init.borrow_mut() = Some(s),
            Err(e) => {
                web_sys::console::error_1(&format!("GalaxyCanvas GL init failed: {e}").into());
                // §11 P-⑥: WebGL2 unavailable → signal the host to fall back to
                // the Table view. Permanent switch (CanvasView watches this).
                fallback.set(true);
                return;
            }
        }

        // ResizeObserver: keep canvas dimensions in sync with its CSS container
        // and refresh cached canvas_origin (D: avoid per-hover reflow).
        let scene_resize = scene_init.clone();
        let canvas_origin_resize = canvas_origin_init.clone();
        let resize_cb: Closure<dyn FnMut(js_sys::Array)> =
            Closure::new(move |entries: js_sys::Array| {
                if let Ok(entry) = entries.get(0).dyn_into::<web_sys::ResizeObserverEntry>() {
                    let rect = entry.content_rect();
                    let w = rect.width().max(1.0) as i32;
                    let h = rect.height().max(1.0) as i32;
                    // Also resize the canvas backing store.
                    if let Ok(target) = entry.target().dyn_into::<web_sys::HtmlCanvasElement>() {
                        target.set_width(w as u32);
                        target.set_height(h as u32);
                        // Refresh cached origin after layout change.
                        let r = target.get_bounding_client_rect();
                        canvas_origin_resize.set((r.left() as f32, r.top() as f32));
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

        start_raf_loop(
            el.clone(),
            scene_init.clone(),
            selected_node,
            hovered_node,
            hover_label,
            select_label,
        );
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
        let Some(id) = focus_request.get() else {
            return;
        };
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

    // --- Intent channel Effect: highlight edges ---
    let scene_hle = scene.clone();
    Effect::new(move |_| {
        let hle = highlight_edges_request.get();
        if let Some(s) = scene_hle.borrow_mut().as_mut() {
            s.set_highlight_edges(hle);
        }
    });

    // --- Intent channel Effect: LOD / edge density ---
    let scene_lod = scene.clone();
    Effect::new(move |_| {
        let lod = lod_request.get();
        if let Some(s) = scene_lod.borrow_mut().as_mut() {
            s.set_lod(lod);
        }
    });

    // --- Pointer events ---
    // Drag delta → scene.on_drag; click → pick → on_event; hover → pick → on_event.

    let last_ptr_pd = last_ptr.clone();
    let ptr_down_pd = ptr_down.clone();
    let down_pos_pd = down_pos.clone();
    let canvas_origin_pd = canvas_origin.clone();
    let active_ptrs_pd = active_ptrs.clone();
    let pinch_base_pd = pinch_base.clone();
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
        // Register this pointer for pinch tracking. When a second finger lands,
        // capture the baseline distance from the first two active pointers.
        {
            let mut ptrs = active_ptrs_pd.borrow_mut();
            ptrs.retain(|(id, _, _)| *id != ev.pointer_id());
            ptrs.push((ev.pointer_id(), pos.0, pos.1));
            if ptrs.len() >= 2 {
                let a = (ptrs[0].1, ptrs[0].2);
                let b = (ptrs[1].1, ptrs[1].2);
                pinch_base_pd.set(super::gl::pinch::dist(a, b));
            }
        }
    };

    let scene_pm = scene.clone();
    let last_ptr_pm = last_ptr.clone();
    let ptr_down_pm = ptr_down.clone();
    let canvas_origin_pm = canvas_origin.clone();
    let last_hover_pm = last_hover.clone();
    let last_hover_pick_pm = last_hover_pick_ms.clone();
    let on_event_pm = on_event;
    let active_ptrs_pm = active_ptrs.clone();
    let pinch_base_pm = pinch_base.clone();
    let on_pointermove = move |ev: web_sys::PointerEvent| {
        let cx = ev.client_x() as f32;
        let cy = ev.client_y() as f32;

        // Update this pointer's tracked position.
        {
            let mut ptrs = active_ptrs_pm.borrow_mut();
            if let Some(p) = ptrs.iter_mut().find(|(id, _, _)| *id == ev.pointer_id()) {
                p.1 = cx;
                p.2 = cy;
            }
            // Pinch path: ≥2 active pointers → distance ratio drives zoom, NOT
            // orbit. Takes priority over the single-finger drag branch below.
            if ptrs.len() >= 2 {
                let a = (ptrs[0].1, ptrs[0].2);
                let b = (ptrs[1].1, ptrs[1].2);
                let cur = super::gl::pinch::dist(a, b);
                let base = pinch_base_pm.get();
                let factor = super::gl::pinch::pinch_zoom_factor(base, cur);
                let t_ms = perf_now();
                if let Some(s) = scene_pm.borrow_mut().as_mut() {
                    s.camera.zoom(factor);
                    s.camera.note_interaction(t_ms);
                }
                // Rebase the baseline each frame so zoom is incremental, not
                // absolute (mirrors the wheel's per-event accumulation).
                pinch_base_pm.set(cur);
                return;
            }
        }

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
            // On mobile, throttle picks to MOBILE_HOVER_PICK_MS and widen the
            // pick radius to absorb coarse touch movement.
            let mobile = is_mobile.get_untracked();
            let radius = if mobile {
                TOUCH_PICK_RADIUS_PX
            } else {
                MOUSE_PICK_RADIUS_PX
            };
            if mobile {
                let now = perf_now();
                if now - last_hover_pick_pm.get() < MOBILE_HOVER_PICK_MS {
                    return;
                }
                last_hover_pick_pm.set(now);
            }
            let (ox, oy) = canvas_origin_pm.get();
            let local = (cx - ox, cy - oy);
            let hit = scene_pm
                .borrow()
                .as_ref()
                .and_then(|s| s.pick(local, radius));
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
    let active_ptrs_pu = active_ptrs.clone();
    let pinch_base_pu = pinch_base.clone();
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

            // Click (not drag): pick the node under the cursor. Touch uses the
            // wide 44px radius so fat-finger taps still land on a node.
            if dist < CLICK_THRESHOLD_PX {
                let radius = if is_mobile.get_untracked() {
                    TOUCH_PICK_RADIUS_PX
                } else {
                    MOUSE_PICK_RADIUS_PX
                };
                let (ox, oy) = canvas_origin_pu.get();
                let local = (cx - ox, cy - oy);
                let hit = scene_pu
                    .borrow()
                    .as_ref()
                    .and_then(|s| s.pick(local, radius));
                match hit {
                    Some(id) => on_event_pu.run(CanvasEvent::SelectNode(id)),
                    None => on_event_pu.run(CanvasEvent::DeselectNode),
                }
            }
        }
        ptr_down_pu.set(false);
        // Drop this pointer; below 2 active pointers ends the pinch gesture.
        {
            let mut ptrs = active_ptrs_pu.borrow_mut();
            ptrs.retain(|(id, _, _)| *id != ev.pointer_id());
            if ptrs.len() < 2 {
                pinch_base_pu.set(0.0);
            }
        }
    };

    let ptr_down_pc = ptr_down.clone();
    let active_ptrs_pc = active_ptrs.clone();
    let pinch_base_pc = pinch_base.clone();
    let on_pointercancel = move |ev: web_sys::PointerEvent| {
        ptr_down_pc.set(false);
        let mut ptrs = active_ptrs_pc.borrow_mut();
        ptrs.retain(|(id, _, _)| *id != ev.pointer_id());
        if ptrs.len() < 2 {
            pinch_base_pc.set(0.0);
        }
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
        // Wrapper with relative positioning so label divs (absolute) are anchored to the canvas.
        <div class="relative w-full h-full">
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
            // Hovered node label overlay (A).
            {move || hover_label.get().map(|l| {
                let style = format!(
                    "left:{:.1}px; top:{:.1}px; transform: translate(-50%, -130%);",
                    l.x, l.y
                );
                view! {
                    <div
                        class="pointer-events-none absolute text-xs text-white/80 bg-black/40 \
                               rounded px-1.5 py-0.5 whitespace-nowrap select-none"
                        style=style
                    >
                        {l.name}
                    </div>
                }
            })}
            // Selected node label overlay (A).
            {move || select_label.get().map(|l| {
                let style = format!(
                    "left:{:.1}px; top:{:.1}px; transform: translate(-50%, -130%);",
                    l.x, l.y
                );
                view! {
                    <div
                        class="pointer-events-none absolute text-xs font-semibold text-white \
                               bg-black/55 rounded px-1.5 py-0.5 whitespace-nowrap select-none \
                               ring-1 ring-white/20"
                        style=style
                    >
                        {l.name}
                    </div>
                }
            })}
        </div>
    }
}

/// Start the `requestAnimationFrame` recursive loop.
/// Also updates label overlay signals each frame from screen-projected node positions (A).
/// The closure holds a strong reference to itself through the `Rc<RefCell<Option<…>>>` trick.
///
/// `canvas_el` is used each frame to gate rendering: when the canvas is hidden via
/// `display:none` keep-alive, `offset_parent()` returns `None` and we skip `render`
/// and label-overlay updates. The rAF reschedule always fires so the loop stays alive
/// and resumes instantly when the canvas becomes visible again.
fn start_raf_loop(
    canvas_el: web_sys::HtmlCanvasElement,
    scene: Rc<RefCell<Option<Scene>>>,
    selected_node: RwSignal<Option<String>>,
    hovered_node: RwSignal<Option<String>>,
    hover_label: RwSignal<Option<LabelInfo>>,
    select_label: RwSignal<Option<LabelInfo>>,
) {
    // `cb` holds the closure; `cb2` is the clone captured inside the closure.
    let cb: Rc<RefCell<Option<Closure<dyn FnMut(f64)>>>> = Rc::new(RefCell::new(None));
    let cb2 = cb.clone();

    *cb.borrow_mut() = Some(Closure::wrap(Box::new(move |t: f64| {
        // Skip render and label updates when the canvas is hidden (display:none keep-alive).
        // offset_parent() is None under display:none; also guard against zero client size.
        let visible = canvas_el.offset_parent().is_some()
            && canvas_el.client_width() > 0
            && canvas_el.client_height() > 0;

        if visible {
            if let Some(s) = scene.borrow_mut().as_mut() {
                s.render(t);
            }

            // Update label overlays (A): project hovered + selected node to screen.
            // Read scene immutably after render so last_vp is current.
            let new_hover = scene.borrow().as_ref().and_then(|s| {
                let id = hovered_node.get_untracked()?;
                let name = s.node_name(&id)?.to_owned();
                let (x, y) = s.screen_pos_of(&id)?;
                Some(LabelInfo { name, x, y })
            });
            hover_label.set(new_hover);

            let new_select = scene.borrow().as_ref().and_then(|s| {
                let id = selected_node.get_untracked()?;
                let name = s.node_name(&id)?.to_owned();
                let (x, y) = s.screen_pos_of(&id)?;
                Some(LabelInfo { name, x, y })
            });
            select_label.set(new_select);
        }

        // Always reschedule — loop must never stop so it resumes instantly when shown.
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
