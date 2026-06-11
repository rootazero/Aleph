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
use crate::i18n::{t_string, I18nLocaleTrait, use_i18n};

/// Recording lifecycle. `Idle ↔ Recording`, then a one-shot `Transcribing`
/// while the audio round-trips through STT and the send.
#[derive(Clone, Copy, PartialEq, Eq)]
enum RecState {
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
fn transcribe_and_send(
    dash: DashboardState,
    chat: ChatState,
    base64: String,
    mime: String,
    state: RwSignal<RecState>,
    error: RwSignal<Option<String>>,
) {
    state.set(RecState::Transcribing);
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
                    state.set(RecState::Idle);
                    return;
                }
                // Send as a normal chat turn (R4: pure I/O — the core runs the
                // agent loop). Mirror the composer's send inputs from ChatState.
                chat.push_user_message(&text);
                let sk = chat.session_key.get_untracked();
                let aid = chat.agent_id.get_untracked();
                let pr = chat.active_project_root.get_untracked();
                let mo = chat.selected_model.get_untracked();
                match ChatApi::send(
                    &dash,
                    &text,
                    sk.as_deref(),
                    Vec::<ChatAttachment>::new(),
                    aid.as_deref(),
                    pr.as_deref(),
                    mo.as_ref(),
                )
                .await
                {
                    Ok(resp) => {
                        chat.session_key.set(Some(resp.session_key));
                        // Speak this run's reply when it completes (events.rs).
                        chat.mark_speak_run(&resp.run_id);
                    }
                    Err(e) => error.set(Some(e)),
                }
            }
            Err(e) => error.set(Some(e)),
        }
        state.set(RecState::Idle);
    });
}

/// Read a recorded blob to base64, then hand off to [`transcribe_and_send`].
fn read_blob_then(
    dash: DashboardState,
    chat: ChatState,
    blob: web_sys::Blob,
    mime: String,
    state: RwSignal<RecState>,
    error: RwSignal<Option<String>>,
) {
    let Ok(reader) = web_sys::FileReader::new() else {
        state.set(RecState::Idle);
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
            state.set(RecState::Idle);
            return;
        }
        transcribe_and_send(dash, chat, base64, mime.clone(), state, error);
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
    state: RwSignal<RecState>,
    error: RwSignal<Option<String>>,
) {
    error.set(None);
    // Leave Idle synchronously so a second click while the permission dialog is
    // up is ignored (see `RecState::Starting`) rather than firing a duplicate
    // `record_start` that would race the first.
    state.set(RecState::Starting);
    spawn_local(async move {
        match dash
            .rpc_call("voice.record_start", serde_json::json!({}))
            .await
        {
            Ok(_) => {
                handle.borrow_mut().native = true;
                state.set(RecState::Recording);
            }
            Err(e) => {
                if e.contains("NATIVE_AUDIO_UNAVAILABLE") {
                    // No native helper (Windows/Linux, or signed macOS) → browser.
                    browser_start(handle, dash, chat, state, error);
                } else {
                    // A real failure (e.g. mic permission denied on macOS).
                    error.set(Some(e));
                    state.set(RecState::Idle);
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
    state: RwSignal<RecState>,
    error: RwSignal<Option<String>>,
) {
    spawn_local(async move {
        let Some(nav) = web_sys::window().map(|w| w.navigator()) else {
            error.set(Some("Microphone unavailable".into()));
            state.set(RecState::Idle);
            return;
        };
        let Ok(media_devices) = nav.media_devices() else {
            error.set(Some("Microphone not supported in this browser".into()));
            state.set(RecState::Idle);
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
            state.set(RecState::Idle);
            return;
        };
        let stream: web_sys::MediaStream = match JsFuture::from(promise).await {
            Ok(s) => s.unchecked_into(),
            Err(_) => {
                error.set(Some("Microphone permission denied".into()));
                state.set(RecState::Idle);
                return;
            }
        };

        let Ok(recorder) = web_sys::MediaRecorder::new_with_media_stream(&stream) else {
            stop_tracks(&stream);
            error.set(Some("Recorder init failed".into()));
            state.set(RecState::Idle);
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
                Some(blob) => read_blob_then(dash, chat, blob, mime, state, error),
                None => state.set(RecState::Idle),
            }
        }) as Box<dyn FnMut(web_sys::Event)>);
        recorder.set_onstop(Some(on_stop.as_ref().unchecked_ref()));

        if recorder.start().is_err() {
            stop_tracks(&stream);
            error.set(Some("Recording failed to start".into()));
            state.set(RecState::Idle);
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
        state.set(RecState::Recording);
    });
}

/// Stop the in-flight recording. Native → bridge `record_stop` RPC returns the
/// bytes; browser → stop the `MediaRecorder` (its `onstop` does the rest).
fn finish(
    handle: Handle,
    dash: DashboardState,
    chat: ChatState,
    state: RwSignal<RecState>,
    error: RwSignal<Option<String>>,
) {
    let native = handle.borrow().native;
    if native {
        state.set(RecState::Transcribing);
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
                        state.set(RecState::Idle);
                        return;
                    }
                    transcribe_and_send(dash, chat, base64, mime, state, error);
                }
                Err(e) => {
                    error.set(Some(e));
                    state.set(RecState::Idle);
                }
            }
        });
    } else {
        let rec = handle.borrow().recorder.clone();
        match rec {
            Some(rec) => {
                let _ = rec.stop();
            }
            None => state.set(RecState::Idle),
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
    let i18n = use_i18n();

    let state = RwSignal::new(RecState::Idle);
    let error = RwSignal::new(Option::<String>::None);
    let handle: Handle = Rc::new(RefCell::new(Recorder::default()));

    let on_click = move |_: web_sys::MouseEvent| {
        if disabled.get_untracked() {
            return;
        }
        match state.get_untracked() {
            RecState::Idle => begin(handle.clone(), dashboard, chat, state, error),
            RecState::Recording => finish(handle.clone(), dashboard, chat, state, error),
            // Starting (mic dialog up) / Transcribing (round-tripping) — ignore
            // extra clicks so we don't fire a duplicate record_start/stop.
            RecState::Starting | RecState::Transcribing => {}
        }
    };

    let title = move || {
        let key = match state.get() {
            RecState::Idle => t_string!(i18n, chat.voice_start),
            RecState::Starting => t_string!(i18n, chat.voice_start),
            RecState::Recording => t_string!(i18n, chat.voice_stop),
            RecState::Transcribing => t_string!(i18n, chat.voice_transcribing),
        };
        match error.get() {
            Some(e) => format!("{key} — {e}"),
            None => key.to_string(),
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
                // Microphone glyph for idle + recording (recording pulses red).
                _ => view! {
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
