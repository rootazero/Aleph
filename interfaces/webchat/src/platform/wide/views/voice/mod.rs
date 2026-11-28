//! Immersive voice overlay mounted above the persistent Chat view.

pub(crate) mod audio;
pub(crate) mod caption_state;
pub(crate) mod level;
pub(crate) mod machine;
pub(crate) mod orb;
pub(crate) mod sentence;
mod session;
mod turn;
pub(crate) mod vad;
pub(crate) mod wav;

use leptos::prelude::*;

use crate::context::DashboardState;
use crate::i18n::{t, t_string, use_i18n};
use crate::state::sessions::SessionMap;
use crate::views::chat::ChatState;
use machine::VoicePhase;
use orb::VoiceOrb;
use sentence::SentenceSplitter;
use session::VoiceUi;

/// App-level switch for the immersive overlay. Provided in `app.rs`; the
/// composer mini-orb (Task 8) flips `open` to enter/leave the session.
#[derive(Clone, Copy)]
pub(crate) struct VoiceMode {
    pub open: RwSignal<bool>,
    shutdown: StoredValue<Option<Box<dyn FnOnce()>>, LocalStorage>,
}

impl VoiceMode {
    pub(crate) fn new() -> Self {
        Self {
            open: RwSignal::new(false),
            shutdown: StoredValue::new_local(None),
        }
    }

    pub(crate) fn set_open(self, open: bool) {
        if !open {
            if let Some(shutdown) = self.shutdown.try_update_value(Option::take).flatten() {
                shutdown();
            }
        }
        self.open.set(open);
    }

    fn on_close(self, shutdown: impl FnOnce() + 'static) {
        self.shutdown.set_value(Some(Box::new(shutdown)));
    }
}

/// Who the caption is quoting.
#[derive(Clone, PartialEq)]
enum Caption {
    Idle,
    /// Live streaming transcript: render the two-stage [`caption_state::CaptionState`] held in
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
    let i18n = use_i18n();
    let dash = expect_context::<DashboardState>();
    let chat = expect_context::<ChatState>();
    let sessions = expect_context::<SessionMap>();
    let voice_mode = expect_context::<VoiceMode>();
    // Native shell vs. browser decides how a mic denial is remediated (OS TCC
    // deep-link vs. browser per-site permission). Resolved once at mount.
    let native = audio::is_native_shell();

    let ui = VoiceUi::new();
    session::mount(ui, dash, chat, sessions, voice_mode, i18n);
    let VoiceUi {
        phase,
        level,
        caption,
        caption_state,
        error_flash,
        mic_error,
    } = ui;

    let status_text = move || match phase.get() {
        VoicePhase::Listening => t_string!(i18n, voice.phase_listening).to_string(),
        VoicePhase::Processing => t_string!(i18n, voice.phase_processing).to_string(),
        VoicePhase::Speaking => t_string!(i18n, voice.phase_speaking).to_string(),
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
                    {e.caption(native, i18n)}
                </a>
            }
            .into_any(),
            None => view! { {e.caption(native, i18n)} }.into_any(),
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
                on:click=move |_| voice_mode.set_open(false)
            >
                {t!(i18n, voice.exit_hint)}
            </button>
        </div>
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
