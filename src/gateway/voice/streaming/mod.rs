//! Provider-neutral streaming transcription contract.
//!
//! Core defines the `{committed, interim}` semantics; vendor protocols live in
//! per-adapter submodules. Panel never sees a vendor wire format — only the
//! normalized [`TranscriptDelta`] pushed over the `voice.transcribe.delta` topic.

pub mod deepgram;
// Adapter submodules (whisperlive / relay) are declared by Tasks 3–4 as they are created.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

/// Backend-agnostic transcript update. `committed` is locked (won't change);
/// `interim` is the floating hypothesis (may be rewritten next delta).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct TranscriptDelta {
    #[serde(default)]
    pub committed: String,
    #[serde(default)]
    pub interim: String,
    /// Backend explicitly signaled end-of-utterance (best-effort; Panel VAD is
    /// authoritative for turn segmentation, this is advisory only).
    #[serde(default)]
    pub utterance_end: bool,
}

/// Per-session open parameters handed to an adapter.
#[derive(Debug, Clone)]
pub struct StreamConfig {
    pub sample_rate: u32,
    pub language: Option<String>,
}

impl StreamConfig {
    #[must_use]
    pub fn new(language: Option<String>) -> Self {
        Self { sample_rate: 16_000, language }
    }
}

/// Channels bridging the relay and a backend session: relay pushes s16le audio
/// frames into `audio_tx`; backend deltas arrive on `delta_rx`.
#[must_use]
pub struct StreamHandles {
    /// s16le PCM frames, 16 kHz mono (matches `StreamConfig::sample_rate`).
    pub audio_tx: mpsc::Sender<Vec<u8>>,
    pub delta_rx: mpsc::Receiver<TranscriptDelta>,
}

/// The Aleph contract. Each vendor protocol implements this; `build_transcriber`
/// picks the impl from config (provider-neutral — local self-host and cloud are
/// just different `base_url`/`provider` values).
#[async_trait]
pub trait StreamingTranscriber: Send + Sync {
    async fn open(&self, cfg: StreamConfig) -> anyhow::Result<StreamHandles>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transcript_delta_committed_only_serializes_snake_case() {
        let d = TranscriptDelta { committed: "你好".into(), interim: String::new(), utterance_end: false };
        let j = serde_json::to_value(&d).unwrap();
        assert_eq!(j["committed"], "你好");
        assert_eq!(j["interim"], "");
        assert_eq!(j["utterance_end"], false);
    }

    #[test]
    fn stream_config_defaults_to_16k() {
        let c = StreamConfig::new(None);
        assert_eq!(c.sample_rate, 16_000);
        assert!(c.language.is_none());
    }
}
