//! Provider-neutral streaming transcription contract.
//!
//! Core defines the `{committed, interim}` semantics; vendor protocols live in
//! per-adapter submodules. Panel never sees a vendor wire format — only the
//! normalized [`TranscriptDelta`] pushed over the `voice.transcribe.delta` topic.

pub mod deepgram;
pub mod whisperlive;

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

/// Resolved streaming target (from `[voice.streaming]` config, provider-neutral).
#[derive(Debug, Clone)]
pub struct StreamingTarget {
    /// `"deepgram"` | `"whisperlive"` (case-insensitive; unknown → Deepgram).
    pub provider: String,
    pub base_url: String,
    pub api_key: String,
    pub language: Option<String>,
}

/// Which vendor wire protocol an adapter speaks.
pub enum StreamingProvider {
    Deepgram,
    WhisperLive,
}

/// Map a provider string to its wire protocol. Unknown values fall back to the
/// Deepgram `/v1/listen` protocol (the lingua franca; WhisperLiveKit speaks it).
#[must_use]
pub fn classify_provider(provider: &str) -> StreamingProvider {
    match provider.trim().to_ascii_lowercase().as_str() {
        "whisperlive" => StreamingProvider::WhisperLive,
        // "deepgram", "whisperlivekit", unknown → Deepgram /v1/listen lingua franca
        _ => StreamingProvider::Deepgram,
    }
}

/// Build the adapter for a resolved target. Construction is cheap and infallible;
/// the network connection happens lazily in [`StreamingTranscriber::open`].
#[must_use]
pub fn build_transcriber(t: StreamingTarget) -> Box<dyn StreamingTranscriber> {
    match classify_provider(&t.provider) {
        StreamingProvider::Deepgram => Box::new(deepgram::DeepgramStream::new(t)),
        StreamingProvider::WhisperLive => Box::new(whisperlive::WhisperLiveStream::new(t)),
    }
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

    #[test]
    fn build_transcriber_picks_adapter_by_provider() {
        let cfg = StreamingTarget {
            provider: "whisperlive".into(),
            base_url: "ws://127.0.0.1:9090".into(),
            api_key: String::new(),
            language: None,
        };
        assert!(matches!(classify_provider(&cfg.provider), StreamingProvider::WhisperLive));
        let cfg2 = StreamingTarget {
            provider: "deepgram".into(),
            base_url: "wss://api.deepgram.com".into(),
            api_key: "k".into(),
            language: None,
        };
        assert!(matches!(classify_provider(&cfg2.provider), StreamingProvider::Deepgram));
        // unknown defaults to deepgram (the lingua-franca protocol)
        assert!(matches!(classify_provider("mystery"), StreamingProvider::Deepgram));
    }

    #[tokio::test]
    #[ignore = "needs a live WhisperLiveKit at WL_URL; run manually"]
    async fn deepgram_adapter_round_trips_against_whisperlivekit() {
        let url = std::env::var("WL_URL").unwrap(); // e.g. ws://127.0.0.1:8000
        let t = StreamingTarget {
            provider: "deepgram".into(),
            base_url: url,
            api_key: String::new(),
            language: Some("zh".into()),
        };
        let tr = build_transcriber(t);
        let mut h = tr.open(StreamConfig::new(Some("zh".into()))).await.unwrap();
        h.audio_tx.send(vec![0u8; 32_000]).await.unwrap();
        drop(h.audio_tx);
        let _ = h.delta_rx.recv().await;
    }
}
