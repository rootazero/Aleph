//! Energy-threshold VAD with hangover. Pure — host-testable, zero web_sys.
//! Frames arrive at a fixed cadence (cfg.frame_ms); caller computes RMS.

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct VadConfig {
    /// RMS (0..1) above which a frame counts as speech.
    pub start_rms: f32,
    /// Continuous quiet that ends an utterance.
    pub end_silence_ms: u32,
    /// Utterances shorter than this are noise blips — discarded.
    pub min_speech_ms: u32,
    /// Caller's frame cadence.
    pub frame_ms: u32,
}

impl Default for VadConfig {
    fn default() -> Self {
        Self { start_rms: 0.06, end_silence_ms: 700, min_speech_ms: 300, frame_ms: 50 }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum VadState {
    #[default]
    Quiet,
    Speech { speech_ms: u32, silence_ms: u32 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum VadEvent {
    /// Voice crossed the threshold — caller starts MediaRecorder segment.
    SpeechStart,
    /// Hangover elapsed after real speech — caller stops segment and transcribes.
    UtteranceEnd { speech_ms: u32 },
    /// Hangover elapsed but speech was too short — caller drops the segment.
    Discarded,
}

pub(crate) fn vad_step(state: VadState, rms: f32, cfg: &VadConfig) -> (VadState, Option<VadEvent>) {
    let loud = rms >= cfg.start_rms;
    match state {
        VadState::Quiet if loud => (
            VadState::Speech { speech_ms: cfg.frame_ms, silence_ms: 0 },
            Some(VadEvent::SpeechStart),
        ),
        VadState::Quiet => (VadState::Quiet, None),
        VadState::Speech { speech_ms, .. } if loud => (
            VadState::Speech { speech_ms: speech_ms + cfg.frame_ms, silence_ms: 0 },
            None,
        ),
        VadState::Speech { speech_ms, silence_ms } => {
            let silence_ms = silence_ms + cfg.frame_ms;
            if silence_ms >= cfg.end_silence_ms {
                let ev = if speech_ms >= cfg.min_speech_ms {
                    VadEvent::UtteranceEnd { speech_ms }
                } else {
                    VadEvent::Discarded
                };
                (VadState::Quiet, Some(ev))
            } else {
                (VadState::Speech { speech_ms, silence_ms }, None)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CFG: VadConfig = VadConfig {
        start_rms: 0.06,
        end_silence_ms: 700,
        min_speech_ms: 300,
        frame_ms: 50,
    };

    /// Feed n frames of constant rms, return (final state, all events).
    fn feed(mut st: VadState, rms: f32, n: u32) -> (VadState, Vec<VadEvent>) {
        let mut evs = Vec::new();
        for _ in 0..n {
            let (next, ev) = vad_step(st, rms, &CFG);
            st = next;
            if let Some(e) = ev {
                evs.push(e);
            }
        }
        (st, evs)
    }

    #[test]
    fn silence_never_emits() {
        let (st, evs) = feed(VadState::default(), 0.01, 100);
        assert_eq!(st, VadState::Quiet);
        assert!(evs.is_empty());
    }

    #[test]
    fn speech_start_fires_once_on_threshold() {
        let (_, evs) = feed(VadState::default(), 0.2, 10);
        assert_eq!(evs, vec![VadEvent::SpeechStart]);
    }

    #[test]
    fn short_blip_below_min_speech_is_discarded() {
        // 100ms speech (2 frames) then long silence: no UtteranceEnd
        let (st, evs) = feed(VadState::default(), 0.2, 2);
        assert_eq!(evs, vec![VadEvent::SpeechStart]);
        let (st, evs) = feed(st, 0.01, 30);
        assert_eq!(st, VadState::Quiet);
        assert_eq!(evs, vec![VadEvent::Discarded]);
    }

    #[test]
    fn normal_utterance_ends_after_silence_hangover() {
        // 1s speech then silence: UtteranceEnd after 700ms (14 frames) of quiet
        let (st, _) = feed(VadState::default(), 0.2, 20);
        let (st, evs) = feed(st, 0.01, 13); // 650ms quiet: not yet
        assert!(evs.is_empty());
        assert!(matches!(st, VadState::Speech { .. }));
        let (st, evs) = feed(st, 0.01, 1); // 700ms reached
        assert_eq!(st, VadState::Quiet);
        assert_eq!(evs, vec![VadEvent::UtteranceEnd { speech_ms: 1000 }]);
    }

    #[test]
    fn mid_utterance_pause_shorter_than_hangover_resumes() {
        let (st, _) = feed(VadState::default(), 0.2, 10); // 500ms speech
        let (st, evs) = feed(st, 0.01, 10); // 500ms pause < 700ms
        assert!(evs.is_empty());
        let (st, evs) = feed(st, 0.2, 10); // resume: speech_ms accumulates
        assert!(evs.is_empty());
        assert!(matches!(st, VadState::Speech { silence_ms: 0, .. }));
        let _ = st;
    }
}
