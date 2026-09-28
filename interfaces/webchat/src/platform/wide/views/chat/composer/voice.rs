//! Composer voice button — the full Panel voice loop.
//!
//! Press to record, press again to stop. The endpoint captures audio, the core
//! transcribes it (`voice.transcribe`), the transcript is sent as a normal chat
//! turn, and when the agent replies the core synthesizes speech that the
//! endpoint plays back (see [`super::super::voice_playback`]). Capture and
//! playback are endpoint I/O (R1/R6); STT/LLM/TTS live in the core.
//!
//! Two capture backends, picked transparently:
//!   - **native** (macOS): the unsigned `WKWebView` cannot reach `getUserMedia`,
//!     so we drive the Swift bridge's `AVFoundation` recorder via
//!     `voice.record_start` / `voice.record_stop`, which hands back base64 audio.
//!   - **browser** (Windows/Linux, signed builds): the Web `MediaRecorder` API.
//!     We try the native RPC first; the `NATIVE_AUDIO_UNAVAILABLE` sentinel is
//!     the signal to fall back to the browser path.

use std::cell::RefCell;
use std::rc::Rc;

use leptos::prelude::*;
use leptos::task::spawn_local;
use serde_json::Value;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::JsFuture;

use super::super::state::ChatState;
use crate::api::chat::{ChatApi, ChatAttachment};
use crate::context::DashboardState;
use crate::i18n::{t_string, use_i18n};
use crate::state::sessions::SessionMap;

/// Recording lifecycle. `Idle ↔ Recording`, then a one-shot `Transcribing`
/// while the audio round-trips through STT and the send.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum RecState {
    #[default]
    Idle,
    /// Mic requested, backend not yet confirmed (on macOS the native TCC
    /// permission dialog is up). Clicks are ignored so a duplicate
    /// `record_start` can't race the first one — see `recordStart` in the
    /// Swift bridge.
    Starting,
    Recording,
    Transcribing,
}

/// Non-reactive handles kept alive for one recording. For the browser backend
/// the `MediaRecorder`, `MediaStream`, and event closures must outlive the
/// recording; the `native` flag records which backend the in-flight capture is
/// using so the stop path knows whether to call the bridge RPC or stop the
/// `MediaRecorder`.
#[derive(Default)]
struct Recorder {
    native: bool,
    recorder: Option<web_sys::MediaRecorder>,
    stream: Option<web_sys::MediaStream>,
    chunks: Vec<web_sys::Blob>,
    // Kept alive until the next recording replaces them.
    _on_data: Option<Closure<dyn FnMut(web_sys::BlobEvent)>>,
    _on_stop: Option<Closure<dyn FnMut(web_sys::Event)>>,
}

type Handle = Rc<RefCell<Recorder>>;

/// Outcome of [`VoiceButtonState::on_pointer_up`]. The component
/// pattern-matches on this to decide whether to call `finish()`, open
/// voice mode, or noop.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum PointerUpAction {
    /// User released while `Recording` → emit finish (stop capture).
    Finish,
    /// Quick tap on the idle mic → open immersive voice mode.
    OpenVoiceMode,
    /// Ghost gesture, mid round-trip, or release after finish — noop.
    Ignored,
}

/// Pure state-machine core for the voice button. Lives in a
/// `StoredValue<RefCell<VoiceButtonState>>` inside the component so the
/// pointer-event closures (which Leptos cannot call from tests) can
/// drive the same transitions tests pin in `mod tests` below.
///
/// The component owns the `RwSignal<RecState>` for UI reactivity; the
/// helper [`apply_state`] closure keeps it mirrored with `rec_state`
/// here. The 450 ms long-press timer is owned by the component as a
/// gloo `TimeoutHandle` (cleared via `h.clear()`); this struct only
/// tracks whether one *should* be armed — the actual handle lives
/// alongside because `TimeoutHandle` cannot be constructed in tests.
///
/// Invariants pinned by `mod tests`:
///   - `on_pointer_down` in Idle arms the press-timer slot; the state
///     itself does not change until the timer fires.
///   - `fire_long_press_timer` in Idle+armed moves to Starting.
///   - `on_backend_ready` in Starting moves to Recording.
///   - `on_pointer_up` in Recording moves to Transcribing and emits
///     `PointerUpAction::Finish`.
///   - `on_pointer_up` in Starting emits `Ignored` (no ghost finish).
///   - `on_pointer_up` always clears the press-timer slot.
///   - `on_pointer_up` called twice in the same recording cycle emits
///     `Finish` at most once.
#[derive(Clone, Default)]
pub(super) struct VoiceButtonState {
    rec_state: RecState,
    press_timer_active: bool,
    long_press: bool,
    finish_emitted: bool,
    finish_calls: u32,
}

#[allow(
    dead_code,
    reason = "methods are exercised by `mod tests`; production paths route transitions through `apply_state`/`set_rec_state` instead."
)]
impl VoiceButtonState {
    pub(super) fn new() -> Self {
        Self::default()
    }

    // Read-only views used by the component and by tests.
    pub(super) fn rec_state(&self) -> RecState {
        self.rec_state
    }
    pub(super) fn press_timer_active(&self) -> bool {
        self.press_timer_active
    }
    pub(super) fn long_press(&self) -> bool {
        self.long_press
    }
    pub(super) fn finish_emitted(&self) -> bool {
        self.finish_emitted
    }
    pub(super) fn finish_calls(&self) -> u32 {
        self.finish_calls
    }

    /// Pointer down. In `Idle`, arm the press-timer slot (state does
    /// not change yet — the 450 ms timer decides tap-vs-hold). In any
    /// non-Idle state, do nothing: a press while already recording
    /// means "stop", not "start a new gesture".
    pub(super) fn on_pointer_down(&mut self) {
        self.long_press = false;
        if self.rec_state == RecState::Idle {
            self.press_timer_active = true;
            // New gesture cycle — clear the prior finish guard.
            self.finish_emitted = false;
        }
    }

    /// Long-press timer fired (the 450 ms timeout closure called this).
    /// Idle + armed → `Starting`, set `long_press` so the matching
    /// pointer_up knows this was a hold-to-dictate gesture. Returns
    /// `true` iff the transition happened (callers use this to gate
    /// `begin()`).
    pub(super) fn fire_long_press_timer(&mut self) -> bool {
        if self.rec_state == RecState::Idle && self.press_timer_active {
            self.long_press = true;
            self.rec_state = RecState::Starting;
            self.press_timer_active = false;
            self.finish_emitted = false;
            true
        } else {
            false
        }
    }

    /// Backend (native bridge or browser `MediaRecorder`) reports ready
    /// to record. `Starting` → `Recording`.
    pub(super) fn on_backend_ready(&mut self) {
        if self.rec_state == RecState::Starting {
            self.rec_state = RecState::Recording;
            self.finish_emitted = false;
        }
    }

    /// Backend reports failure (mic permission denied, decoder
    /// missing). `Starting` → `Idle`.
    pub(super) fn on_backend_failed(&mut self) {
        if self.rec_state == RecState::Starting {
            self.rec_state = RecState::Idle;
            self.press_timer_active = false;
            self.long_press = false;
            self.finish_emitted = false;
        }
    }

    /// External state transition (e.g. async transcription completion).
    /// Resets cycle-scoped flags when entering `Idle`.
    pub(super) fn set_rec_state(&mut self, new_state: RecState) {
        self.rec_state = new_state;
        if new_state == RecState::Idle {
            self.press_timer_active = false;
            self.long_press = false;
            self.finish_emitted = false;
        }
    }

    /// Pointer up. Always clears the press-timer slot (bug 2). Returns
    /// the action the caller should take.
    ///
    /// Three guarded paths correspond to the three documented bugs:
    ///   - `Starting` → `Ignored` (bug 1: ghost gesture — backend never
    ///     reached Recording, so no finish).
    ///   - `Recording` + first call → `Finish` (clears timer, sets
    ///     `finish_emitted`).
    ///   - `Recording` + second call → `Ignored` (bug 3: double-finish
    ///     guard for browser backend where `MediaRecorder::stop()`
    ///     does not synchronously advance state to `Transcribing`).
    pub(super) fn on_pointer_up(&mut self) -> PointerUpAction {
        // Bug 2: always clear the press-timer slot on every pointer_up.
        self.press_timer_active = false;
        match self.rec_state {
            // Bug 1: Starting — the gesture was a hold-to-dictate that
            // never made it to Recording (permission dialog dismissed,
            // mic unavailable). The button is already visually disabled
            // in this state, so we leave it as-is and emit no finish.
            RecState::Starting => PointerUpAction::Ignored,
            // Recording + not yet finished → emit finish.
            RecState::Recording if !self.finish_emitted => {
                self.finish_emitted = true;
                self.finish_calls += 1;
                self.long_press = false;
                self.rec_state = RecState::Transcribing;
                PointerUpAction::Finish
            }
            // Bug 3: Recording but finish already emitted → ignore.
            RecState::Recording => PointerUpAction::Ignored,
            RecState::Idle => {
                self.long_press = false;
                PointerUpAction::OpenVoiceMode
            }
            RecState::Transcribing => PointerUpAction::Ignored,
        }
    }
}

/// Mirrors a state transition into both the gesture state machine and
/// the UI-facing `RwSignal<RecState>`. The component's sync closures
/// and the async helpers (`begin`, `browser_start`, `finish`,
/// `transcribe_and_send`, `read_blob_then`) all route their transitions
/// through this so the testable core and the Leptos view stay aligned.
fn apply_state(
    core: &StoredValue<RefCell<VoiceButtonState>, LocalStorage>,
    state: &RwSignal<RecState>,
    new_state: RecState,
) {
    core.get_value().borrow_mut().set_rec_state(new_state);
    state.set(new_state);
}

/// Stop every track on a stream so the OS mic indicator clears promptly.
fn stop_tracks(stream: &web_sys::MediaStream) {
    let tracks = stream.get_tracks();
    for i in 0..tracks.length() {
        if let Ok(track) = tracks.get(i).dyn_into::<web_sys::MediaStreamTrack>() {
            track.stop();
        }
    }
}

/// Transcribe base64 audio via the core, then send the transcript as a chat
/// turn and register the run for spoken playback. Shared tail of both capture
/// backends — once we have bytes, the rest is backend-agnostic.
#[allow(clippy::too_many_arguments)]
fn transcribe_and_send(
    dash: DashboardState,
    chat: ChatState,
    sessions: SessionMap,
    base64: String,
    mime: String,
    core: StoredValue<RefCell<VoiceButtonState>, LocalStorage>,
    state: RwSignal<RecState>,
    error: RwSignal<Option<String>>,
) {
    let i18n = crate::i18n::use_i18n();
    apply_state(&core, &state, RecState::Transcribing);
    spawn_local(async move {
        let mut params = serde_json::json!({ "audio_base64": base64 });
        if !mime.is_empty() {
            params["mime_type"] = Value::String(mime);
        }
        match dash.rpc_call("voice.transcribe", params).await {
            Ok(val) => {
                let text = val
                    .get("text")
                    .and_then(|t| t.as_str())
                    .unwrap_or("")
                    .trim()
                    .to_string();
                if text.is_empty() {
                    apply_state(&core, &state, RecState::Idle);
                    return;
                }
                // Send as a normal chat turn (R4: pure I/O — the core runs the
                // agent loop). Mirror the composer's send inputs from ChatState.
                chat.push_user_message(&text);
                // Post-`.await`, so `crate::disposed_reads` applies. Every read
                // below is on root-owned `ChatState`, and this path deliberately
                // does NOT bail when the composer is gone: the user already
                // finished speaking, and navigating away mid-transcription must
                // not silently swallow the utterance. `.flatten()` therefore
                // keeps the behaviour byte-identical rather than short-circuiting.
                let sk = chat.session_key.try_get_untracked().flatten();
                let aid = chat.agent_id.try_get_untracked().flatten();
                let room_project_id = chat.room_project_id.try_get_untracked().flatten();
                let pr = if room_project_id.is_some() {
                    None
                } else {
                    chat.active_project_root.try_get_untracked().flatten()
                };
                let mo = chat.selected_model.try_get_untracked().flatten();
                // One rule for all four send paths. This path used to
                // re-implement the first-send-only carriage inline, which is
                // how it ended up carrying two of the dials and not the other
                // two once there were four.
                let dials = shared_ui_logic::state::session_dials_for_send(
                    sk.is_some(),
                    &chat.session_knobs(),
                );
                // Bind to the conversation active at send time (I1), same as the
                // typed-send path in `composer/mod.rs`.
                let send_conv = sessions.active_conv();
                match ChatApi::send(
                    &dash,
                    &text,
                    sk.as_deref(),
                    Vec::<ChatAttachment>::new(),
                    aid.as_deref(),
                    pr.as_deref(),
                    room_project_id.as_deref(),
                    mo.as_ref(),
                    &dials,
                    // Dictated speech: arm the voice-mode prompt layer + model pin.
                    true,
                )
                .await
                {
                    Ok(resp) => {
                        if let Some(conv) = send_conv {
                            sessions.bind_run(&resp.run_id, conv, Some(&resp.session_key));
                        }
                        chat.session_key.set(Some(resp.session_key));
                        // Speak this run's reply when it completes (events.rs).
                        chat.mark_speak_run(&resp.run_id);
                    }
                    Err(e) => error.set(Some(
                        crate::components::admin_refusal::settings_write_error(i18n, &e, |e| {
                            e.to_string()
                        }),
                    )),
                }
            }
            Err(e) => error.set(Some(
                crate::components::admin_refusal::settings_write_error(i18n, &e, |e| e.to_string()),
            )),
        }
        apply_state(&core, &state, RecState::Idle);
    });
}

/// Read a recorded blob to base64, then hand off to [`transcribe_and_send`].
#[allow(clippy::too_many_arguments)]
fn read_blob_then(
    dash: DashboardState,
    chat: ChatState,
    sessions: SessionMap,
    blob: web_sys::Blob,
    mime: String,
    core: StoredValue<RefCell<VoiceButtonState>, LocalStorage>,
    state: RwSignal<RecState>,
    error: RwSignal<Option<String>>,
) {
    let Ok(reader) = web_sys::FileReader::new() else {
        apply_state(&core, &state, RecState::Idle);
        return;
    };
    let reader_clone = reader.clone();
    let onload = Closure::wrap(Box::new(move || {
        let data_url = reader_clone
            .result()
            .ok()
            .and_then(|v| v.as_string())
            .unwrap_or_default();
        // data URL shape: "data:<mime>;base64,<payload>"
        let base64 = data_url.split(',').nth(1).unwrap_or("").to_string();
        if base64.is_empty() {
            apply_state(&core, &state, RecState::Idle);
            return;
        }
        transcribe_and_send(
            dash,
            chat,
            sessions,
            base64,
            mime.clone(),
            core,
            state,
            error,
        );
    }) as Box<dyn FnMut()>);
    reader.set_onload(Some(onload.as_ref().unchecked_ref()));
    // One-shot per recording; leaking a single small closure mirrors the
    // attachment reader (`attachments.rs`).
    onload.forget();
    let _ = reader.read_as_data_url(&blob);
}

/// Begin a recording. Tries the native bridge first; on the
/// `NATIVE_AUDIO_UNAVAILABLE` sentinel, falls back to browser capture.
fn begin(
    handle: Handle,
    dash: DashboardState,
    chat: ChatState,
    sessions: SessionMap,
    core: StoredValue<RefCell<VoiceButtonState>, LocalStorage>,
    state: RwSignal<RecState>,
    error: RwSignal<Option<String>>,
) {
    let i18n = crate::i18n::use_i18n();
    error.set(None);
    // Leave Idle synchronously so a second click while the permission dialog is
    // up is ignored (see `RecState::Starting`) rather than firing a duplicate
    // `record_start` that would race the first.
    apply_state(&core, &state, RecState::Starting);
    spawn_local(async move {
        match dash
            .rpc_call("voice.record_start", serde_json::json!({}))
            .await
        {
            Ok(_) => {
                handle.borrow_mut().native = true;
                apply_state(&core, &state, RecState::Recording);
            }
            Err(e) => {
                if e.contains("NATIVE_AUDIO_UNAVAILABLE") {
                    // No native helper (Windows/Linux, or signed macOS) → browser.
                    browser_start(handle, dash, chat, sessions, core, state, error);
                } else {
                    // A real failure (e.g. mic permission denied on macOS).
                    error.set(Some(
                        crate::components::admin_refusal::settings_write_error(i18n, &e, |e| {
                            e.to_string()
                        }),
                    ));
                    apply_state(&core, &state, RecState::Idle);
                }
            }
        }
    });
}

/// Browser capture backend: request the mic, wire `MediaRecorder` events, start.
fn browser_start(
    handle: Handle,
    dash: DashboardState,
    chat: ChatState,
    sessions: SessionMap,
    core: StoredValue<RefCell<VoiceButtonState>, LocalStorage>,
    state: RwSignal<RecState>,
    error: RwSignal<Option<String>>,
) {
    spawn_local(async move {
        let Some(nav) = web_sys::window().map(|w| w.navigator()) else {
            error.set(Some("Microphone unavailable".into()));
            apply_state(&core, &state, RecState::Idle);
            return;
        };
        let Ok(media_devices) = nav.media_devices() else {
            error.set(Some("Microphone not supported in this browser".into()));
            apply_state(&core, &state, RecState::Idle);
            return;
        };

        let constraints = web_sys::MediaStreamConstraints::new();
        let _ = js_sys::Reflect::set(
            constraints.as_ref(),
            &JsValue::from_str("audio"),
            &JsValue::TRUE,
        );
        let Ok(promise) = media_devices.get_user_media_with_constraints(&constraints) else {
            error.set(Some("Microphone access failed".into()));
            apply_state(&core, &state, RecState::Idle);
            return;
        };
        let stream: web_sys::MediaStream = match JsFuture::from(promise).await {
            Ok(s) => s.unchecked_into(),
            Err(_) => {
                error.set(Some("Microphone permission denied".into()));
                apply_state(&core, &state, RecState::Idle);
                return;
            }
        };

        let Ok(recorder) = web_sys::MediaRecorder::new_with_media_stream(&stream) else {
            stop_tracks(&stream);
            error.set(Some("Recorder init failed".into()));
            apply_state(&core, &state, RecState::Idle);
            return;
        };

        // dataavailable → collect chunks into the shared handle.
        let h_data = handle.clone();
        let on_data = Closure::wrap(Box::new(move |ev: web_sys::BlobEvent| {
            if let Some(blob) = ev.data() {
                h_data.borrow_mut().chunks.push(blob);
            }
        }) as Box<dyn FnMut(web_sys::BlobEvent)>);
        recorder.set_ondataavailable(Some(on_data.as_ref().unchecked_ref()));

        // stop → assemble the blob, release the mic, transcribe + send.
        let h_stop = handle.clone();
        let on_stop = Closure::wrap(Box::new(move |_ev: web_sys::Event| {
            let (blob, mime) = {
                let r = h_stop.borrow();
                let arr = js_sys::Array::new();
                for c in &r.chunks {
                    arr.push(c);
                }
                let mime = r
                    .recorder
                    .as_ref()
                    .map(web_sys::MediaRecorder::mime_type)
                    .unwrap_or_default();
                let blob = web_sys::Blob::new_with_blob_sequence(arr.as_ref()).ok();
                (blob, mime)
            };
            if let Some(stream) = h_stop.borrow().stream.clone() {
                stop_tracks(&stream);
            }
            match blob {
                Some(blob) => {
                    read_blob_then(dash, chat, sessions, blob, mime, core, state, error)
                }
                None => apply_state(&core, &state, RecState::Idle),
            }
        }) as Box<dyn FnMut(web_sys::Event)>);
        recorder.set_onstop(Some(on_stop.as_ref().unchecked_ref()));

        if recorder.start().is_err() {
            stop_tracks(&stream);
            error.set(Some("Recording failed to start".into()));
            apply_state(&core, &state, RecState::Idle);
            return;
        }

        {
            let mut r = handle.borrow_mut();
            r.native = false;
            r.chunks.clear();
            r.recorder = Some(recorder);
            r.stream = Some(stream);
            r._on_data = Some(on_data);
            r._on_stop = Some(on_stop);
        }
        apply_state(&core, &state, RecState::Recording);
    });
}

/// Stop the in-flight recording. Native → bridge `record_stop` RPC returns the
/// bytes; browser → stop the `MediaRecorder` (its `onstop` does the rest).
fn finish(
    handle: Handle,
    dash: DashboardState,
    chat: ChatState,
    sessions: SessionMap,
    core: StoredValue<RefCell<VoiceButtonState>, LocalStorage>,
    state: RwSignal<RecState>,
    error: RwSignal<Option<String>>,
) {
    let i18n = crate::i18n::use_i18n();
    let native = handle.borrow().native;
    if native {
        apply_state(&core, &state, RecState::Transcribing);
        let handle = handle;
        spawn_local(async move {
            handle.borrow_mut().native = false;
            match dash
                .rpc_call("voice.record_stop", serde_json::json!({}))
                .await
            {
                Ok(val) => {
                    let base64 = val
                        .get("audio_base64")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let mime = val
                        .get("mime_type")
                        .and_then(|v| v.as_str())
                        .unwrap_or("audio/mp4")
                        .to_string();
                    if base64.is_empty() {
                        apply_state(&core, &state, RecState::Idle);
                        return;
                    }
                    transcribe_and_send(
                        dash,
                        chat,
                        sessions,
                        base64,
                        mime,
                        core,
                        state,
                        error,
                    );
                }
                Err(e) => {
                    error.set(Some(
                        crate::components::admin_refusal::settings_write_error(i18n, &e, |e| {
                            e.to_string()
                        }),
                    ));
                    apply_state(&core, &state, RecState::Idle);
                }
            }
        });
    } else {
        let rec = handle.borrow().recorder.clone();
        match rec {
            Some(rec) => {
                let _ = rec.stop();
            }
            None => apply_state(&core, &state, RecState::Idle),
        }
    }
}

/// Mic toggle button mounted beside the composer paperclip. A full voice loop:
/// record → STT → send → spoken reply.
#[component]
pub(super) fn VoiceInputButton(
    /// Disable while a message send is in flight.
    #[prop(into)]
    disabled: Signal<bool>,
) -> impl IntoView {
    let dashboard = expect_context::<DashboardState>();
    let chat = expect_context::<ChatState>();
    let sessions = expect_context::<SessionMap>();
    let voice_mode = expect_context::<crate::views::voice::VoiceMode>();
    let i18n = use_i18n();

    let state = RwSignal::new(RecState::Idle);
    let error = RwSignal::new(Option::<String>::None);
    // The recorder handle is `Rc` (`!Send`) and is reached from several closures
    // (the deferred long-press timer via `start_dictation`, pointerup, click),
    // so it lives in a LocalStorage arena slot whose `Copy` handle each closure
    // captures freely — a bare `Rc` could only be moved into one closure.
    let handle: StoredValue<Handle, LocalStorage> =
        StoredValue::new_local(Rc::new(RefCell::new(Recorder::default())));

    // Dual-gesture state. A press starts a 450 ms timer: if it fires first the
    // gesture is a long-press → the original dictation flow (`begin`); a
    // pointerup before then is a tap → enter immersive voice mode. The state
    // machine (press-timer slot, `long_press` flag, `finish_emitted` guard,
    // and `finish_calls` counter) lives in [`VoiceButtonState`]; this
    // component owns one via a `StoredValue` so the same transitions tests
    // pin in `voice::tests` are what production gestures drive. The actual
    // gloo `TimeoutHandle` for `set_timeout_with_handle` lives alongside
    // because `TimeoutHandle` cannot be constructed in tests — only its
    // presence is tracked by the core. `pointer_used` stays local: it is a
    // pure UI-event demultiplexer between mouse pointer events (which fire
    // `pointerup` first and own the gesture) and keyboard activations
    // (which fire only `click`).
    let core: StoredValue<RefCell<VoiceButtonState>, LocalStorage> =
        StoredValue::new_local(RefCell::new(VoiceButtonState::new()));
    let press_timer: StoredValue<Option<TimeoutHandle>, LocalStorage> =
        StoredValue::new_local(None);
    let pointer_used = StoredValue::new(false);

    // Long-press dictation entry (the original Idle→record path), deferred
    // 450 ms. Captures only `Copy` handles, so it is itself `Copy` and can be
    // moved into a fresh timeout closure on every pointerdown.
    let start_dictation = move || {
        if disabled.get_untracked() {
            return;
        }
        let started = core.get_value().borrow_mut().fire_long_press_timer();
        if started {
            // Mirror the core's transition to the UI signal and kick off the
            // capture backend. `begin` itself will call `on_backend_ready`
            // via `apply_state` when the bridge confirms.
            state.set(core.get_value().borrow().rec_state());
            begin(
                handle.get_value(),
                dashboard,
                chat,
                sessions,
                core,
                state,
                error,
            );
        }
    };

    let on_pointer_down = move |_: web_sys::PointerEvent| {
        if disabled.get_untracked() {
            return;
        }
        pointer_used.set_value(true);
        let cell = core.get_value();
        let should_arm_timer = {
            let mut c = cell.borrow_mut();
            let was_idle = c.rec_state() == RecState::Idle;
            c.on_pointer_down();
            // Only arm a fresh 450 ms timer if we just armed the slot in
            // Idle — a press while already recording means "stop", not
            // "start a new gesture".
            was_idle && c.rec_state() == RecState::Idle && c.press_timer_active()
        };
        state.set(core.get_value().borrow().rec_state());
        if should_arm_timer {
            if let Ok(h) = set_timeout_with_handle(
                start_dictation,
                std::time::Duration::from_millis(450),
            ) {
                press_timer.set_value(Some(h));
            }
        }
    };

    let on_pointer_up = move |_: web_sys::PointerEvent| {
        // Clear the gloo timer handle first — bug 2 fix is the core's
        // unconditional `press_timer_active = false`, this is the matching
        // I/O side that actually cancels the queued macrotask.
        if let Some(h) = press_timer.try_update_value(Option::take).flatten() {
            h.clear();
        }
        if disabled.get_untracked() {
            return;
        }
        let action = core.get_value().borrow_mut().on_pointer_up();
        state.set(core.get_value().borrow().rec_state());
        match action {
            PointerUpAction::Finish => {
                finish(handle.get_value(), dashboard, chat, sessions, core, state, error);
            }
            PointerUpAction::OpenVoiceMode => {
                voice_mode.open.set(true);
            }
            PointerUpAction::Ignored => {}
        }
    };

    // Keyboard activation fallback: a `<button>` reached via Enter/Space fires
    // `click` but never pointer events. Mouse clicks DO fire pointerup first
    // (which owns the gesture and sets `pointer_used`), so we swallow those
    // here to avoid double-handling.
    let on_click = move |_: web_sys::MouseEvent| {
        if pointer_used.try_update_value(|u| std::mem::replace(u, false)) == Some(true) {
            return;
        }
        if disabled.get_untracked() {
            return;
        }
        let action = core.get_value().borrow_mut().on_pointer_up();
        state.set(core.get_value().borrow().rec_state());
        match action {
            PointerUpAction::Finish => {
                finish(handle.get_value(), dashboard, chat, sessions, core, state, error);
            }
            PointerUpAction::OpenVoiceMode => {
                voice_mode.open.set(true);
            }
            PointerUpAction::Ignored => {}
        }
    };

    let title = move || {
        let key = match state.get() {
            // Idle: the mini orb is the immersive-mode entry; the hint spells
            // out both gestures (tap = immersive, hold = dictate-to-text).
            RecState::Idle => t_string!(i18n, chat.voice_idle_hint).to_string(),
            RecState::Starting => t_string!(i18n, chat.voice_start).to_string(),
            RecState::Recording => t_string!(i18n, chat.voice_stop).to_string(),
            RecState::Transcribing => t_string!(i18n, chat.voice_transcribing).to_string(),
        };
        match error.get() {
            Some(e) => format!("{key} — {e}"),
            None => key,
        }
    };

    let button_class = move || {
        let base = "p-1.5 rounded-lg transition-colors flex-shrink-0 ";
        match state.get() {
            RecState::Recording => {
                format!("{base}text-danger bg-danger/15 hover:bg-danger/25 animate-pulse")
            }
            RecState::Starting | RecState::Transcribing => format!("{base}text-primary"),
            RecState::Idle if error.get().is_some() => {
                format!("{base}text-danger hover:text-text-primary hover:bg-surface-sunken")
            }
            RecState::Idle => {
                format!("{base}text-text-tertiary hover:text-text-primary hover:bg-surface-sunken")
            }
        }
    };

    view! {
        <div class="relative flex-shrink-0">
        // Visible failure surface: capture/STT/send rejections (permission
        // denied, no provider) otherwise live only in the tooltip, which reads
        // as "nothing happened". Click the bubble to dismiss.
        <Show when=move || error.get().is_some()>
            <div
                class="absolute bottom-full left-0 mb-1 max-w-[220px] px-2 py-1 rounded-md
                       bg-danger text-white text-xs leading-snug shadow-lg cursor-pointer z-50"
                on:click=move |_| error.set(None)
            >
                {move || error.get().unwrap_or_default()}
            </div>
        </Show>
        <button
            class=button_class
            title=title
            disabled=move || disabled.get()
                || state.get() == RecState::Starting
                || state.get() == RecState::Transcribing
            on:pointerdown=on_pointer_down
            on:pointerup=on_pointer_up
            on:click=on_click
        >
            {move || match state.get() {
                // Spinner while preparing the mic (permission dialog) or
                // transcribing.
                RecState::Starting | RecState::Transcribing => view! {
                    <svg xmlns="http://www.w3.org/2000/svg" class="w-5 h-5 animate-spin"
                         viewBox="0 0 24 24" fill="none">
                        <circle class="opacity-25" cx="12" cy="12" r="10"
                                stroke="currentColor" stroke-width="3" />
                        <path class="opacity-75" fill="currentColor"
                              d="M4 12a8 8 0 0 1 8-8V0C5.4 0 0 5.4 0 12h4z" />
                    </svg>
                }.into_any(),
                // Idle → the mini stream-flow orb, signalling that a tap drops
                // into immersive voice mode (long-press still dictates).
                RecState::Idle => view! {
                    <div class="voice-orb voice-orb--mini voice-orb--listening">
                        <div class="voice-orb-flow"></div>
                        <div class="voice-orb-sheen"></div>
                    </div>
                }.into_any(),
                // Recording → the original mic glyph (button_class pulses it red),
                // keeping the long-press dictation flow visually unambiguous.
                RecState::Recording => view! {
                    <svg xmlns="http://www.w3.org/2000/svg" class="w-5 h-5"
                         viewBox="0 0 20 20" fill="currentColor">
                        <path d="M10 2a2.5 2.5 0 0 0-2.5 2.5v5a2.5 2.5 0 0 0 5 0v-5A2.5 2.5 0 0 0 10 2Z" />
                        <path d="M5.5 9.5a.75.75 0 0 0-1.5 0 6 6 0 0 0 5.25 5.954V17.5a.75.75 0 0 0 1.5 0v-2.046A6 6 0 0 0 16 9.5a.75.75 0 0 0-1.5 0 4.5 4.5 0 0 1-9 0Z" />
                    </svg>
                }.into_any(),
            }}
        </button>
        </div>
    }
}

#[cfg(test)]
mod tests {
    //! State-machine tests for [`VoiceButtonState`]. Each test pins ONE
    //! gesture invariant so a regression surfaces as a single failing
    //! assertion tied to the exact transition that broke. The component
    //! uses this struct via `StoredValue<RefCell<VoiceButtonState>>` and
    //! routes every `state.set(...)` through [`apply_state`]; these tests
    //! are therefore the same code paths production gestures drive.

    use super::*;

    fn fresh() -> VoiceButtonState {
        VoiceButtonState::new()
    }

    /// Idle + pointer_down arms the press-timer slot, then the simulated
    /// long-press timer firing moves Idle → Starting. A pointer_down
    /// alone does not transition state — the 450 ms timer is what
    /// distinguishes tap from hold.
    #[test]
    fn idle_pointer_down_then_timer_fire_moves_to_starting() {
        let mut s = fresh();
        assert_eq!(s.rec_state(), RecState::Idle);
        assert!(!s.press_timer_active());

        s.on_pointer_down();
        // The press armed a timer; state still Idle (timer decides tap vs hold).
        assert_eq!(s.rec_state(), RecState::Idle);
        assert!(s.press_timer_active(), "Idle pointer_down must arm the press-timer slot");

        // Timer fired (450 ms later) — the gesture was a long press.
        assert!(s.fire_long_press_timer(), "timer fired in Idle+armed must transition");
        assert_eq!(s.rec_state(), RecState::Starting);
        assert!(s.long_press(), "long_press flag should be set after timer fires");
        assert!(!s.press_timer_active(), "timer slot consumed after fire");
    }

    /// After the bridge (or browser MediaRecorder) confirms the
    /// recording started, Starting → Recording. on_backend_ready on any
    /// other state is a no-op (it's the bridge callback arrival, not a
    /// user gesture).
    #[test]
    fn starting_backend_ready_moves_to_recording() {
        let mut s = fresh();
        s.on_pointer_down();
        s.fire_long_press_timer();
        assert_eq!(s.rec_state(), RecState::Starting);

        s.on_backend_ready();
        assert_eq!(s.rec_state(), RecState::Recording);

        // Idempotent — calling it again does nothing harmful.
        s.on_backend_ready();
        assert_eq!(s.rec_state(), RecState::Recording);
    }

    /// Recording + pointer_up → Transcribing and emits Finish. This is
    /// the happy path: a long press, hold to dictate, release.
    #[test]
    fn recording_pointer_up_transitions_to_transcribing_and_emits_finish() {
        let mut s = fresh();
        s.on_pointer_down();
        s.fire_long_press_timer();
        s.on_backend_ready();
        assert_eq!(s.rec_state(), RecState::Recording);

        let action = s.on_pointer_up();
        assert_eq!(action, PointerUpAction::Finish);
        assert_eq!(s.rec_state(), RecState::Transcribing);
        assert_eq!(s.finish_calls(), 1);
        assert!(s.finish_emitted(), "finish_emitted should guard any second call");
    }

    /// Bug 1 fix: pointer_up while still in Starting (mic permission
    /// dialog dismissed, `getUserMedia` rejected) must NOT emit a
    /// finish. The backend never reached Recording, so there is no
    /// in-flight capture to stop — any `recorder.stop()` call here
    /// would race the bridge's `record_start` that has not even
    /// resolved yet.
    #[test]
    fn starting_pointer_up_does_not_emit_finish() {
        let mut s = fresh();
        s.on_pointer_down();
        s.fire_long_press_timer();
        assert_eq!(s.rec_state(), RecState::Starting);

        let action = s.on_pointer_up();
        assert_ne!(action, PointerUpAction::Finish, "ghost finish on Starting state");
        assert_eq!(s.finish_calls(), 0, "no finish should have been emitted");
    }

    /// Bug 2 fix: every pointer_up clears the press-timer slot,
    /// regardless of which state the gesture ended in. Without this
    /// guard, a pointer_up fired while the timer is still armed would
    /// leak the `TimeoutHandle` (and fire `start_dictation` 450 ms
    /// later, opening voice mode on an already-resolved gesture).
    #[test]
    fn pointer_up_always_clears_press_timer_slot() {
        let mut s = fresh();
        s.on_pointer_down();
        assert!(s.press_timer_active(), "press-timer slot should be armed after Idle pointer_down");

        // Pre-timer release — gesture was a tap, timer still queued.
        let action = s.on_pointer_up();
        assert_eq!(action, PointerUpAction::OpenVoiceMode);
        assert!(!s.press_timer_active(), "press-timer slot leaked after pointer_up");
    }

    /// Bug 3 fix: a double release (a quick double-tap on the mic
    /// glyph, or two pointerup events before the browser backend's
    /// `MediaRecorder::stop()` has synchronised state to
    /// `Transcribing`) must emit Finish once and only once. The
    /// `finish_emitted` guard catches the second call even when the
    /// state signal has not yet caught up.
    #[test]
    fn double_pointer_up_emits_only_one_finish() {
        let mut s = fresh();
        s.on_pointer_down();
        s.fire_long_press_timer();
        s.on_backend_ready();
        assert_eq!(s.rec_state(), RecState::Recording);

        // First release — the recording finishes.
        let first = s.on_pointer_up();
        assert_eq!(first, PointerUpAction::Finish);
        let n_after_first = s.finish_calls();
        assert_eq!(n_after_first, 1);

        // Second release — already finished (state is Transcribing now,
        // and finish_emitted is also set); must be ignored.
        let second = s.on_pointer_up();
        assert_ne!(second, PointerUpAction::Finish, "double finish emitted");
        assert_eq!(s.finish_calls(), n_after_first, "finish_calls must not advance on second release");

        // And even if state were still Recording (simulate the browser
        // backend race where `MediaRecorder::stop()` has not yet hit
        // the state signal), the `finish_emitted` guard still
        // suppresses the second emit.
        let mut s2 = fresh();
        s2.on_pointer_down();
        s2.fire_long_press_timer();
        s2.on_backend_ready();
        assert_eq!(s2.on_pointer_up(), PointerUpAction::Finish);
        // Force the core back to Recording to simulate the browser
        // race window between `rec.stop()` and `onstop`.
        s2.set_rec_state(RecState::Recording);
        assert_eq!(s2.on_pointer_up(), PointerUpAction::Ignored, "finish_emitted guard must block re-entry");
    }
}
