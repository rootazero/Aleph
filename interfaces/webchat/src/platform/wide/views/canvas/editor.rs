//! The whiteboard editor shell: three stacked layers sharing one world
//! transform, plus the viewport gestures (wheel zoom/pan, pointer pan).
//!
//! # The three layers
//!
//! 1. The outer `<div>` — the input plane. `touch-action: none` (mandatory
//!    for pointer surfaces: without it the browser claims pans for scrolling
//!    and the app never sees them), wheel + pointer handlers.
//! 2. One `<svg>` with a single `<g transform=…>` — every vector shape,
//!    rendered by [`super::shape_view::ShapeView`], keyed by shape id and
//!    z-ordered by [`FracIndex`].
//! 3. An HTML overlay `<div>` carrying the *same* transform
//!    (`transform-origin: 0 0`) — empty in this task; text editing (Task 14)
//!    and sandboxed iframes (Task 16) land here. `pointer-events: none` so
//!    the input plane below keeps receiving gestures.
//!
//! # Keyed `<For>` + per-id memo
//!
//! The document signal is replaced wholesale on every fetch/reconcile, so a
//! `<For>` keyed by id alone would pin each row to the *first* value it saw
//! (keyed reconciliation reuses the view without re-running children — the
//! reason the library page keys rows by `(id, revision)`). Keying by content
//! instead would rebuild every DOM node on every move. The split: key by id
//! for DOM identity, and hand each child a `Memo` that looks its shape up in
//! a shared id→shape map — content changes flow through the memo, and
//! unchanged shapes (`PartialEq` on the memo value) don't re-render at all.
//!
//! # Space-bar pan and the keep-alive container
//!
//! `MainContent` hides this view with CSS instead of unmounting it, so the
//! window key listeners outlive the *visible* editor. Three gates keep the
//! space chord from leaking: the editor only mounts while a document is
//! open (structural), the route must actually be `/canvas`
//! (`PanelMode::from_path` — the same single source the sidebar uses), and
//! `focus_is_editable` keeps space working inside inputs. Keyup clears
//! unconditionally: a gated keyup would leave the pan mode latched on.

use std::collections::HashMap;

use aleph_protocol::canvas::Shape;
use leptos::ev::{keydown, keyup};
use leptos::prelude::*;
use leptos_router::hooks::use_location;
use wasm_bindgen::JsCast;

use super::shape_view::ShapeView;
use super::viewport::{self, PanDrag};
use crate::components::mode_sidebar::PanelMode;
use crate::state::canvas::{CanvasState, CanvasTool};
use crate::state::hotkey::focus_is_editable;

/// Render order: z ascending (FracIndex is lexicographic), id as the
/// tie-break so two shapes minted with the same index still sort stably.
fn z_sorted_ids(shapes: &[Shape]) -> Vec<String> {
    let mut refs: Vec<&Shape> = shapes.iter().collect();
    refs.sort_by(|a, b| {
        a.common()
            .z
            .cmp(&b.common().z)
            .then_with(|| a.id().cmp(b.id()))
    });
    refs.into_iter().map(|s| s.id().to_string()).collect()
}

/// Id → shape, the lookup side of the keyed-`<For>` split (module doc).
fn shapes_by_id(shapes: &[Shape]) -> HashMap<String, Shape> {
    shapes
        .iter()
        .map(|s| (s.id().to_string(), s.clone()))
        .collect()
}

/// The editor surface. Mounted by `OpenCanvasPane` once the document has
/// arrived; reads everything through [`CanvasState`].
#[component]
pub(super) fn CanvasEditor() -> impl IntoView {
    let canvas = expect_context::<CanvasState>();
    let camera = canvas.camera;
    let doc = canvas.doc;
    let pathname = use_location().pathname;

    let sorted_ids = Memo::new(move |_| {
        doc.with(|d| {
            d.as_ref()
                .map(|d| z_sorted_ids(&d.shapes))
                .unwrap_or_default()
        })
    });
    let shape_map = Memo::new(move |_| {
        doc.with(|d| {
            d.as_ref()
                .map(|d| shapes_by_id(&d.shapes))
                .unwrap_or_default()
        })
    });

    let space_down = RwSignal::new(false);
    let pan_drag: RwSignal<Option<PanDrag>> = RwSignal::new(None);

    let down_handle = window_event_listener(keydown, move |ev: web_sys::KeyboardEvent| {
        if ev.code() != "Space"
            || focus_is_editable()
            || PanelMode::from_path(&pathname.get_untracked()) != PanelMode::Canvas
        {
            return;
        }
        // Claim the key: without this, space scrolls whatever can scroll.
        ev.prevent_default();
        space_down.set(true);
    });
    let up_handle = window_event_listener(keyup, move |ev: web_sys::KeyboardEvent| {
        // Unconditional — if keydown passed the gates and the user tabbed
        // away before releasing, a gated keyup would latch the pan mode.
        if ev.code() == "Space" {
            space_down.set(false);
        }
    });
    on_cleanup(move || {
        down_handle.remove();
        up_handle.remove();
    });

    let on_wheel = move |ev: web_sys::WheelEvent| {
        ev.prevent_default();
        let dy = viewport::normalized_wheel_px(ev.delta_mode(), ev.delta_y());
        match viewport::wheel_intent(ev.ctrl_key(), ev.meta_key()) {
            viewport::WheelIntent::Zoom => {
                // The zoom anchor needs element-local coordinates; the pan
                // path below needs none, so only this arm pays the reflow.
                let Some(el) = ev
                    .current_target()
                    .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
                else {
                    return;
                };
                let rect = el.get_bounding_client_rect();
                let cursor = (
                    f64::from(ev.client_x()) - rect.left(),
                    f64::from(ev.client_y()) - rect.top(),
                );
                camera.update(|c| {
                    *c = viewport::zoom_at(*c, cursor, viewport::wheel_zoom_factor(dy));
                });
            }
            viewport::WheelIntent::Pan => {
                let dx = viewport::normalized_wheel_px(ev.delta_mode(), ev.delta_x());
                camera.update(|c| *c = viewport::wheel_pan(*c, dx, dy));
            }
        }
    };

    let on_pointerdown = move |ev: web_sys::PointerEvent| {
        if !viewport::pan_gesture(
            ev.button(),
            space_down.get_untracked(),
            canvas.tool.get_untracked(),
        ) {
            return;
        }
        ev.prevent_default();
        // Capture on the surface itself (current_target, not target): a
        // capture on a shape that a broadcast frame removes mid-drag would
        // silently end the pan.
        if let Some(el) = ev
            .current_target()
            .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
        {
            let _ = el.set_pointer_capture(ev.pointer_id());
        }
        pan_drag.set(Some(PanDrag::begin(
            ev.pointer_id(),
            f64::from(ev.client_x()),
            f64::from(ev.client_y()),
        )));
    };
    let on_pointermove = move |ev: web_sys::PointerEvent| {
        let Some(mut drag) = pan_drag.get_untracked() else {
            return;
        };
        let Some((dx, dy)) = drag.advance(
            ev.pointer_id(),
            f64::from(ev.client_x()),
            f64::from(ev.client_y()),
        ) else {
            return;
        };
        pan_drag.set(Some(drag));
        camera.update(|c| *c = viewport::pan_by(*c, dx, dy));
    };
    let end_pan = move |ev: web_sys::PointerEvent| {
        let Some(drag) = pan_drag.get_untracked() else {
            return;
        };
        if drag.pointer_id == ev.pointer_id() {
            pan_drag.set(None);
        }
    };

    view! {
        <div
            class=move || {
                let base = "relative flex-1 overflow-hidden bg-surface select-none";
                if pan_drag.get().is_some() {
                    format!("{base} cursor-grabbing")
                } else if space_down.get() || canvas.tool.get() == CanvasTool::Pan {
                    format!("{base} cursor-grab")
                } else {
                    base.to_string()
                }
            }
            style="touch-action: none"
            on:wheel=on_wheel
            on:pointerdown=on_pointerdown
            on:pointermove=on_pointermove
            on:pointerup=end_pan
            on:pointercancel=end_pan
        >
            <svg class="absolute inset-0 w-full h-full block">
                <g transform=move || viewport::svg_transform(camera.get())>
                    <For
                        each=move || sorted_ids.get()
                        key=Clone::clone
                        children=move |id: String| {
                            let shape =
                                Memo::new(move |_| shape_map.with(|m| m.get(&id).cloned()));
                            view! { <ShapeView shape=shape /> }
                        }
                    />
                </g>
            </svg>
            // HTML overlay: the same world transform. Hosts nothing yet —
            // text editing (Task 14) and sandboxed iframes (Task 16) mount
            // here. pointer-events:none keeps the svg surface as the input
            // plane until a child opts back in.
            <div
                class="absolute inset-0 pointer-events-none"
                style=move || viewport::css_transform(camera.get())
            ></div>
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aleph_protocol::canvas::{FracIndex, ShapeCommon, ShapeStyle};

    fn note(id: &str, z: FracIndex) -> Shape {
        Shape::Note {
            common: ShapeCommon {
                id: id.to_string(),
                x: 0.0,
                y: 0.0,
                w: 100.0,
                h: 100.0,
                z,
                parent_id: None,
            },
            style: ShapeStyle::default(),
            text: String::new(),
        }
    }

    #[test]
    fn shapes_render_in_fractional_index_order_not_document_order() {
        let top = FracIndex::first();
        let above = FracIndex::between(Some(&top), None);
        let below = FracIndex::between(None, Some(&top));
        // Document order is deliberately scrambled.
        let shapes = vec![
            note("mid", top.clone()),
            note("hi", above),
            note("lo", below),
        ];
        assert_eq!(z_sorted_ids(&shapes), vec!["lo", "mid", "hi"]);
    }

    #[test]
    fn equal_indexes_tie_break_on_id_for_a_stable_order() {
        let z = FracIndex::first();
        let shapes = vec![note("b", z.clone()), note("a", z.clone())];
        assert_eq!(z_sorted_ids(&shapes), vec!["a", "b"]);
        let map = shapes_by_id(&shapes);
        assert_eq!(map.len(), 2);
        assert_eq!(map["a"].id(), "a");
    }
}
