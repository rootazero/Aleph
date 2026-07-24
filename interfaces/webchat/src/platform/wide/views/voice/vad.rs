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
        Self {
            start_rms: 0.06,
            end_silence_ms: 700,
            min_speech_ms: 300,
            frame_ms: 50,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum VadState {
    #[default]
    Quiet,
    Speech {
        speech_ms: u32,
        silence_ms: u32,
    },
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

/// Echo-aware barge-in detector, used ONLY while the assistant is Speaking.
///
/// The assistant's own TTS plays through a separate `AudioContext` that is NOT in
/// the mic's echo-cancellation reference path, so its voice leaks into the mic.
/// A naive energy VAD then mistakes that leaked echo for the user speaking and
/// kills the reply — clipped opening (AEC coldest at playback onset) or sudden
/// mid-reply silence (steady echo above the threshold). This detector fires only
/// when the mic energy is (a) clearly above the current playback echo floor AND
/// (b) sustained past a debounce, and never within an onset grace window where
/// AEC is still cold. Pure — host-testable, zero web_sys.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct BargeConfig {
    /// Absolute floor the mic must clear (main gate against steady echo). Higher
    /// than `VadConfig::start_rms` — a barge-in is a deliberate, near-field cut-in.
    pub floor_rms: f32,
    /// Additive echo-floor estimate: how much playback level couples into the mic.
    /// `threshold = floor_rms + output_level * echo_coupling`. Only ever RAISES the
    /// bar, so a mis-calibrated `output_level` can only make barge-in more
    /// conservative — never cause the false-silence it is meant to prevent.
    pub echo_coupling: f32,
    /// Continuous qualifying energy required before a barge-in fires (debounce).
    pub sustain_ms: u32,
    /// After Speaking begins, ignore barge-in for this long — AEC convergence
    /// window that protects the reply's opening.
    pub onset_grace_ms: u32,
    /// Caller's frame cadence.
    pub frame_ms: u32,
}

impl Default for BargeConfig {
    fn default() -> Self {
        Self {
            floor_rms: 0.13,
            echo_coupling: 0.5,
            sustain_ms: 300,
            onset_grace_ms: 350,
            frame_ms: 50,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct BargeState {
    /// ms elapsed since Speaking began (drives the onset grace).
    speaking_ms: u32,
    /// Consecutive ms of qualifying (above-echo) mic energy (drives the debounce).
    active_ms: u32,
}

/// Feed one frame while Speaking. `rms` is the mic level, `output_level` the
/// playback analyser level. Returns the next state and whether to barge in NOW.
/// Reset `state` to default whenever NOT Speaking so each reply's onset grace
/// starts fresh.
pub(crate) fn barge_step(
    state: BargeState,
    rms: f32,
    output_level: f32,
    cfg: &BargeConfig,
) -> (BargeState, bool) {
    let speaking_ms = state.speaking_ms.saturating_add(cfg.frame_ms);
    let threshold = cfg.floor_rms + output_level * cfg.echo_coupling;
    let active_ms = if rms >= threshold {
        state.active_ms.saturating_add(cfg.frame_ms)
    } else {
        0
    };
    let in_grace = speaking_ms <= cfg.onset_grace_ms;
    let fire = !in_grace && active_ms >= cfg.sustain_ms;
    let next = BargeState {
        speaking_ms,
        // Reset the debounce on fire so we trigger once, not every later frame.
        active_ms: if fire { 0 } else { active_ms },
    };
    (next, fire)
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

    const BARGE: BargeConfig = BargeConfig {
        floor_rms: 0.13,
        echo_coupling: 0.5,
        sustain_ms: 300,
        onset_grace_ms: 350,
        frame_ms: 50,
    };

    /// Feed n frames of constant (rms, output_level) through the barge detector;
    /// return (final state, number of fires).
    fn feed_barge(mut st: BargeState, rms: f32, out: f32, n: u32) -> (BargeState, u32) {
        let mut fires = 0;
        for _ in 0..n {
            let (next, fire) = barge_step(st, rms, out, &BARGE);
            st = next;
            if fire {
                fires += 1;
            }
        }
        (st, fires)
    }

    #[test]
    fn steady_echo_never_barges() {
        // AI playing loud (out=0.4 → threshold 0.33); echo leaks into mic at 0.20,
        // well above plain start_rms (0.06) but below the echo-aware bar. 2s of it
        // must NOT trigger a barge-in (this is the mid-reply false-silence bug).
        let (_, fires) = feed_barge(BargeState::default(), 0.20, 0.4, 40);
        assert_eq!(fires, 0);
    }

    #[test]
    fn onset_grace_blocks_early_barge() {
        // Loud qualifying energy from the very first frame (e.g. cold-AEC echo at
        // playback onset). Within the 350ms grace nothing fires — protects the
        // reply's opening ("lost opening text").
        let (_, fires) = feed_barge(BargeState::default(), 0.9, 0.0, 7); // 350ms
        assert_eq!(fires, 0);
    }

    #[test]
    fn sustained_loud_speech_barges_once() {
        // Real near-field speech (rms 0.5) qualifying from frame 1: the 350ms
        // grace covers frames 1-7; frame 8 is the first past-grace frame and by
        // then active_ms (400) has cleared the 300ms debounce → exactly one fire.
        // (The pure detector re-arms and would fire again under continuous feed;
        // the live caller stops feeding on the phase-flip, so we cap at frame 8.)
        let (_, fires) = feed_barge(BargeState::default(), 0.5, 0.0, 8);
        assert_eq!(fires, 1);
    }

    #[test]
    fn transient_blip_does_not_barge() {
        // Past the grace, then a single loud frame (50ms) — well under the 300ms
        // debounce — followed by quiet. No barge-in.
        let (st, _) = feed_barge(BargeState::default(), 0.0, 0.0, 8); // clear grace, quiet
        let (st, evs1) = feed_barge(st, 0.6, 0.0, 1); // one loud frame
        let (_, evs2) = feed_barge(st, 0.0, 0.0, 10); // back to quiet
        assert_eq!(evs1 + evs2, 0);
    }

    #[test]
    fn threshold_falls_back_in_playback_gap() {
        // Between clips the AI is silent (out=0.0 → threshold = floor 0.13). A
        // user speaking at 0.2 now clears the bar and, sustained past the grace +
        // debounce, barges in — full-duplex still works in the inter-clip gaps.
        // Capped at frame 8 (first post-grace fire) for the same reason as above.
        let (_, fires) = feed_barge(BargeState::default(), 0.2, 0.0, 8);
        assert_eq!(fires, 1);
    }

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
