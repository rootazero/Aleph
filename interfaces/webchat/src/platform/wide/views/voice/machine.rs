//! Voice session phase machine. Pure — decides transitions and side-effect
//! commands; the component layer executes them.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum VoicePhase {
    Listening,
    Processing,
    Speaking,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum VoiceEvent {
    /// Utterance captured + sent for transcription/chat.
    UtteranceSent,
    /// First TTS sentence is playing.
    FirstAudioReady,
    /// TTS queue fully drained after run completion.
    PlaybackDrained,
    /// User spoke while assistant audio was playing.
    BargeIn,
    TranscribeFailed,
    RunFailed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Action {
    None,
    /// Stop audio and clear the TTS queue.
    StopPlayback,
    /// Show the "Didn't catch that / error" caption.
    ShowError,
}

pub(crate) fn on_event(phase: VoicePhase, ev: VoiceEvent) -> (VoicePhase, Action) {
    use {Action as A, VoiceEvent as E, VoicePhase as P};
    match (phase, ev) {
        (P::Listening, E::UtteranceSent) => (P::Processing, A::None),
        (P::Processing, E::FirstAudioReady) => (P::Speaking, A::None),
        // A run may finish with nothing speakable — drain straight back.
        (P::Processing | P::Speaking, E::PlaybackDrained) => (P::Listening, A::None),
        (P::Speaking, E::BargeIn) => (P::Listening, A::StopPlayback),
        (P::Processing | P::Speaking, E::TranscribeFailed | E::RunFailed) => {
            (P::Listening, A::ShowError)
        }
        (P::Listening, E::TranscribeFailed) => (P::Listening, A::ShowError),
        // Stale audio arriving after we've already returned to Listening.
        (P::Listening, E::FirstAudioReady) => (P::Listening, A::StopPlayback),
        // Everything else: ignore.
        (p, _) => (p, A::None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_happy_loop() {
        let p = VoicePhase::Listening;
        let (p, a) = on_event(p, VoiceEvent::UtteranceSent);
        assert_eq!(p, VoicePhase::Processing);
        assert_eq!(a, Action::None);
        let (p, a) = on_event(p, VoiceEvent::FirstAudioReady);
        assert_eq!(p, VoicePhase::Speaking);
        assert_eq!(a, Action::None);
        let (p, a) = on_event(p, VoiceEvent::PlaybackDrained);
        assert_eq!(p, VoicePhase::Listening);
        assert_eq!(a, Action::None);
    }

    #[test]
    fn barge_in_only_valid_while_speaking() {
        let (p, a) = on_event(VoicePhase::Speaking, VoiceEvent::BargeIn);
        assert_eq!(p, VoicePhase::Listening);
        assert_eq!(a, Action::StopPlayback);
        // BargeIn while listening is a no-op (it's just normal speech)
        let (p, a) = on_event(VoicePhase::Listening, VoiceEvent::BargeIn);
        assert_eq!(p, VoicePhase::Listening);
        assert_eq!(a, Action::None);
    }

    #[test]
    fn errors_return_to_listening_with_caption() {
        let (p, a) = on_event(VoicePhase::Processing, VoiceEvent::TranscribeFailed);
        assert_eq!(p, VoicePhase::Listening);
        assert_eq!(a, Action::ShowError);
        let (p, a) = on_event(VoicePhase::Speaking, VoiceEvent::RunFailed);
        assert_eq!(p, VoicePhase::Listening);
        assert_eq!(a, Action::ShowError);
    }

    #[test]
    fn stale_events_are_rejected() {
        // playback events can't fire while still processing-before-audio… except
        // run can complete with NO tts-able text: PlaybackDrained from Processing is legal
        let (p, _) = on_event(VoicePhase::Processing, VoiceEvent::PlaybackDrained);
        assert_eq!(p, VoicePhase::Listening);
        // but FirstAudioReady when already back in Listening must not flip state
        let (p, a) = on_event(VoicePhase::Listening, VoiceEvent::FirstAudioReady);
        assert_eq!(p, VoicePhase::Listening);
        assert_eq!(a, Action::StopPlayback); // stale audio gets cleaned up
    }
}
