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

pub mod keymap;
pub mod render;
pub mod session;

use aleph_protocol::pty::{
    PtyAttachResponse, PtyScreenFrame, PtySpawnResponse, PTY_LIST_METHOD, PTY_SCREEN_TOPIC,
};
use leptos::prelude::*;
use leptos::task::spawn_local;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::JsCast;
use web_sys::CanvasRenderingContext2d;

use crate::components::admin_refusal;
use crate::context::DashboardState;
use session::{ApplyOutcome, AttachOutcome, ClientScreen};

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
    let resync = move |sid: String| {
        let state = state;
        spawn_local(async move {
            screen.update_value(|s| {
                if let Some(s) = s {
                    s.begin_attach();
                }
            });
            match state
                .rpc_call("pty.attach", serde_json::json!({ "session_id": sid }))
                .await
            {
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
                    Err(e) => error.set(Some(admin_refusal::settings_load_error(
                        i18n,
                        &e.to_string(),
                        |e| format!("attach decode failed: {e}"),
                    ))),
                },
                // An Err is never read as an empty screen: the server said
                // something, and what it said is not "the terminal is idle".
                // `settings_load_error` replaces only the generic
                // admin-privilege refusal (`pty.*` is fully admin-gated);
                // every other message -- including the policy gate's own
                // "[policies.terminal] enabled = false" text -- passes
                // through verbatim, which is what names the remedy.
                Err(e) => error.set(Some(admin_refusal::settings_load_error(i18n, &e, |e| {
                    e.to_string()
                }))),
            }
        });
    };

    // Mount: subscribe BEFORE resolving a session. Whatever the shell prints
    // on startup then cannot land in the gap between spawn and subscribe.
    //
    // Resolving is list-then-spawn, NOT spawn. See the note below this block.
    Effect::new(move |_| {
        let state = state;
        spawn_local(async move {
            if let Err(e) = state.subscribe_topic_ephemeral(PTY_SCREEN_TOPIC).await {
                error.set(Some(admin_refusal::settings_load_error(i18n, &e, |e| {
                    e.to_string()
                })));
                return;
            }

            // A live session already on the server IS this view's session.
            // A refresh, a second tab and a reconnect all arrive here.
            //
            // The three-way classification lives in `resolve_attach_target`
            // (pure, unit-tested there) because "nothing to adopt" and "I
            // could not get an answer" are different facts, and only the
            // first may create a shell.
            let existing: Option<String> = match session::resolve_attach_target(
                state.rpc_call(PTY_LIST_METHOD, serde_json::json!({})).await,
            ) {
                session::AttachDecision::Attach(sid) => Some(sid),
                session::AttachDecision::Spawn => None,
                // A `pty.list` that did not succeed is never read as "there
                // are no sessions" — spawning on a guess leaves a second
                // shell beside a live one whose screen is still on the
                // server, and nothing on this page pointing at it.
                session::AttachDecision::Fail(msg) => {
                    error.set(Some(admin_refusal::settings_load_error(i18n, &msg, |e| {
                        e.to_string()
                    })));
                    return;
                }
            };

            if let Some(sid) = existing {
                // Dimensions and seq arrive with the attach response and
                // `finish_attach` re-seats both, so this placeholder is never
                // observed. Setting the id BEFORE the RPC matters: the frame
                // handler DROPS frames whose session it cannot name, and a
                // dropped frame is not a buffered one.
                screen.set_value(Some(ClientScreen::new(24, 80, 0, sid.clone())));
                session_id.set_value(Some(sid.clone()));
                // Correct the placeholder immediately: this reattached
                // session may have been sized by someone else entirely, and
                // our own measured viewport has not been reported yet (the
                // resize effect below raced this async continuation and, on
                // its first synchronous run, had no session id to report
                // against). See `measure_and_report`'s doc for why this call
                // is safe post-`.await`.
                measure_and_report(
                    canvas_ref,
                    state,
                    Some(sid.clone()),
                    font_family.get_value(),
                    font_size_px.get_value(),
                    repaint_tick,
                );
                resync(sid);
                return;
            }

            let (rows, cols) = (24_u16, 80_u16); // replaced by the measured
                                                 // viewport in the resize step below
            match state
                .rpc_call(
                    "pty.spawn",
                    serde_json::json!({ "rows": rows, "cols": cols }),
                )
                .await
            {
                Ok(v) => match serde_json::from_value::<PtySpawnResponse>(v) {
                    Ok(resp) => {
                        screen.set_value(Some(ClientScreen::new(
                            resp.rows,
                            resp.cols,
                            resp.seq,
                            resp.session_id.clone(),
                        )));
                        session_id.set_value(Some(resp.session_id.clone()));
                        measure_and_report(
                            canvas_ref,
                            state,
                            Some(resp.session_id.clone()),
                            font_family.get_value(),
                            font_size_px.get_value(),
                            repaint_tick,
                        );
                        resync(resp.session_id);
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
                // every other message, gate and jail included, passes
                // through unframed.
                Err(e) => error.set(Some(admin_refusal::settings_write_error(i18n, &e, |e| {
                    e.to_string()
                }))),
            }
        });
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
            measure_and_report(
                canvas_ref,
                state,
                session_id.get_value(),
                font_family.get_value(),
                font_size_px.get_value(),
                repaint_tick,
            );
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
                Some(ApplyOutcome::Applied) => repaint_tick.update(|n| *n = n.wrapping_add(1)),
                Some(ApplyOutcome::Gap { .. }) => {
                    if let Some(mine) = session_id.get_value() {
                        resync(mine);
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

    // Keydown -> the PTY, base64-encoded (raw bytes, not JS `btoa`: control
    // bytes and multi-byte UTF-8 -- Ctrl-C is 0x03, CJK input is more than
    // one byte per character -- round-trip corrupted through `btoa`'s
    // Latin-1 assumption). Reuses the voice-capture path's pure encoder
    // rather than adding a second one.
    //
    // A key `encode_key` does not claim is left alone entirely, not even
    // `prevent_default()`, so any browser/OS shortcut we do not mean to
    // swallow keeps working. Meta-chord letters (Cmd-C copy, Cmd-V paste on
    // macOS) are the same case by a different route: `encode_key` has no way
    // to express "meta" (its tested contract is key/ctrl/alt/shift only), so
    // the copy/paste the user almost certainly means is handled here, by
    // never forwarding a meta chord at all, rather than by teaching
    // `keymap.rs` a modifier it cannot report on.
    let on_keydown = move |ev: web_sys::KeyboardEvent| {
        if ev.meta_key() {
            return;
        }
        let Some(sid) = session_id.get_value() else {
            return;
        };
        let Some(bytes) =
            keymap::encode_key(&ev.key(), ev.ctrl_key(), ev.alt_key(), ev.shift_key())
        else {
            return;
        };
        ev.prevent_default();
        let data = crate::views::voice::wav::base64_encode(&bytes);
        let state = state;
        spawn_local(async move {
            let _ = state
                .rpc_call(
                    "pty.input",
                    serde_json::json!({ "session_id": sid, "data": data, "base64": true }),
                )
                .await;
        });
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
            {move || error.get().map(|e| view! {
                <div class="px-3 py-2 text-sm text-danger" role="alert">{e}</div>
            })}
            <canvas
                node_ref=canvas_ref
                tabindex="0"
                class="flex-1 min-h-0 outline-none"
                on:keydown=on_keydown
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
