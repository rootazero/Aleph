//! Deepgram `/v1/listen` streaming protocol adapter.
//! Covers Deepgram cloud AND self-hosted WhisperLiveKit (`/v1/listen` compat).

use super::TranscriptDelta;

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
