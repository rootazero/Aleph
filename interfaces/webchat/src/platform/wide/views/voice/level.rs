//! Orb level pipeline — perceptual (dB-domain) smoothing of the raw mic/output
//! RMS. Pure — host-testable, zero web_sys.
//!
//! Ported from FluidVoice's `AudioCapturePipeline.calculateAudioLevel`
//! (noise gate → dB normalize → asymmetric exponential smoothing → floor
//! clamp). Raw RMS is perceptually wrong for a level meter: speech RMS spans
//! ~0.02–0.3, so a linear map either pins quiet speech at "invisible" or loud
//! speech at "saturated". Mapping through dB with a −55 dB floor gives the orb
//! a full, natural swing; the fast-attack / slow-decay smoothing keeps it
//! lively on onsets without flickering in pauses.
//!
//! This drives the ORB ONLY. The VAD (`vad.rs`) stays on raw RMS — its
//! thresholds are calibrated in that domain and turn segmentation must not
//! change with a visual-pipeline tweak.

/// Below this raw RMS the input is treated as pure noise floor (gate → 0).
const NOISE_GATE_RMS: f32 = 0.002;
/// dB range mapped onto 0..1: −55 dB → 0.0, 0 dB → 1.0.
const DB_FLOOR: f32 = -55.0;
/// Attack weight: how much of a NEW (higher) level lands each frame.
const ATTACK: f32 = 0.7;
/// Post-smoothing floor — levels below this render as silence so the orb
/// visually rests between words instead of shimmering.
const REST_FLOOR: f32 = 0.04;

/// One-pole smoother state. Feed one raw RMS per tick; read `value()`.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct LevelSmoother {
    smoothed: f32,
}

impl LevelSmoother {
    /// Ingest one raw RMS sample (0..1) and return the smoothed orb level.
    pub(crate) fn step(&mut self, rms: f32) -> f32 {
        let target = normalize_db(rms);
        // Fast attack, slow decay: rises snap (ATTACK of the way there each
        // frame), falls glide (1−ATTACK) — mirrors FluidVoice's 0.7/0.3 blend.
        let w = if target > self.smoothed {
            ATTACK
        } else {
            1.0 - ATTACK
        };
        self.smoothed = w.mul_add(target, (1.0 - w) * self.smoothed);
        if self.smoothed < REST_FLOOR {
            0.0
        } else {
            self.smoothed
        }
    }
}

/// Map raw RMS → perceptual 0..1 via dB with a −55 dB floor.
fn normalize_db(rms: f32) -> f32 {
    if rms < NOISE_GATE_RMS {
        return 0.0;
    }
    let db = 20.0 * rms.max(1e-10).log10();
    ((db - DB_FLOOR) / -DB_FLOOR).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noise_floor_gates_to_zero() {
        let mut s = LevelSmoother::default();
        for _ in 0..20 {
            assert_eq!(s.step(0.001), 0.0, "sub-gate RMS must render silent");
        }
    }

    #[test]
    fn quiet_speech_is_clearly_visible() {
        // RMS 0.05 ≈ −26 dB → ~0.53 normalized — under the old linear ×3.5
        // map this was a barely-there 0.175. A few frames of attack get close.
        let mut s = LevelSmoother::default();
        let mut level = 0.0;
        for _ in 0..6 {
            level = s.step(0.05);
        }
        assert!(
            level > 0.4,
            "quiet speech should visibly move the orb: {level}"
        );
    }

    #[test]
    fn attack_is_faster_than_decay() {
        let mut s = LevelSmoother::default();
        let up1 = s.step(0.2); // one frame of attack from rest
        let mut s2 = LevelSmoother::default();
        for _ in 0..30 {
            s2.step(0.2); // settle high
        }
        let settled = s2.step(0.2);
        let down1 = s2.step(0.0); // one frame of decay
        assert!(
            up1 > settled - down1,
            "one attack frame ({up1}) should move further than one decay frame ({})",
            settled - down1
        );
    }

    #[test]
    fn decay_returns_to_rest() {
        let mut s = LevelSmoother::default();
        for _ in 0..10 {
            s.step(0.3);
        }
        let mut level = 1.0;
        for _ in 0..40 {
            level = s.step(0.0);
        }
        assert_eq!(level, 0.0, "sustained silence must settle to rest");
    }

    #[test]
    fn loud_speech_saturates_sanely() {
        let mut s = LevelSmoother::default();
        let mut level = 0.0;
        for _ in 0..20 {
            level = s.step(0.9);
        }
        assert!(level > 0.9 && level <= 1.0);
    }
}
