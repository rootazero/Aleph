//! Composer voice button — browser microphone capture → STT transcription.
//!
//! Click to record, click again to stop. We capture audio via the Web
//! `MediaRecorder` API, base64-encode the blob (reusing the same
//! `FileReader → data-URL` dance as the attachment reader), and post it to
//! the `voice.transcribe` gateway RPC. The returned text is appended to the
//! composer draft for the user to review before sending — the panel stays a
//! pure I/O surface (R4); all transcription happens in the Rust core.

use std::cell::RefCell;
use std::rc::Rc;

use leptos::prelude::*;
use leptos::task::spawn_local;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::JsFuture;

use crate::context::DashboardState;
use crate::i18n::*;

/// Recording lifecycle. `Idle ↔ Recording`, then a one-shot `Transcribing`
/// while the blob round-trips through the STT RPC.
#[derive(Clone, Copy, PartialEq, Eq)]
enum RecState {
    Idle,
    Recording,
    Transcribing,
}

/// Non-reactive handles kept alive for one recording. The `MediaRecorder`,
/// `MediaStream`, and their event closures must outlive the recording, and the
/// chunks are shared between the `dataavailable` and `stop` callbacks.
#[derive(Default)]
struct Recorder {
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

/// Read a recorded blob to base64 and POST it to `voice.transcribe`, appending
/// the transcript to the composer draft on success.
fn transcribe_blob(
    dash: DashboardState,
    blob: web_sys::Blob,
    mime: String,
    input_text: RwSignal<String>,
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
        let mime = mime.clone();
        spawn_local(async move {
            let mut params = serde_json::json!({ "audio_base64": base64 });
            if !mime.is_empty() {
                params["mime_type"] = serde_json::Value::String(mime);
            }
            match dash.rpc_call("voice.transcribe", params).await {
                Ok(val) => {
                    let text = val
                        .get("text")
                        .and_then(|t| t.as_str())
                        .unwrap_or("")
                        .trim()
                        .to_string();
                    if !text.is_empty() {
                        input_text.update(|cur| {
                            if cur.trim().is_empty() {
                                *cur = text;
                            } else {
                                if !cur.ends_with(' ') {
                                    cur.push(' ');
                                }
                                cur.push_str(&text);
                            }
                        });
                    }
                }
                Err(e) => error.set(Some(e)),
            }
            state.set(RecState::Idle);
        });
    }) as Box<dyn FnMut()>);
    reader.set_onload(Some(onload.as_ref().unchecked_ref()));
    // One-shot per recording; leaking a single small closure is acceptable and
    // mirrors the attachment reader (`attachments.rs`).
    onload.forget();
    let _ = reader.read_as_data_url(&blob);
}

/// Begin a recording: request the mic, wire recorder events, and start.
fn start_recording(
    handle: Handle,
    dash: DashboardState,
    input_text: RwSignal<String>,
    state: RwSignal<RecState>,
    error: RwSignal<Option<String>>,
) {
    error.set(None);
    spawn_local(async move {
        let Some(nav) = web_sys::window().map(|w| w.navigator()) else {
            error.set(Some("Microphone unavailable".into()));
            return;
        };
        let Ok(media_devices) = nav.media_devices() else {
            error.set(Some("Microphone not supported in this browser".into()));
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
            return;
        };
        let stream: web_sys::MediaStream = match JsFuture::from(promise).await {
            Ok(s) => s.unchecked_into(),
            Err(_) => {
                error.set(Some("Microphone permission denied".into()));
                return;
            }
        };

        let Ok(recorder) = web_sys::MediaRecorder::new_with_media_stream(&stream) else {
            stop_tracks(&stream);
            error.set(Some("Recorder init failed".into()));
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

        // stop → assemble the blob, release the mic, kick off transcription.
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
                    .map(|rec| rec.mime_type())
                    .unwrap_or_default();
                let blob = web_sys::Blob::new_with_blob_sequence(arr.as_ref()).ok();
                (blob, mime)
            };
            if let Some(stream) = h_stop.borrow().stream.clone() {
                stop_tracks(&stream);
            }
            match blob {
                Some(blob) => {
                    state.set(RecState::Transcribing);
                    transcribe_blob(dash, blob, mime, input_text, state, error);
                }
                None => state.set(RecState::Idle),
            }
        }) as Box<dyn FnMut(web_sys::Event)>);
        recorder.set_onstop(Some(on_stop.as_ref().unchecked_ref()));

        if recorder.start().is_err() {
            stop_tracks(&stream);
            error.set(Some("Recording failed to start".into()));
            return;
        }

        {
            let mut r = handle.borrow_mut();
            r.chunks.clear();
            r.recorder = Some(recorder);
            r.stream = Some(stream);
            r._on_data = Some(on_data);
            r._on_stop = Some(on_stop);
        }
        state.set(RecState::Recording);
    });
}

/// Mic toggle button mounted beside the composer paperclip.
#[component]
pub(super) fn VoiceInputButton(
    /// Composer draft. Transcribed text is appended here for review.
    input_text: RwSignal<String>,
    /// Disable while a message send is in flight.
    #[prop(into)] disabled: Signal<bool>,
) -> impl IntoView {
    let dashboard = expect_context::<DashboardState>();
    let i18n = use_i18n();

    let state = RwSignal::new(RecState::Idle);
    let error = RwSignal::new(Option::<String>::None);
    let handle: Handle = Rc::new(RefCell::new(Recorder::default()));

    let on_click = move |_: web_sys::MouseEvent| {
        if disabled.get_untracked() {
            return;
        }
        match state.get_untracked() {
            RecState::Idle => start_recording(
                handle.clone(),
                dashboard,
                input_text,
                state,
                error,
            ),
            RecState::Recording => {
                let rec = handle.borrow().recorder.clone();
                match rec {
                    Some(rec) => {
                        let _ = rec.stop();
                    }
                    None => state.set(RecState::Idle),
                }
            }
            // Busy round-tripping — ignore extra clicks.
            RecState::Transcribing => {}
        }
    };

    let title = move || {
        let key = match state.get() {
            RecState::Idle => t_string!(i18n, chat.voice_start),
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
            RecState::Transcribing => format!("{base}text-primary"),
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
        // Visible failure surface: getUserMedia rejections (permission denied,
        // unsupported webview) otherwise live only in the button tooltip, which
        // reads as "nothing happened". Click the bubble to dismiss.
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
            disabled=move || disabled.get() || state.get() == RecState::Transcribing
            on:click=on_click
        >
            {move || match state.get() {
                // Spinner while transcribing.
                RecState::Transcribing => view! {
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
