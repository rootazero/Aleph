//! Energy-threshold VAD with hangover. Pure — host-testable, zero web_sys.
//! Frames arrive at a fixed cadence (cfg.frame_ms); caller computes RMS.

use aleph_protocol::panel_voice::{MAX_UTTERANCE_SAMPLES, SAMPLE_RATE_HZ};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VadConfig {
    /// RMS (0..1) above which a frame counts as speech.
    pub start_rms: f32,
    /// Continuous quiet that ends an utterance.
    pub end_silence_ms: u32,
    /// Utterances shorter than this are noise blips — discarded.
    pub min_speech_ms: u32,
    /// Caller's frame cadence.
    pub frame_ms: u32,
}

const DEFAULT_VAD_CONFIG: VadConfig = VadConfig {
    start_rms: 0.06,
    end_silence_ms: 700,
    min_speech_ms: 300,
    frame_ms: 50,
};

impl Default for VadConfig {
    fn default() -> Self {
        DEFAULT_VAD_CONFIG
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum VadState {
    #[default]
    Quiet,
    Speech {
        speech_ms: u32,
        silence_ms: u32,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VadEvent {
    /// Voice crossed the threshold.
    SpeechStart,
    /// Hangover elapsed after enough speech.
    UtteranceEnd { speech_ms: u32 },
    /// Hangover elapsed but speech was too short.
    Discarded,
}

pub fn vad_step(state: VadState, rms: f32, cfg: &VadConfig) -> (VadState, Option<VadEvent>) {
    let loud = rms >= cfg.start_rms;
    match state {
        VadState::Quiet if loud => (
            VadState::Speech {
                speech_ms: cfg.frame_ms,
                silence_ms: 0,
            },
            Some(VadEvent::SpeechStart),
        ),
        VadState::Quiet => (VadState::Quiet, None),
        VadState::Speech { speech_ms, .. } if loud => (
            VadState::Speech {
                speech_ms: speech_ms + cfg.frame_ms,
                silence_ms: 0,
            },
            None,
        ),
        VadState::Speech {
            speech_ms,
            silence_ms,
        } => {
            let silence_ms = silence_ms + cfg.frame_ms;
            if silence_ms >= cfg.end_silence_ms {
                let ev = if speech_ms >= cfg.min_speech_ms {
                    VadEvent::UtteranceEnd { speech_ms }
                } else {
                    VadEvent::Discarded
                };
                (VadState::Quiet, Some(ev))
            } else {
                (
                    VadState::Speech {
                        speech_ms,
                        silence_ms,
                    },
                    None,
                )
            }
        }
    }
}

pub const CAPTURE_FRAME_SAMPLES: usize =
    SAMPLE_RATE_HZ as usize * DEFAULT_VAD_CONFIG.frame_ms as usize / 1_000;
pub const CAPTURE_PREROLL_MS: u32 = 320;
pub const CAPTURE_PREROLL_SAMPLES: usize =
    SAMPLE_RATE_HZ as usize * CAPTURE_PREROLL_MS as usize / 1_000;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CaptureDecision {
    #[default]
    Continue,
    Seal {
        first_sample: usize,
        end_sample: usize,
        limit_reached: bool,
    },
    NoSpeech,
    InvalidInput,
}

/// Decides one clip's half-open sample range without owning or copying audio.
/// Hosts must supply contiguous, fixed-size RMS frames of normalized PCM, keep
/// the selected audio available and abort on device/clock loss.
#[derive(Default)]
pub struct CaptureVad {
    vad: VadState,
    observed: usize,
    first_sample: Option<usize>,
    decision: CaptureDecision,
}

impl CaptureVad {
    pub fn observed_samples(&self) -> usize {
        self.observed
    }

    /// `end_sample` is the exclusive offset in the capture stream, not the
    /// wall-clock time of frame arrival or the size of an RPC batch.
    pub fn observe(&mut self, end_sample: usize, rms: f32) -> CaptureDecision {
        if self.decision != CaptureDecision::Continue {
            return self.decision;
        }
        if self.observed + CAPTURE_FRAME_SAMPLES != end_sample || !(0.0..=1.0).contains(&rms) {
            self.decision = CaptureDecision::InvalidInput;
            return self.decision;
        }

        let frame_start = self.observed;
        self.observed = end_sample;
        let (next, event) = vad_step(self.vad, rms, &DEFAULT_VAD_CONFIG);
        self.vad = next;
        if event == Some(VadEvent::SpeechStart) {
            self.first_sample = Some(frame_start.saturating_sub(CAPTURE_PREROLL_SAMPLES));
        }
        if let Some(first_sample) = self.first_sample {
            let limit = first_sample + MAX_UTTERANCE_SAMPLES;
            if end_sample >= limit {
                self.decision = CaptureDecision::Seal {
                    first_sample,
                    end_sample: limit,
                    limit_reached: true,
                };
                return self.decision;
            }
            match event {
                Some(VadEvent::UtteranceEnd { .. }) => {
                    self.decision = CaptureDecision::Seal {
                        first_sample,
                        end_sample,
                        limit_reached: false,
                    };
                }
                Some(VadEvent::Discarded) => self.first_sample = None,
                _ => {}
            }
        }
        if self.first_sample.is_none() && end_sample >= MAX_UTTERANCE_SAMPLES {
            self.decision = CaptureDecision::NoSpeech;
        }
        self.decision
    }

    pub fn finish(&mut self) -> CaptureDecision {
        if self.decision == CaptureDecision::Continue {
            self.decision = match (self.first_sample, self.vad) {
                (Some(first_sample), VadState::Speech { speech_ms, .. })
                    if speech_ms >= DEFAULT_VAD_CONFIG.min_speech_ms =>
                {
                    CaptureDecision::Seal {
                        first_sample,
                        end_sample: self.observed,
                        limit_reached: false,
                    }
                }
                _ => CaptureDecision::NoSpeech,
            };
        }
        self.decision
    }
}

#[cfg(test)]
mod capture_tests {
    use super::*;

    fn advance(capture: &mut CaptureVad, frames: usize, rms: f32) -> CaptureDecision {
        let first_end = capture.observed_samples() + CAPTURE_FRAME_SAMPLES;
        let mut decision = CaptureDecision::Continue;
        for index in 0..frames {
            decision = capture.observe(first_end + index * CAPTURE_FRAME_SAMPLES, rms);
            if decision != CaptureDecision::Continue {
                break;
            }
        }
        decision
    }

    #[test]
    fn capture_silence_and_device_gaps_never_seal() {
        let mut silence = CaptureVad::default();
        assert_eq!(advance(&mut silence, 401, 0.0), CaptureDecision::NoSpeech);
        assert_eq!(silence.finish(), CaptureDecision::NoSpeech);

        let mut lost = CaptureVad::default();
        assert_eq!(advance(&mut lost, 6, 0.2), CaptureDecision::Continue);
        let skipped = lost.observed_samples() + 2 * CAPTURE_FRAME_SAMPLES;
        assert_eq!(lost.observe(skipped, 0.2), CaptureDecision::InvalidInput);
        assert_eq!(lost.finish(), CaptureDecision::InvalidInput);

        let mut malformed = CaptureVad::default();
        assert_eq!(
            malformed.observe(CAPTURE_FRAME_SAMPLES, f32::NAN),
            CaptureDecision::InvalidInput
        );
    }

    #[test]
    fn capture_limit_is_exact_including_pre_roll() {
        let mut capture = CaptureVad::default();
        assert_eq!(advance(&mut capture, 9, 0.0), CaptureDecision::Continue);
        let first_sample = 9 * CAPTURE_FRAME_SAMPLES - CAPTURE_PREROLL_SAMPLES;
        assert_eq!(
            advance(&mut capture, 400, 0.2),
            CaptureDecision::Seal {
                first_sample,
                end_sample: first_sample + MAX_UTTERANCE_SAMPLES,
                limit_reached: true,
            },
        );
    }

    #[test]
    fn capture_natural_and_manual_finish_require_voiced_audio() {
        let mut capture = CaptureVad::default();
        assert_eq!(advance(&mut capture, 5, 0.2), CaptureDecision::Continue);
        assert_eq!(capture.finish(), CaptureDecision::NoSpeech);

        let mut capture = CaptureVad::default();
        assert_eq!(advance(&mut capture, 4, 0.0), CaptureDecision::Continue);
        assert_eq!(advance(&mut capture, 6, 0.2), CaptureDecision::Continue);
        assert_eq!(advance(&mut capture, 13, 0.0), CaptureDecision::Continue);
        assert_eq!(
            advance(&mut capture, 1, 0.0),
            CaptureDecision::Seal {
                first_sample: 0,
                end_sample: 24 * CAPTURE_FRAME_SAMPLES,
                limit_reached: false
            },
        );

        let mut manual = CaptureVad::default();
        assert_eq!(advance(&mut manual, 6, 0.2), CaptureDecision::Continue);
        assert_eq!(
            manual.finish(),
            CaptureDecision::Seal {
                first_sample: 0,
                end_sample: 6 * CAPTURE_FRAME_SAMPLES,
                limit_reached: false
            },
        );
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
    }

    #[test]
    fn aligned_frame_cadences_reach_the_same_hangover_boundary() {
        for frame_ms in [20, 50, 100] {
            let config = VadConfig {
                frame_ms,
                ..VadConfig::default()
            };
            let mut state = VadState::default();
            let mut events = Vec::new();
            for start_ms in (0..1800).step_by(frame_ms as usize) {
                let rms = if (100..1100).contains(&start_ms) {
                    0.2
                } else {
                    0.01
                };
                let (next, event) = vad_step(state, rms, &config);
                state = next;
                if let Some(event) = event {
                    events.push((start_ms, event));
                }
            }
            assert_eq!(state, VadState::Quiet);
            assert_eq!(
                events,
                [
                    (100, VadEvent::SpeechStart),
                    (1800 - frame_ms, VadEvent::UtteranceEnd { speech_ms: 1000 }),
                ],
            );
        }
    }
}
