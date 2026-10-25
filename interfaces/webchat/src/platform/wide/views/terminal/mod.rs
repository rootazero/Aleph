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

mod keymap;
mod render;
mod session;
mod tabs;

use aleph_protocol::pty::{
    PtyAttachResponse, PtyScreenFrame, PtySpawnResponse, PTY_EXIT_TOPIC, PTY_LIST_METHOD,
    PTY_SCREEN_TOPIC,
};
use leptos::prelude::*;
use leptos::task::spawn_local;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::JsCast;
use web_sys::CanvasRenderingContext2d;

use crate::components::admin_refusal;
use crate::context::DashboardState;
use session::{ApplyOutcome, AttachOutcome, ClientScreen};
use tabs::{TabBar, TabModel};

#[component]
pub fn TerminalView() -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let i18n = crate::i18n::use_i18n();
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
    let tabs = RwSignal::new(TabModel::default());
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
    let resync = move |sid: String, generation: u64| {
        let state = state;
        spawn_local(async move {
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
            let result = state
                .rpc_call(
                    "pty.attach",
                    serde_json::json!({ "session_id": sid.clone() }),
                )
                .await;
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
                        error.set(Some(admin_refusal::settings_load_error(
                            i18n,
                            &e.to_string(),
                            |e| format!("attach decode failed: {e}"),
                        )));
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
                    error.set(Some(admin_refusal::settings_load_error(i18n, &e, |e| {
                        e.to_string()
                    })));
                }
            }
        });
    };

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
    let attach_to = move |sid: String, force: bool| {
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
            state,
            Some(sid.clone()),
            family,
            size,
            repaint_tick,
        );
        resync(sid, generation);
    };

    // Open a new shell.
    //
    // The spawn response is adopted straight into the tab model rather than
    // re-listing to discover what we just created: that second round trip can
    // itself fail, and a running shell with no tab pointing at it is exactly
    // the shape D2 was.
    let spawn_new = move || {
        spawn_local(async move {
            match state
                .rpc_call("pty.spawn", serde_json::json!({ "rows": 24, "cols": 80 }))
                .await
            {
                Ok(v) => match serde_json::from_value::<PtySpawnResponse>(v) {
                    Ok(resp) => {
                        tabs.update(|m| m.adopt_spawned(&resp.session_id, &resp.shell));
                        attach_to(resp.session_id, false);
                    }
                    Err(e) => error.set(Some(admin_refusal::settings_write_error(
                        i18n,
                        &e.to_string(),
                        |e| format!("spawn decode failed: {e}"),
                    ))),
                },
                // Covers both refusals that have a way out: the gate
                // ([policies.terminal] enabled = false) and the cwd jail. The
                // server's message names the remedy; show it verbatim.
                // `settings_write_error` replaces only the generic
                // admin-privilege refusal (`pty.*` is fully admin-gated) --
                // every other message, gate and jail included, passes through
                // unframed.
                Err(e) => error.set(Some(admin_refusal::settings_write_error(i18n, &e, |e| {
                    e.to_string()
                }))),
            }
        });
    };

    // Ask the server what exists and rebuild the tab strip from it.
    //
    // `allow_spawn` is only true on the first pass. A later refresh that finds
    // nothing open means the user closed everything, and re-opening a shell
    // they just closed would be the page arguing with them.
    let refresh_tabs = move |allow_spawn: bool, force_attach: bool| {
        spawn_local(async move {
            // "Nothing to adopt" and "I could not get an answer" are different
            // facts, and only the first may create a shell. The classification
            // lives in `session::resolve_session_list` (pure, unit-tested
            // there); the "may I spawn" half is `session::should_spawn`, which
            // a `Fail` cannot even be handed because it carries no rows.
            let sessions = match session::resolve_session_list(
                state.rpc_call(PTY_LIST_METHOD, serde_json::json!({})).await,
            ) {
                session::ListOutcome::Read(sessions) => sessions,
                session::ListOutcome::Fail(msg) => {
                    error.set(Some(admin_refusal::settings_load_error(i18n, &msg, |e| {
                        e.to_string()
                    })));
                    return;
                }
            };

            tabs.update(|m| m.reconcile(&sessions));

            match tabs
                .try_with_untracked(|m| m.selected_id().map(str::to_string))
                .flatten()
            {
                Some(sid) => attach_to(sid, force_attach),
                None if allow_spawn && session::should_spawn(&sessions) => spawn_new(),
                None => {}
            }
        });
    };

    // Ask the server to end a session.
    //
    // Deliberately does NOT remove the tab here. `pty.exit` is what says the
    // shell is gone; a tab removed optimistically on a close that was refused
    // would hide a shell that is still running (判据 §8 — the request is not
    // the outcome).
    let close_tab = move |sid: String| {
        spawn_local(async move {
            if let Err(e) = state
                .rpc_call("pty.close", serde_json::json!({ "session_id": sid }))
                .await
            {
                error.set(Some(admin_refusal::settings_write_error(i18n, &e, |e| {
                    e.to_string()
                })));
            }
        });
    };

    // Subscribe before listing on mount and after every reconnect. Whatever a
    // shell prints on startup then cannot land in the gap between spawn and
    // subscribe.
    Effect::new(move |_| {
        let _epoch = state.connection_epoch.get();
        if !state.is_connected.get() {
            return;
        }
        let state = state;
        spawn_local(async move {
            if let Err(e) = state.subscribe_topic_ephemeral(PTY_SCREEN_TOPIC).await {
                error.set(Some(admin_refusal::settings_load_error(i18n, &e, |e| {
                    e.to_string()
                })));
                return;
            }
            // A session that exits while this view is open must not leave a
            // tab that attaches to nothing. A failed subscription here is
            // reported but does not stop the rest: the tabs still work, they
            // just stop noticing exits until the next list.
            if let Err(e) = state.subscribe_topic_ephemeral(PTY_EXIT_TOPIC).await {
                error.set(Some(admin_refusal::settings_load_error(i18n, &e, |e| {
                    e.to_string()
                })));
            }

            let Some(allow_spawn) = first_load.try_get_value() else {
                return;
            };
            refresh_tabs(allow_spawn, !allow_spawn);
            first_load.set_value(false);
        });
    });

    // `pty.exit` is the session lifecycle edge for the tab strip.
    Effect::new(move |_| {
        let handler_id = state.subscribe_events(move |ev| {
            if ev.topic != PTY_EXIT_TOPIC {
                return;
            }
            // Hand-parsed: `shared/protocol` has a type for `pty.list` and for
            // the screen frame, but none for this event's payload, and that
            // crate is frozen for this round (see the report's concerns). A
            // missing or non-string `session_id` is DROPPED rather than
            // guessed at — closing the wrong tab is worse than missing one.
            let Some(sid) = ev
                .data
                .get("session_id")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
            else {
                return;
            };
            tabs.update(|m| m.on_exit(&sid));
            if let Some(next) = tabs
                .try_with_untracked(|m| m.selected_id().map(str::to_string))
                .flatten()
            {
                attach_to(next, false);
            }
            // `on_exit` only MARKS the tab closed; `reconcile` is what drops
            // it, and until this call nothing ever scheduled one, so the
            // dimmed dead tab survived for the life of the mount and the
            // method's own doc described behaviour that did not exist
            // (判据 §1). The row stays visible for exactly one round trip —
            // long enough to see WHY it went away, not longer.
            refresh_tabs(false, false);
        });
        on_cleanup(move || state.unsubscribe_events(handler_id));
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
    Effect::new(move |_| {
        let state = state;
        spawn_local(async move {
            let Ok(policies) = state
                .rpc_call("config.get", serde_json::json!({ "section": "policies" }))
                .await
            else {
                return;
            };
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
            measure_and_report(canvas_ref, state, sid, family, size, repaint_tick);
        });
    });

    // Frame handler.
    Effect::new(move |_| {
        let handler_id = state.subscribe_events(move |ev| {
            if ev.topic != PTY_SCREEN_TOPIC {
                return;
            }
            let Ok(frame) = serde_json::from_value::<PtyScreenFrame>(ev.data) else {
                return;
            };
            // NOT a `frame.session_id != mine` test here. `ClientScreen`
            // knows its own id and `apply` answers `WrongSession`; a second
            // copy of that comparison is a second answer to one question, and
            // the copy that drifts is always the one without the tests.
            let outcome = screen.try_update_value(|s| {
                s.as_mut()
                    .map_or(ApplyOutcome::Buffered, |s| s.apply(frame))
            });
            match outcome {
                Some(ApplyOutcome::Applied) => {
                    repaint_tick.update(|n| *n = n.wrapping_add(1));
                    // S7: `ClientScreen::title()` had no reader at all — the
                    // server parsed OSC 0/2 and the client stored it for
                    // nobody. It names the tab now. Guarded on "did it
                    // change" because this runs on every applied frame and an
                    // unconditional signal write would re-render the strip at
                    // the frame rate.
                    if let (Some(sid), Some(title)) = (
                        session_id.get_value(),
                        screen
                            .with_value(|s| s.as_ref().and_then(|s| s.title().map(str::to_string))),
                    ) {
                        // The predicate is the model's, and it asks about
                        // `osc_title` rather than the DERIVED `title` — see
                        // `TabModel::osc_title_differs` for what comparing the
                        // derived name against the raw one silently lost.
                        let changed = tabs
                            .try_with_untracked(|m| m.osc_title_differs(&sid, &title))
                            .unwrap_or(false);
                        if changed {
                            tabs.update(|m| m.on_title(&sid, &title));
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
                // Buffered / Discarded / WrongSession: nothing to draw and
                // nothing to recover. `Discarded` especially must NOT resync --
                // it means a frame we already hold arrived a second time, and
                // re-attaching on it turns a duplicate into a round trip.
                _ => {}
            }
        });
        on_cleanup(move || state.unsubscribe_events(handler_id));
    });

    // Repaint: coalesced onto the browser's own paint cycle rather than drawn
    // synchronously from the frame handler above, so a burst of frames (e.g.
    // `yes | head -100000`) triggers at most one paint per animation frame
    // instead of one per frame received off the wire.
    Effect::new(move |_| {
        repaint_tick.get();
        let family = font_family.get_value();
        let size_px = font_size_px.get_value();
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
            state,
            session_id.get_value(),
            font_family.get_value(),
            font_size_px.get_value(),
            repaint_tick,
        );

        let cb: Closure<dyn FnMut(js_sys::Array)> = Closure::new(move |_entries: js_sys::Array| {
            measure_and_report(
                canvas_ref,
                state,
                session_id.get_value(),
                font_family.get_value(),
                font_size_px.get_value(),
                repaint_tick,
            );
        });
        if let Some((canvas, _ctx)) = canvas_ctx(canvas_ref) {
            if let Ok(observer) = web_sys::ResizeObserver::new(cb.as_ref().unchecked_ref()) {
                observer.observe(&canvas);
                on_cleanup(move || observer.disconnect());
            }
        }
        cb.forget();
    });

    // Bytes -> the PTY, base64-encoded (raw bytes, not JS `btoa`: control
    // bytes and multi-byte UTF-8 -- Ctrl-C is 0x03, CJK input is more than
    // one byte per character -- round-trip corrupted through `btoa`'s Latin-1
    // assumption). Reuses the voice-capture path's pure encoder rather than
    // adding a second one.
    //
    // One function for both keystrokes and pastes: they are the same wire
    // call, and a second copy of it is where the two would drift.
    let send_bytes = move |bytes: Vec<u8>| {
        let Some(sid) = session_id.get_value() else {
            return;
        };
        let data = crate::views::voice::wav::base64_encode(&bytes);
        spawn_local(async move {
            let _ = state
                .rpc_call(
                    "pty.input",
                    serde_json::json!({ "session_id": sid, "data": data, "base64": true }),
                )
                .await;
        });
    };

    // A key the keymap does not claim is left alone entirely, not even
    // `prevent_default()`, so any browser/OS shortcut we do not mean to
    // swallow keeps working. That now includes the PASTE chords: the meta
    // bail that used to live here has moved into `keymap::encode_key`, which
    // takes `meta` and answers `Browser` for Cmd-V and Ctrl-Shift-V — one
    // decision in one place rather than half a rule on each side (判据 §6).
    let on_keydown = move |ev: web_sys::KeyboardEvent| match keymap::encode_key(
        &ev.key(),
        ev.ctrl_key(),
        ev.alt_key(),
        ev.shift_key(),
        ev.meta_key(),
    ) {
        keymap::KeyAction::Bytes(bytes) => {
            ev.prevent_default();
            send_bytes(bytes);
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
        send_bytes(session::encode_paste(&text, bracketed));
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
                // Selection belongs to the local tab model. A click is accepted
                // only when the existing tab is open, then the view attaches to it.
                on_select=Callback::new(move |sid: String| {
                    let accepted = tabs.try_update(|m| m.select(&sid)).unwrap_or(false);
                    if accepted {
                        attach_to(sid, false);
                    }
                })
                on_close=Callback::new(move |sid: String| close_tab(sid))
                on_new=Callback::new(move |(): ()| spawn_new())
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
    state: DashboardState,
    session_id: Option<String>,
    font_family: String,
    font_size_px: f64,
    repaint_tick: RwSignal<u32>,
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
        let _ = state
            .rpc_call(
                "pty.resize",
                serde_json::json!({ "session_id": sid, "rows": rows, "cols": cols }),
            )
            .await;
    });
}

fn device_pixel_ratio() -> f64 {
    web_sys::window()
        .map(|w| w.device_pixel_ratio())
        .unwrap_or(1.0)
        .max(1.0)
}

#[cfg(test)]
mod tests {
    /// Source-level guard for the layout bug this view already shipped
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
        let src = include_str!("mod.rs").replace('\r', "");
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
