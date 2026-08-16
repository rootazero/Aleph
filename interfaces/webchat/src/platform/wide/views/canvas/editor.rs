//! The whiteboard editor shell: three stacked layers sharing one world
//! transform, viewport gestures (wheel zoom/pan, pointer pan), and the
//! Select-tool wiring over the pure interaction/ops modules.
//!
//! # The three layers
//!
//! 1. The outer `<div>` — the input plane. `touch-action: none` (mandatory
//!    for pointer surfaces: without it the browser claims pans for scrolling
//!    and the app never sees them), wheel + pointer handlers.
//! 2. One `<svg>` with a single `<g transform=…>` — every vector shape,
//!    rendered by [`super::shape_view::ShapeView`], keyed by shape id and
//!    z-ordered by [`FracIndex`], plus the marquee rectangle and the
//!    selection outline with its eight resize handles.
//! 3. An HTML overlay `<div>` carrying the *same* transform
//!    (`transform-origin: 0 0`) — empty in this task; text editing (Task 14)
//!    and sandboxed iframes (Task 16) land here. `pointer-events: none` so
//!    the input plane below keeps receiving gestures.
//!
//! # Who decides what
//!
//! DOM handlers here only convert coordinates and read the current signals;
//! every decision lives in [`super::interaction`] (pointer state machine) and
//! [`super::ops`] (optimistic pipeline). The handlers execute the returned
//! [`Effect`]s:
//!
//! - `SetSelection` writes the selection signal (a debounced `Effect` below
//!   pushes it to `canvas.selection.set` server-side, 300 ms).
//! - `Preview` applies tentative geometry to the doc signal — local only.
//! - `Commit` optimistically applies, records undo, and hands the ops to the
//!   single-flight [`SendQueue`]; [`pump`] drives the wire conversation,
//!   including `REVISION_CONFLICT` recovery (refetch + replay + resend).
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
//! chords from leaking: the editor only mounts while a document is open
//! (structural), the route must actually be `/canvas`
//! (`PanelMode::from_path` — the same single source the sidebar uses), and
//! `focus_is_editable` keeps every editing key working inside inputs. Keyup
//! clears unconditionally: a gated keyup would leave the pan mode latched on.

use std::collections::HashMap;

use aleph_protocol::canvas::{CanvasOp, Shape};
use gloo_timers::future::TimeoutFuture;
use leptos::ev::{keydown, keyup};
use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::hooks::use_location;
use wasm_bindgen::JsCast;

use super::id_mint;
use super::interaction::{self, Bbox, Handle, InteractionState};
use super::ops::{self, SendQueue, UndoStack};
use super::shape_view::ShapeView;
use super::viewport::{self, PanDrag};
use crate::api::canvas::CanvasApi;
use crate::components::admin_refusal;
use crate::components::mode_sidebar::PanelMode;
use crate::context::DashboardState;
use crate::state::canvas::{Camera, CanvasState, CanvasTool};
use crate::state::hotkey::focus_is_editable;

/// Click tolerance in *screen* pixels: a press that stays inside this box is
/// a click (select), not a move. Divided by zoom before it reaches the
/// machine, which thinks in world units.
const CLICK_SLOP_PX: f64 = 3.0;
/// Resize handle edge length in screen pixels (constant apparent size).
const HANDLE_PX: f64 = 8.0;
/// How long the selection may settle before it is pushed to the server.
const SELECTION_DEBOUNCE_MS: u32 = 300;
/// Arrow-key nudge distances, world units (shift = the larger one).
const NUDGE_STEP: f64 = 1.0;
const NUDGE_STEP_SHIFT: f64 = 10.0;

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

/// Element-local pointer position → world, via the event's current target
/// (the surface — its listeners are the only callers).
fn world_of(ev: &web_sys::PointerEvent, camera: Camera) -> Option<(f64, f64)> {
    let el = ev.current_target()?.dyn_into::<web_sys::Element>().ok()?;
    let rect = el.get_bounding_client_rect();
    Some(viewport::screen_to_world(
        camera,
        f64::from(ev.client_x()) - rect.left(),
        f64::from(ev.client_y()) - rect.top(),
    ))
}

/// Drive the wire conversation for one send queue: send, ack, promote the
/// next batch — and on a revision conflict, refetch + replay + resend.
///
/// One pump runs per non-empty queue lifetime ([`SendQueue::push`] returns a
/// batch only when idle, and the pump keeps draining until `on_ack` has
/// nothing left), so `canvas.apply` is strictly single-flight per client.
///
/// Every signal read past an await is `try_*`: the editor (whose scope owns
/// the queue) can unmount while a response is in flight, and a disposed
/// `StoredValue` answering `None` is the pump's normal exit.
fn pump(
    state: DashboardState,
    canvas: CanvasState,
    queue: StoredValue<SendQueue>,
    i18n: crate::i18n::I18nCtx,
    id: String,
    first: Vec<CanvasOp>,
) {
    spawn_local(async move {
        let mut batch = first;
        loop {
            // Base = the doc's revision: server truth, set by fetch and by
            // ack, never bumped optimistically (ops.rs module doc).
            let base = canvas
                .doc
                .try_with_untracked(|d| d.as_ref().filter(|d| d.id == id).map(|d| d.revision))
                .flatten();
            let Some(base) = base else {
                // The canvas closed (or switched) under us: the queue's
                // scope is gone with it.
                let _ = queue.try_update_value(SendQueue::clear);
                return;
            };
            match CanvasApi::apply(&state, &id, base, batch.clone()).await {
                Ok(revision) => {
                    let Some(open) = canvas.open_canvas.try_get_untracked() else {
                        return;
                    };
                    if open.as_deref() != Some(id.as_str()) {
                        let _ = queue.try_update_value(SendQueue::clear);
                        return;
                    }
                    let _ = canvas.doc.try_update(|d| {
                        if let Some(d) = d.as_mut() {
                            if d.id == id {
                                d.revision = revision;
                            }
                        }
                    });
                    match queue.try_update_value(SendQueue::on_ack).flatten() {
                        Some(next) => batch = next,
                        None => return,
                    }
                }
                Err(e) if ops::is_revision_conflict(&e) => {
                    // Someone else landed first. Refetch the truth, replay
                    // everything pending (refused batch + queued, original
                    // order) on top, resend against the fresh revision.
                    let _ = canvas.pending_conflict.try_update(|v| *v = true);
                    let Some(pending) = queue.try_update_value(SendQueue::recover).flatten() else {
                        let _ = canvas.pending_conflict.try_update(|v| *v = false);
                        return;
                    };
                    match CanvasApi::get(&state, &id).await {
                        Ok(envelope) => {
                            let Some(open) = canvas.open_canvas.try_get_untracked() else {
                                return;
                            };
                            if open.as_deref() != Some(id.as_str()) {
                                let _ = queue.try_update_value(SendQueue::clear);
                                return;
                            }
                            let mut fresh = envelope.canvas;
                            ops::apply_local(&mut fresh, &pending);
                            // Deliberately NOT adopt_envelope: the server's
                            // selection snapshot must not clobber the local
                            // one mid-edit.
                            let _ = canvas.asset_base.try_update(|v| *v = envelope.asset_base);
                            let _ = canvas.doc.try_update(|v| *v = Some(fresh));
                            let _ = canvas.pending_conflict.try_update(|v| *v = false);
                            batch = pending;
                        }
                        Err(e) => {
                            let _ = queue.try_update_value(SendQueue::clear);
                            let _ = canvas.load_error.try_update(|v| {
                                *v = Some(admin_refusal::settings_load_error(i18n, &e, |e| {
                                    format!("Failed to reload canvas after a conflict: {e}")
                                }));
                            });
                            return;
                        }
                    }
                }
                Err(e) => {
                    // A non-conflict refusal (or a dead transport): the
                    // optimistic doc may now be a lie. Drop what is pending
                    // and refetch server truth — replaying a batch the
                    // server rejected on its merits would fail forever.
                    let _ = queue.try_update_value(SendQueue::clear);
                    let _ = canvas.load_error.try_update(|v| {
                        *v = Some(admin_refusal::settings_write_error(i18n, &e, |e| {
                            format!("Failed to sync canvas edits: {e}")
                        }));
                    });
                    if let Ok(envelope) = CanvasApi::get(&state, &id).await {
                        let Some(open) = canvas.open_canvas.try_get_untracked() else {
                            return;
                        };
                        if open.as_deref() == Some(id.as_str()) {
                            let _ = canvas.asset_base.try_update(|v| *v = envelope.asset_base);
                            let _ = canvas.doc.try_update(|v| *v = Some(envelope.canvas));
                        }
                    }
                    return;
                }
            }
        }
    });
}

/// The editor surface. Mounted by `OpenCanvasPane` once the document has
/// arrived; reads everything through [`CanvasState`].
#[component]
pub(super) fn CanvasEditor() -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let canvas = expect_context::<CanvasState>();
    let i18n = crate::i18n::use_i18n();
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
    // Union bbox of the selection — follows previews live, so the handles
    // ride along during a drag.
    let sel_bbox = Memo::new(move |_| {
        let sel = canvas.selection.get();
        doc.with(|d| {
            d.as_ref()
                .and_then(|d| interaction::selection_bbox(&d.shapes, &sel))
        })
    });

    // Per-open-canvas interaction scope: this component unmounts when the
    // doc closes (OpenCanvasPane's memoized gate), taking the drag state,
    // the undo history and the send queue with it — undo must not leak
    // across canvases.
    let machine = StoredValue::new(InteractionState::new());
    let undo_stack = StoredValue::new(UndoStack::new());
    let queue = StoredValue::new(SendQueue::new());
    let marquee_sig: RwSignal<Option<Bbox>> = RwSignal::new(None);
    let surface_ref: NodeRef<leptos::html::Div> = NodeRef::new();

    let space_down = RwSignal::new(false);
    let pan_drag: RwSignal<Option<PanDrag>> = RwSignal::new(None);

    // ---- effect execution (interaction.rs decides; this executes) --------
    let commit = move |redo: Vec<CanvasOp>, undo: Vec<CanvasOp>| {
        canvas.doc.update(|d| {
            if let Some(d) = d.as_mut() {
                ops::apply_local(d, &redo);
            }
        });
        undo_stack.try_update_value(|u| u.record(redo.clone(), undo));
        let Some(id) = canvas.open_canvas.get_untracked() else {
            return;
        };
        if let Some(first) = queue.try_update_value(|q| q.push(redo)).flatten() {
            pump(state, canvas, queue, i18n, id, first);
        }
    };
    // Undo/redo travel the same wire as any edit but are never re-recorded.
    let apply_unrecorded = move |batch: Vec<CanvasOp>| {
        canvas.doc.update(|d| {
            if let Some(d) = d.as_mut() {
                ops::apply_local(d, &batch);
            }
        });
        let Some(id) = canvas.open_canvas.get_untracked() else {
            return;
        };
        if let Some(first) = queue.try_update_value(|q| q.push(batch)).flatten() {
            pump(state, canvas, queue, i18n, id, first);
        }
    };
    let run_effects = move |effects: Vec<interaction::Effect>| {
        for effect in effects {
            match effect {
                interaction::Effect::SetSelection(sel) => canvas.selection.set(sel),
                interaction::Effect::Preview(shapes) => {
                    let batch: Vec<CanvasOp> = shapes
                        .into_iter()
                        .map(|shape| CanvasOp::UpsertShape { shape })
                        .collect();
                    canvas.doc.update(|d| {
                        if let Some(d) = d.as_mut() {
                            ops::apply_local(d, &batch);
                        }
                    });
                }
                interaction::Effect::Commit { redo, undo } => commit(redo, undo),
            }
        }
    };
    let sync_marquee = move || {
        marquee_sig.set(
            machine
                .try_with_value(InteractionState::marquee_rect)
                .flatten(),
        );
    };

    // ---- selection push: debounce 300 ms → canvas.selection.set ----------
    let selection_generation = RwSignal::new(0u64);
    Effect::new(move |prev: Option<()>| {
        let sel = canvas.selection.get();
        // The mount run carries the selection the server just handed us —
        // pushing that back would be an echo, not information.
        if prev.is_none() {
            return;
        }
        let generation = selection_generation.get_untracked() + 1;
        selection_generation.set(generation);
        spawn_local(async move {
            TimeoutFuture::new(SELECTION_DEBOUNCE_MS).await;
            if selection_generation.try_get_untracked() != Some(generation) {
                return; // superseded by a newer selection
            }
            let Some(open) = canvas.open_canvas.try_get_untracked() else {
                return;
            };
            let Some(id) = open else {
                return;
            };
            // Last write wins server-side, by design; a failed push is not
            // an error surface (the selection is advisory context for the
            // model, not document state).
            let _ = CanvasApi::selection_set(&state, &id, sel).await;
        });
    });

    // ---- keyboard ---------------------------------------------------------
    let down_handle = window_event_listener(keydown, move |ev: web_sys::KeyboardEvent| {
        if PanelMode::from_path(&pathname.get_untracked()) != PanelMode::Canvas {
            return;
        }
        if ev.code() == "Space" {
            if focus_is_editable() {
                return;
            }
            // Claim the key: without this, space scrolls whatever can scroll.
            ev.prevent_default();
            space_down.set(true);
            return;
        }
        let key = ev.key();
        if key == "Escape" {
            // Esc only concerns an active drag: cancel it and roll the
            // preview back to the snapshot.
            if machine
                .try_with_value(InteractionState::is_dragging)
                .unwrap_or(false)
            {
                ev.prevent_default();
                let effects = machine
                    .try_update_value(InteractionState::cancel)
                    .unwrap_or_default();
                run_effects(effects);
                sync_marquee();
            }
            return;
        }
        if focus_is_editable() {
            return;
        }
        // No keyboard edits mid-drag: a delete landing between a preview and
        // its commit would let the commit resurrect the deleted shape.
        if machine
            .try_with_value(InteractionState::is_dragging)
            .unwrap_or(false)
        {
            return;
        }
        let meta = ev.meta_key() || ev.ctrl_key();
        if meta && key.eq_ignore_ascii_case("z") {
            ev.prevent_default();
            let batch = undo_stack
                .try_update_value(|u| if ev.shift_key() { u.redo() } else { u.undo() })
                .flatten();
            if let Some(batch) = batch {
                apply_unrecorded(batch);
            }
            return;
        }
        if meta && key.eq_ignore_ascii_case("d") {
            ev.prevent_default();
            let sel = canvas.selection.get_untracked();
            if sel.is_empty() {
                return;
            }
            let minted = canvas.doc.with_untracked(|d| {
                d.as_ref().map(|d| {
                    let mut mint = id_mint::mint_shape_id;
                    let (redo, new_ids) = interaction::duplicate_ops(&d.shapes, &sel, &mut mint);
                    let undo = ops::invert(d, &redo);
                    (redo, undo, new_ids)
                })
            });
            if let Some((redo, undo, new_ids)) = minted {
                if !redo.is_empty() {
                    commit(redo, undo);
                    canvas.selection.set(new_ids);
                }
            }
            return;
        }
        let nudge = match key.as_str() {
            "ArrowLeft" => Some((-1.0, 0.0)),
            "ArrowRight" => Some((1.0, 0.0)),
            "ArrowUp" => Some((0.0, -1.0)),
            "ArrowDown" => Some((0.0, 1.0)),
            _ => None,
        };
        if let Some((dx, dy)) = nudge {
            let sel = canvas.selection.get_untracked();
            if sel.is_empty() {
                return;
            }
            ev.prevent_default();
            let step = if ev.shift_key() {
                NUDGE_STEP_SHIFT
            } else {
                NUDGE_STEP
            };
            let pair = canvas.doc.with_untracked(|d| {
                d.as_ref().map(|d| {
                    let redo = interaction::nudge_ops(&d.shapes, &sel, dx * step, dy * step);
                    let undo = ops::invert(d, &redo);
                    (redo, undo)
                })
            });
            if let Some((redo, undo)) = pair {
                if !redo.is_empty() {
                    commit(redo, undo);
                }
            }
            return;
        }
        if key == "Delete" || key == "Backspace" {
            let sel = canvas.selection.get_untracked();
            if sel.is_empty() {
                return;
            }
            ev.prevent_default();
            let pair = canvas.doc.with_untracked(|d| {
                d.as_ref().map(|d| {
                    let redo = interaction::delete_ops(&sel);
                    let undo = ops::invert(d, &redo);
                    (redo, undo)
                })
            });
            if let Some((redo, undo)) = pair {
                commit(redo, undo);
                canvas.selection.set(Vec::new());
            }
        }
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

    // ---- wheel ------------------------------------------------------------
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

    // ---- pointer ----------------------------------------------------------
    let on_pointerdown = move |ev: web_sys::PointerEvent| {
        if viewport::pan_gesture(
            ev.button(),
            space_down.get_untracked(),
            canvas.tool.get_untracked(),
        ) {
            ev.prevent_default();
            // Capture on the surface itself (current_target, not target): a
            // capture on a shape that a broadcast frame removes mid-drag
            // would silently end the pan.
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
            return;
        }
        if ev.button() != 0 || canvas.tool.get_untracked() != CanvasTool::Select {
            // Creation tools land in Tasks 14/15.
            return;
        }
        let Some(world) = world_of(&ev, camera.get_untracked()) else {
            return;
        };
        if let Some(el) = ev
            .current_target()
            .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
        {
            let _ = el.set_pointer_capture(ev.pointer_id());
        }
        let sel = canvas.selection.get_untracked();
        let shift = ev.shift_key();
        let effects = canvas.doc.with_untracked(|d| {
            let Some(d) = d.as_ref() else {
                return Vec::new();
            };
            let hit = interaction::topmost_hit(&d.shapes, world);
            machine
                .try_update_value(|m| {
                    m.pointer_down(world, hit, &sel, &d.shapes, CanvasTool::Select, shift)
                })
                .unwrap_or_default()
        });
        run_effects(effects);
        sync_marquee();
    };
    let on_pointermove = move |ev: web_sys::PointerEvent| {
        if let Some(mut drag) = pan_drag.get_untracked() {
            let Some((dx, dy)) = drag.advance(
                ev.pointer_id(),
                f64::from(ev.client_x()),
                f64::from(ev.client_y()),
            ) else {
                return;
            };
            pan_drag.set(Some(drag));
            camera.update(|c| *c = viewport::pan_by(*c, dx, dy));
            return;
        }
        if !machine
            .try_with_value(InteractionState::is_dragging)
            .unwrap_or(false)
        {
            return;
        }
        let cam = camera.get_untracked();
        let Some(world) = world_of(&ev, cam) else {
            return;
        };
        let slop = CLICK_SLOP_PX / cam.zoom;
        let effects = canvas.doc.with_untracked(|d| {
            let Some(d) = d.as_ref() else {
                return Vec::new();
            };
            machine
                .try_update_value(|m| m.pointer_move(world, &d.shapes, slop))
                .unwrap_or_default()
        });
        run_effects(effects);
        sync_marquee();
    };
    let end_pan = move |ev: &web_sys::PointerEvent| -> bool {
        let Some(drag) = pan_drag.get_untracked() else {
            return false;
        };
        if drag.pointer_id == ev.pointer_id() {
            pan_drag.set(None);
            return true;
        }
        false
    };
    let on_pointerup = move |ev: web_sys::PointerEvent| {
        if end_pan(&ev) {
            return;
        }
        if !machine
            .try_with_value(InteractionState::is_dragging)
            .unwrap_or(false)
        {
            return;
        }
        let Some(world) = world_of(&ev, camera.get_untracked()) else {
            return;
        };
        let effects = canvas.doc.with_untracked(|d| {
            let Some(d) = d.as_ref() else {
                return Vec::new();
            };
            machine
                .try_update_value(|m| m.pointer_up(world, &d.shapes))
                .unwrap_or_default()
        });
        run_effects(effects);
        sync_marquee();
    };
    let on_pointercancel = move |ev: web_sys::PointerEvent| {
        let _ = end_pan(&ev);
        // A canceled pointer must roll back, not commit — the browser is
        // telling us the gesture never finished.
        let effects = machine
            .try_update_value(InteractionState::cancel)
            .unwrap_or_default();
        run_effects(effects);
        sync_marquee();
    };

    // Pointer down on a resize handle: capture on the *surface* (the handle
    // itself re-renders with every previewed geometry change — a capture on
    // it would die mid-drag) and arm the machine.
    let handle_down = move |ev: web_sys::PointerEvent, handle: Handle| {
        if ev.button() != 0 {
            return;
        }
        ev.stop_propagation();
        ev.prevent_default();
        if let Some(surface) = surface_ref.get_untracked() {
            let _ = surface.set_pointer_capture(ev.pointer_id());
        }
        let sel = canvas.selection.get_untracked();
        let originals = canvas.doc.with_untracked(|d| {
            d.as_ref()
                .map(|d| interaction::shapes_for(&d.shapes, &sel))
                .unwrap_or_default()
        });
        let effects = machine
            .try_update_value(|m| m.begin_resize(handle, originals))
            .unwrap_or_default();
        run_effects(effects);
    };

    view! {
        <div
            node_ref=surface_ref
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
            on:pointerup=on_pointerup
            on:pointercancel=on_pointercancel
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
                    // Marquee rectangle — world coords, screen-constant hairline.
                    {move || marquee_sig.get().map(|b| {
                        let zoom = camera.get().zoom;
                        view! {
                            <rect
                                x=b.x y=b.y width=b.w height=b.h
                                style="fill: color-mix(in oklch, var(--color-primary) 10%, transparent); \
                                       stroke: var(--color-primary);"
                                stroke-width=1.0 / zoom
                                stroke-dasharray=format!("{} {}", 4.0 / zoom, 3.0 / zoom)
                            />
                        }
                    })}
                    // Selection outline + the eight resize handles.
                    {move || sel_bbox.get().map(|b| {
                        let zoom = camera.get().zoom;
                        let hs = HANDLE_PX / zoom;
                        view! {
                            <g>
                                <rect
                                    x=b.x y=b.y width=b.w height=b.h
                                    fill="none"
                                    style="stroke: var(--color-primary);"
                                    stroke-width=1.5 / zoom
                                />
                                {Handle::ALL
                                    .iter()
                                    .map(|&handle| {
                                        let (cx, cy) = handle.center(b);
                                        view! {
                                            <rect
                                                x=cx - hs / 2.0
                                                y=cy - hs / 2.0
                                                width=hs
                                                height=hs
                                                style=format!(
                                                    "fill: var(--color-surface-raised); \
                                                     stroke: var(--color-primary); cursor: {};",
                                                    handle.cursor_css()
                                                )
                                                stroke-width=1.0 / zoom
                                                on:pointerdown=move |ev: web_sys::PointerEvent| {
                                                    handle_down(ev, handle);
                                                }
                                            />
                                        }
                                    })
                                    .collect_view()}
                            </g>
                        }
                    })}
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
