//! Embedded terminal view.
//!
//! The VT emulator is on the server (see `src/gateway/pty/screen/`), so this
//! view is a renderer: it subscribes to `pty.screen`, paints a grid, and
//! sends keystrokes. Unmounting is lossless — the screen survives on the
//! server and `pty.attach` restores it — which is why the subscription is
//! ephemeral and there is no park/reveal machinery here.
//!
//! This module is the wide/desktop entry point only (Task 14). There is no
//! phone screen yet — `app.rs`'s `MainContent` renders nothing for
//! `PanelMode::Terminal` on a phone form factor, the same treatment
//! `PanelMode::Projects` already gets there.
//!
//! # What keeps the tab strip current
//!
//! The terminal view is driven by PTY session snapshots and exit events.

use aleph_protocol::pty::{PtyScreenFrame, PTY_EXIT_TOPIC, PTY_SCREEN_TOPIC};
use leptos::prelude::*;
use leptos::task::spawn_local;
#[cfg(not(test))]
use wasm_bindgen::closure::Closure;
use wasm_bindgen::{JsCast, JsValue};
use web_sys::CanvasRenderingContext2d;

use crate::host::{TerminalEventStream, TerminalServices, TerminalState, TerminalText};
use crate::input::{InputError, InputPipeline, Selection};
use crate::session::ClientScreen;
use crate::tabs::TabBar;
use crate::{keymap, render, session};
use futures::{FutureExt, StreamExt};
#[cfg(not(test))]
use gloo_timers::future::TimeoutFuture;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

#[cfg(test)]
async fn terminal_event_tick() {
    futures::future::pending::<()>().await;
}

#[cfg(not(test))]
async fn terminal_event_tick() {
    TimeoutFuture::new(16).await;
}

#[cfg(not(test))]
fn terminal_now_ms() -> u64 {
    js_sys::Date::now().max(0.0) as u64
}

#[cfg(test)]
fn terminal_now_ms() -> u64 {
    0
}

// single-threaded browser runtime. This marker keeps local WASM resources in
// that hook; no instance is ever sent to another thread.
struct LocalCleanup<T>(T);
// SAFETY: LocalCleanup wraps resources that remain strictly within the
// single-threaded browser/WASM execution environment and are never transferred
// across thread boundaries.
unsafe impl<T> Send for LocalCleanup<T> {}
// SAFETY: Same as Send above; instances are accessed only from the single-threaded
// UI event loop in browser/WASM.
unsafe impl<T> Sync for LocalCleanup<T> {}

impl
    LocalCleanup<(
        Rc<Cell<bool>>,
        Rc<RefCell<Vec<TerminalEventStream>>>,
        futures::channel::mpsc::UnboundedSender<()>,
    )>
{
    fn dispose(self) {
        let (stop, streams, wake) = self.0;
        stop.set(true);
        let _ = wake.unbounded_send(());
        for stream in streams.borrow_mut().drain(..) {
            (stream.dispose)();
        }
    }
}

#[cfg(not(test))]
impl LocalCleanup<(web_sys::ResizeObserver, Closure<dyn FnMut(js_sys::Array)>)> {
    fn dispose(self) {
        self.0 .0.disconnect();
    }
}

#[component]
pub fn TerminalContent(state: TerminalState) -> impl IntoView {
    let services = Rc::new(state.services.clone());
    let tabs = state.tabs;
    let selection_request = state.selection_request;
    let new_request = state.new_request;
    let close_request = state.close_request;
    let canvas_ref = NodeRef::<leptos::html::Canvas>::new();
    let controller = StoredValue::new(crate::recovery::Recovery::new(tabs.get_untracked()));
    let session_id = StoredValue::new(None::<String>);
    let alive = StoredValue::new(true);
    let repaint_tick = RwSignal::new(0_u32);
    let error = RwSignal::new(None::<String>);
    let status = RwSignal::new(false);
    let retry_request = RwSignal::new(false);
    let actions_disabled = state.actions_disabled;
    actions_disabled.set(true);
    let font_family = StoredValue::new(render::FALLBACK_FONT_FAMILY.to_string());
    let font_size_px = StoredValue::new(render::FALLBACK_FONT_SIZE_PX);
    // The text-entry surface, the write FIFO, the selection and the size
    // coalescer are this view's own state. The recovery keeps the authority
    // on WHICH terminal is current; the fence it hands out is what ties a
    // write to that terminal and to nothing else.
    let input_ref = NodeRef::<leptos::html::Input>::new();
    let pipeline = StoredValue::new(InputPipeline::new());
    let selection = RwSignal::new(None::<Selection>);
    let drag_anchor = StoredValue::new(None::<(u16, u16)>);
    let dragging = StoredValue::new(false);
    let cell_metrics = StoredValue::new(None::<render::CellMetrics>);
    let resize = StoredValue::new(render::ResizeCoalescer::new());
    let input_error = RwSignal::new(None::<String>);
    let focus_request = state.focus_request;
    // The animation frame that is scheduled, if any: one at a time, and
    // cancelled when the view goes away.
    let frame = StoredValue::new(None::<AnimationFrameRequestHandle>);

    let stop_events = Rc::new(Cell::new(false));
    let event_streams = Rc::new(RefCell::new(Vec::<TerminalEventStream>::new()));
    let (wake, mut wake_rx) = futures::channel::mpsc::unbounded::<()>();
    let settlements = state.settlements;
    let mut settled_revision = settlements.revision();
    settlements.listen(wake.clone());
    let cleanup_state = LocalCleanup((
        Rc::clone(&stop_events),
        Rc::clone(&event_streams),
        wake.clone(),
    ));
    on_cleanup(move || {
        actions_disabled.try_set(true);
        // The view is gone: the unsent suffix is dropped (it belongs to a
        // terminal nobody can see any more), and nothing is measured or
        // reported from here on.
        pipeline.try_update_value(|p| p.discard_waiting());
        resize.try_update_value(|r| r.release());
        cleanup_state.dispose();
    });
    // Gestures are consumed before dispatch, never replayed by a new owner.
    new_request.set(0);
    selection_request.set(None);
    close_request.set(None);
    let services_for_events = Rc::clone(&services);
    let inbox = Rc::new(RefCell::new(Vec::<(
        crate::recovery::Request,
        Result<serde_json::Value, String>,
    )>::new()));

    // Focus follows an explicit gesture, never a background update.
    //
    // A reconnect that lands while the user is typing somewhere else must
    // not move the caret into a terminal they are not looking at, so the
    // only two things that focus the surface are a click on the canvas and
    // a New/select gesture the user actually made.
    let focus_surface: Rc<dyn Fn()> = {
        let input_ref = input_ref;
        Rc::new(move || {
            if let Some(el) = input_ref.try_get_untracked().flatten() {
                let el: web_sys::HtmlInputElement = el.unchecked_into();
                let _ = el.focus();
            }
        })
    };

    // Every byte the user produces goes through ONE funnel: the FIFO.
    //
    // A keystroke, an IME commit and a paste are the same wire call, and a
    // second copy of it is where the two would drift -- the ordering
    // guarantee only holds if there is one place that decides what is
    // written next.
    let enqueue: Rc<dyn Fn(String)> = {
        let services = Rc::clone(&services);
        let controller = controller;
        let pipeline = pipeline;
        let input_error = input_error;
        let text = services.text.clone();
        Rc::new(move |payload: String| {
            let connection = services.connection.get_untracked();
            // The write must belong to the terminal that is on screen NOW.
            // A write queued for a terminal that has been switched away from
            // is dropped, not redirected: a half-typed command arriving in
            // the wrong shell is worse than no command.
            let fence = controller
                .try_with_value(|c| {
                    (c.ready && c.current_connection() == connection).then(|| c.fence())
                })
                .flatten();
            let Some(fence) = fence else { return; };
            match pipeline.update_value(|p| p.enqueue(payload, fence)) {
                Ok(()) => {}
                Err(InputError::TooLarge { .. }) => {
                    input_error.set(Some(text.input_too_large.clone()));
                }
                Err(_) => {
                    input_error.set(Some(text.input_queue_full.clone()));
                }
            }
        })
    };

    spawn_local(async move {
        let mut painted_revision = 0_u64;
        let services = services_for_events;
        let mut last_connection = services.connection.get_untracked();
        let mut last_bracketed: Option<bool> = None;
        while !stop_events.get() && alive.try_get_value().is_some() {
            let now = terminal_now_ms();
            let connection = services.connection.get_untracked();
            controller.update_value(|c| c.connect(connection));
            // Observe before accepting replies: a list issued before the old
            // write settled cannot authorize the replacement owner's model.
            let revision = settlements.revision();
            if settled_revision != revision {
                settled_revision = revision;
                controller.update_value(|c| c.mutation_settled());
            }
            if connection != last_connection {
                for stream in event_streams.borrow_mut().drain(..) {
                    (stream.dispose)();
                }
                last_connection = connection;
                // The connection went away: the writes that were waiting
                // belong to a socket that no longer exists and are dropped
                // rather than replayed into the replacement.
                pipeline.update_value(|p| p.on_selection_changed());
                selection.set(None);
            }
            let (subscriptions, replies): (Vec<_>, Vec<_>) = inbox
                .borrow_mut()
                .drain(..)
                .partition(|(request, _)| request.kind == crate::recovery::RequestKind::Subscribe);
            // Coverage must be accepted before draining its streams. All queued
            // frames/loss must then reach the active stage before an attach reply
            // can commit, including when the response is the only wake-up.
            for (request, result) in subscriptions {
                let result = result.map_err(|message| read_error(&services.text, &message));
                controller.update_value(|c| c.complete(request, result, now));
            }
            let mut loss = None;
            let mut events = Vec::new();
            for stream in event_streams.borrow().iter() {
                match (stream.drain)() {
                    Ok(batch) => events.extend(batch),
                    Err(message) => {
                        loss = Some(message);
                        break;
                    }
                }
            }
            if let Some(message) = loss {
                controller.update_value(|c| c.loss(now, message));
                for stream in event_streams.borrow_mut().drain(..) {
                    (stream.dispose)();
                }
            } else {
                for event in events {
                    let result = controller.try_update_value(|c| {
                        if event.topic == PTY_EXIT_TOPIC {
                            if let Some(sid) = event
                                .data
                                .get("session_id")
                                .and_then(serde_json::Value::as_str)
                            {
                                c.exited(sid);
                                return Ok(());
                            }
                        } else if event.topic == PTY_SCREEN_TOPIC {
                            if let Ok(frame) = serde_json::from_value::<PtyScreenFrame>(event.data)
                            {
                                c.frame(frame, now);
                                return Ok(());
                            }
                        }
                        Err("Terminal event could not be decoded".to_string())
                    });
                    if let Some(Err(message)) = result {
                        controller.update_value(|c| c.loss(now, message));
                        for stream in event_streams.borrow_mut().drain(..) {
                            (stream.dispose)();
                        }
                        break;
                    }
                }
            }
            // The paste mode belongs to the terminal, and it decides whether
            // a paste is bracketed. A mode change invalidates the payloads
            // that were encoded for the old one.
            let bracketed = controller.with_value(|c| {
                c.screen
                    .as_ref()
                    .and_then(ClientScreen::bracketed_paste)
            });
            if bracketed != last_bracketed {
                last_bracketed = bracketed;
                pipeline.update_value(|p| p.discard_waiting());
            }
            for (request, result) in replies {
                let effect = matches!(
                    request.kind,
                    crate::recovery::RequestKind::Spawn | crate::recovery::RequestKind::Close
                );
                let attached = request.kind == crate::recovery::RequestKind::Attach
                    && controller.with_value(|c| c.accepts(&request));
                let result = result.map_err(|message| {
                    if effect {
                        write_error(&services.text, &message)
                    } else {
                        read_error(&services.text, &message)
                    }
                });
                controller.update_value(|c| {
                    if effect {
                        c.complete_effect(request, result);
                    } else {
                        c.complete(request, result, now);
                    }
                });
                if attached && controller.with_value(|c| c.ready) {
                    measure_and_report(
                        canvas_ref,
                        font_family.get_value(),
                        font_size_px.get_value(),
                        repaint_tick,
                        cell_metrics,
                        resize,
                    );
                    // A session has never been told the size of the surface
                    // it now lives on. Report it again even when it did not
                    // change: a PTY that inherited the size of the session it
                    // replaced is sized wrong until somebody tells it.
                    resize.update_value(|r| r.force());
                    // Focus is the user's gesture, never the attach's: a
                    // background reconnect must not pull the caret into a
                    // terminal nobody is looking at.
                    if focus_request.get_untracked() {
                        focus_request.set(false);
                        focus_surface();
                    }
                }
            }
            if retry_request.get_untracked() {
                retry_request.set(false);
                controller.update_value(|c| c.retry());
            }
            if let Some(sid) = selection_request.get_untracked() {
                selection_request.set(None);
                controller.update_value(|c| c.select(&sid));
                // The writes that were waiting for the previous terminal
                // belong to a terminal that is no longer on screen.
                pipeline.update_value(|p| p.on_selection_changed());
                selection.set(None);
            }
            let close = close_request.get_untracked();
            close_request.set(None);
            let new = new_request.get_untracked() != 0;
            new_request.set(0);
            let effect = if close.is_some() || new {
                settlements.reserve().and_then(|ticket| {
                    let request = controller.try_update_value(|c| c.gesture(close)).flatten();
                    if request.is_none() {
                        settlements.cancel(ticket);
                    }
                    request.map(|request| (request, Some(ticket)))
                })
            } else {
                None
            };
            let request = controller.try_update_value(|c| c.next(now)).flatten();
            for (request, ticket) in effect
                .into_iter()
                .chain(request.map(|request| (request, None)))
            {
                let services = Rc::clone(&services);
                let inbox = Rc::clone(&inbox);
                let stop = Rc::clone(&stop_events);
                let streams = Rc::clone(&event_streams);
                let wake = wake.clone();
                let settlements = Rc::clone(&settlements);
                spawn_local(async move {
                    if stop.get() {
                        if let Some(ticket) = ticket {
                            settlements.cancel(ticket);
                        }
                        return;
                    }
                    let (result, opened) =
                        if request.kind == crate::recovery::RequestKind::Subscribe {
                            match open_terminal_streams(
                                Rc::clone(&services),
                                request.fence.epoch,
                                Rc::clone(&stop),
                            )
                            .await
                            {
                                Ok(streams) => (Ok(serde_json::Value::Null), Some(streams)),
                                Err(message) => (Err(message), None),
                            }
                        } else {
                            let (method, params) = request.rpc();
                            ((services.invoke)(method.to_string(), params).await, None)
                        };
                    if let Some(opened) = opened {
                        let current = !stop.get()
                            && services.connection.get_untracked().epoch == request.fence.epoch
                            && controller
                                .try_with_value(|c| c.accepts(&request))
                                .unwrap_or(false);
                        if current {
                            for stream in streams.borrow_mut().drain(..) {
                                (stream.dispose)();
                            }
                            streams.borrow_mut().extend(opened);
                        } else {
                            for stream in opened {
                                (stream.dispose)();
                            }
                        }
                    }
                    // Release on actual settlement, even if the initiating UI
                    // or SDK was disposed. Only invalidation crosses owners.
                    if let Some(ticket) = ticket {
                        settlements.settled(ticket);
                    }
                    if !stop.get() {
                        inbox.borrow_mut().push((request, result));
                        let _ = wake.unbounded_send(());
                    }
                });
            }
            controller.with_value(|c| {
                if tabs.get_untracked() != c.tabs {
                    tabs.set(c.tabs.clone());
                }
                let selected = c.selected_id().map(str::to_string);
                if session_id.get_value() != selected {
                    session_id.set_value(selected);
                }
                if error.get_untracked() != c.error {
                    error.set(c.error.clone());
                }
                if status.get_untracked() != c.ready {
                    status.set(c.ready);
                }
                let disabled = settlements.pending()
                    || c.effect_pending()
                    || !c.covered
                    || !c.current_connection().connected;
                if actions_disabled.get_untracked() != disabled {
                    actions_disabled.set(disabled);
                }
                if painted_revision != c.paint_revision {
                    painted_revision = c.paint_revision;
                    repaint_tick.update(|tick| *tick = tick.wrapping_add(1));
                }
            });
            // Report the measured size, but only the LATEST one: a burst of
            // geometry changes must not become a burst of RPCs, and the size
            // that is still true when the frame is drawn is the one worth
            // sending.
            if let Some((rows, cols)) = resize.update_value(|r| r.take()) {
                if let Some(sid) = controller.with_value(|c| c.selected_id().map(str::to_string)) {
                    let services = Rc::clone(&services);
                    spawn_local(async move {
                        let _ = (services.invoke)(
                            "pty.resize".to_string(),
                            serde_json::json!({ "session_id": sid, "rows": rows, "cols": cols }),
                        )
                        .await;
                    });
                }
            }

            // One write in flight. The FIFO decides what goes next and this
            // loop is the only place a `pty.input` starts, so the ordering is
            // one decision in one place.
            if controller.with_value(|c| c.ready) {
                if let Some(sid) = controller.with_value(|c| c.selected_id().map(str::to_string)) {
                    let fence = controller.with_value(|c| c.fence());
                    if let Some(item) = pipeline.update_value(|p| p.take_next(fence)) {
                        let services = Rc::clone(&services);
                        let controller = controller;
                        let pipeline = pipeline;
                        let input_error = input_error;
                        let text = services.text.clone();
                        spawn_local(async move {
                            let result = (services.invoke)(
                                "pty.input".to_string(),
                                serde_json::json!({
                                    "session_id": sid,
                                    "data": item.payload,
                                    "base64": false,
                                }),
                            )
                            .await;
                            let ok = result.is_ok();
                            // The bytes are gone either way. An ambiguous
                            // failure is NOT a license to replay them, so the
                            // suffix behind them is dropped and the user is
                            // told, instead of a command fragment being sent.
                            let failed = controller
                                .try_with_value(|c| c.fence())
                                .and_then(|fence| {
                                    pipeline.try_update_value(|p| p.settled(ok, fence)).flatten()
                                });
                            if failed.is_some() {
                                input_error.try_update(|e| *e = Some(text.input_failed.clone()));
                            }
                        });
                    }
                }
            }
            futures::future::select(terminal_event_tick().boxed_local(), wake_rx.next()).await;
        }
    });

    // Font config: read `[policies.terminal]`'s font fields once at mount.
    //
    // `config.get`'s `section` param is ONE top-level key, not a dotted
    // path -- `"policies.terminal"` is not a key `config_json` has, so the
    // server answers `{}` for it on every machine regardless of what is in
    // `config.toml`, and reads it exactly like "field absent". Confirmed
    // against the live handler (`handle_get_full_config` in
    // `src/gateway/handlers/config.rs`): `config_json.get(section)` is a
    // single `serde_json::Map` lookup. The correct call is `"policies"`
    // (which the server DOES resolve, returning EFFECTIVE values with
    // struct defaults already applied), then index `["terminal"]` locally.
    //
    // A refused or failed read is not "no font configured" -- it just means
    // we cannot tell, and the terminal has to draw something regardless, so
    // the `StoredValue`s above already hold a font known to work. Nothing
    // is shown to the user for this specific failure: falling back here is
    // normal operation, not a degraded state (the settings design already
    // treats "config unreadable" and "unset" as the same terminal render,
    // by construction -- there is no separate wording to get wrong).
    let config_services = Rc::clone(&services);
    Effect::new(move |_| {
        let services = Rc::clone(&config_services);
        spawn_local(async move {
            let Ok(policies) = (services.invoke)(
                "config.get".to_string(),
                serde_json::json!({ "section": "policies" }),
            )
            .await
            else {
                return;
            };
            if alive.try_get_value().is_none() {
                return;
            }
            let Some(v) = policies.get("terminal") else {
                return;
            };
            if let Some(family) = v.get("font_family").and_then(serde_json::Value::as_str) {
                if !family.is_empty() {
                    font_family.set_value(family.to_string());
                }
            }
            if let Some(size) = v.get("font_size_px").and_then(serde_json::Value::as_f64) {
                font_size_px.set_value(size);
            }
            // The mount effect above may already have measured and resized
            // using the fallback stack if it won this race; re-derive
            // everything now that the real config is in, rather than
            // leaving the canvas showing the fallback font until whatever
            // next resize or frame happens to trigger a repaint.
            //
            // `try_` for the same reason `attach_to` above uses it: this is
            // the far side of `config.get`, and the user may well have left
            // the terminal view while it was in flight. `None` from any of
            // them means the owner is gone and there is no canvas to measure.
            let (Some(family), Some(size)) = (
                font_family.try_get_value(),
                font_size_px.try_get_value(),
            ) else {
                return;
            };
            measure_and_report(
                canvas_ref,
                family,
                size,
                repaint_tick,
                cell_metrics,
                resize,
            );
        });
    });

    // Repaint: coalesced onto the browser's own paint cycle rather than drawn
    // synchronously from the frame handler above, so a burst of frames (e.g.
    // `yes | head -100000`) triggers at most one paint per animation frame
    // instead of one per frame received off the wire.
    Effect::new(move |_| {
        repaint_tick.get();
        let family = font_family.get_value();
        let size_px = font_size_px.get_value();
        #[cfg(not(test))]
        {
            // One frame at a time: the previous request is cancelled instead
            // of being left to fire into a view that is already gone.
            if let Some(handle) = frame.try_get_value().flatten() {
                handle.cancel();
            }
            let handle = request_animation_frame_with_handle(move || {
                frame.try_set_value(None);
                let Some((canvas, ctx)) = canvas_ctx(canvas_ref) else {
                    return;
                };
                controller.with_value(|c| {
                    let theme = render::Theme::dark();
                    if let Some(s) = &c.screen {
                        let m = render::measure(&ctx, &family, size_px);
                        render::paint(&ctx, s, m, &theme);
                        // The selection is painted over the cells it covers,
                        // with the metrics the pointer maths used.
                        if let Some(sel) = selection.get_untracked() {
                            if !sel.is_empty() {
                                render::paint_selection(&ctx, s, m, &theme, &sel);
                            }
                        }
                    } else {
                        // Authoritative empty/last close must erase the old paint
                        // even when the backing-store geometry has not changed.
                        ctx.set_fill_style_str(theme.bg);
                        ctx.fill_rect(
                            0.0,
                            0.0,
                            f64::from(canvas.width()),
                            f64::from(canvas.height()),
                        );
                    }
                });
            });
            if let Ok(handle) = handle {
                frame.set_value(Some(handle));
            }
        }
        #[cfg(test)]
        let _ = (family, size_px);
    });

    // Resize: report our measured viewport to the server whenever the
    // canvas's box changes, and once now. This does NOT touch
    // `ClientScreen`'s own dimensions -- sizing is smallest-wins across every
    // attached client (Task 10); the geometry that actually took effect
    // arrives on the next frame and `ClientScreen::apply` adopts it there.
    // Writing our own request locally would be a second, driftable answer to
    // "how big is this PTY" in exactly the multi-client case Task 10 exists
    // for.
    Effect::new(move |_| {
        measure_and_report(
            canvas_ref,
            font_family.get_value(),
            font_size_px.get_value(),
            repaint_tick,
            cell_metrics,
            resize,
        );

        #[cfg(not(test))]
        {
            let cb: Closure<dyn FnMut(js_sys::Array)> =
                Closure::new(move |_entries: js_sys::Array| {
                    measure_and_report(
                        canvas_ref,
                        font_family.get_value(),
                        font_size_px.get_value(),
                        repaint_tick,
                        cell_metrics,
                        resize,
                    );
                });
            if let Some((canvas, _ctx)) = canvas_ctx(canvas_ref) {
                if let Ok(observer) = web_sys::ResizeObserver::new(cb.as_ref().unchecked_ref()) {
                    observer.observe(&canvas);
                    let cleanup = LocalCleanup((observer, cb));
                    on_cleanup(move || cleanup.dispose());
                }
            }
        }
    });

    // ---- The keyboard and the pointer -------------------------------------
    //
    // A canvas cannot hold text, so an IME has nothing to compose into and
    // the clipboard has nowhere to land. The terminal therefore owns a real,
    // focusable text field: the canvas is the VIEW of the terminal and this
    // field is its KEYBOARD.
    //
    // One funnel for every byte the user produces -- a keystroke, an IME
    // commit, a paste -- because they are the same wire call, and a second
    // copy of it is where the two would drift.
    let copy_selection: Rc<dyn Fn()> = {
        let controller = controller;
        let selection = selection;
        let input_error = input_error;
        let text = services.text.clone();
        Rc::new(move || {
            let Some(sel) = selection.get_untracked() else {
                return;
            };
            if sel.is_empty() {
                return;
            }
            // The text is derived from the painted runs, so a wide glyph's
            // continuation cell is not copied twice.
            let content = controller.with_value(|c| {
                c.screen
                    .as_ref()
                    .map(|screen| crate::input::selection_text(screen, &sel))
            });
            let Some(content) = content else {
                return;
            };
            write_clipboard(&content, &text.copy_failed, &input_error);
        })
    };

    // A key the keymap does not claim is left alone entirely, not even
    // `prevent_default()`, so any browser/OS shortcut we do not mean to
    // swallow keeps working. That includes the PASTE chords: the keymap
    // answers `Browser` for Cmd-V and Ctrl-Shift-V -- one decision in one
    // place rather than half a rule on each side.
    //
    // A PRINTABLE key is not claimed either: the text field inserts it and
    // the `input` event delivers it exactly once. Only a control byte --
    // something with no text to insert -- is claimed here, because the
    // field would otherwise swallow it.
    let handle_key = {
        let enqueue = Rc::clone(&enqueue);
        let selection = selection;
        let copy_selection = Rc::clone(&copy_selection);
        move |ev: web_sys::KeyboardEvent, claim_printable: bool| {
            let key = ev.key();
            let ctrl = ev.ctrl_key();
            let alt = ev.alt_key();
            let shift = ev.shift_key();
            let meta = ev.meta_key();
            // A copy chord with something selected is a copy. Without a
            // selection `Ctrl-C` stays the terminal's interrupt.
            if selection.get_untracked().is_some_and(|s| !s.is_empty())
                && crate::input::is_copy_chord(&key, ctrl, shift, meta)
            {
                ev.prevent_default();
                copy_selection();
                return;
            }
            match keymap::encode_key(&key, ctrl, alt, shift, meta) {
                keymap::KeyAction::Bytes(bytes) => {
                    if claim_printable || is_control_bytes(&bytes) {
                        ev.prevent_default();
                        enqueue(String::from_utf8_lossy(&bytes).to_string());
                    }
                }
                keymap::KeyAction::Browser => {}
            }
        }
    };

    let on_surface_keydown = {
        let handle_key = handle_key.clone();
        move |ev: web_sys::KeyboardEvent| handle_key(ev, false)
    };

    // The canvas holds no text of its own, so a printable key pressed while
    // it has the focus is sent straight away: there is no field to insert it
    // into.
    let on_canvas_keydown = {
        let handle_key = handle_key.clone();
        move |ev: web_sys::KeyboardEvent| handle_key(ev, true)
    };

    // Paste. The clipboard's TEXT is only readable from a `paste` event -- a
    // `keydown` handler cannot reach it at all -- which is the whole reason
    // the keymap hands the paste chords back to the browser instead of
    // encoding them.
    //
    // Wrapped in `ESC[200~ ... ESC[201~` only when the program has asked for
    // bracketed paste. `ClientScreen::bracketed_paste()` answers `None` until
    // the server has said, and `encode_paste` treats unknown as "do not
    // wrap": the escape sequences showing up literally in someone's command
    // line is a visible corruption, while not sending them only gives up a
    // hint.
    let on_paste = {
        let enqueue = Rc::clone(&enqueue);
        let pipeline = pipeline;
        let controller = controller;
        let input_error = input_error;
        let too_large = services.text.input_too_large.clone();
        move |ev: web_sys::ClipboardEvent| {
            let Some(text) = ev
                .clipboard_data()
                .and_then(|d| d.get_data("text").ok())
                .filter(|t| !t.is_empty())
            else {
                return;
            };
            // Read synchronously, inside the event handler: the mode belongs
            // to the screen as it is NOW, not as it may be by the time the
            // `pty.input` RPC resolves.
            let bracketed =
                controller.with_value(|c| c.screen.as_ref().and_then(ClientScreen::bracketed_paste));
            ev.prevent_default();
            match crate::input::encode_paste(&text, bracketed) {
                Ok(payload) => {
                    // The browser delivers the pasted text AGAIN as an
                    // `input` event. That repeat is dropped: a paste is one
                    // write, not two.
                    pipeline.update_value(|p| p.suppress_repeat(&text));
                    enqueue(payload);
                }
                Err(InputError::TooLarge { .. }) => {
                    // Refused BEFORE anything is queued. A paste that is
                    // split is a paste the program never finishes reading.
                    input_error.try_update(|e| *e = Some(too_large.clone()));
                }
                Err(_) => {}
            }
        }
    };

    // The text field's `input` event: the browser's own report of what was
    // inserted. A preedit is not text yet -- the IME is still typing -- and
    // a commit the browser already delivered through `compositionend` is a
    // repeat of the same bytes.
    let on_input = {
        let enqueue = Rc::clone(&enqueue);
        let pipeline = pipeline;
        let controller = controller;
        move |ev: web_sys::Event| {
            let ev: web_sys::InputEvent = ev.unchecked_into();
            if ev.is_composing() || ev.input_type() == "insertCompositionText" {
                // A preedit is not a commit.
                return;
            }
            let Some(data) = ev.data() else {
                return;
            };
            if data.is_empty() {
                return;
            }
            let fence = controller.with_value(|c| c.fence());
            let payload = pipeline.update_value(|p| p.ordinary_input(&data, fence));
            if let Some(payload) = payload {
                enqueue(payload);
            }
        }
    };

    // The IME. The preedit is not text yet, so nothing is sent until the
    // commit -- and the commit is sent exactly once.
    let on_composition_start = {
        let pipeline = pipeline;
        let controller = controller;
        move |_ev: web_sys::CompositionEvent| {
            let fence = controller.with_value(|c| c.fence());
            pipeline.update_value(|p| p.composition_start(fence));
        }
    };

    let on_composition_update = {
        let pipeline = pipeline;
        move |_ev: web_sys::CompositionEvent| {
            pipeline.update_value(|p| p.composition_update(""));
        }
    };

    let on_composition_end = {
        let enqueue = Rc::clone(&enqueue);
        let pipeline = pipeline;
        let controller = controller;
        move |ev: web_sys::CompositionEvent| {
            let fence = controller.with_value(|c| c.fence());
            let committed =
                pipeline.update_value(|p| p.composition_end(fence, &ev.data().unwrap_or_default()));
            if let Some(committed) = committed {
                enqueue(committed);
            }
        }
    };

    // A blur cancels the transient interaction: the composition belonged to a
    // field nobody is typing into any more, and the drag belonged to a
    // pointer that is no longer over the canvas. Neither changes the PTY's
    // modes.
    let on_surface_blur = {
        let pipeline = pipeline;
        let dragging = dragging;
        let drag_anchor = drag_anchor;
        move |_ev: web_sys::FocusEvent| {
            pipeline.update_value(|p| p.composition_cancel());
            dragging.set(false);
            drag_anchor.set_value(None);
        }
    };

    // Pointer selection: the drag picks CELLS, not pixels, and the cells
    // are the ones the painter used -- the same metrics, so a cell can
    // never be two different things.
    let on_canvas_mousedown = {
        let focus_surface = Rc::clone(&focus_surface);
        let canvas_ref = canvas_ref;
        let cell_metrics = cell_metrics;
        let drag_anchor = drag_anchor;
        let dragging = dragging;
        let selection = selection;
        let repaint_tick = repaint_tick;
        move |ev: web_sys::MouseEvent| {
            // The canvas is the VIEW; the text field is the KEYBOARD. The
            // click focuses the field, so an IME and the clipboard have
            // somewhere to go.
            ev.prevent_default();
            focus_surface();
            let Some(cell) = cell_at(&ev, canvas_ref, cell_metrics.get_value()) else {
                selection.set(None);
                repaint_tick.update(|tick| *tick = tick.wrapping_add(1));
                return;
            };
            drag_anchor.set_value(Some(cell));
            dragging.set(true);
            selection.set(Some(Selection::new(cell.0, cell.1, cell.0, cell.1)));
            repaint_tick.update(|tick| *tick = tick.wrapping_add(1));
        }
    };

    let on_canvas_mousemove = {
        let canvas_ref = canvas_ref;
        let cell_metrics = cell_metrics;
        let drag_anchor = drag_anchor;
        let dragging = dragging;
        let selection = selection;
        let repaint_tick = repaint_tick;
        move |ev: web_sys::MouseEvent| {
            if !dragging.get_untracked() {
                return;
            }
            let Some(anchor) = drag_anchor.get_value() else {
                return;
            };
            let Some(cell) = cell_at(&ev, canvas_ref, cell_metrics.get_value()) else {
                return;
            };
            selection.set(Some(Selection::new(anchor.0, anchor.1, cell.0, cell.1)));
            repaint_tick.update(|tick| *tick = tick.wrapping_add(1));
        }
    };

    let on_canvas_mouseup = {
        let dragging = dragging;
        move |_ev: web_sys::MouseEvent| {
            dragging.set(false);
        }
    };
    let reconnecting = services.text.reconnecting.clone();
    let retry_label = services.text.retry.clone();
    let new_label = services.text.new_tab.clone();
    let input_label = services.text.input_label.clone();
    let copy_label = services.text.copy.clone();
    view! {
        // `data-terminal-view=""`: anchor for Part 2's `qa/terminal`
        // real-device rig. Zero in-crate consumers today -- keep it anyway.
        //
        // `h-full` is load-bearing, not decoration: `app.rs` mounts this
        // view straight under `<main>` (through a `style:display="contents"`
        // wrapper, which drops out of the box tree entirely) and `<main>` is
        // `overflow-y-auto` -- a block formatting context, NOT `display:
        // flex`. `flex-1`/`flex-col` on THIS div therefore have no container
        // to size against and are inert; without an explicit height this
        // view falls back to content sizing, which cascades to the
        // `<canvas>`'s own intrinsic (bitmap) aspect ratio -- a stable,
        // self-consistent, and wrong fixed point (measured: 446 of 717
        // available px, `rows=26` where ~42 would fit). `h-full` resolves
        // against `<main>`'s own already-definite height (itself flex-
        // stretched by the shell's `flex h-screen` root) and matches the
        // idiom every sibling tab already uses in this exact parent slot
        // (see `TeamsView`'s root: `flex-1 flex flex-col h-full
        // overflow-hidden`). See `terminal_view_root_carries_an_explicit_height_class`
        // below for the guard.
        <div class="flex flex-1 min-w-0 min-h-0 h-full flex-col" data-terminal-view="">
            <TabBar
                disabled=Signal::derive(move || actions_disabled.get())
                tabs=Signal::derive(move || tabs.get().tabs().to_vec())
                selected=Signal::derive(move || tabs.get().selected_id().map(str::to_string))
                new_tab_label=services.text.new_tab.clone()
                close_tab_label=services.text.close_tab.clone()
                // Selection belongs to the local tab model. A click is accepted
                // only when the existing tab is open, then the view attaches to it.
                //
                // The gesture also asks for the focus: the user picked this
                // terminal, so the caret belongs here -- but only if the
                // terminal is still the visible one when it is ready.
                on_select=Callback::new(move |sid: String| {
                    selection_request.set(Some(sid));
                    focus_request.set(true);
                })
                on_close=Callback::new(move |sid: String| close_request.set(Some(sid)))
                on_new=Callback::new(move |(): ()| {
                    new_request.update(|request| *request += 1);
                    focus_request.set(true);
                })
            />
            <Show when=move || tabs.get().tabs().is_empty()>
                <button type="button" class="px-3 py-2 text-sm" disabled=move || actions_disabled.get() on:click=move |_| {
                    new_request.update(|n| *n += 1);
                    focus_request.set(true);
                }>{new_label.clone()}</button>
            </Show>
            <div class="px-3 py-1 text-sm" role="status" data-terminal-status="">
                {move || if status.get() { String::new() } else { reconnecting.clone() }}
                <Show when=move || !status.get() && error.get().is_some()>
                    <button type="button" on:click=move |_| retry_request.set(true)>{retry_label.clone()}</button>
                </Show>
            </div>
            {move || error.get().map(|e| view! {
                <div class="px-3 py-2 text-sm text-danger" role="alert">{e}</div>
            })}
            {move || input_error.get().map(|e| view! {
                <div class="px-3 py-2 text-sm text-danger" role="alert" data-terminal-input-error="">{e}</div>
            })}
            <Show when=move || selection.get().is_some_and(|s| !s.is_empty())>
                <div class="px-3 py-1 text-sm">
                    <button type="button" class="px-2 py-1" data-terminal-copy="" on:click=move |_| copy_selection()>
                        {copy_label.clone()}
                    </button>
                </div>
            </Show>
            // The terminal's KEYBOARD: a real, focusable text field, hidden
            // from sight but not from the focus ring. A canvas cannot hold
            // text, so without this field an IME has nothing to compose
            // into and the clipboard has nowhere to land.
            <input
                node_ref=input_ref
                type="text"
                autocomplete="off"
                aria-label=input_label
                style="position:absolute;width:1px;height:1px;padding:0;margin:-1px;overflow:hidden;clip:rect(0,0,0,0);white-space:nowrap;border:0"
                on:keydown=on_surface_keydown
                on:input=on_input
                on:compositionstart=on_composition_start
                on:compositionupdate=on_composition_update
                on:compositionend=on_composition_end
                on:paste=on_paste
                on:blur=on_surface_blur
            />
            <canvas
                node_ref=canvas_ref
                tabindex="0"
                class="flex-1 min-h-0 outline-none"
                on:keydown=on_canvas_keydown
                on:paste=on_paste
                on:mousedown=on_canvas_mousedown
                on:mousemove=on_canvas_mousemove
                on:mouseup=on_canvas_mouseup
            />
        </div>
    }
}

/// The one function in this view that reads `canvas_ref` from outside a
/// synchronous render pass -- deferred contexts (`request_animation_frame`,
/// `ResizeObserver`'s callback, a `spawn_local` continuation after an
/// `.await`) all funnel through here rather than reading the `NodeRef`
/// directly. `NodeRef::get`/`get_untracked` unwrap and panic on a disposed
/// ref, and a callback firing a frame late is routinely late enough for the
/// component to have unmounted by then. Mirrors `picker_nav.rs`'s
/// `publish_more_below`: `try_get_untracked().flatten()` is behaviourally
/// identical to `get_untracked()` while the ref is live, and simply absent
/// instead of panicking once it is not.
fn canvas_ctx(
    canvas_ref: NodeRef<leptos::html::Canvas>,
) -> Option<(web_sys::HtmlCanvasElement, CanvasRenderingContext2d)> {
    let el = canvas_ref.try_get_untracked().flatten()?;
    let canvas: web_sys::HtmlCanvasElement = el.unchecked_into();
    let ctx = canvas
        .get_context("2d")
        .ok()??
        .unchecked_into::<CanvasRenderingContext2d>();
    Some((canvas, ctx))
}

/// Measure the canvas's current CSS box, resize its DPR-scaled backing store
/// to match, and report the resulting grid to the server if a session is
/// already known. A no-op (not even the local resize) when the box is not
/// laid out yet (zero-sized) or the canvas is not mounted.
///
/// Resizing the backing store resets the 2D context -- both its transform
/// and its pixels -- so the scale is re-applied and a repaint forced every
/// time this runs, not just the first.
///
/// Deferred-safe: goes through [`canvas_ctx`], so this can be called from
/// `ResizeObserver`'s callback or after an `.await` as freely as from a
/// synchronous effect body.
fn measure_and_report(
    canvas_ref: NodeRef<leptos::html::Canvas>,
    font_family: String,
    font_size_px: f64,
    repaint_tick: RwSignal<u32>,
    cell_metrics: StoredValue<Option<render::CellMetrics>>,
    resize: StoredValue<render::ResizeCoalescer>,
) {
    let Some((canvas, ctx)) = canvas_ctx(canvas_ref) else {
        return;
    };
    let rect = canvas.get_bounding_client_rect();
    let (css_w, css_h) = (rect.width(), rect.height());
    if css_w <= 0.0 || css_h <= 0.0 {
        // Mid-layout (first paint, or hidden behind a `display:none`
        // ancestor). Nothing measurable yet; the next ResizeObserver tick
        // tries again.
        return;
    }
    let dpr = device_pixel_ratio();
    canvas.set_width((css_w * dpr).round().max(1.0) as u32);
    canvas.set_height((css_h * dpr).round().max(1.0) as u32);
    let _ = ctx.scale(dpr, dpr);
    repaint_tick.update(|n| *n = n.wrapping_add(1));

    // ONE measurement: the metrics the pointer maths uses are the metrics
    // the painter used, so a cell can never be two different things.
    let m = render::measure(&ctx, &font_family, font_size_px);
    cell_metrics.try_set_value(Some(m));
    let (rows, cols) = render::viewport_cells(css_w, css_h, &m);
    // The size is NOT sent from here. A burst of geometry changes would
    // become a burst of RPCs; the coalescer keeps the latest one and the
    // loop sends that.
    resize.try_update_value(|r| r.observe(rows, cols));
}

async fn open_terminal_streams(
    services: Rc<TerminalServices>,
    expected_epoch: u64,
    stopped: Rc<Cell<bool>>,
) -> Result<Vec<TerminalEventStream>, String> {
    if stopped.get() {
        return Err("Terminal view disposed".into());
    }
    let connection = services.connection.get_untracked();
    if stopped.get() || !connection.connected || connection.epoch != expected_epoch {
        return Err("Terminal connection changed".into());
    }
    let mut opened = Vec::new();
    for topic in [PTY_SCREEN_TOPIC, PTY_EXIT_TOPIC] {
        match (services.subscribe)(topic.to_string()).await {
            Ok(stream)
                if !stopped.get()
                    && stream.coverage_epoch == expected_epoch
                    && services.connection.get_untracked().connected
                    && services.connection.get_untracked().epoch == expected_epoch =>
            {
                opened.push(stream);
            }
            Ok(stream) => {
                (stream.dispose)();
                for stream in opened {
                    (stream.dispose)();
                }
                return Err(format!(
                    "terminal subscription crossed connection epoch {expected_epoch}"
                ));
            }
            Err(error) => {
                for stream in opened {
                    (stream.dispose)();
                }
                return Err(error);
            }
        }
    }
    if stopped.get() || services.connection.get_untracked().epoch != expected_epoch {
        for stream in opened {
            (stream.dispose)();
        }
        return Err(format!(
            "terminal subscription crossed connection epoch {expected_epoch}"
        ));
    }
    Ok(opened)
}
fn device_pixel_ratio() -> f64 {
    web_sys::window()
        .map(|w| w.device_pixel_ratio())
        .unwrap_or(1.0)
        .max(1.0)
}

/// Is this a CONTROL byte -- something with no text to insert?
///
/// A printable key is inserted by the text field and arrives through its
/// `input` event; a control byte (`Enter`, `Tab`, `Escape`, `Ctrl-C`) has no
/// text at all, so it has to be claimed by the key handler or the field
/// would swallow it.
fn is_control_bytes(bytes: &[u8]) -> bool {
    match bytes.first() {
        Some(byte) => *byte < 0x20 || *byte == 0x7f,
        None => false,
    }
}

/// The cell under the pointer.
///
/// The metrics are the PAINTER's metrics: the painter works in CSS pixels
/// (the device-pixel scale is applied to the context, not to the
/// coordinates), so the pointer's client coordinates are divided by the
/// cell size in the same space. One measurement, one set of cells.
fn cell_at(
    ev: &web_sys::MouseEvent,
    canvas_ref: NodeRef<leptos::html::Canvas>,
    metrics: Option<render::CellMetrics>,
) -> Option<(u16, u16)> {
    let metrics = metrics?;
    if metrics.width <= 0.0 || metrics.height <= 0.0 {
        return None;
    }
    let el = canvas_ref.try_get_untracked().flatten()?;
    let rect = el.get_bounding_client_rect();
    let x = f64::from(ev.client_x()) - rect.left();
    let y = f64::from(ev.client_y()) - rect.top();
    if x < 0.0 || y < 0.0 {
        return None;
    }
    let col = (x / metrics.width).floor();
    let row = (y / metrics.height).floor();
    if col < 0.0 || row < 0.0 {
        return None;
    }
    Some((row as u16, col as u16))
}

/// Write to the clipboard from a user gesture.
///
/// The clipboard is permission-gated and can fail for reasons the user can
/// act on. A failure that is invisible is a clipboard that silently did not
/// update, so the failure is surfaced -- WITHOUT the content, which must
/// not end up in a message or a log.
fn write_clipboard(content: &str, failed: &str, error: &RwSignal<Option<String>>) {
    let Some(window) = web_sys::window() else {
        error.try_update(|slot| *slot = Some(failed.to_string()));
        return;
    };
    let Some(clipboard) = window.navigator().clipboard() else {
        error.try_update(|slot| *slot = Some(failed.to_string()));
        return;
    };
    let promise = clipboard.write_text(content);
    let on_ok = wasm_bindgen::closure::Closure::once_into_js(std::panic::AssertUnwindSafe({
        let error = *error;
        move || {
            error.try_update(|slot| *slot = None);
        }
    }));
    let on_err = wasm_bindgen::closure::Closure::once_into_js(std::panic::AssertUnwindSafe({
        let error = *error;
        let failed = failed.to_string();
        move || {
            error.try_update(|slot| *slot = Some(failed.clone()));
        }
    }));
    let then = js_sys::Reflect::get(&promise, &JsValue::from_str("then"));
    if let Ok(then) = then {
        if let Some(function) = then.dyn_ref::<js_sys::Function>() {
            let _ = function.call2(&promise, &on_ok, &on_err);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        open_terminal_streams, TerminalContent, TerminalContentProps, PTY_EXIT_TOPIC,
        PTY_SCREEN_TOPIC,
    };
    use crate::host::{
        TerminalConnection, TerminalEvent, TerminalEventStream, TerminalServices, TerminalText,
    };
    use futures::FutureExt;
    use leptos::prelude::*;
    use serde_json::json;
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    fn test_text() -> TerminalText {
        TerminalText {
            new_tab: "new".to_string(),
            close_tab: "close".to_string(),
            reconnecting: "Reconnecting…".to_string(),
            retry: "Retry".to_string(),
            read_refusal: "read".to_string(),
            write_refusal: "write".to_string(),
        }
    }

    fn test_stream(epoch: u64, disposed: Rc<Cell<u32>>) -> TerminalEventStream {
        TerminalEventStream::new(
            epoch,
            Rc::new(|| Ok(Vec::<TerminalEvent>::new())),
            Rc::new(move || disposed.set(disposed.get() + 1)),
        )
    }

    #[test]
    fn service_subscription_rejects_mixed_epoch_and_releases_every_stream() {
        let epoch = RwSignal::new(1_u64);
        let disposed = Rc::new(Cell::new(0_u32));
        let calls = Rc::new(Cell::new(0_u32));
        let subscribe = {
            let calls = Rc::clone(&calls);
            let disposed = Rc::clone(&disposed);
            Rc::new(move |_topic: String| {
                let call = calls.get();
                calls.set(call + 1);
                let disposed = Rc::clone(&disposed);
                async move {
                    if call == 1 {
                        epoch.set(2);
                    }
                    Ok(test_stream(epoch.get_untracked(), disposed))
                }
                .boxed_local()
            })
        };
        let services = Rc::new(TerminalServices {
            invoke: Rc::new(|_, _| async { Ok(json!({})) }.boxed_local()),
            subscribe,
            connection: Signal::derive(move || TerminalConnection {
                connected: true,
                epoch: epoch.get(),
            }),
            text: test_text(),
        });
        let result = futures::executor::block_on(open_terminal_streams(
            services,
            1,
            Rc::new(Cell::new(false)),
        ));
        assert!(result.is_err());
        assert_eq!(calls.get(), 2);
        assert_eq!(disposed.get(), 2);
    }

    #[test]
    fn service_subscription_canceled_after_first_lease_rolls_back_without_second_acquire() {
        let stopped = Rc::new(Cell::new(false));
        let disposed = Rc::new(Cell::new(0));
        let calls = Rc::new(Cell::new(0));
        let subscribe = {
            let stopped = Rc::clone(&stopped);
            let disposed = Rc::clone(&disposed);
            let calls = Rc::clone(&calls);
            Rc::new(move |_: String| {
                calls.set(calls.get() + 1);
                let stopped = Rc::clone(&stopped);
                let disposed = Rc::clone(&disposed);
                async move {
                    stopped.set(true);
                    Ok(test_stream(1, disposed))
                }
                .boxed_local()
            })
        };
        let services = Rc::new(TerminalServices {
            invoke: Rc::new(|_, _| async { Ok(json!({})) }.boxed_local()),
            subscribe,
            connection: Signal::derive(|| TerminalConnection {
                connected: true,
                epoch: 1,
            }),
            text: test_text(),
        });
        assert!(futures::executor::block_on(open_terminal_streams(services, 1, stopped)).is_err());
        assert_eq!(calls.get(), 1);
        assert_eq!(disposed.get(), 1);
    }

    #[test]
    fn mounted_attach_drains_queued_gap_before_committing_reply() {
        let _ = any_spawner::Executor::init_futures_executor();
        let owner = Owner::new();
        let queue = Rc::new(RefCell::new(Vec::new()));
        let attach_calls = Rc::new(Cell::new(0));
        let (reply, response) = futures::channel::oneshot::channel();
        let response = Rc::new(RefCell::new(Some(response)));
        let invoke = {
            let attach_calls = Rc::clone(&attach_calls);
            Rc::new(move |method: String, _params| {
                let response = Rc::clone(&response);
                let attach_calls = Rc::clone(&attach_calls);
                async move {
                    match method.as_str() {
                        "pty.list" => Ok(json!({"sessions":[{"session_id":"s","shell":"sh","created_at":0,"closed":false}]})),
                        "pty.attach" => {
                            attach_calls.set(attach_calls.get() + 1);
                            let response = response.borrow_mut().take();
                            if let Some(response) = response {
                                Ok(response.await.unwrap())
                            } else {
                                futures::future::pending().await
                            }
                        }
                        _ => Ok(json!({})),
                    }
                }.boxed_local()
            })
        };
        let subscribe = {
            let queue = Rc::clone(&queue);
            Rc::new(move |topic: String| {
                let queue = Rc::clone(&queue);
                async move {
                    Ok(TerminalEventStream::new(
                        1,
                        Rc::new(move || {
                            Ok(if topic == PTY_SCREEN_TOPIC {
                                queue.borrow_mut().drain(..).collect()
                            } else {
                                Vec::new()
                            })
                        }),
                        Rc::new(|| {}),
                    ))
                }
                .boxed_local()
            })
        };
        owner.with(|| {
            let state = crate::terminal_state(TerminalServices {
                invoke,
                subscribe,
                connection: Signal::derive(|| TerminalConnection {
                    connected: true,
                    epoch: 1,
                }),
                text: test_text(),
            });
            let _mounted = TerminalContent(TerminalContentProps { state }).into_view();
        });
        any_spawner::Executor::poll_local();
        assert_eq!(attach_calls.get(), 1);
        for seq in [12, 14] {
            queue.borrow_mut().push(crate::host::TerminalEvent {
                topic: PTY_SCREEN_TOPIC.to_string(),
                data: json!({"session_id":"s","seq":seq,"rows":24,"cols":80,"patch":{"rows":[{"row":0,"runs":[{"text":"must not commit"}]}]}}),
            });
        }
        reply.send(json!({"seq":11,"rows":24,"cols":80,"patch":{"rows":[{"row":0,"runs":[{"text":"snapshot"}]}]},"scrollback_len":0})).unwrap();
        any_spawner::Executor::poll_local();
        assert_eq!(
            attach_calls.get(),
            1,
            "a queued replay gap must back off, not commit then immediately attach again"
        );
        owner.cleanup();
    }

    #[test]
    fn mounted_old_owner_settlement_relists_without_adopting_or_replaying() {
        let _ = any_spawner::Executor::init_futures_executor();
        for close in [false, true] {
            for failed in [false, true] {
                for timing in ["before-mount", "after-list", "during-list"] {
                    let root = Owner::new();
                    let selection = root.with(crate::TerminalSelection::default);
                    let lists = Rc::new(Cell::new(0));
                    let writes = Rc::new(Cell::new(0));
                    let (first_list_tx, first_list_rx) = futures::channel::oneshot::channel();
                    let first_list_rx = Rc::new(RefCell::new(Some(first_list_rx)));
                    let (replacement_list_tx, replacement_list_rx) =
                        futures::channel::oneshot::channel();
                    let replacement_list_rx = Rc::new(RefCell::new(Some(replacement_list_rx)));
                    let (reconcile_tx, reconcile_rx) = futures::channel::oneshot::channel();
                    let reconcile_rx = Rc::new(RefCell::new(Some(reconcile_rx)));
                    let (settle, settlement) = futures::channel::oneshot::channel();
                    let settlement = Rc::new(RefCell::new(Some(settlement)));
                    let services = root.with(|| TerminalServices {
                        invoke: {
                            let lists = Rc::clone(&lists);
                            let writes = Rc::clone(&writes);
                            Rc::new(move |method: String, _params| {
                                let lists = Rc::clone(&lists);
                                let writes = Rc::clone(&writes);
                                let first_list_rx = Rc::clone(&first_list_rx);
                                let replacement_list_rx = Rc::clone(&replacement_list_rx);
                                let settlement = Rc::clone(&settlement);
                                let reconcile_rx = Rc::clone(&reconcile_rx);
                                async move {
                                    match method.as_str() {
                                        "pty.list" => {
                                            let n = lists.get() + 1;
                                            lists.set(n);
                                            let deferred = if n == 1 {
                                                first_list_rx.borrow_mut().take()
                                            } else if n == 2 && timing == "during-list" {
                                                replacement_list_rx.borrow_mut().take()
                                            } else if n == 3 {
                                                reconcile_rx.borrow_mut().take()
                                            } else {
                                                None
                                            };
                                            if let Some(deferred) = deferred {
                                                return Ok(deferred.await.unwrap());
                                            }
                                            Ok(if close && timing != "before-mount" {
                                                json!({"sessions":[{"session_id":"s","shell":"sh","created_at":0,"closed":false}]})
                                            } else if close {
                                                json!({"sessions":[]})
                                            } else {
                                                json!({"sessions":[{"session_id":"list-authority","shell":"sh","created_at":0,"closed":false}]})
                                            })
                                        }
                                        "pty.spawn" | "pty.close" => {
                                            writes.set(writes.get() + 1);
                                            let deferred = settlement.borrow_mut().take().unwrap();
                                            deferred.await.unwrap()
                                        }
                                        "pty.attach" => Ok(json!({"seq":0,"rows":24,"cols":80,"patch":{"rows":[]},"scrollback_len":0})),
                                        _ => Ok(json!({})),
                                    }
                                }.boxed_local()
                            })
                        },
                        subscribe: Rc::new(|_| async { Ok(test_stream(1, Rc::new(Cell::new(0)))) }.boxed_local()),
                        connection: Signal::derive(|| TerminalConnection { connected: true, epoch: 1 }),
                        text: test_text(),
                    });
                    // This is the default App route boundary: only selection is
                    // parent-owned; every route entry creates a fresh state.
                    let old = root.with(Owner::new);
                    let old_state = old
                        .with(|| crate::terminal_state_with_selection(services.clone(), selection));
                    old.with(|| {
                        let _ = TerminalContent(TerminalContentProps {
                            state: old_state.clone(),
                        })
                        .into_view();
                    });
                    any_spawner::Executor::poll_local();
                    if close {
                        old_state.close_request.set(Some("s".into()));
                    } else {
                        old_state.new_request.set(1);
                    }
                    first_list_tx.send(json!({"sessions":[{"session_id":"s","shell":"sh","created_at":0,"closed":false}]})).unwrap();
                    any_spawner::Executor::poll_local();
                    assert_eq!(writes.get(), 1);
                    old.cleanup();
                    let result = if failed {
                        Err("old-owner-error-sentinel".to_string())
                    } else {
                        Ok(json!({"session_id":"old-payload-sentinel","shell":"sh"}))
                    };
                    let settlement = if timing == "before-mount" {
                        settle.send(result).unwrap();
                        any_spawner::Executor::poll_local();
                        None
                    } else {
                        Some((settle, result))
                    };
                    let replacement = root.with(Owner::new);
                    let state = replacement
                        .with(|| crate::terminal_state_with_selection(services.clone(), selection));
                    replacement.with(|| {
                        let _ = TerminalContent(TerminalContentProps {
                            state: state.clone(),
                        })
                        .into_view();
                    });
                    any_spawner::Executor::poll_local();
                    assert_eq!(lists.get(), 2);
                    if let Some((settle, result)) = settlement {
                        assert!(state.actions_disabled.get_untracked(), "replacement buttons must remain locked while old mutation is pending");
                        for attempt_close in [false, true] {
                            if attempt_close {
                                state.close_request.set(Some("s".into()));
                            } else {
                                state.new_request.set(1);
                            }
                            // Poll both actual dispatch paths, not just the UI flag.
                            state.settlements.wake();
                            any_spawner::Executor::poll_local();
                            assert_eq!(writes.get(), 1, "replacement cannot dispatch a second mutation");
                        }
                        let tabs = state.tabs.get_untracked();
                        settle.send(result).unwrap();
                        any_spawner::Executor::poll_local();
                        assert_eq!(
                            state.tabs.get_untracked(),
                            tabs,
                            "settlement payload must never change the replacement model"
                        );
                        if timing == "during-list" {
                            replacement_list_tx.send(json!({"sessions":[{"session_id":"stale-list-sentinel","shell":"sh","created_at":0,"closed":false}]})).unwrap();
                            any_spawner::Executor::poll_local();
                        }
                        assert_eq!(
                            lists.get(),
                            3,
                            "quiet replacement must re-list exactly once after old settlement"
                        );
                        assert_eq!(
                            state.tabs.get_untracked(),
                            tabs,
                            "neither an old effect nor its invalidated list may be adopted"
                        );
                        reconcile_tx.send(if close {
                            json!({"sessions":[]})
                        } else {
                            json!({"sessions":[{"session_id":"list-authority","shell":"sh","created_at":0,"closed":false}]})
                        }).unwrap();
                        any_spawner::Executor::poll_local();
                        assert_eq!(lists.get(), 3, "settlement is consumed once");
                    }
                    assert!(
                        !state.actions_disabled.get_untracked(),
                        "old errors must not disable the replacement"
                    );
                    assert_eq!(writes.get(), 1, "settlement must never replay the gesture");
                    assert_eq!(
                        state.tabs.get_untracked().selected_id(),
                        if close { None } else { Some("list-authority") }
                    );
                    replacement.cleanup();
                    root.cleanup();
                }
            }
        }
    }

    #[test]
    fn mounted_terminal_content_cleanup_disposes_host_streams() {
        let disposed = Rc::new(Cell::new(0_u32));
        let calls = Rc::new(RefCell::new(Vec::<String>::new()));
        let invoke = {
            let calls = Rc::clone(&calls);
            Rc::new(move |method: String, _params| {
                calls.borrow_mut().push(method.clone());
                async move {
                    if method == "pty.list" {
                        Err("list unavailable".to_string())
                    } else {
                        Ok(json!({}))
                    }
                }
                .boxed_local()
            })
        };
        let subscribe_calls = Rc::new(RefCell::new(Vec::<String>::new()));
        let subscribe = {
            let disposed = Rc::clone(&disposed);
            let subscribe_calls = Rc::clone(&subscribe_calls);
            Rc::new(move |topic: String| {
                subscribe_calls.borrow_mut().push(topic);
                let disposed = Rc::clone(&disposed);
                async move { Ok(test_stream(1, disposed)) }.boxed_local()
            })
        };
        let services = TerminalServices {
            invoke,
            subscribe,
            connection: Signal::derive(|| TerminalConnection {
                connected: true,
                epoch: 1,
            }),
            text: test_text(),
        };
        let _ = any_spawner::Executor::init_futures_executor();
        let owner = Owner::new();
        owner.with(|| {
            let state = crate::terminal_state(services);
            let _mounted = TerminalContent(TerminalContentProps { state }).into_view();
        });
        any_spawner::Executor::poll_local();
        assert_eq!(
            subscribe_calls.borrow().as_slice(),
            [PTY_SCREEN_TOPIC.to_string(), PTY_EXIT_TOPIC.to_string()]
        );
        assert_eq!(disposed.get(), 0);
        owner.cleanup();
        assert_eq!(disposed.get(), 2);
        assert!(calls.borrow().iter().any(|method| method == "pty.list"));
        assert!(!calls.borrow().iter().any(|method| method == "pty.spawn"));
    }

    /// once: `flex-1`/`flex-col` on the root element are inert unless its
    /// DOM parent is itself `display: flex`, and it is not. `app.rs` mounts
    /// this view straight under `<main>` (`overflow-y-auto`, a block
    /// formatting context, not a flex container) through a
    /// `style:display="contents"` wrapper that drops out of the box tree
    /// entirely. Without an explicit height class, the view falls back to
    /// content sizing, which cascades down to the `<canvas>`'s own
    /// intrinsic (bitmap) aspect ratio -- a stable, self-consistent, and
    /// WRONG fixed point (measured live: 446 of 717 available px, `rows=26`
    /// where roughly 42 would fit).
    ///
    /// No runtime assertion can catch this -- "the right pixel count"
    /// changes with the window -- so the rule is pinned against the source
    /// instead: the root element must carry an explicit, non-flex height
    /// class. `h-full` is the idiom every sibling tab already uses in this
    /// exact parent slot (e.g. `TeamsView`'s root:
    /// `flex-1 flex flex-col h-full overflow-hidden`).
    #[test]
    fn terminal_view_root_carries_an_explicit_height_class() {
        let src = include_str!("view.rs").replace('\r', "");
        // The LAST occurrence, not the first: this file's own doc comment a
        // few lines above the tag also mentions this literal string.
        let anchor = src
            .rfind("data-terminal-view=\"\"")
            .expect("the view's root element carries this anchor");
        let tag_start = src[..anchor]
            .rfind("<div")
            .expect("the anchor sits inside a <div ...> opening tag");
        let tag_end = tag_start
            + src[tag_start..]
                .find('>')
                .expect("the opening tag is eventually closed");
        let opening_tag = &src[tag_start..=tag_end];
        assert!(
            opening_tag.contains("h-full"),
            "must carry an explicit height class (`h-full`); `flex-1` alone \
             does not size it -- its parent `<main>` is not `display: flex` \
             -- see this test's doc for the full story. \
             Root tag was: {opening_tag:?}"
        );
    }
}

fn read_error(text: &TerminalText, error: &str) -> String {
    if error.to_ascii_lowercase().contains("admin") {
        text.read_refusal.clone()
    } else {
        error.to_string()
    }
}

fn write_error(text: &TerminalText, error: &str) -> String {
    if error.to_ascii_lowercase().contains("admin") {
        text.write_refusal.clone()
    } else {
        error.to_string()
    }
}

#[component]
pub fn TerminalSidebar(state: TerminalState) -> impl IntoView {
    let tabs = state.tabs;
    let selection_request = state.selection_request;
    let new_request = state.new_request;
    let close_request = state.close_request;
    let text = state.services.text;
    let actions_disabled = state.actions_disabled;
    view! {
        <aside class="flex flex-col min-w-0" data-terminal-sidebar="">
            <TabBar
                disabled=Signal::derive(move || actions_disabled.get())
                tabs=Signal::derive(move || tabs.get().tabs().to_vec())
                selected=Signal::derive(move || tabs.get().selected_id().map(str::to_string))
                new_tab_label=text.new_tab
                close_tab_label=text.close_tab
                on_select=Callback::new(move |sid: String| {
                    selection_request.set(Some(sid));
                })
                on_close=Callback::new(move |sid: String| close_request.set(Some(sid)))
                on_new=Callback::new(move |(): ()| new_request.update(|request| *request += 1))
            />
        </aside>
    }
}
