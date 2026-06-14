//! Deepgram `/v1/listen` streaming protocol adapter.
//! Covers Deepgram cloud AND self-hosted WhisperLiveKit (`/v1/listen` compat).

use super::{StreamConfig, StreamHandles, StreamingTarget, StreamingTranscriber, TranscriptDelta};
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
                interim: String::new(),
                utterance_end: true,
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
                    if self.committed.is_empty() {
                        self.committed = text.to_string();
                    } else {
                        self.committed.push(' ');
                        self.committed.push_str(text);
                    }
                    Some(TranscriptDelta {
                        committed: self.committed.clone(),
                        interim: String::new(),
                        utterance_end: false,
                    })
                } else {
                    Some(TranscriptDelta {
                        committed: self.committed.clone(),
                        interim: text.to_string(),
                        utterance_end: false,
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

#[async_trait]
impl StreamingTranscriber for DeepgramStream {
    async fn open(&self, cfg: StreamConfig) -> anyhow::Result<StreamHandles> {
        // /v1/listen with linear16/16k/interim_results/utterance_end so the
        // server emits BOTH interim and final + UtteranceEnd.
        let base = self.target.base_url.trim_end_matches('/');
        let host = base.replace("https://", "wss://").replace("http://", "ws://");
        let lang = cfg.language.or_else(|| self.target.language.clone()).unwrap_or_default();
        let mut url = format!(
            "{host}/v1/listen?encoding=linear16&sample_rate={}&channels=1&interim_results=true&utterance_end_ms=1000",
            cfg.sample_rate
        );
        if !lang.is_empty() {
            url.push_str(&format!("&language={lang}"));
        }

        let mut req = url.into_client_request()?;
        if !self.target.api_key.is_empty() {
            req.headers_mut()
                .insert("Authorization", format!("Token {}", self.target.api_key).parse()?);
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
        // second final appends with a space-joined growth
        let d = dec.push(&results(true, "世界")).unwrap();
        assert_eq!(d.committed, "你好 世界");
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
}
