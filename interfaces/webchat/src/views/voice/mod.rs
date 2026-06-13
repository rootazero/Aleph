//! Immersive voice mode — full-screen overlay hosting the voice session loop.
//!
//! Wires the pure cores (VAD / sentence splitter / phase machine) to the wasm
//! audio glue (mic capture + sequential TTS) and the chat send/stream pipeline.
//! The overlay sits on top of the always-mounted `ChatView`, so the
//! `stream.*` subscription that feeds `chat.messages` keeps running underneath.

pub(crate) mod audio;
pub(crate) mod machine;
pub(crate) mod orb;
pub(crate) mod sentence;
pub(crate) mod vad;

use std::cell::RefCell;
use std::rc::Rc;

use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::api::chat::ChatApi;
use crate::context::DashboardState;
use crate::views::chat::ChatState;
use audio::{MicSession, TtsPlayer};
use machine::{on_event, Action, VoiceEvent, VoicePhase};
use orb::VoiceOrb;
use sentence::SentenceSplitter;
use vad::{vad_step, VadConfig, VadEvent, VadState};

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
    let voice_mode = expect_context::<VoiceMode>();

    let phase = RwSignal::new(VoicePhase::Listening);
    let level = RwSignal::new(0.0_f64);
    let caption = RwSignal::new(Caption::Idle);
    let error_flash = RwSignal::new(false);
    let mic_denied = RwSignal::new(false);

    // The mic and the interval handle outlive their writer (the async mic-open
    // task) and must be reachable from `on_cleanup`, whose closure bound is
    // `Send + Sync`. A plain `Rc` is `!Send`, so they live in `LocalStorage`
    // arena slots instead — the `StoredValue` handle is `Copy + Send + Sync`
    // for any payload (wasm is single-threaded; the `Send + Sync` bound on the
    // handle is a type-system formality, not real cross-thread access).
    let mic: StoredValue<Option<Rc<MicSession>>, LocalStorage> = StoredValue::new_local(None);
    let vad = Rc::new(RefCell::new(VadState::default()));
    let vad_cfg = VadConfig::default();
    let splitter = Rc::new(RefCell::new(SentenceSplitter::default()));
    let speak_run: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));
    let consecutive_errors = Rc::new(RefCell::new(0u32));

    // The player is constructed after `dispatch` (whose callbacks the player
    // needs) and is read from three closures — `dispatch` (StopPlayback on
    // barge-in / stale audio), the tick loop, and `on_cleanup`. It therefore
    // lives in a shared LocalStorage slot, populated once construction
    // completes. `on_cleanup`'s `Send + Sync` bound rules out a plain `Rc`.
    let player_slot: StoredValue<Option<Rc<TtsPlayer>>, LocalStorage> = StoredValue::new_local(None);
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
                caption.set(Caption::Error("没听清，再说一次？".into()));
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

    // Mic open + 50 ms VAD tick loop. Async because getUserMedia awaits a
    // permission grant; the interval handle is stashed (LocalStorage, same
    // reason as `mic`) for cleanup.
    let tick_handle: StoredValue<Option<IntervalHandle>, LocalStorage> = StoredValue::new_local(None);
    {
        let vad = Rc::clone(&vad);
        let player = Rc::clone(&player);
        let splitter = Rc::clone(&splitter);
        let speak_run = Rc::clone(&speak_run);
        let consecutive_errors = Rc::clone(&consecutive_errors);
        spawn_local(async move {
            let session = match MicSession::open().await {
                Ok(s) => s,
                Err(_) => {
                    mic_denied.set(true);
                    return;
                }
            };
            mic.set_value(Some(Rc::clone(&session)));
            let handle = set_interval_with_handle(
                move || {
                    let rms = session.rms();
                    level.set(f64::from(rms.min(1.0)));
                    let (next, ev) = vad_step(*vad.borrow(), rms, &vad_cfg);
                    *vad.borrow_mut() = next;
                    match ev {
                        Some(VadEvent::SpeechStart) => {
                            if phase.get_untracked() == VoicePhase::Speaking {
                                dispatch(VoiceEvent::BargeIn);
                            }
                            let _ = session.start_segment();
                        }
                        Some(VadEvent::Discarded) => {
                            let s = Rc::clone(&session);
                            spawn_local(async move {
                                let _ = s.stop_segment().await;
                            });
                        }
                        Some(VadEvent::UtteranceEnd { .. }) => {
                            handle_utterance(
                                Rc::clone(&session),
                                dash,
                                chat,
                                dispatch,
                                caption,
                                Rc::clone(&player),
                                Rc::clone(&splitter),
                                Rc::clone(&speak_run),
                                Rc::clone(&consecutive_errors),
                                voice_mode,
                            );
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

    // Drive TTS from the streaming assistant message. Reacts to `chat.messages`
    // growing; pushes newly-completed sentences into the player and, once the
    // bubble stops streaming, flushes the tail and finalizes.
    {
        let splitter = Rc::clone(&splitter);
        let speak_run = Rc::clone(&speak_run);
        let player = Rc::clone(&player);
        Effect::new(move |_| {
            // Snapshot the active run id and release the borrow before any
            // later `borrow_mut()` on the same cell.
            let run_id = speak_run.borrow().clone();
            let Some(run_id) = run_id else {
                return;
            };
            let target = format!("assistant-{run_id}");
            let (content, streaming) = chat.messages.with(|msgs| {
                msgs.iter()
                    .rev()
                    .find(|m| m.id == target)
                    .map(|m| (m.content.clone(), m.is_streaming))
                    .unwrap_or_default()
            });
            for s in splitter.borrow_mut().push(&content) {
                player.enqueue(dash, s);
            }
            if !streaming && !content.is_empty() {
                if let Some(tail) = splitter.borrow_mut().finish_with(&content) {
                    player.enqueue(dash, tail);
                }
                player.finalize(dash);
                *speak_run.borrow_mut() = None;
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
                p.stop_all();
            }
        });
    });

    let status_text = move || match phase.get() {
        _ if mic_denied.get() => "需要麦克风权限：系统设置 → 隐私与安全 → 麦克风".to_string(),
        VoicePhase::Listening => "正在聆听".to_string(),
        VoicePhase::Processing => "正在思考".to_string(),
        VoicePhase::Speaking => "正在说话 · 开口即可打断".to_string(),
    };
    let caption_text = move || match caption.get() {
        Caption::Idle => String::new(),
        Caption::User(t) => format!("“{t}”"),
        Caption::Assistant(t) | Caption::Error(t) => t,
    };

    view! {
        <div class="voice-stage">
            <VoiceOrb
                phase=Signal::derive(move || phase.get())
                level=Signal::derive(move || level.get())
                error_flash=Signal::derive(move || error_flash.get())
            />
            <div class="voice-caption mt-8">{caption_text}</div>
            <div class="voice-hint mt-2">{status_text}</div>
            <button
                class="voice-hint mt-10 hover:text-text-primary transition-colors"
                on:click=move |_| voice_mode.open.set(false)
            >
                "✕ esc 退出"
            </button>
        </div>
    }
}

/// One utterance: stop capture, transcribe, send to chat, arm the speak Effect.
#[allow(clippy::too_many_arguments)]
fn handle_utterance(
    session: Rc<MicSession>,
    dash: DashboardState,
    chat: ChatState,
    dispatch: impl Fn(VoiceEvent) + Clone + 'static,
    caption: RwSignal<Caption>,
    player: Rc<TtsPlayer>,
    splitter: Rc<RefCell<SentenceSplitter>>,
    speak_run: Rc<RefCell<Option<String>>>,
    consecutive_errors: Rc<RefCell<u32>>,
    voice_mode: VoiceMode,
) {
    spawn_local(async move {
        let Ok((base64, mime)) = session.stop_segment().await else {
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
        dispatch(VoiceEvent::UtteranceSent);

        // Reset the TTS pipeline for the new turn before any stream arrives.
        player.reset();
        *splitter.borrow_mut() = SentenceSplitter::default();
        chat.push_user_message(&text);
        let sk = chat.session_key.get_untracked();
        match ChatApi::send(&dash, &text, sk.as_deref(), vec![], None, None, None).await {
            Ok(resp) => {
                chat.session_key.set(Some(resp.session_key.clone()));
                chat.start_assistant_message(&resp.run_id);
                // IMPORTANT: do NOT call chat.mark_speak_run — the immersive
                // session owns TTS itself; marking the run would double-speak
                // (the composer voice-loop reader would also synthesize it).
                *speak_run.borrow_mut() = Some(resp.run_id);
            }
            Err(_) => {
                dispatch(VoiceEvent::RunFailed);
                let _ = voice_mode;
            }
        }
    });
}
