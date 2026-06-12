//! Deterministic mock engines for server/lifecycle tests.

use super::{SttEngine, SttResult, TtsAudio, TtsEngine};

/// Echoes the sample count so tests can assert the decode path ran.
pub struct MockStt;

impl SttEngine for MockStt {
    fn transcribe(&self, samples: &[f32], language: Option<&str>) -> anyhow::Result<SttResult> {
        Ok(SttResult {
            text: format!("mock transcript ({} samples)", samples.len()),
            language: language.map(str::to_string),
        })
    }
}

/// Produces 100 ms of 440 Hz sine at 24 kHz regardless of input.
pub struct MockTts;

impl TtsEngine for MockTts {
    fn synthesize(&self, _text: &str, _voice: &str, _speed: f32) -> anyhow::Result<TtsAudio> {
        let rate = 24_000u32;
        let samples = (0..rate / 10)
            .map(|i| (i as f32 * 440.0 * 2.0 * std::f32::consts::PI / rate as f32).sin() * 0.5)
            .collect();
        Ok(TtsAudio { samples, sample_rate: rate })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mock_stt_reports_sample_count_and_language() {
        let r = MockStt.transcribe(&[0.0; 320], Some("zh")).unwrap();
        assert!(r.text.contains("320"));
        assert_eq!(r.language.as_deref(), Some("zh"));
    }

    #[test]
    fn mock_tts_emits_100ms_audio() {
        let a = MockTts.synthesize("hi", "0", 1.0).unwrap();
        assert_eq!(a.sample_rate, 24_000);
        assert_eq!(a.samples.len(), 2_400);
    }
}
