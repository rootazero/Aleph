//! Engine abstraction — isolates sherpa-rs behind small traits so the
//! protocol/lifecycle layers never see the backend (swap-friendly, mock-testable).

pub mod mock;
#[cfg(feature = "sherpa")]
pub mod sherpa;

/// Transcription result.
#[derive(Debug, Clone)]
pub struct SttResult {
    pub text: String,
    pub language: Option<String>,
}

/// Synthesized audio: mono f32 PCM.
#[derive(Debug, Clone)]
pub struct TtsAudio {
    pub samples: Vec<f32>,
    pub sample_rate: u32,
}

/// Speech-to-text engine. Input is 16 kHz mono f32 PCM.
/// Sync on purpose — callers run it inside `spawn_blocking`.
pub trait SttEngine: Send + Sync {
    fn transcribe(&self, samples: &[f32], language: Option<&str>) -> anyhow::Result<SttResult>;
}

/// Text-to-speech engine.
pub trait TtsEngine: Send + Sync {
    fn synthesize(&self, text: &str, voice: &str, speed: f32) -> anyhow::Result<TtsAudio>;
}
