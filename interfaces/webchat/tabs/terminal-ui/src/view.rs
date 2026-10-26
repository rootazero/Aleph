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

use aleph_protocol::pty::{
    PtyAttachResponse, PtyScreenFrame, PtySpawnResponse, PTY_EXIT_TOPIC, PTY_LIST_METHOD,
    PTY_SCREEN_TOPIC,
};
use leptos::prelude::*;
use leptos::task::spawn_local;
#[cfg(not(test))]
use wasm_bindgen::closure::Closure;
use wasm_bindgen::JsCast;
use web_sys::CanvasRenderingContext2d;

use crate::host::{TerminalEventStream, TerminalServices, TerminalState, TerminalText};
use crate::session::{ApplyOutcome, AttachOutcome, ClientScreen};
use crate::tabs::TabBar;
use crate::{keymap, render, session};
use futures::future::LocalBoxFuture;
use futures::FutureExt;
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

// single-threaded browser runtime. This marker keeps local WASM resources in
// that hook; no instance is ever sent to another thread.
struct LocalCleanup<T>(T);
unsafe impl<T> Send for LocalCleanup<T> {}
unsafe impl<T> Sync for LocalCleanup<T> {}

impl LocalCleanup<(Rc<Cell<bool>>, Rc<RefCell<Vec<TerminalEventStream>>>)> {
    fn dispose(self) {
        let (stop, streams) = self.0;
        stop.set(true);
        for stream in streams.borrow_mut().drain(..) {
            (stream.dispose)();
        }
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
    // `StoredValue` rather than a signal: the screen is mutated from an event
    // callback on every frame, and a signal write per frame would re-run every
    // subscriber 60 times a second for a canvas that repaints itself anyway.
    let screen = StoredValue::new(None::<ClientScreen>);
    let session_id = StoredValue::new(None::<String>);
    let attach_generation = RwSignal::new(0_u64);
    let first_load = StoredValue::new(true);
    // A signal, unlike `screen`: the tab strip is a view of it, so a change
    // here has to re-render. It changes on a session list, an exit and a
    // title — human-paced events, not the 60/s frame rate that made `screen`
    // a `StoredValue`.
    let repaint_tick = RwSignal::new(0_u32);
    let error = RwSignal::new(None::<String>);
    // Server-configured canvas font (`[policies.terminal]`). Seeded with the
    // same last-resort literal `render::apply_font` falls back to -- NOT a
    // Nerd Font stack of our own (see `FALLBACK_FONT_FAMILY`'s doc for why
    // the Panel must not hold that opinion). A config fetch that has not
    // resolved yet, one that fails, and one that is refused all render
    // identically -- plain `monospace` -- which is correct: the terminal
    // has to draw something before the RPC round trip finishes regardless
    // of why, and the real default (with Nerd Font names) lives exactly
    // once, in `alephcore`'s `TerminalConfig`, and reaches here as the
    // server's EFFECTIVE config, defaults already applied.
    let font_family = StoredValue::new(render::FALLBACK_FONT_FAMILY.to_string());
    let font_size_px = StoredValue::new(render::FALLBACK_FONT_SIZE_PX);

    // Re-attach: the one recovery path. A gap means the bounded broadcast
    // dropped a frame for this subscriber; the screen is on the server, so the
    // fix is to ask for it again rather than to guess.
    let resync_services = Rc::clone(&services);
    let resync: Rc<dyn Fn(String, u64)> = Rc::new(move |sid: String, generation: u64| {
        let services = Rc::clone(&resync_services);
        spawn_local(async move {
            if first_load.try_get_value().is_none() {
                return;
            }
            let (Some(current_generation), Some(current_session)) = (
                attach_generation.try_get_untracked(),
                session_id.try_get_value(),
            ) else {
                return;
            };
            if current_generation != generation || current_session.as_deref() != Some(sid.as_str())
            {
                return;
            }
            screen.update_value(|s| {
                if let Some(s) = s {
                    s.begin_attach();
                }
            });
            let result = (services.invoke)(
                "pty.attach".to_string(),
                serde_json::json!({ "session_id": sid.clone() }),
            )
            .await;
            if first_load.try_get_value().is_none() {
                return;
            }
            let (Some(current_generation), Some(current_session)) = (
                attach_generation.try_get_untracked(),
                session_id.try_get_value(),
            ) else {
                return;
            };
            if current_generation != generation || current_session.as_deref() != Some(sid.as_str())
            {
                return;
            }
            match result {
                Ok(v) => match serde_json::from_value::<PtyAttachResponse>(v) {
                    Ok(resp) => {
                        // `finish_attach` reports whether the replay of
                        // frames buffered during this RPC hit a hole. It
                        // stops AT the hole rather than skipping it, so the
                        // screen never claims to be more current than it is.
                        let outcome =
                            screen.try_update_value(|s| s.as_mut().map(|s| s.finish_attach(resp)));
                        repaint_tick.update(|n| *n = n.wrapping_add(1));
                        // Deliberately NOT a re-attach from here. `resync` is
                        // a closure and cannot call itself, and an immediate
                        // retry is the shape that loops when the bus is
                        // dropping faster than we attach. `finish_attach`
                        // leaves `seq` at the last frame it actually applied,
                        // so the next live frame gaps on its own and the frame
                        // handler's existing `Gap` arm re-attaches -- one path,
                        // already tested.
                        //
                        // The honest cost: on a terminal that goes quiet right
                        // after the hole, no live frame arrives, so the rows
                        // the missing frame would have touched stay wrong until
                        // it speaks again. Worth knowing, not worth a retry
                        // loop; if it ever matters the fix is a one-shot
                        // re-attach guarded by a flag, not recursion.
                        if let Some(Some(AttachOutcome::Gap { expected, got })) = outcome {
                            leptos::logging::log!(
                                "pty attach replay hit a hole: expected {expected}, got {got}"
                            );
                        }
                    }
                    Err(e) => {
                        screen.update_value(|s| {
                            if let Some(s) = s {
                                s.abort_attach();
                            }
                        });
                        error.set(Some(read_error(&services.text, &e.to_string())));
                    }
                },
                // An Err is never read as an empty screen: the server said
                // something, and what it said is not "the terminal is idle".
                // `settings_load_error` replaces only the generic
                // admin-privilege refusal (`pty.*` is fully admin-gated);
                // every other message -- including the policy gate's own
                // "[policies.terminal] enabled = false" text -- passes
                // through verbatim, which is what names the remedy.
                Err(e) => {
                    screen.update_value(|s| {
                        if let Some(s) = s {
                            s.abort_attach();
                        }
                    });
                    error.set(Some(read_error(&services.text, &e)));
                }
            }
        });
    });

    // Point this view at one session: seat a fresh `ClientScreen` for it,
    // report our viewport, and attach.
    //
    // A no-op when it is already the showing session unless the caller is
    // recovering a connection, which must refresh the server-side attach
    // state even when the selected id did not change. That makes it safe to
    // call from every place that can settle on a selection (mount, a tab
    // click, an exit falling to a neighbour, or a reconnect) without any of
    // them having to know what the others did.
    //
    // The 24x80 is a placeholder that is never observed: `finish_attach`
    // re-seats both the geometry and `seq` from the attach response, and
    // `measure_and_report` corrects the server's idea of our viewport in the
    // same breath. Setting the id BEFORE the RPC matters — the frame handler
    // DROPS frames whose session it cannot name, and a dropped frame is not a
    // buffered one.
    // `try_` on every read, and all three up front so nothing is written
    // before the bail: this closure runs from two `spawn_local` continuations
    // on the far side of an `.await` (`pty.spawn`, `pty.list`) and from the
    // exit event's callback. `StoredValue::get_value` unwraps, so a disposed
    // owner panics and the whole page goes to the recovery overlay
    // (`crate::disposed_reads`). The window is ordinary — the user leaves this
    // view while the RPC is in flight. The scope that owns these is gone, so
    // there is no view left to point at a session.
    let attach_services = Rc::clone(&services);
    let attach_resync = Rc::clone(&resync);
    let attach_to: Rc<dyn Fn(String, bool)> = Rc::new(move |sid: String, force: bool| {
        let services = Rc::clone(&attach_services);
        let (Some(current), Some(family), Some(size)) = (
            session_id.try_get_value(),
            font_family.try_get_value(),
            font_size_px.try_get_value(),
        ) else {
            return;
        };
        if !force && current.as_deref() == Some(sid.as_str()) {
            return;
        }
        attach_generation.update(|generation| *generation = generation.wrapping_add(1));
        let generation = attach_generation.get_untracked();
        screen.set_value(Some(ClientScreen::new(24, 80, 0, sid.clone())));
        session_id.set_value(Some(sid.clone()));
        measure_and_report(
            canvas_ref,
            Rc::clone(&services),
            Some(sid.clone()),
            family,
            size,
            repaint_tick,
            first_load,
        );
        attach_resync(sid, generation);
    });

    // Open a new shell.
    //
    // The spawn response is adopted straight into the tab model rather than
    // re-listing to discover what we just created: that second round trip can
    // itself fail, and a running shell with no tab pointing at it is exactly
    // the shape D2 was.
    let spawn_services = Rc::clone(&services);
    let spawn_attach = Rc::clone(&attach_to);
    let spawn_new: Rc<dyn Fn()> = Rc::new(move || {
        let services = Rc::clone(&spawn_services);
        let spawn_attach = Rc::clone(&spawn_attach);
        spawn_local(async move {
            let result = (services.invoke)(
                "pty.spawn".to_string(),
                serde_json::json!({ "rows": 24, "cols": 80 }),
            )
            .await;
            if first_load.try_get_value().is_none() {
                return;
            }
            match result {
                Ok(v) => match serde_json::from_value::<PtySpawnResponse>(v) {
                    Ok(resp) => {
                        tabs.update(|m| m.adopt_spawned(&resp.session_id, &resp.shell));
                        spawn_attach(resp.session_id, false);
                    }
                    Err(e) => error.set(Some(write_error(&services.text, &e.to_string()))),
                },
                // Covers both refusals that have a way out: the gate
                // ([policies.terminal] enabled = false) and the cwd jail. The
                // server's message names the remedy; show it verbatim.
                // `settings_write_error` replaces only the generic
                // admin-privilege refusal (`pty.*` is fully admin-gated) --
                // every other message, gate and jail included, passes through
                // unframed.
                Err(e) => error.set(Some(write_error(&services.text, &e))),
            }
        });
    });

    // Ask the server what exists and rebuild the tab strip from it.
    //
    // `allow_spawn` is only true on the first pass. A later refresh that finds
    // nothing open means the user closed everything, and re-opening a shell
    // they just closed would be the page arguing with them.
    let refresh_services = Rc::clone(&services);
    let refresh_attach = Rc::clone(&attach_to);
    let refresh_spawn = Rc::clone(&spawn_new);
    let refresh_tabs: Rc<dyn Fn(bool, bool)> =
        Rc::new(move |allow_spawn: bool, force_attach: bool| {
            let services = Rc::clone(&refresh_services);
            let refresh_attach = Rc::clone(&refresh_attach);
            let refresh_spawn = Rc::clone(&refresh_spawn);
            spawn_local(async move {
                // "Nothing to adopt" and "I could not get an answer" are different
                // facts, and only the first may create a shell. The classification
                // lives in `session::resolve_session_list` (pure, unit-tested
                // there); the "may I spawn" half is `session::should_spawn`, which
                // a `Fail` cannot even be handed because it carries no rows.
                let list_result =
                    (services.invoke)(PTY_LIST_METHOD.to_string(), serde_json::json!({})).await;
                if first_load.try_get_value().is_none() {
                    return;
                }
                let sessions = match session::resolve_session_list(list_result) {
                    session::ListOutcome::Read(sessions) => sessions,
                    session::ListOutcome::Fail(msg) => {
                        error.set(Some(read_error(&services.text, &msg)));
                        return;
                    }
                };

                tabs.update(|m| m.reconcile(&sessions));

                match tabs
                    .try_with_untracked(|m| m.selected_id().map(str::to_string))
                    .flatten()
                {
                    Some(sid) => refresh_attach(sid, force_attach),
                    None if allow_spawn && session::should_spawn(&sessions) => refresh_spawn(),
                    None => {}
                }
            });
        });

    // Ask the server to end a session.
    //
    // Deliberately does NOT remove the tab here. `pty.exit` is what says the
    // shell is gone; a tab removed optimistically on a close that was refused
    // would hide a shell that is still running (判据 §8 — the request is not
    // the outcome).
    let close_services = Rc::clone(&services);
    let close_tab = move |sid: String| {
        let services = Rc::clone(&close_services);
        spawn_local(async move {
            let result = (services.invoke)(
                "pty.close".to_string(),
                serde_json::json!({ "session_id": sid }),
            )
            .await;
            if first_load.try_get_value().is_none() {
                return;
            }
            if let Err(e) = result {
                error.set(Some(write_error(&services.text, &e)));
            }
        });
    };

    // Subscribe through the host-owned exact-topic streams. Each stream is
    // drained locally so the reusable view never reaches into DashboardState.
    let mut last_connection = services.connection.get_untracked();
    let stop_events = Rc::new(Cell::new(false));
    let event_streams = Rc::new(RefCell::new(Vec::<TerminalEventStream>::new()));
    let stop_for_cleanup = Rc::clone(&stop_events);
    let streams_for_cleanup = Rc::clone(&event_streams);
    let cleanup_state = LocalCleanup((stop_for_cleanup, streams_for_cleanup));
    on_cleanup(move || cleanup_state.dispose());

    let subscribe_all: Rc<
        dyn Fn() -> LocalBoxFuture<'static, Result<Vec<TerminalEventStream>, String>>,
    > = {
        let services = Rc::clone(&services);
        Rc::new(move || {
            let services = Rc::clone(&services);
            async move { open_terminal_streams(services).await }.boxed_local()
        })
    };

    let services_for_events = Rc::clone(&services);
    let streams_for_events = Rc::clone(&event_streams);
    let stop_for_events = Rc::clone(&stop_events);
    {
        let services = Rc::clone(&services_for_events);
        let streams = Rc::clone(&streams_for_events);
        let stop = Rc::clone(&stop_for_events);
        let subscribe_all = Rc::clone(&subscribe_all);
        spawn_local(async move {
            let opened = match subscribe_all().await {
                Ok(opened) => opened,
                Err(subscription_error) => {
                    if first_load.try_get_value().is_some() {
                        error.set(Some(read_error(&services.text, &subscription_error)));
                    }
                    return;
                }
            };
            if first_load.try_get_value().is_none() || stop.get() {
                for stream in opened {
                    (stream.dispose)();
                }
                return;
            }
            streams.borrow_mut().extend(opened);
            let Some(allow_spawn) = first_load.try_get_value() else {
                return;
            };
            refresh_tabs(allow_spawn, !allow_spawn);
            first_load.try_update_value(|first| *first = false);

            let mut seen_new_request = 0_u64;
            while !stop.get() {
                let connection = services.connection.get_untracked();
                if connection.connected && connection.epoch != last_connection.epoch {
                    last_connection = connection;
                    let old_streams = streams.borrow_mut().drain(..).collect::<Vec<_>>();
                    for stream in old_streams {
                        (stream.dispose)();
                    }
                    match subscribe_all().await {
                        Ok(opened) if first_load.try_get_value().is_some() && !stop.get() => {
                            streams.borrow_mut().extend(opened);
                            refresh_tabs(false, true);
                        }
                        Ok(opened) => {
                            for stream in opened {
                                (stream.dispose)();
                            }
                            return;
                        }
                        Err(subscription_error) => {
                            if first_load.try_get_value().is_some() {
                                error.set(Some(read_error(&services.text, &subscription_error)));
                            }
                            return;
                        }
                    }
                } else if !connection.connected {
                    last_connection = connection;
                }
                if let Some(session_id) = selection_request.try_get_untracked().flatten() {
                    selection_request.set(None);
                    attach_to(session_id, false);
                }
                let requested_new = new_request.get_untracked();
                if requested_new != seen_new_request {
                    seen_new_request = requested_new;
                    spawn_new();
                }
                if let Some(session_id) = close_request.try_get_untracked().flatten() {
                    close_request.set(None);
                    close_tab(session_id);
                }
                let events = streams
                    .borrow()
                    .iter()
                    .flat_map(|stream| (stream.drain)().unwrap_or_default())
                    .collect::<Vec<_>>();
                for event in events {
                    if event.topic == PTY_EXIT_TOPIC {
                        let Some(sid) = event
                            .data
                            .get("session_id")
                            .and_then(serde_json::Value::as_str)
                            .map(str::to_string)
                        else {
                            continue;
                        };
                        tabs.update(|model| model.on_exit(&sid));
                        if let Some(next) = tabs
                            .try_with_untracked(|model| model.selected_id().map(str::to_string))
                            .flatten()
                        {
                            attach_to(next, false);
                        }
                        refresh_tabs(false, false);
                        continue;
                    }
                    let Ok(frame) = serde_json::from_value::<PtyScreenFrame>(event.data) else {
                        continue;
                    };
                    let outcome = screen.try_update_value(|current| {
                        current
                            .as_mut()
                            .map_or(ApplyOutcome::Buffered, |current| current.apply(frame))
                    });
                    match outcome {
                        Some(ApplyOutcome::Applied) => {
                            repaint_tick.update(|tick| *tick = tick.wrapping_add(1));
                            if let (Some(sid), Some(title)) = (
                                session_id.try_get_value().flatten(),
                                screen.with_value(|current| {
                                    current
                                        .as_ref()
                                        .and_then(|screen| screen.title().map(str::to_string))
                                }),
                            ) {
                                if tabs
                                    .try_with_untracked(|model| {
                                        model.osc_title_differs(&sid, &title)
                                    })
                                    .unwrap_or(false)
                                {
                                    tabs.update(|model| model.on_title(&sid, &title));
                                }
                            }
                        }
                        Some(ApplyOutcome::Gap { .. }) => {
                            if let (Some(mine), Some(generation)) = (
                                session_id.try_get_value().flatten(),
                                attach_generation.try_get_untracked(),
                            ) {
                                resync(mine, generation);
                            }
                        }
                        _ => {}
                    }
                }
                terminal_event_tick().await;
            }
        });
    }

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
            if first_load.try_get_value().is_none() {
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
            let (Some(sid), Some(family), Some(size)) = (
                session_id.try_get_value(),
                font_family.try_get_value(),
                font_size_px.try_get_value(),
            ) else {
                return;
            };
            measure_and_report(
                canvas_ref,
                Rc::clone(&services),
                sid,
                family,
                size,
                repaint_tick,
                first_load,
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
        request_animation_frame(move || {
            let Some((_canvas, ctx)) = canvas_ctx(canvas_ref) else {
                return;
            };
            let m = render::measure(&ctx, &family, size_px);
            if !m.is_usable() {
                return;
            }
            screen.with_value(|s| {
                if let Some(s) = s {
                    render::paint(&ctx, s, m, &render::Theme::dark());
                }
            });
        });
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
    let resize_services = Rc::clone(&services);
    Effect::new(move |_| {
        measure_and_report(
            canvas_ref,
            Rc::clone(&resize_services),
            session_id.get_value(),
            font_family.get_value(),
            font_size_px.get_value(),
            repaint_tick,
            first_load,
        );

        #[cfg(not(test))]
        {
            let resize_services_for_callback = Rc::clone(&resize_services);
            let cb: Closure<dyn FnMut(js_sys::Array)> =
                Closure::new(move |_entries: js_sys::Array| {
                    measure_and_report(
                        canvas_ref,
                        Rc::clone(&resize_services_for_callback),
                        session_id.get_value(),
                        font_family.get_value(),
                        font_size_px.get_value(),
                        repaint_tick,
                        first_load,
                    );
                });
            if let Some((canvas, _ctx)) = canvas_ctx(canvas_ref) {
                if let Ok(observer) = web_sys::ResizeObserver::new(cb.as_ref().unchecked_ref()) {
                    observer.observe(&canvas);
                    on_cleanup(move || observer.disconnect());
                }
            }
            cb.forget();
        }
    });

    // Bytes -> the PTY, base64-encoded (raw bytes, not JS `btoa`: control
    // bytes and multi-byte UTF-8 -- Ctrl-C is 0x03, CJK input is more than
    // one byte per character -- round-trip corrupted through `btoa`'s Latin-1
    // assumption). Uses a small local encoder to avoid coupling terminal UI to voice code.
    // adding a second one.
    //
    // One function for both keystrokes and pastes: they are the same wire
    // call, and a second copy of it is where the two would drift.
    let send_services = Rc::clone(&services);
    let send_bytes: Rc<dyn Fn(Vec<u8>)> = Rc::new(move |bytes: Vec<u8>| {
        let Some(sid) = session_id.get_value() else {
            return;
        };
        let data = encode_base64(&bytes);
        let services = Rc::clone(&send_services);
        spawn_local(async move {
            let _ = (services.invoke)(
                "pty.input".to_string(),
                serde_json::json!({ "session_id": sid, "data": data, "base64": true }),
            )
            .await;
        });
    });

    // A key the keymap does not claim is left alone entirely, not even
    // `prevent_default()`, so any browser/OS shortcut we do not mean to
    // swallow keeps working. That now includes the PASTE chords: the meta
    // bail that used to live here has moved into `keymap::encode_key`, which
    // takes `meta` and answers `Browser` for Cmd-V and Ctrl-Shift-V — one
    // decision in one place rather than half a rule on each side (判据 §6).
    let key_send_bytes = Rc::clone(&send_bytes);
    let on_keydown = move |ev: web_sys::KeyboardEvent| match keymap::encode_key(
        &ev.key(),
        ev.ctrl_key(),
        ev.alt_key(),
        ev.shift_key(),
        ev.meta_key(),
    ) {
        keymap::KeyAction::Bytes(bytes) => {
            ev.prevent_default();
            key_send_bytes(bytes);
        }
        keymap::KeyAction::Browser => {}
    };

    // Paste. The clipboard's TEXT is only readable from a `paste` event — a
    // `keydown` handler cannot reach it at all — which is the whole reason
    // the keymap hands the paste chords back to the browser instead of
    // encoding them.
    //
    // Wrapped in `ESC[200~ ... ESC[201~` only when the program has asked for
    // bracketed paste. `ClientScreen::bracketed_paste()` answers `None` until
    // the server has said, and `encode_paste` treats unknown as "do not
    // wrap": the escape sequences showing up literally in someone's command
    // line is a visible corruption, while not sending them only gives up a
    // hint (spec §5).
    let paste_send_bytes = Rc::clone(&send_bytes);
    let on_paste = move |ev: web_sys::ClipboardEvent| {
        let Some(text) = ev
            .clipboard_data()
            .and_then(|d| d.get_data("text").ok())
            .filter(|t| !t.is_empty())
        else {
            return;
        };
        // Read synchronously, inside the event handler, and passed to
        // `send_bytes` as a value: the mode belongs to the screen as it is
        // NOW, not as it may be by the time the `pty.input` RPC resolves.
        let bracketed = screen.with_value(|s| s.as_ref().and_then(ClientScreen::bracketed_paste));
        ev.prevent_default();
        paste_send_bytes(session::encode_paste(&text, bracketed));
    };

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
                tabs=Signal::derive(move || tabs.get().tabs().to_vec())
                selected=Signal::derive(move || tabs.get().selected_id().map(str::to_string))
                new_tab_label=services.text.new_tab.clone()
                close_tab_label=services.text.close_tab.clone()
                // Selection belongs to the local tab model. A click is accepted
                // only when the existing tab is open, then the view attaches to it.
                on_select=Callback::new(move |sid: String| {
                    let accepted = tabs.try_update(|m| m.select(&sid)).unwrap_or(false);
                    if accepted {
                        selection_request.set(Some(sid));
                    }
                })
                on_close=Callback::new(move |sid: String| close_request.set(Some(sid)))
                on_new=Callback::new(move |(): ()| new_request.update(|request| *request += 1))
            />
            {move || error.get().map(|e| view! {
                <div class="px-3 py-2 text-sm text-danger" role="alert">{e}</div>
            })}
            <canvas
                node_ref=canvas_ref
                tabindex="0"
                class="flex-1 min-h-0 outline-none"
                on:keydown=on_keydown
                on:paste=on_paste
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
    services: Rc<TerminalServices>,
    session_id: Option<String>,
    font_family: String,
    font_size_px: f64,
    repaint_tick: RwSignal<u32>,
    owner: StoredValue<bool>,
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

    let m = render::measure(&ctx, &font_family, font_size_px);
    let (rows, cols) = render::viewport_cells(css_w, css_h, &m);
    let Some(sid) = session_id else { return };
    spawn_local(async move {
        let _ = (services.invoke)(
            "pty.resize".to_string(),
            serde_json::json!({ "session_id": sid, "rows": rows, "cols": cols }),
        )
        .await;
        if owner.try_get_value().is_some() {
            repaint_tick.update(|n| *n = n.wrapping_add(1));
        }
    });
}

async fn open_terminal_streams(
    services: Rc<TerminalServices>,
) -> Result<Vec<TerminalEventStream>, String> {
    let expected_epoch = services.connection.get_untracked().epoch;
    let mut opened = Vec::new();
    for topic in [PTY_SCREEN_TOPIC, PTY_EXIT_TOPIC] {
        match (services.subscribe)(topic.to_string()).await {
            Ok(stream)
                if stream.coverage_epoch == expected_epoch
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
    if services.connection.get_untracked().epoch != expected_epoch {
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
        let result = futures::executor::block_on(open_terminal_streams(services));
        assert!(result.is_err());
        assert_eq!(calls.get(), 2);
        assert_eq!(disposed.get(), 2);
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
        assert_eq!(calls.borrow().first().map(String::as_str), Some("pty.list"));
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
    #[test]
    fn pty_input_base64_preserves_control_and_utf8_bytes() {
        assert_eq!(super::encode_base64(&[0x03, 0x00, 0xff]), "AwD/");
        assert_eq!(super::encode_base64("终端".as_bytes()), "57uI56uv");
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

fn encode_base64(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut output = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let first = chunk[0];
        let second = chunk.get(1).copied().unwrap_or(0);
        let third = chunk.get(2).copied().unwrap_or(0);
        output.push(TABLE[(first >> 2) as usize] as char);
        output.push(TABLE[((first << 4 | second >> 4) & 0x3f) as usize] as char);
        output.push(if chunk.len() > 1 {
            TABLE[((second << 2 | third >> 6) & 0x3f) as usize] as char
        } else {
            '='
        });
        output.push(if chunk.len() > 2 {
            TABLE[(third & 0x3f) as usize] as char
        } else {
            '='
        });
    }
    output
}

#[component]
pub fn TerminalSidebar(state: TerminalState) -> impl IntoView {
    let tabs = state.tabs;
    let selection_request = state.selection_request;
    let new_request = state.new_request;
    let close_request = state.close_request;
    let text = state.services.text;
    view! {
        <aside class="flex flex-col min-w-0" data-terminal-sidebar="">
            <TabBar
                tabs=Signal::derive(move || tabs.get().tabs().to_vec())
                selected=Signal::derive(move || tabs.get().selected_id().map(str::to_string))
                new_tab_label=text.new_tab
                close_tab_label=text.close_tab
                on_select=Callback::new(move |sid: String| {
                    if tabs.try_update(|model| model.select(&sid)).unwrap_or(false) {
                        selection_request.set(Some(sid));
                    }
                })
                on_close=Callback::new(move |sid: String| close_request.set(Some(sid)))
                on_new=Callback::new(move |(): ()| new_request.update(|request| *request += 1))
            />
        </aside>
    }
}
