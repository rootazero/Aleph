//! collabora WhisperLive native protocol adapter (`segments[].completed`).

use super::TranscriptDelta;

#[derive(Default)]
pub struct WhisperLiveDecoder {
    /// All completed segment texts seen so far, in order. WhisperLive resends a
    /// trailing window of completed segments, so we rebuild from the union by
    /// appending only segments we have not locked yet.
    committed_segments: Vec<String>,
}

impl WhisperLiveDecoder {
    pub fn push(&mut self, msg: &serde_json::Value) -> Option<TranscriptDelta> {
        let segs = msg.get("segments").and_then(|s| s.as_array())?;
        let mut interim = String::new();
        let mut completed_this_msg: Vec<String> = Vec::new();
        for s in segs {
            let text = s
                .get("text")
                .and_then(|t| t.as_str())
                .unwrap_or("")
                .trim()
                .to_string();
            if text.is_empty() {
                continue;
            }
            let completed = s
                .get("completed")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false);
            if completed {
                completed_this_msg.push(text);
            } else {
                interim = text; // last interim wins
            }
        }
        // Lock newly-completed segments by position. WhisperLive resends a trailing
        // window of completed segments; once a position is locked we keep its text
        // (committed is stable / won't change — see `TranscriptDelta`), so we only
        // append positions we have not locked yet. Revisions to already-locked
        // segments are ignored here; live correction happens on `interim` instead.
        for (i, text) in completed_this_msg.iter().enumerate() {
            if self.committed_segments.get(i).is_none() {
                self.committed_segments.push(text.clone());
            }
        }
        if completed_this_msg.is_empty() && interim.is_empty() {
            return None;
        }
        Some(TranscriptDelta {
            committed: self.committed_segments.concat(),
            interim,
            utterance_end: false,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seg(text: &str, completed: bool) -> serde_json::Value {
        serde_json::json!({ "start": "0.0", "end": "1.0", "text": text, "completed": completed })
    }
    fn msg(segs: Vec<serde_json::Value>) -> serde_json::Value {
        serde_json::json!({ "segments": segs })
    }

    #[test]
    fn completed_segments_lock_into_committed_last_floats() {
        let mut dec = WhisperLiveDecoder::default();
        // two completed + one trailing interim
        let d = dec
            .push(&msg(vec![seg("你好", true), seg("世界", true), seg("现", false)]))
            .unwrap();
        assert_eq!(d.committed, "你好世界");
        assert_eq!(d.interim, "现");
    }

    #[test]
    fn committed_dedup_does_not_double_count_across_messages() {
        let mut dec = WhisperLiveDecoder::default();
        let _ = dec.push(&msg(vec![seg("你好", true), seg("在", false)]));
        // next message re-sends the same completed segment (send_last_n_segments window)
        let d = dec.push(&msg(vec![seg("你好", true), seg("世界", true), seg("吗", false)])).unwrap();
        assert_eq!(d.committed, "你好世界");
        assert_eq!(d.interim, "吗");
    }

    #[test]
    fn server_ready_message_is_ignored() {
        let mut dec = WhisperLiveDecoder::default();
        let ready = serde_json::json!({ "message": "SERVER_READY", "backend": "faster_whisper" });
        assert!(dec.push(&ready).is_none());
    }

    #[test]
    fn all_completed_no_interim_still_emits() {
        let mut dec = WhisperLiveDecoder::default();
        let d = dec.push(&msg(vec![seg("你好", true), seg("世界", true)])).unwrap();
        assert_eq!(d.committed, "你好世界");
        assert_eq!(d.interim, "");
        assert!(!d.utterance_end);
    }
}
