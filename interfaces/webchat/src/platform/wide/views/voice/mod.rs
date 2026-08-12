//! Immersive voice mode — full-screen overlay hosting the voice session loop.
//!
//! Wires the pure cores (VAD / sentence splitter / phase machine) to the wasm
//! audio glue (mic capture + sequential TTS) and the chat send/stream pipeline.
//! The overlay sits on top of the always-mounted `ChatView`, so the
//! `stream.*` subscription that feeds `chat.messages` keeps running underneath.

pub(crate) mod audio;
pub(crate) mod caption_state;
pub(crate) mod level;
pub(crate) mod machine;
pub(crate) mod orb;
pub(crate) mod sentence;
pub(crate) mod vad;
pub(crate) mod wav;

use std::cell::RefCell;
use std::rc::Rc;

use leptos::prelude::*;
use leptos::task::spawn_local;

use shared_ui_logic::state::session_dials_for_send;

use crate::api::chat::ChatApi;
use crate::context::{DashboardState, GatewayEvent};
use crate::state::sessions::SessionMap;
use crate::views::chat::ChatState;
use audio::{MicError, MicSession, TtsPlayer};
use caption_state::{apply_delta, apply_formatted, lock, CaptionState, Delta};
use level::LevelSmoother;
use machine::{on_event, Action, VoiceEvent, VoicePhase};
use orb::VoiceOrb;
use sentence::SentenceSplitter;
use vad::{barge_step, vad_step, BargeConfig, BargeState, VadConfig, VadEvent, VadState};

/// Consecutive streaming failures (dead backend, empty utterances) tolerated
/// before the session falls back to the batch `voice.transcribe` path.
const STREAM_STRIKE_LIMIT: u32 = 2;

/// Fire-and-forget server-side cancel of a superseded run. Reuses the existing
/// `chat.abort` lifecycle RPC (A4) — a barge-in or a follow-up utterance
/// already silences the old reply locally; this stops the abandoned run from
/// burning tokens on an answer nobody will hear.
fn abort_run(dash: DashboardState, run_id: String) {
    spawn_local(async move {
        let _ = dash
            .rpc_call("chat.abort", serde_json::json!({ "run_id": run_id }))
            .await;
    });
}

/// App-level switch for the immersive overlay. Provided in `app.rs`; the
/// composer mini-orb (Task 8) flips `open` to enter/leave the session.
#[derive(Clone, Copy)]
pub(crate) struct VoiceMode {
    pub open: RwSignal<bool>,
}

impl VoiceMode {
    pub(crate) fn new() -> Self {
        Self {
            open: RwSignal::new(false),
        }
    }
}

/// Who the caption is quoting.
#[derive(Clone, PartialEq)]
enum Caption {
    Idle,
    /// Live streaming transcript: render the two-stage [`CaptionState`] held in
    /// the `caption_state` signal (committed solid + interim gray, `.voice-lock`
    /// when the utterance has ended). Carries no text itself — the content lives
    /// in `caption_state` so the delta subscription and the lock point can mutate
    /// it independently of which mode the caption is in.
    Live,
    User(String),
    Assistant(String),
    Error(String),
}

/// Overlay host: mounts the live [`VoiceSession`] only while `open`. Mounting
/// is the lifecycle boundary — the mic stream opens on mount and `on_cleanup`
/// tears it down on close, so an idle (closed) overlay holds no resources.
#[component]
pub(crate) fn ImmersiveVoiceView() -> impl IntoView {
    let voice_mode = expect_context::<VoiceMode>();
    view! {
        <Show when=move || voice_mode.open.get()>
            <VoiceSession />
        </Show>
    }
}

#[component]
fn VoiceSession() -> impl IntoView {
    let dash = expect_context::<DashboardState>();
    let chat = expect_context::<ChatState>();
    let sessions = expect_context::<SessionMap>();
    let voice_mode = expect_context::<VoiceMode>();
    // Native shell vs. browser decides how a mic denial is remediated (OS TCC
    // deep-link vs. browser per-site permission). Resolved once at mount.
    let native = audio::is_native_shell();

    let phase = RwSignal::new(VoicePhase::Listening);
    let level = RwSignal::new(0.0_f64);
    let caption = RwSignal::new(Caption::Idle);
    // Two-stage live transcript for the streaming path. Rendered when `caption`
    // is `Caption::Live`; mutated by the `voice.transcribe.delta` subscription
    // (`apply_delta`) and at the VAD utterance-end point (`lock`). A `RwSignal`
    // (not a `RefCell`) so the subscription closure can write it through a
    // `Copy + Send + Sync` handle (the subscribe handler bound is `Send + Sync`).
    let caption_state = RwSignal::new(CaptionState::default());
    let error_flash = RwSignal::new(false);
    let mic_error: RwSignal<Option<MicError>> = RwSignal::new(None);

    // Current streaming stream_id (None ⇒ batch fallback, i.e. BYO streaming
    // disabled in core). Lives in a LocalStorage slot so the (Send + Sync)
    // delta-subscription closure, the frame sink, the VAD utterance-end branch,
    // and the async mic-open task can all reach it. `StoredValue` handles are
    // `Copy + Send + Sync` for any payload (wasm is single-threaded).
    let stream_id: StoredValue<Option<String>, LocalStorage> = StoredValue::new_local(None);
    // Monotonic "open generation" guarding `start_listening_stream` against an
    // in-flight-overwrite leak: if two Listening-entries fire while the first
    // `voice.stream.start` is still pending, the slot is still None at the
    // second entry (the synchronous defensive stop can't see it), so both
    // resolve and the first id would be overwritten and never stopped. Each
    // open bumps the gen; only the latest installs — a superseded open closes
    // the stream it just opened. Both guards together fully close the leak.
    let stream_gen: StoredValue<u64, LocalStorage> = StoredValue::new_local(0);
    // Handle for the one `voice.transcribe.delta` subscription, released in
    // `on_cleanup` so closing/reopening the overlay does not leak handlers.
    let voice_sub_id: StoredValue<Option<usize>, LocalStorage> = StoredValue::new_local(None);
    // The run currently executing for this voice session (set at send, cleared
    // at finalize). A barge-in or a follow-up utterance takes it and fires
    // `chat.abort` so the superseded run stops server-side too.
    let active_run: StoredValue<Option<String>, LocalStorage> = StoredValue::new_local(None);
    // Streaming health strikes: dead-backend events and empty utterance-ends.
    // At STREAM_STRIKE_LIMIT the session flips `streaming_off` and every later
    // utterance rides the (independently working) batch WAV path.
    let stream_strikes: StoredValue<u32> = StoredValue::new(0);
    // "This session rides batch." Latched by the strike limit above OR by core
    // answering `voice.stream.start` with an explicit `stream_id: null` (BYO
    // streaming disabled in config — a fact that cannot change mid-session, so
    // re-asking once per utterance was pure round-trip waste).
    let streaming_off: StoredValue<bool> = StoredValue::new(false);

    // The mic and the interval handle outlive their writer (the async mic-open
    // task) and must be reachable from `on_cleanup`, whose closure bound is
    // `Send + Sync`. A plain `Rc` is `!Send`, so they live in `LocalStorage`
    // arena slots instead — the `StoredValue` handle is `Copy + Send + Sync`
    // for any payload (wasm is single-threaded; the `Send + Sync` bound on the
    // handle is a type-system formality, not real cross-thread access).
    let mic: StoredValue<Option<Rc<MicSession>>, LocalStorage> = StoredValue::new_local(None);
    let vad = Rc::new(RefCell::new(VadState::default()));
    let vad_cfg = VadConfig::default();
    // Echo-aware barge-in detector, run instead of the listening VAD while the
    // assistant is Speaking. Reset to default on every non-Speaking frame so each
    // reply's onset grace starts fresh.
    let barge = Rc::new(RefCell::new(BargeState::default()));
    let barge_cfg = BargeConfig::default();
    let splitter = Rc::new(RefCell::new(SentenceSplitter::default()));
    // Reactive run-id gate for the TTS Effect. It is a SIGNAL (not a plain
    // `RefCell`) on purpose: arming it must re-run the Effect against whatever
    // reply content is ALREADY present, so a warm round whose reply streamed in
    // and completed BEFORE the gate was armed still gets spoken (see the Effect).
    let speak_run = RwSignal::new(None::<String>);
    // Last-writer-wins generation guard for `speak_run`. Bumped on barge-in and
    // at each utterance's send-arming point so an abandoned/overtaken run can
    // neither keep speaking (its deltas find `speak_run == None`) nor clobber a
    // newer run (the guarded final assignment checks the gen still matches).
    let speak_gen = Rc::new(RefCell::new(0u64));
    let consecutive_errors = Rc::new(RefCell::new(0u32));

    // The player is constructed after `dispatch` (whose callbacks the player
    // needs) and is read from three closures — `dispatch` (StopPlayback on
    // barge-in / stale audio), the tick loop, and `on_cleanup`. It therefore
    // lives in a shared LocalStorage slot, populated once construction
    // completes. `on_cleanup`'s `Send + Sync` bound rules out a plain `Rc`.
    let player_slot: StoredValue<Option<Rc<TtsPlayer>>, LocalStorage> =
        StoredValue::new_local(None);
    let dispatch = move |ev: VoiceEvent| {
        let (next, action) = on_event(phase.get_untracked(), ev);
        phase.set(next);
        match action {
            Action::None => {}
            Action::StopPlayback => {
                player_slot.with_value(|p| {
                    if let Some(p) = p.as_ref() {
                        p.stop_all();
                    }
                });
            }
            Action::ShowError => {
                // "Didn't catch that" is honest for a transcription miss but a lie for a
                // failed agent run — tell the user which side actually broke.
                let msg = if ev == VoiceEvent::RunFailed {
                    "回复出错了，稍等再说一次？"
                } else {
                    "没听清，再说一次？"
                };
                caption.set(Caption::Error(msg.into()));
                error_flash.set(true);
                set_timeout(
                    move || error_flash.set(false),
                    std::time::Duration::from_millis(900),
                );
            }
        }
    };

    // `dispatch` is `Copy` (it captures only signal + StoredValue handles), so
    // each callback copies it in directly — no `.clone()` needed.
    let player = TtsPlayer::new(
        move || dispatch(VoiceEvent::FirstAudioReady),
        move || dispatch(VoiceEvent::PlaybackDrained),
        move |sentence| caption.set(Caption::Assistant(sentence)),
    );
    player_slot.set_value(Some(Rc::clone(&player)));

    // Subscribe ONCE to `voice.transcribe.delta` for the whole session (not per
    // utterance — that would leak handlers). The core fan-out gates on an
    // `events.subscribe` first, mirroring the chat `stream.*` / approval paths.
    // The handler bound is `Send + Sync`; it captures only the `Copy` signal /
    // slot handles (wasm is single-threaded — the bound is a type formality).
    {
        spawn_local(async move {
            let _ = dash.subscribe_topic("voice.transcribe.delta").await;
        });
        let id = dash.subscribe_events(move |event: GatewayEvent| {
            if event.topic != "voice.transcribe.delta" {
                return;
            }
            // Drop deltas that belong to a previous utterance's stream so stale
            // text from a just-closed stream can't bleed into the new one.
            let ev_sid = event.data.get("stream_id").and_then(|s| s.as_str());
            let matches =
                stream_id.with_value(|cur| cur.as_deref().is_some_and(|cur| Some(cur) == ev_sid));
            if !matches {
                return;
            }
            // Terminal / fatal markers from the relay: `{closed: true}` when the
            // pump exits, or a delta carrying `error` (backend WAIT / ERROR /
            // DISCONNECT). A *matching* slot means the death was NOT
            // client-initiated (normal stops clear the slot before the RPC
            // resolves) — clear it and strike. The armed WAV segment keeps the
            // current utterance alive on the batch path.
            let closed = event
                .data
                .get("closed")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false);
            let stream_err = event
                .data
                .pointer("/delta/error")
                .and_then(|e| e.as_str())
                .map(str::to_string);
            if closed || stream_err.is_some() {
                if let Some(e) = stream_err {
                    web_sys::console::warn_1(&format!("voice stream error: {e}").into());
                }
                stream_id.set_value(None);
                let n = stream_strikes
                    .try_update_value(|n| {
                        *n += 1;
                        *n
                    })
                    .unwrap_or(0);
                if n >= STREAM_STRIKE_LIMIT {
                    streaming_off.set_value(true);
                    web_sys::console::warn_1(
                        &"voice streaming: repeated backend failures — falling back to batch STT"
                            .into(),
                    );
                }
                return;
            }
            let Some(delta) = event.data.get("delta") else {
                return;
            };
            let committed = delta
                .get("committed")
                .and_then(|c| c.as_str())
                .unwrap_or("")
                .to_string();
            let interim = delta
                .get("interim")
                .and_then(|i| i.as_str())
                .unwrap_or("")
                .to_string();
            // The Panel VAD is authoritative for the lock (the UtteranceEnd
            // branch fires it); `delta.utterance_end` is intentionally ignored
            // here so a backend end signal can't pre-empt the local wave.
            caption_state.update(|s| apply_delta(s, Delta { committed, interim }));
            // Switch the caption into live two-stage rendering on first delta.
            if caption.get_untracked() != Caption::Live {
                caption.set(Caption::Live);
            }
        });
        voice_sub_id.set_value(Some(id));
    }

    // Mic open + 50 ms VAD tick loop. Async because getUserMedia awaits a
    // permission grant; the interval handle is stashed (LocalStorage, same
    // reason as `mic`) for cleanup.
    let tick_handle: StoredValue<Option<IntervalHandle>, LocalStorage> =
        StoredValue::new_local(None);
    {
        let vad = Rc::clone(&vad);
        let barge = Rc::clone(&barge);
        let player = Rc::clone(&player);
        let splitter = Rc::clone(&splitter);
        let speak_gen = Rc::clone(&speak_gen);
        let consecutive_errors = Rc::clone(&consecutive_errors);
        // dB-domain fast-attack/slow-decay smoothing for the orb (FluidVoice's
        // level pipeline). Visual only — the VAD below stays on raw RMS.
        let orb_level = Rc::new(RefCell::new(LevelSmoother::default()));
        spawn_local(async move {
            let session = match MicSession::open().await {
                Ok(s) => s,
                Err(e) => {
                    // NOT routed through `admin_refusal`: a `MicError` is local
                    // hardware/permission state, never a gateway verdict.
                    mic_error.set(Some(e));
                    return;
                }
            };
            // The `await` above parks on a browser permission prompt for as long
            // as the user looks at it, so the overlay can be closed — and this
            // component disposed — before it resolves. `on_cleanup` has then
            // already drained both slots, and everything below would re-fill
            // them for nobody: an interval no one will ever `clear()` (ticking
            // every 50 ms on disposed signals) and a `MicSession` no one will
            // ever `close()` — the OS recording indicator stays lit for the rest
            // of the page's life. The panic is the loud half of that; the mic
            // leak is the quiet half, and it is the one users notice.
            //
            // `try_get_untracked` on a component-owned signal (`phase` is an
            // `RwSignal::new` in this function) is the disposal probe.
            if phase.try_get_untracked().is_none() {
                session.close();
                return;
            }
            mic.set_value(Some(Rc::clone(&session)));
            // Mic grant is a strong user activation — unlock the output context
            // now so the first reply's buffer source plays (a suspended context
            // is silent, and the reply arrives long after the entry tap).
            player.resume_output();

            // Open the first streaming session now that the mic exists. Every
            // subsequent listening period re-opens a FRESH stream via the
            // phase-watching Effect below (the mic slot is populated by then, so
            // that Effect handles re-entries; this one-shot covers the very first
            // entry, which fires before the mic is ready).
            start_listening_stream(
                Rc::clone(&session),
                dash,
                stream_id,
                stream_gen,
                caption_state,
                streaming_off,
            );

            let handle = set_interval_with_handle(
                move || {
                    let rms = session.rms();
                    // Belt and braces: `on_cleanup` clears this interval, but it
                    // does so from the same `Owner::cleanup()` that disposes the
                    // signals, so the tick must not assume it wins that race.
                    // Skipping a tick costs one 50 ms frame of orb animation.
                    let Some(current_phase) = phase.try_get_untracked() else {
                        return;
                    };
                    let speaking = current_phase == VoicePhase::Speaking;
                    // Orb level: perceptual dB mapping + fast-attack/slow-decay
                    // smoothing (level.rs) so ordinary speech (RMS ~0.06-0.2)
                    // fills the orb's range instead of an imperceptible nudge.
                    // While the reply is speaking the mic mostly hears the AI's own
                    // playback echo, so drive the orb from the playback analyser.
                    let out_level = if speaking { player.output_level() } else { 0.0 };
                    let raw = if speaking { out_level } else { rms };
                    level.set(f64::from(orb_level.borrow_mut().step(raw)));

                    if speaking {
                        // The AI's TTS plays through a separate AudioContext outside
                        // the mic's AEC reference, so its voice leaks into `rms`. A
                        // naive VAD would mistake that echo for the user and kill the
                        // reply (clipped opening / sudden mid-reply silence). Use the
                        // echo-aware, debounced, onset-graced detector instead, and
                        // do NOT run the listening VAD here — echo must not pollute
                        // its state. The barge-in flips the phase to Listening; the
                        // non-Speaking branch below then resets a clean VAD + barge
                        // state for the next utterance.
                        let (next, fire) = barge_step(*barge.borrow(), rms, out_level, &barge_cfg);
                        *barge.borrow_mut() = next;
                        if fire {
                            web_sys::console::debug_1(
                                &format!("voice barge-in: rms={rms:.3} out={out_level:.3}").into(),
                            );
                            // Invalidate the old run BEFORE stopping audio: bump the
                            // gen and clear speak_run so the Effect early-returns on
                            // any further deltas from the abandoned run (it is
                            // intentionally never finalized), then drain queue + audio.
                            *speak_gen.borrow_mut() += 1;
                            speak_run.set(None);
                            // Cancel the superseded run server-side too — the user
                            // cut it off; letting it keep generating only burns
                            // tokens on an answer nobody will hear.
                            if let Some(old) = active_run.try_update_value(Option::take).flatten() {
                                abort_run(dash, old);
                            }
                            dispatch(VoiceEvent::BargeIn);
                            session.start_segment();
                        }
                        return;
                    }

                    // Not Speaking: reset the barge detector so the next reply's
                    // onset grace starts fresh, and run the listening VAD.
                    *barge.borrow_mut() = BargeState::default();
                    let (next, ev) = vad_step(*vad.borrow(), rms, &vad_cfg);
                    *vad.borrow_mut() = next;
                    match ev {
                        Some(VadEvent::SpeechStart) => {
                            session.start_segment();
                        }
                        Some(VadEvent::Discarded) => {
                            session.discard_segment();
                        }
                        Some(VadEvent::UtteranceEnd { .. }) => {
                            match stream_id.with_value(Clone::clone) {
                                // Streaming mode: the live transcript is already
                                // on screen via deltas. Lock it (CSS wave fires),
                                // send the raw text to the agent with zero extra
                                // latency (no `voice.transcribe` round-trip), and
                                // close the backend stream.
                                Some(id) => {
                                    // committed + still-floating interim: short
                                    // utterances often live entirely in the
                                    // interim when the VAD fires (and
                                    // faster-whisper never flushes a final after
                                    // END_OF_AUDIO). Then the whole-utterance
                                    // hallucination gate — the SAME shared filter
                                    // the batch STT path applies server-side.
                                    // Same tick, same race as `phase` above.
                                    let Some(st) = caption_state.try_get_untracked() else {
                                        return;
                                    };
                                    let raw = aleph_protocol::voice_text::merge_utterance(
                                        &st.committed,
                                        &st.interim,
                                    );
                                    let raw = aleph_protocol::voice_text::filter_transcript(&raw);
                                    if raw.trim().is_empty() {
                                        // Empty utterance-end: noise blip, deltas
                                        // lagging, or a silently dead decoder.
                                        // Strike, and swap in a FRESH stream —
                                        // keeping the same one risked feeding a
                                        // dead decoder forever (the phase stays
                                        // Listening, so no Effect edge would ever
                                        // replace it). At STREAM_STRIKE_LIMIT the
                                        // session falls back to batch.
                                        dispatch(VoiceEvent::TranscribeFailed);
                                        let n = stream_strikes
                                            .try_update_value(|n| {
                                                *n += 1;
                                                *n
                                            })
                                            .unwrap_or(0);
                                        if n >= STREAM_STRIKE_LIMIT {
                                            streaming_off.set_value(true);
                                            web_sys::console::warn_1(
                                                &"voice streaming: repeated empty utterances — falling back to batch STT".into(),
                                            );
                                        }
                                        session.stop_streaming();
                                        start_listening_stream(
                                            Rc::clone(&session),
                                            dash,
                                            stream_id,
                                            stream_gen,
                                            caption_state,
                                            streaming_off,
                                        );
                                    } else {
                                        // Healthy round — reset the strike count.
                                        stream_strikes.set_value(0);
                                        // Real utterance:
                                        // 1) lock caption → committed gray→white wave
                                        caption_state.update(lock);
                                        // stop the local frame tap (about to close)
                                        session.stop_streaming();
                                        // Snapshot the raw text for AI-formatting
                                        // before `raw` is moved into the agent send.
                                        let raw_for_format = raw.clone();
                                        // 2) raw committed text → Agent (reuse send
                                        //    half). UtteranceSent flips the phase to
                                        //    Processing; the return to Listening
                                        //    re-opens a fresh stream via the Effect.
                                        let player = Rc::clone(&player);
                                        let splitter = Rc::clone(&splitter);
                                        let speak_gen = Rc::clone(&speak_gen);
                                        spawn_local(send_utterance(
                                            raw, dash, chat, sessions, dispatch, player, splitter,
                                            speak_run, speak_gen, active_run,
                                        ));
                                        // 3) close the stream (its delta pump exits)
                                        spawn_local(async move {
                                            let _ = dash
                                                .rpc_call(
                                                    "voice.stream.stop",
                                                    serde_json::json!({ "stream_id": id }),
                                                )
                                                .await;
                                        });
                                        // 4) clear the slot so the next listening
                                        //    period opens a FRESH backend decoder.
                                        stream_id.set_value(None);
                                        // 5) AI-polish the raw transcript
                                        //    (display-only, non-blocking). On success,
                                        //    quietly fade-swap the locked caption text
                                        //    — this NEVER changes what was sent to the
                                        //    agent. A slow format response must not
                                        //    bleed stale text into the NEXT utterance:
                                        //    a later Listening-entry resets
                                        //    caption_state, so only swap while THIS
                                        //    utterance's locked text is still on screen
                                        //    (`locked && committed == raw_for_format`).
                                        //    Any failure → keep the raw white line
                                        //    (voice.format also degrades to raw
                                        //    server-side; P7 graceful degradation).
                                        spawn_local(async move {
                                            let polished = dash
                                                .rpc_call(
                                                    "voice.format",
                                                    serde_json::json!({
                                                        "text": &raw_for_format
                                                    }),
                                                )
                                                .await
                                                .ok()
                                                .and_then(|v| {
                                                    v.get("formatted")
                                                        .and_then(|s| s.as_str())
                                                        .filter(|s| !s.is_empty())
                                                        .map(str::to_string)
                                                });
                                            if let Some(p) = polished {
                                                caption_state.update(|s| {
                                                    if s.locked && s.committed == raw_for_format {
                                                        apply_formatted(s, &p);
                                                    }
                                                });
                                            }
                                        });
                                    }
                                }
                                // Batch fallback (streaming disabled in core or
                                // struck out): unchanged WAV transcribe → send.
                                None => {
                                    handle_utterance(
                                        Rc::clone(&session),
                                        dash,
                                        chat,
                                        sessions,
                                        dispatch,
                                        caption,
                                        Rc::clone(&player),
                                        Rc::clone(&splitter),
                                        speak_run,
                                        Rc::clone(&speak_gen),
                                        active_run,
                                        Rc::clone(&consecutive_errors),
                                    );
                                }
                            }
                        }
                        None => {}
                    }
                },
                std::time::Duration::from_millis(50),
            );
            if let Ok(h) = handle {
                tick_handle.set_value(Some(h));
            }
        });
    }

    // One fresh stream per LISTENING period. Watches `phase` and fires on the
    // `!= Listening → == Listening` edge — which is every path back to Listening:
    // normal turn completion (PlaybackDrained), barge-in (the user cuts in during
    // Speaking), and the error returns. This guarantees utterance 2+ each get a
    // brand-new backend decoder (the previous one was closed at its UtteranceEnd),
    // and that streaming is active ONLY while listening — never during the AI's
    // own TTS playback (Speaking), where the mic would otherwise feed the AI's
    // voice back to the transcriber. The very first entry fires before the mic is
    // ready (slot still None) and is skipped here — the mic-open task does that
    // first open; this Effect owns every re-entry. `Effect::new`'s prev-value
    // arg carries the last phase so we detect the edge without a separate Cell.
    Effect::new(move |prev: Option<VoicePhase>| {
        let cur = phase.get();
        let entering = prev != Some(VoicePhase::Listening) && cur == VoicePhase::Listening;
        if entering {
            if let Some(session) = mic.with_value(Clone::clone) {
                start_listening_stream(
                    session,
                    dash,
                    stream_id,
                    stream_gen,
                    caption_state,
                    streaming_off,
                );
            }
        }
        cur
    });

    // Drive TTS from the streaming assistant message. Reacts to `chat.messages`
    // growing; pushes newly-completed sentences into the player and, once the
    // bubble stops streaming, flushes the tail and finalizes.
    {
        let splitter = Rc::clone(&splitter);
        let player = Rc::clone(&player);
        Effect::new(move |_| {
            // Two reactive dependencies, BOTH required:
            //   1. `speak_run.get()` — arming the run (or clearing it on
            //      barge-in) re-runs this Effect. This is what rescues the warm
            //      round where the whole reply streams in and COMPLETES before
            //      `handle_utterance` arms the gate: the arm itself triggers a
            //      reprocess of the already-present content. Without it the run
            //      would never be spoken and the phase would freeze at "thinking".
            //   2. `chat.messages` (via `.with`) — every stream delta wakes the
            //      Effect for incremental stream-while-speaking while the gate is already armed.
            // Both reads happen unconditionally on the path below, so Leptos
            // registers both subscriptions on every run.
            let run_id = speak_run.get();
            let found = chat.messages.with(|msgs| {
                let run_id = run_id.as_deref()?;
                let target = format!("assistant-{run_id}");
                msgs.iter()
                    .rev()
                    .find(|m| m.id == target)
                    .map(|m| (m.content.clone(), m.is_streaming, m.error.is_some()))
            });
            let Some((content, streaming, has_error)) = found else {
                return;
            };
            // Terminal-but-empty bubble: the run errored (`fail_run` clears
            // `is_streaming` and stamps `error`) or completed with nothing to
            // say. `drive_step` deliberately never finalizes empty content, so
            // without this arm the phase deadlocked at thinking forever.
            if !streaming && content.is_empty() {
                active_run.set_value(None);
                speak_run.update_untracked(|r| *r = None);
                dispatch(if has_error {
                    VoiceEvent::RunFailed
                } else {
                    VoiceEvent::PlaybackDrained
                });
                return;
            }
            let (sentences, finalized) =
                drive_step(&mut splitter.borrow_mut(), &content, streaming);
            for s in sentences {
                player.enqueue(dash, s);
            }
            if finalized {
                // The run reached its natural end — nothing left to abort.
                active_run.set_value(None);
                player.finalize(dash);
                // Clear without notifying: we are INSIDE the Effect that reads
                // `speak_run`, so a tracked write would schedule one redundant
                // extra run (which would just read `None` and return).
                speak_run.update_untracked(|r| *r = None);
            }
        });
    }

    // Overlay close (the `<Show>` unmounts this component) tears down the live
    // session: clear the tick, release the mic stream so the OS indicator goes
    // dark, and stop any in-flight TTS playback. The slot handles are `Copy`.
    on_cleanup(move || {
        if let Some(h) = tick_handle.try_update_value(Option::take).flatten() {
            h.clear();
        }
        if let Some(m) = mic.try_update_value(Option::take).flatten() {
            m.close();
        }
        player_slot.with_value(|p| {
            if let Some(p) = p.as_ref() {
                // Full teardown: stop playback AND close the output AudioContext.
                // stop_all() alone leaks a running context per exit → coreaudiod CPU.
                p.close();
            }
        });
        // Release the delta subscription (the handler slot is reused as a no-op)
        // and stop any still-open backend stream so reopening the overlay starts
        // clean. Both are best-effort.
        if let Some(id) = voice_sub_id.try_update_value(Option::take).flatten() {
            dash.unsubscribe_events(id);
        }
        if let Some(sid) = stream_id.try_update_value(Option::take).flatten() {
            spawn_local(async move {
                let _ = dash
                    .rpc_call("voice.stream.stop", serde_json::json!({ "stream_id": sid }))
                    .await;
            });
        }
    });

    let status_text = move || match phase.get() {
        VoicePhase::Listening => "正在聆听".to_string(),
        VoicePhase::Processing => "正在思考".to_string(),
        VoicePhase::Speaking => "正在说话 · 开口即可打断".to_string(),
    };
    // Mic-open failure replaces the phase hint. A permission denial becomes a
    // click-through to the OS privacy pane (settings_url); other failures (busy
    // device, insecure context) render as plain, honest text.
    let mic_hint = move || {
        match mic_error.get() {
        None => view! { {status_text} }.into_any(),
        Some(e) => match e.settings_url(native) {
            Some(url) => view! {
                <a
                    class="underline underline-offset-2 cursor-pointer hover:text-text-primary transition-colors"
                    href=url
                    target="_blank"
                >
                    {e.caption(native)}
                </a>
            }
            .into_any(),
            None => view! { {e.caption(native)} }.into_any(),
        },
    }
    };
    // Caption renderer. `Live` (streaming) renders the two-stage transcript from
    // `caption_state` — committed (solid `.voice-committed`) + interim (gray
    // `.voice-interim`), with `.voice-lock` on the row once the utterance ends
    // (Task 10 styles the wave). All other variants render plain/quoted text.
    let caption_view = move || match caption.get() {
        Caption::Idle => view! { "" }.into_any(),
        Caption::Live => {
            let s = caption_state.get();
            let row_class = if s.locked {
                "voice-live voice-lock"
            } else {
                "voice-live"
            };
            // `.formatted` toggles the quiet fade-swap (vs the lock C-wave) once
            // AI-polished text has replaced the raw committed line.
            let committed_class = if s.formatted {
                "voice-committed formatted"
            } else {
                "voice-committed"
            };
            view! {
                <span class=row_class>
                    <span class=committed_class>{s.committed}</span>
                    <span class="voice-interim">{s.interim}</span>
                </span>
            }
            .into_any()
        }
        Caption::User(t) => view! { {format!("“{t}”")} }.into_any(),
        Caption::Assistant(t) | Caption::Error(t) => view! { {t} }.into_any(),
    };

    view! {
        <div class="voice-stage">
            <VoiceOrb
                phase=Signal::derive(move || phase.get())
                level=Signal::derive(move || level.get())
                error_flash=Signal::derive(move || error_flash.get())
            />
            <div class="voice-caption mt-8">{caption_view}</div>
            <div class="voice-hint mt-2">{mic_hint}</div>
            <button
                class="voice-hint mt-10 hover:text-text-primary transition-colors"
                on:click=move |_| voice_mode.open.set(false)
            >
                "✕ esc 退出"
            </button>
        </div>
    }
}

/// Open ONE fresh streaming session for the listening period that is about to
/// begin: start the mic's streaming tap, ask core for a `stream_id`, and — when
/// streaming is enabled — reset the live caption and install the frame sink
/// (which flushes everything the tap buffered while the open was in flight).
///
/// The tap starts BEFORE the RPC on purpose. Frames were previously produced
/// only after the open resolved, so the whole open latency was silent: the
/// 320 ms pre-roll ring covered a fast local server, but a cloud TLS handshake
/// or a busy self-hosted backend swallowed the first syllables of a fast
/// follow-up or post-barge-in utterance. The tap now buffers into
/// `Capture::pending` and `set_frame_sink` drains it in arrival order.
///
/// `stream_id == None` means BYO streaming is disabled in core; the speculative
/// tap is undone, `streaming_off` latches so later periods skip the pointless
/// round trip, and the batch path (`handle_utterance`) handles every utterance
/// unchanged. Called from the mic-open task (first entry) and the phase-watching
/// Effect (every re-entry into Listening). Each call gives the next utterance its
/// own backend decoder — the previous stream was closed at its UtteranceEnd.
fn start_listening_stream(
    session: Rc<MicSession>,
    dash: DashboardState,
    stream_id: StoredValue<Option<String>, LocalStorage>,
    stream_gen: StoredValue<u64, LocalStorage>,
    caption_state: RwSignal<CaptionState>,
    streaming_off: StoredValue<bool>,
) {
    // This session rides the batch path — either core reported streaming
    // disabled, or repeated backend failures / empty utterances struck it out.
    // Leave the slot None so `handle_utterance` owns every later turn.
    if streaming_off.get_value() {
        return;
    }
    // Defensive (guard A): a stale id should never survive (UtteranceEnd clears
    // it), but if one does, stop that backend stream and clear the slot before
    // opening a new one so we never leak a decoder or feed two streams at once.
    if let Some(stale) = stream_id.try_update_value(Option::take).flatten() {
        spawn_local(async move {
            let _ = dash
                .rpc_call(
                    "voice.stream.stop",
                    serde_json::json!({ "stream_id": stale }),
                )
                .await;
        });
    }
    // Guard B: claim a generation. Covers the case guard A can't — the slot is
    // still None because a PRIOR open's `voice.stream.start` hasn't returned
    // yet. Only the open whose generation is still the latest installs its
    // stream; an open that was superseded mid-flight closes the stream it just
    // opened instead of overwriting (and leaking) the newer one.
    let my_gen = stream_gen
        .try_update_value(|g| {
            *g += 1;
            *g
        })
        .unwrap_or(0);
    // Arm the tap NOW so the open latency isn't a silent hole (see the doc
    // comment). Frames land in `Capture::pending` until the sink is installed.
    session.start_streaming();
    spawn_local(async move {
        let resp = dash
            .rpc_call(
                "voice.stream.start",
                serde_json::json!({ "language": Option::<String>::None }),
            )
            .await;
        // Distinguish "core says streaming is off" (an explicit `stream_id: null`
        // that will never change within this session) from a transient RPC
        // failure (worth retrying on the next listening period).
        let disabled = resp
            .as_ref()
            .is_ok_and(|v| v.get("stream_id").is_none_or(serde_json::Value::is_null));
        let sid = resp.ok().and_then(|v| {
            v.get("stream_id")
                .and_then(|s| s.as_str())
                .map(str::to_string)
        });
        let still_current = stream_gen.try_with_value(|g| *g == my_gen).unwrap_or(false);
        if still_current {
            stream_id.set_value(sid.clone());
            match sid {
                Some(id) => {
                    // Fresh listening period → blank the live caption so the previous
                    // utterance's committed/interim text isn't shown under the new one.
                    caption_state.set(CaptionState::default());
                    // Installing the sink also flushes the frames captured while
                    // this open was in flight, in arrival order.
                    session.set_frame_sink(move |b64| {
                        let id = id.clone();
                        spawn_local(async move {
                            let _ = dash
                                .rpc_call(
                                    "voice.stream.audio",
                                    serde_json::json!({ "stream_id": id, "pcm_base64": b64 }),
                                )
                                .await;
                        });
                    });
                }
                // Batch fallback: undo the speculative tap (frees the pending
                // backlog) and, when core said so explicitly, latch it so later
                // periods don't re-pay the round trip per utterance.
                None => {
                    session.stop_streaming();
                    if disabled {
                        streaming_off.set_value(true);
                    }
                }
            }
        } else if let Some(id) = sid {
            // A newer Listening-entry superseded this open while its start RPC
            // was in flight. Do NOT install (no slot write, no caption reset) —
            // just close this orphaned decoder so it can't leak.
            let _ = dash
                .rpc_call("voice.stream.stop", serde_json::json!({ "stream_id": id }))
                .await;
        }
    });
}

/// Batch path: one utterance → stop capture, transcribe (WAV round-trip), then
/// hand the text to [`send_utterance`]. Runs only when streaming is disabled in
/// core (`voice.stream.start` returned `stream_id: null`). The streaming path
/// skips this transcribe and feeds `send_utterance` the committed text from the
/// live deltas directly.
#[allow(clippy::too_many_arguments)]
fn handle_utterance(
    session: Rc<MicSession>,
    dash: DashboardState,
    chat: ChatState,
    sessions: SessionMap,
    dispatch: impl Fn(VoiceEvent) + Clone + 'static,
    caption: RwSignal<Caption>,
    player: Rc<TtsPlayer>,
    splitter: Rc<RefCell<SentenceSplitter>>,
    speak_run: RwSignal<Option<String>>,
    speak_gen: Rc<RefCell<u64>>,
    active_run: StoredValue<Option<String>, LocalStorage>,
    consecutive_errors: Rc<RefCell<u32>>,
) {
    spawn_local(async move {
        let Some((base64, mime)) = session.take_segment_wav() else {
            dispatch(VoiceEvent::TranscribeFailed);
            return;
        };
        let resp = dash
            .rpc_call(
                "voice.transcribe",
                serde_json::json!({ "audio_base64": base64, "mime_type": mime }),
            )
            .await;
        let text = resp
            .ok()
            .and_then(|v| {
                v.get("text")
                    .and_then(|t| t.as_str())
                    .map(str::trim)
                    .map(str::to_string)
            })
            .filter(|t| !t.is_empty());
        let Some(text) = text else {
            let n = {
                let mut c = consecutive_errors.borrow_mut();
                *c += 1;
                *c
            };
            dispatch(VoiceEvent::TranscribeFailed);
            if n >= 3 {
                caption.set(Caption::Error("连续没听清——可以 esc 退出用文字".into()));
            }
            return;
        };
        *consecutive_errors.borrow_mut() = 0;
        caption.set(Caption::User(text.clone()));
        send_utterance(
            text, dash, chat, sessions, dispatch, player, splitter, speak_run, speak_gen,
            active_run,
        )
        .await;
    });
}

/// Send half shared by both paths (DRY): claim this turn's generation, reset the
/// TTS pipeline, push the user bubble, fire the run, and arm the speak Effect.
/// `text` is the final user utterance — the WAV transcript (batch) or the raw
/// committed streaming text (streaming). The caller owns showing the user
/// caption (batch sets `Caption::User`; streaming keeps the locked live line).
#[allow(clippy::too_many_arguments)]
async fn send_utterance(
    text: String,
    dash: DashboardState,
    chat: ChatState,
    sessions: SessionMap,
    dispatch: impl Fn(VoiceEvent) + Clone + 'static,
    player: Rc<TtsPlayer>,
    splitter: Rc<RefCell<SentenceSplitter>>,
    speak_run: RwSignal<Option<String>>,
    speak_gen: Rc<RefCell<u64>>,
    active_run: StoredValue<Option<String>, LocalStorage>,
) {
    dispatch(VoiceEvent::UtteranceSent);

    // A new utterance supersedes any run still executing (the user spoke again
    // while the previous reply was thinking/streaming) — cancel it server-side
    // so it stops burning tokens, mirroring the barge-in path.
    if let Some(old) = active_run.try_update_value(Option::take).flatten() {
        abort_run(dash, old);
    }

    // Claim this turn's generation. A later utterance or a barge-in bumps the
    // gen; the guarded assignment below then refuses to arm a run that a newer
    // turn has already superseded (last-writer-wins, even if our send finished
    // out of order).
    let my_gen = {
        let mut g = speak_gen.borrow_mut();
        *g += 1;
        *g
    };
    // Reset the TTS pipeline for the new turn before any stream arrives.
    player.reset();
    *splitter.borrow_mut() = SentenceSplitter::default();
    chat.push_user_message(&text);
    // Mirror the composer send's full context — agent binding, project root,
    // model pick, exec tier — so a voice turn behaves exactly like the same
    // words typed (previously all None: voice turns silently ignored the
    // session's agent/model/tier selections).
    let sk = chat.session_key.get_untracked();
    let aid = chat.agent_id.get_untracked();
    let room_project_id = chat.room_project_id.get_untracked();
    let pr = if room_project_id.is_some() {
        None
    } else {
        chat.active_project_root.get_untracked()
    };
    let mo = chat.selected_model.get_untracked();
    // The per-session dials, through the shared rule rather than a fourth
    // hand-rolled copy of it. This path used to spell the first-send-only test
    // inline — correctly, but a copy of a rule is a rule that will be updated in
    // three places and forgotten in the fourth, which is exactly what happened
    // when the plan phase joined: the two typed composers learned it and the two
    // voice paths would not have.
    let dials = session_dials_for_send(
        sk.is_some(),
        chat.session_exec_tier.get_untracked(),
        chat.session_mode.get_untracked(),
        chat.session_plan_phase.get_untracked(),
    );
    // Bind to the conversation active at send time (I1), same as the typed
    // path, so run events route to this conversation's bubble.
    let send_conv = sessions.active_conv();
    // voice_input=true: core arms VoiceModeLayer (spoken-reply prompt style)
    // and the `[voice]` low-TTFT model pin for this session turn.
    match ChatApi::send(
        &dash,
        &text,
        sk.as_deref(),
        vec![],
        aid.as_deref(),
        pr.as_deref(),
        room_project_id.as_deref(),
        mo.as_ref(),
        dials.exec_tier.as_deref(),
        dials.session_mode.as_deref(),
        dials.plan_phase.as_deref(),
        true,
    )
    .await
    {
        Ok(resp) => {
            // Only arm the pipeline if no newer utterance / barge-in superseded
            // us while send was in flight.
            if *speak_gen.borrow() == my_gen {
                if let Some(conv) = send_conv {
                    sessions.bind_run(&resp.run_id, conv, Some(&resp.session_key));
                }
                chat.session_key.set(Some(resp.session_key.clone()));
                // Do NOT open the assistant bubble here. The `run_accepted`
                // stream event already creates exactly one `assistant-{run}`
                // bubble (and `begin_step` stamps its iteration). Creating a
                // second one appended an empty, trailing shadow bubble that the
                // TTS Effect's `.rev().find` then tracked instead of the real
                // content bubble — so the reply was never spoken and the phase
                // froze at "thinking" (the round-2 no-audio bug). Also do NOT call
                // chat.mark_speak_run — the immersive session owns TTS itself;
                // marking the run would double-speak (the composer voice-loop
                // reader would also synthesize it).
                active_run.set_value(Some(resp.run_id.clone()));
                speak_run.set(Some(resp.run_id));
            } else {
                // Superseded mid-send: the accepted run will never be armed —
                // cancel it so it doesn't run headless.
                abort_run(dash, resp.run_id);
            }
        }
        Err(_) => {
            if *speak_gen.borrow() == my_gen {
                dispatch(VoiceEvent::RunFailed);
            }
        }
    }
}

/// Pure TTS-drive step shared by the streaming Effect: feed the current
/// accumulated bubble `content` and whether the run is still `streaming`, and
/// get back the sentences to enqueue plus whether the run is now finalized
/// (queue should drain → playback ends → phase returns to Listening). Kept free
/// of the wasm player so the round-2 "armed after the reply already finished"
/// path is host-testable.
fn drive_step(
    splitter: &mut SentenceSplitter,
    content: &str,
    streaming: bool,
) -> (Vec<String>, bool) {
    let mut out = splitter.push(content);
    let finalized = !streaming && !content.is_empty();
    if finalized {
        if let Some(tail) = splitter.finish_with(content) {
            out.push(tail);
        }
    }
    (out, finalized)
}

#[cfg(test)]
mod drive_tests {
    use super::*;

    #[test]
    fn incremental_stream_emits_then_finalizes() {
        let mut sp = SentenceSplitter::default();
        let full = "你好！我能听到你说话。有什么我可以帮你的吗？";
        // Mid-stream snapshot (still streaming): the first sentence is emitted,
        // the short trailing one is held back to merge forward.
        let (mid, fin) = drive_step(&mut sp, full, true);
        assert_eq!(mid, vec!["你好！我能听到你说话。"]);
        assert!(!fin);
        // Completion snapshot (same text, no longer streaming): the held tail is
        // flushed and the run is finalized so playback can drain.
        let (tail, fin) = drive_step(&mut sp, full, false);
        assert_eq!(tail, vec!["有什么我可以帮你的吗？"]);
        assert!(fin);
    }

    #[test]
    fn armed_after_reply_finished_emits_everything() {
        // Regression for "round-2 silent + stuck thinking": on a warm turn the
        // whole reply streams in and COMPLETES before the gate is armed, so the
        // Effect's first (reactive-arm) run sees the full, finished content in
        // one shot. That single drive must emit BOTH sentences and finalize —
        // nothing lost, playback drains, the phase leaves "thinking".
        let mut sp = SentenceSplitter::default();
        let full = "你好！我能听到你说话。有什么我可以帮你的吗？";
        let (out, fin) = drive_step(&mut sp, full, false);
        assert_eq!(
            out,
            vec!["你好！我能听到你说话。", "有什么我可以帮你的吗？"]
        );
        assert!(fin);
    }
}
