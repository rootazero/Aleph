use super::water::{Surface, SURFACE_POINTS};
use leptos::html;
use leptos::leptos_dom::helpers::{
    request_animation_frame_with_handle, AnimationFrameRequestHandle,
};
use leptos::prelude::*;
use wasm_bindgen::JsCast;
use web_sys::{CanvasRenderingContext2d, HtmlCanvasElement};

const WIDTH: f64 = 36.0;
const HEIGHT: f64 = 42.0;

struct Slosh {
    surface: Surface,
    level: f64,
    timestamp: f64,
    elapsed: f64,
}

fn can_animate() -> bool {
    let Some(window) = web_sys::window() else {
        return false;
    };
    window.document().is_some_and(|document| !document.hidden())
        && window
            .match_media("(prefers-reduced-motion: no-preference)")
            .ok()
            .flatten()
            .is_some_and(|query| query.matches())
}

fn paint(context: &CanvasRenderingContext2d, slosh: &Slosh) {
    context.clear_rect(0.0, 0.0, WIDTH, HEIGHT);
    context.begin_path();
    context.move_to(0.0, HEIGHT);
    for (i, offset) in slosh.surface.offsets().iter().enumerate() {
        let x = WIDTH * i as f64 / (SURFACE_POINTS - 1) as f64;
        let y = HEIGHT * (1.0 - (slosh.level + offset).clamp(0.0, 1.0));
        context.line_to(x, y);
    }
    context.line_to(WIDTH, HEIGHT);
    context.close_path();
    context.set_fill_style_str("currentColor");
    context.fill();
}

fn queue_frame(
    frame: StoredValue<Option<AnimationFrameRequestHandle>>,
    active: RwSignal<bool>,
    context: CanvasRenderingContext2d,
    mut slosh: Slosh,
) {
    let next = request_animation_frame_with_handle(move || {
        let Some(now) = web_sys::window()
            .and_then(|window| window.performance())
            .map(|clock| clock.now())
        else {
            let _ = active.try_set(false);
            return;
        };
        let dt = (now - slosh.timestamp) / 1_000.0;
        slosh.timestamp = now;
        slosh.elapsed += dt;
        slosh.surface.advance(dt);
        if !can_animate() || slosh.elapsed >= 2.0 || slosh.surface.is_settled() {
            let _ = active.try_set(false);
            return;
        }
        paint(&context, &slosh);
        queue_frame(frame, active, context, slosh);
    });
    if let Ok(next) = next {
        let _ = frame.try_update_value(|slot| *slot = Some(next));
    } else {
        let _ = active.try_set(false);
    }
}

#[component]
pub(super) fn WaterDisplay(
    level: Memo<Option<f64>>,
    session: Signal<Option<String>>,
) -> impl IntoView {
    let canvas = NodeRef::<html::Canvas>::new();
    let active = RwSignal::new(false);
    let frame = StoredValue::new(None::<AnimationFrameRequestHandle>);
    let cancel = move || {
        let previous = frame.try_update_value(Option::take).flatten();
        if let Some(previous) = previous {
            previous.cancel();
        }
    };
    on_cleanup(cancel);
    let visibility_listener = window_event_listener_untyped("visibilitychange", move |_| {
        if !can_animate() {
            cancel();
            active.set(false);
        }
    });
    on_cleanup(move || visibility_listener.remove());
    Effect::new(move |previous: Option<(Option<String>, Option<f64>)>| {
        let session = session.get();
        let value = level.get();
        cancel();
        active.set(false);
        if let Some((_, Some(previous_level))) = previous.filter(|(key, _)| key == &session) {
            if let Some(target) = value.filter(|target| (target - previous_level).abs() >= 0.005) {
                let node = canvas
                    .get_untracked()
                    .map(|node| node.unchecked_into::<HtmlCanvasElement>());
                if let Some((context, now)) = node.filter(|_| can_animate()).and_then(|node| {
                    let context = node
                        .get_context("2d")
                        .ok()??
                        .dyn_into::<CanvasRenderingContext2d>()
                        .ok()?;
                    let now = web_sys::window()?.performance()?.now();
                    Some((context, now))
                }) {
                    let mut surface = Surface::default();
                    surface.disturb((target - previous_level) * 5.0);
                    let slosh = Slosh {
                        surface,
                        level: target,
                        timestamp: now,
                        elapsed: 0.0,
                    };
                    paint(&context, &slosh);
                    active.set(true);
                    queue_frame(frame, active, context, slosh);
                }
            }
        }
        (session, value)
    });
    view! {
        <span class="panel-context-vial-fill" style:height=move || {
            format!("{}%", level.get().unwrap_or_default() * 100.0)
        } hidden=move || active.get()></span>
        <canvas node_ref=canvas class="panel-context-vial-canvas" width="36" height="42"
            hidden=move || !active.get()></canvas>
    }
}
