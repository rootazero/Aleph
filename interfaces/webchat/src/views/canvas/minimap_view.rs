use crate::canvas_engine::mini_map::GlobalMiniMap;
use leptos::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;
use wasm_bindgen::JsCast;
use web_sys::{CanvasRenderingContext2d, HtmlCanvasElement, MouseEvent};

const MINIMAP_PX: f32 = 200.0;
const HIT_RADIUS_PX: f32 = 6.0;

#[component]
pub fn MiniMapOverlay(
    minimap: Rc<RefCell<GlobalMiniMap>>,
    focus_id: ReadSignal<Option<String>>,
    focus_neighbor_ids: ReadSignal<Vec<String>>,
    on_pick: impl Fn(String) + 'static + Copy,
) -> impl IntoView {
    let canvas_ref = NodeRef::<leptos::html::Canvas>::new();

    // Repaint whenever data or focus changes
    let mm_render = minimap.clone();
    Effect::new(move |_| {
        let _ = focus_id.get();
        let _ = focus_neighbor_ids.get();
        let Some(canvas_el) = canvas_ref.get() else { return };
        let canvas: HtmlCanvasElement = canvas_el.into();
        let Some(ctx) = canvas
            .get_context("2d")
            .ok()
            .flatten()
            .and_then(|o| o.dyn_into::<CanvasRenderingContext2d>().ok())
        else {
            return;
        };
        ctx.clear_rect(0.0, 0.0, MINIMAP_PX as f64, MINIMAP_PX as f64);
        let neighbors = focus_neighbor_ids.get();
        let focus = focus_id.get();
        mm_render
            .borrow()
            .render(&ctx, focus.as_deref(), &neighbors);
    });

    let mm_click = minimap.clone();
    let on_click = move |ev: MouseEvent| {
        let Some(canvas_el) = canvas_ref.get() else { return };
        let canvas: HtmlCanvasElement = canvas_el.into();
        let rect = canvas.get_bounding_client_rect();
        let mx = ev.client_x() as f32 - rect.left() as f32;
        let my = ev.client_y() as f32 - rect.top() as f32;
        if let Some(id) = mm_click.borrow().pick_at(mx, my, HIT_RADIUS_PX) {
            on_pick(id.to_string());
        }
    };

    view! {
        <div
            class="absolute bottom-4 right-4 rounded-lg overflow-hidden \
                   border border-border/50 bg-surface-raised/80 backdrop-blur"
            style="width: 200px; height: 200px;"
        >
            <canvas
                node_ref=canvas_ref
                width="200"
                height="200"
                on:click=on_click
                class="cursor-pointer block"
            />
        </div>
    }
}
