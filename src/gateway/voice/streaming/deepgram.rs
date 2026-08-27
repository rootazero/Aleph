//! Deepgram `/v1/listen` streaming protocol adapter.
//! Covers Deepgram cloud AND self-hosted WhisperLiveKit (`/v1/listen` compat).

use super::{
    push_joined, StreamConfig, StreamHandles, StreamingTarget, StreamingTranscriber,
    TranscriptDelta,
};
use async_trait::async_trait;
use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::{client::IntoClientRequest, Message};

/// Stateful normalizer: folds Deepgram `Results`/`UtteranceEnd` JSON messages
/// into the Aleph `{committed, interim}` model. Pure — no I/O, host-testable.
#[derive(Default)]
pub struct DeepgramDecoder {
    committed: String,
}

impl DeepgramDecoder {
    /// Returns `None` for non-transcript / empty messages.
    pub fn push(&mut self, msg: &serde_json::Value) -> Option<TranscriptDelta> {
        match msg.get("type").and_then(|t| t.as_str()) {
            Some("UtteranceEnd") => Some(TranscriptDelta {
                committed: self.committed.clone(),
                utterance_end: true,
                ..TranscriptDelta::default()
            }),
            Some("Results") | None => {
                let is_final = msg
                    .get("is_final")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false);
                let text = msg
                    .pointer("/channel/alternatives/0/transcript")
                    .and_then(|t| t.as_str())
                    .unwrap_or("")
                    .trim();
                if text.is_empty() {
                    return None;
                }
                if is_final {
                    // ASCII-boundary join: EN finals still join as words, CJK
                    // finals no longer get the agent-visible stray spaces.
                    push_joined(&mut self.committed, text);
                    Some(TranscriptDelta {
                        committed: self.committed.clone(),
                        ..TranscriptDelta::default()
                    })
                } else {
                    // Hold back replacement glyphs from an interim that split a
                    // multibyte char mid-decode (committed is final by contract).
                    Some(TranscriptDelta {
                        interim: text.chars().filter(|c| *c != '\u{FFFD}').collect(),
                        committed: self.committed.clone(),
                        ..TranscriptDelta::default()
                    })
                }
            }
            _ => None,
        }
    }
}

/// WS-connecting adapter speaking the Deepgram `/v1/listen` protocol.
pub struct DeepgramStream {
    target: StreamingTarget,
}

impl DeepgramStream {
    #[must_use]
    pub fn new(target: StreamingTarget) -> Self {
        Self { target }
    }
}

/// Build the `/v1/listen` URL (pure, so the wire shape is host-testable).
///
/// `interim_results=true&utterance_end_ms=1000` so the server emits BOTH
/// interim and final + UtteranceEnd.
///
/// `endpointing=false` is explicit for two reasons. Aleph never consumes
/// `speech_final` (the Panel VAD is the authoritative turn boundary;
/// backend signals are advisory), so disabling endpointing costs nothing on
/// Deepgram cloud. And since WhisperLiveKit 0.2.26 (#419) the compat layer
/// *validates* query parameters: an omitted `endpointing` defaults to 10 ms,
/// which the server rejects outright when it runs with `--no-vac` — the
/// session would be refused before a single frame flowed. Pinning `false`
/// keeps the handshake legal under both VAC configurations.
fn build_listen_url(
    base_url: &str,
    sample_rate: u32,
    language: Option<&str>,
    model: &str,
) -> String {
    let host = base_url
        .trim_end_matches('/')
        .replace("https://", "wss://")
        .replace("http://", "ws://");
    let mut url = format!(
        "{host}/v1/listen?encoding=linear16&sample_rate={sample_rate}&channels=1&interim_results=true&utterance_end_ms=1000&endpointing=false"
    );
    if let Some(lang) = language.filter(|l| !l.is_empty()) {
        url.push_str(&format!("&language={lang}"));
    }
    let model = model.trim();
    if !model.is_empty() {
        url.push_str(&format!("&model={model}"));
    }
    url
}

#[async_trait]
impl StreamingTranscriber for DeepgramStream {
    async fn open(&self, cfg: StreamConfig) -> anyhow::Result<StreamHandles> {
        let lang = cfg
            .language
            .or_else(|| self.target.language.clone())
            .unwrap_or_default();
        let url = build_listen_url(
            &self.target.base_url,
            cfg.sample_rate,
            Some(lang.as_str()),
            &self.target.model,
        );

        let mut req = url.into_client_request()?;
        if !self.target.api_key.is_empty() {
            req.headers_mut().insert(
                "Authorization",
                format!("Token {}", self.target.api_key).parse()?,
            );
        }
        let (ws, _) = tokio_tungstenite::connect_async(req).await?;
        let (mut sink, mut stream) = ws.split();

        let (audio_tx, mut audio_rx) = mpsc::channel::<Vec<u8>>(64);
        let (delta_tx, delta_rx) = mpsc::channel::<TranscriptDelta>(64);

        tokio::spawn(async move {
            let mut dec = DeepgramDecoder::default();
            loop {
                tokio::select! {
                    frame = audio_rx.recv() => match frame {
                        Some(bytes) => {
                            if sink.send(Message::Binary(bytes.into())).await.is_err() {
                                break;
                            }
                        }
                        None => {
                            // Panel closed: send Deepgram "CloseStream" then finish.
                            let _ = sink.send(Message::Text("{\"type\":\"CloseStream\"}".into())).await;
                            break;
                        }
                    },
                    msg = stream.next() => match msg {
                        Some(Ok(Message::Text(t))) => {
                            if let Ok(v) = serde_json::from_str::<serde_json::Value>(t.as_str()) {
                                if let Some(d) = dec.push(&v) {
                                    if delta_tx.send(d).await.is_err() {
                                        break;
                                    }
                                }
                            }
                        }
                        Some(Ok(_)) => {}
                        Some(Err(_)) | None => break,
                    }
                }
            }
        });
        Ok(StreamHandles { audio_tx, delta_rx })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn results(is_final: bool, transcript: &str) -> serde_json::Value {
        serde_json::json!({
            "type": "Results",
            "is_final": is_final,
            "channel": { "alternatives": [ { "transcript": transcript } ] }
        })
    }

    #[test]
    fn final_result_accumulates_into_committed() {
        let mut dec = DeepgramDecoder::default();
        let d = dec.push(&results(true, "你好")).unwrap();
        assert_eq!(d.committed, "你好");
        assert_eq!(d.interim, "");
        // CJK finals concatenate without the space-join that polluted the
        // agent-visible transcript ("hello world" read as odd spacing).
        let d = dec.push(&results(true, "世界")).unwrap();
        assert_eq!(d.committed, "你好世界");
    }

    #[test]
    fn english_finals_still_join_with_spaces() {
        let mut dec = DeepgramDecoder::default();
        let _ = dec.push(&results(true, "Hello there."));
        let d = dec.push(&results(true, "How are you")).unwrap();
        assert_eq!(d.committed, "Hello there. How are you");
    }

    #[test]
    fn interim_replacement_glyphs_are_held_back() {
        let mut dec = DeepgramDecoder::default();
        let d = dec.push(&results(false, "你\u{FFFD}好")).unwrap();
        assert_eq!(d.interim, "你好");
    }

    #[test]
    fn interim_result_is_floating_not_committed() {
        let mut dec = DeepgramDecoder::default();
        let _ = dec.push(&results(true, "你好"));
        let d = dec.push(&results(false, "世")).unwrap();
        assert_eq!(d.committed, "你好");
        assert_eq!(d.interim, "世");
    }

    #[test]
    fn utterance_end_message_sets_flag() {
        let mut dec = DeepgramDecoder::default();
        let msg = serde_json::json!({ "type": "UtteranceEnd", "last_word_end": 3.4 });
        let d = dec.push(&msg).unwrap();
        assert!(d.utterance_end);
    }

    #[test]
    fn empty_transcript_interim_is_ignored() {
        let mut dec = DeepgramDecoder::default();
        assert!(dec.push(&results(false, "")).is_none());
    }

    // -----------------------------------------------------------------------
    // Empty-bytes defense pin (2026-08-16 round)
    //
    // The interim-empty case (`empty_transcript_interim_is_ignored`) was
    // already covered. The two cases below exercise the alternatives-array
    // path and the full-final-results envelope so future edits to the
    // decoder cannot regress the defense.
    // -----------------------------------------------------------------------

    #[test]
    fn empty_final_transcript_emits_no_delta() {
        let mut dec = DeepgramDecoder::default();
        // `is_final: true` + empty transcript is the same shape as the
        // interim case but on the committed path — must still produce no delta.
        assert!(dec.push(&results(true, "")).is_none());
    }

    #[test]
    fn empty_channel_alternatives_emits_no_delta() {
        let mut dec = DeepgramDecoder::default();
        let v = serde_json::json!({
            "type": "Results",
            "is_final": true,
            "channel": { "alternatives": [] }
        });
        assert!(dec.push(&v).is_none());
    }

    // -----------------------------------------------------------------------
    // Wire-URL contract (2026-08-27 round-4, WLK #419 semantics)
    // -----------------------------------------------------------------------

    #[test]
    fn listen_url_carries_the_full_handshake_contract() {
        let url = build_listen_url("https://api.deepgram.com", 16_000, Some("zh"), "nova-3");
        assert!(url.starts_with("wss://api.deepgram.com/v1/listen?"));
        for needle in [
            "encoding=linear16",
            "sample_rate=16000",
            "channels=1",
            "interim_results=true",
            "utterance_end_ms=1000",
            // Explicitly off: Aleph's Panel VAD owns turn boundaries, and WLK
            // ≥0.2.26 rejects the 10 ms default when the server runs --no-vac.
            "endpointing=false",
            "language=zh",
            "model=nova-3",
        ] {
            assert!(url.contains(needle), "missing {needle} in {url}");
        }
    }

    #[test]
    fn listen_url_omits_empty_language_and_model() {
        let url = build_listen_url("http://127.0.0.1:8000/", 16_000, Some(""), "  ");
        assert!(url.starts_with("ws://127.0.0.1:8000/v1/listen?"));
        assert!(!url.contains("language="), "empty language must not ride the wire: {url}");
        assert!(!url.contains("model="), "blank model must not ride the wire: {url}");
        // No language at all behaves like an empty one.
        let url = build_listen_url("http://h:1", 16_000, None, "");
        assert!(!url.contains("language="));
    }
}
