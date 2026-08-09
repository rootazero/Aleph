//! WebGL2 galaxy canvas host. Owns the `<canvas>`, rAF loop, pointer events.
//!
//! Architecture note (`canvas/mod.rs:48-54`): `Scene` holds web-sys GL handles
//! → it is NOT `Send`. Keep it in `Rc<RefCell<Option<Scene>>>` captured by the
//! rAF closure and Effects. The `on_event` `Callback` captures only `Copy`
//! reactive signals so it satisfies `Send + Sync`.
//!
//! Intent channels (`focus_request`, `highlight_request`, `viewport_cmd`, …) let
//! the host drive the scene without violating the non-Send constraint: the host
//! writes to a `RwSignal`, an Effect inside `GalaxyCanvas` reads it and calls the
//! Scene.
//!
//! Coordinate spaces — pick one and obey it everywhere:
//! - The canvas BACKING STORE is sized in DEVICE pixels (`css * dpr`), so the
//!   galaxy is sharp on retina. `Scene::resize` gets device pixels.
//! - Everything else the host hands the Scene — pointer positions, wheel cursor,
//!   pan deltas — is CSS pixels, canvas-local. `Scene` knows the dpr
//!   (`set_dpr`) and does the conversion internally, and `screen_pos_of` gives
//!   CSS pixels back for the label divs (which are DOM, hence CSS px).
//!
//! Lifetime: the canvas DOES unmount (phone `/memory/graph` swaps the subtree on
//! every visit; wide remounts across the form-factor breakpoints), so the rAF
//! loop, the `ResizeObserver` and the WebGL2 context are all torn down in
//! `on_cleanup` — see [`CanvasResources`].

mod pointer_input;

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use leptos::callback::Callback;
use leptos::prelude::*;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;

use self::pointer_input::{PointerCtx, PointerTracker};
use super::gl::scene::{Scene, ViewportCmd};
use super::gl::GraphData;
use crate::canvas_engine::interaction::CanvasEvent;

/// Upper bound on the device-pixel-ratio we honour. A 3x phone would quadruple
/// the fragment cost of the bloom pipeline for no visible gain.
const MAX_DPR: f32 = 2.0;

/// Label data: node name + canvas-local screen position (CSS px).
#[derive(Clone, PartialEq)]
struct LabelInfo {
    name: String,
    x: f32,
    y: f32,
}

/// The signals the rAF loop reads the current selection/hover from, and writes the
/// projected label positions back to. Grouped because they travel together and are
/// meaningless apart — the frame callback needs all four or none.
#[derive(Clone, Copy)]
struct LabelChannels {
    selected_node: RwSignal<Option<String>>,
    hovered_node: RwSignal<Option<String>>,
    hover_label: RwSignal<Option<LabelInfo>>,
    select_label: RwSignal<Option<LabelInfo>>,
}

/// Everything one mounted canvas must tear down on unmount.
///
/// `on_cleanup` (leptos 0.8) demands a `Send + Sync` closure, but every handle
/// here is `!Send` (GL context, JS closures, the `ResizeObserver`). The panel is
/// single-threaded, so the payload lives in a thread-local registry and the
/// cleanup closure carries only its `Copy` id.
struct CanvasResources {
    scene: Rc<RefCell<Option<Scene>>>,
    alive: Rc<Cell<bool>>,
    raf: Rc<Cell<i32>>,
    raf_cb: Rc<RefCell<Option<Closure<dyn FnMut(f64)>>>>,
    observer: Option<web_sys::ResizeObserver>,
    /// The canvas the listener below is registered on. Held so teardown can
    /// UNREGISTER it before the closure drops — see [`dispose_canvas`].
    canvas: web_sys::HtmlCanvasElement,
    /// Kept alive for exactly as long as the observer / listener that calls them.
    _resize_cb: Closure<dyn FnMut(js_sys::Array)>,
    lost_cb: Closure<dyn FnMut(web_sys::Event)>,
}

thread_local! {
    static CANVAS_RESOURCES: RefCell<HashMap<u32, CanvasResources>> =
        RefCell::new(HashMap::new());
    static NEXT_CANVAS_ID: Cell<u32> = const { Cell::new(0) };
}

fn next_canvas_id() -> u32 {
    NEXT_CANVAS_ID.with(|c| {
        let id = c.get();
        c.set(id.wrapping_add(1));
        id
    })
}

/// Stop the rAF chain, disconnect the observer, and lose the WebGL2 context.
/// Idempotent: a canvas that never finished init has no entry.
fn dispose_canvas(id: u32) {
    let Some(res) = CANVAS_RESOURCES.with(|m| m.borrow_mut().remove(&id)) else {
        return;
    };
    res.alive.set(false);
    if let Some(window) = web_sys::window() {
        let _ = window.cancel_animation_frame(res.raf.get());
    }
    if let Some(obs) = res.observer.as_ref() {
        obs.disconnect();
    }
    // Unregister the lost-context listener BEFORE losing the context. `dispose()`
    // calls `loseContext()`, which makes the browser dispatch `webglcontextlost`
    // ASYNCHRONOUSLY — by which time `res` (and `lost_cb` inside it) has dropped,
    // so the event would call into a freed Rust closure and abort the WASM module.
    let _ = res.canvas.remove_event_listener_with_callback(
        "webglcontextlost",
        res.lost_cb.as_ref().unchecked_ref(),
    );
    if let Some(s) = res.scene.borrow().as_ref() {
        s.dispose();
    }
    *res.scene.borrow_mut() = None;
    // Break the rAF closure's self-reference so it (and its captures) drop.
    *res.raf_cb.borrow_mut() = None;
}

#[component]
#[must_use]
pub fn GalaxyCanvas(
    graph: RwSignal<Option<GraphData>>,
    on_event: Callback<CanvasEvent>,
    /// Intent channel: when `Some(id)`, fly the camera to that node. Consumed
    /// (reset to `None`) by the Effect — a one-shot pulse, so re-focusing the
    /// same node works.
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
    /// Intent channel: viewport command (zoom / fit / reset / focus). Consumed
    /// like `focus_request`.
    viewport_cmd: RwSignal<Option<ViewportCmd>>,
    /// Out-channel: a WebGL2 init failure or a lost context. The host renders it
    /// (`canvas/overlay.rs`) — without it the user gets an unexplained black box.
    gl_error: RwSignal<Option<String>>,
) -> impl IntoView {
    // Only reached by the `gl_error` write below, whose error is a WebGL2 init
    // failure and never a gateway verdict. Routed through `admin_refusal`
    // anyway: the rule is uniform because a non-refusal passes through
    // byte-for-byte, and the alternative — an exception list saying which
    // `Option<String>` error signals carry server errors — is a second source
    // of truth that rots.
    let i18n = crate::i18n::use_i18n();
    let canvas_ref = NodeRef::<leptos::html::Canvas>::new();

    // Non-Send GL scene — lives in Rc<RefCell> never crossed to another thread.
    let scene: Rc<RefCell<Option<Scene>>> = Rc::new(RefCell::new(None));

    // Pointer/gesture state (1 pointer = orbit, 2 = pinch), CSS px canvas-local.
    let tracker: Rc<RefCell<PointerTracker>> = Rc::new(RefCell::new(PointerTracker::default()));

    // Last hovered node id — used to emit HoverNode only on transition.
    let last_hover: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));

    // Canvas element bounding-rect origin cached to avoid forced-reflow on every
    // pointermove. Refreshed on pointerdown and on resize.
    let canvas_origin: Rc<Cell<(f32, f32)>> = Rc::new(Cell::new((0.0, 0.0)));

    // Label overlay signals: Option<LabelInfo> for hovered + selected node.
    // Written by the rAF loop each frame; read by the reactive view below.
    let hover_label: RwSignal<Option<LabelInfo>> = RwSignal::new(None);
    let select_label: RwSignal<Option<LabelInfo>> = RwSignal::new(None);

    // Teardown handle: only this `Copy` id is captured by on_cleanup.
    let canvas_id = next_canvas_id();
    on_cleanup(move || dispose_canvas(canvas_id));

    // --- Init Effect: mount scene once the <canvas> is in the DOM ---
    let scene_init = scene.clone();
    let canvas_origin_init = canvas_origin.clone();
    Effect::new(move |_| {
        let Some(canvas) = canvas_ref.get() else {
            return;
        };
        if scene_init.borrow().is_some() {
            return; // already mounted — never build a second GL context
        }
        let el: web_sys::HtmlCanvasElement = canvas.unchecked_into();

        // Size the backing store in DEVICE pixels; the CSS box stays w-full h-full.
        let dpr = device_pixel_ratio();
        let (cw, ch) = el
            .parent_element()
            .map(|p| {
                let r = p.get_bounding_client_rect();
                (r.width().max(1.0) as f32, r.height().max(1.0) as f32)
            })
            .unwrap_or((800.0, 600.0));
        size_backing_store(&el, cw, ch, dpr);

        // Snapshot the initial canvas origin for hover coordinate mapping.
        {
            let rect = el.get_bounding_client_rect();
            canvas_origin_init.set((rect.left() as f32, rect.top() as f32));
        }

        match Scene::new(&el) {
            Ok(mut s) => {
                s.set_dpr(dpr);
                *scene_init.borrow_mut() = Some(s);
            }
            Err(e) => {
                web_sys::console::error_1(&format!("GalaxyCanvas GL init failed: {e}").into());
                gl_error.set(Some(
                    crate::components::admin_refusal::settings_write_error(i18n, &e, |e| {
                        e.to_string()
                    }),
                ));
                return;
            }
        }

        // ResizeObserver: keep the backing store in sync with the CSS box (in
        // device px) and refresh the cached canvas_origin.
        let scene_resize = scene_init.clone();
        let canvas_origin_resize = canvas_origin_init.clone();
        let resize_cb: Closure<dyn FnMut(js_sys::Array)> =
            Closure::new(move |entries: js_sys::Array| {
                let Ok(entry) = entries.get(0).dyn_into::<web_sys::ResizeObserverEntry>() else {
                    return;
                };
                let rect = entry.content_rect(); // CSS px
                let (cw, ch) = (rect.width().max(1.0) as f32, rect.height().max(1.0) as f32);
                // Re-read the dpr: it changes when the window moves to another display.
                let dpr = device_pixel_ratio();
                let (dw, dh) = device_size(cw, ch, dpr);
                if let Ok(target) = entry.target().dyn_into::<web_sys::HtmlCanvasElement>() {
                    size_backing_store(&target, cw, ch, dpr);
                    // Refresh cached origin after layout change.
                    let r = target.get_bounding_client_rect();
                    canvas_origin_resize.set((r.left() as f32, r.top() as f32));
                }
                if let Some(s) = scene_resize.borrow_mut().as_mut() {
                    s.set_dpr(dpr);
                    s.resize(dw, dh);
                }
            });
        let observer = web_sys::ResizeObserver::new(resize_cb.as_ref().unchecked_ref()).ok();
        if let Some(obs) = observer.as_ref() {
            obs.observe(&el);
        }

        // A driver reset / GPU eviction kills the context: tell the user instead
        // of leaving a black rectangle. prevent_default() keeps restore possible.
        let lost_cb: Closure<dyn FnMut(web_sys::Event)> =
            Closure::new(move |ev: web_sys::Event| {
                ev.prevent_default();
                gl_error.set(Some("WebGL context lost — reload to restore".to_owned()));
            });
        let _ = el
            .add_event_listener_with_callback("webglcontextlost", lost_cb.as_ref().unchecked_ref());

        let alive = Rc::new(Cell::new(true));
        let raf = Rc::new(Cell::new(0));
        let raf_cb = start_raf_loop(
            el.clone(),
            scene_init.clone(),
            alive.clone(),
            raf.clone(),
            LabelChannels {
                selected_node,
                hovered_node,
                hover_label,
                select_label,
            },
        );

        CANVAS_RESOURCES.with(|m| {
            m.borrow_mut().insert(
                canvas_id,
                CanvasResources {
                    scene: scene_init.clone(),
                    alive,
                    raf,
                    raf_cb,
                    observer,
                    canvas: el.clone(),
                    _resize_cb: resize_cb,
                    lost_cb,
                },
            );
        });
    });

    // --- Data Effect: push GraphData into scene when it changes ---
    // TOTAL in `graph`: a `None` (the agent-switch reset) must CLEAR the scene,
    // otherwise the previous agent's stars keep rendering under the new name.
    let scene_data = scene.clone();
    Effect::new(move |_| {
        let data = graph.get().unwrap_or_default();
        if let Some(s) = scene_data.borrow_mut().as_mut() {
            s.set_graph(data);
        }
    });

    // --- Intent channel Effect: fly-to (one-shot pulse) ---
    let scene_focus = scene.clone();
    Effect::new(move |_| {
        let Some(id) = focus_request.get() else {
            return;
        };
        let t_ms = perf_now();
        if let Some(s) = scene_focus.borrow_mut().as_mut() {
            s.fly_to_node(&id, t_ms);
        }
        // Consume the pulse: re-focusing the SAME node must work again.
        focus_request.set(None);
    });

    // --- Intent channel Effect: viewport command (one-shot pulse) ---
    let scene_vp = scene.clone();
    Effect::new(move |_| {
        let Some(cmd) = viewport_cmd.get() else {
            return;
        };
        let t_ms = perf_now();
        if let Some(s) = scene_vp.borrow_mut().as_mut() {
            s.apply_cmd(cmd, selected_node.get_untracked().as_deref(), t_ms);
        }
        viewport_cmd.set(None);
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

    // --- Pointer events (gesture logic + DOM plumbing: pointer_input.rs) ---
    let ptr = PointerCtx {
        scene,
        tracker,
        origin: canvas_origin,
        last_hover,
        on_event,
    };
    let on_pointerdown = ptr.pointerdown();
    let on_pointermove = ptr.pointermove();
    let on_pointerup = ptr.pointerup();
    let on_pointercancel = ptr.pointercancel();
    let on_pointerleave = ptr.pointerleave();
    let on_wheel = ptr.wheel();

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
                on:pointerleave=on_pointerleave
                on:wheel=on_wheel
            />
            // Hovered + selected node label overlays (A).
            {move || hover_label.get().map(|l| label_view(l, HOVER_LABEL_CLASS))}
            {move || select_label.get().map(|l| label_view(l, SELECT_LABEL_CLASS))}
        </div>
    }
}

const HOVER_LABEL_CLASS: &str = "pointer-events-none absolute text-xs text-white/80 bg-black/40 \
                                 rounded px-1.5 py-0.5 whitespace-nowrap select-none";

const SELECT_LABEL_CLASS: &str =
    "pointer-events-none absolute text-xs font-semibold text-white bg-black/55 rounded \
     px-1.5 py-0.5 whitespace-nowrap select-none ring-1 ring-white/20";

/// One node label div, anchored over the canvas. `l.x`/`l.y` are CSS px
/// (`Scene::screen_pos_of` converts out of the device-pixel backing store).
fn label_view(l: LabelInfo, class: &'static str) -> impl IntoView {
    let style = format!(
        "left:{:.1}px; top:{:.1}px; transform: translate(-50%, -130%);",
        l.x, l.y
    );
    view! { <div class=class style=style>{l.name}</div> }
}

/// Device-pixel-ratio, clamped: below 1 there is nothing to gain, above
/// [`MAX_DPR`] the bloom FBO cost grows quadratically for no visible benefit.
fn device_pixel_ratio() -> f32 {
    web_sys::window()
        .map(|w| w.device_pixel_ratio() as f32)
        .unwrap_or(1.0)
        .clamp(1.0, MAX_DPR)
}

/// CSS size → backing-store size in device pixels.
fn device_size(css_w: f32, css_h: f32, dpr: f32) -> (i32, i32) {
    (
        (css_w * dpr).round().max(1.0) as i32,
        (css_h * dpr).round().max(1.0) as i32,
    )
}

fn size_backing_store(el: &web_sys::HtmlCanvasElement, css_w: f32, css_h: f32, dpr: f32) {
    let (w, h) = device_size(css_w, css_h, dpr);
    el.set_width(w as u32);
    el.set_height(h as u32);
}

/// Start the `requestAnimationFrame` recursive loop; returns the cell holding the
/// (self-referential) frame closure so `dispose_canvas` can break the cycle.
/// Also updates label overlay signals each frame from screen-projected node
/// positions (A).
///
/// `canvas_el` is used each frame to gate rendering: when the canvas is hidden via
/// `display:none` keep-alive, `offset_parent()` returns `None` and we skip `render`
/// and label-overlay updates, but keep rescheduling so it resumes instantly when
/// shown. `alive` is cleared by `on_cleanup`: the loop then stops rescheduling.
fn start_raf_loop(
    canvas_el: web_sys::HtmlCanvasElement,
    scene: Rc<RefCell<Option<Scene>>>,
    alive: Rc<Cell<bool>>,
    raf: Rc<Cell<i32>>,
    labels: LabelChannels,
) -> Rc<RefCell<Option<Closure<dyn FnMut(f64)>>>> {
    let LabelChannels {
        selected_node,
        hovered_node,
        hover_label,
        select_label,
    } = labels;
    // `cb` holds the closure; `cb2` is the clone captured inside the closure.
    let cb: Rc<RefCell<Option<Closure<dyn FnMut(f64)>>>> = Rc::new(RefCell::new(None));
    let cb2 = cb.clone();
    let alive_frame = alive.clone();
    let raf_frame = raf.clone();

    *cb.borrow_mut() = Some(Closure::wrap(Box::new(move |t: f64| {
        if !alive_frame.get() {
            return; // unmounted: stop the chain
        }

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
            // Read scene immutably after render so last_vp is current. `screen_pos_of`
            // returns CSS px — the label divs are DOM.
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

        if let Some(c) = cb2.borrow().as_ref() {
            raf_frame.set(request_af(c));
        }
    }) as Box<dyn FnMut(f64)>));

    if let Some(c) = cb.borrow().as_ref() {
        raf.set(request_af(c));
    }
    cb
}

/// Schedule one frame; returns the handle `cancel_animation_frame` needs.
fn request_af(cb: &Closure<dyn FnMut(f64)>) -> i32 {
    web_sys::window()
        .and_then(|w| w.request_animation_frame(cb.as_ref().unchecked_ref()).ok())
        .unwrap_or(0)
}

fn perf_now() -> f64 {
    web_sys::window()
        .and_then(|w| w.performance())
        .map(|p| p.now())
        .unwrap_or(0.0)
}
