//! collabora WhisperLive native protocol adapter (`segments[].completed`).

use super::{
    push_joined, StreamConfig, StreamHandles, StreamingTarget, StreamingTranscriber,
    TranscriptDelta,
};
use async_trait::async_trait;
use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;

/// Default model requested in the handshake when `[voice.streaming] model` is
/// unset. WhisperLive loads a model per client request (unless the server is
/// pinned with `--single_model`), so this must stay a modest default.
const DEFAULT_MODEL: &str = "small";

#[derive(Default)]
pub struct WhisperLiveDecoder {
    /// Accumulated locked text (ASCII-boundary joined).
    committed: String,
    /// `start` timestamp of the last locked segment. WhisperLive resends a
    /// trailing *window* of completed segments (`send_last_n_segments`), so
    /// window position is NOT a stable identity — once more segments complete
    /// than the window holds, positional matching silently drops every new
    /// segment. Segment `start` times are strictly increasing, so "newer than
    /// the last locked start" is the correct dedup key.
    last_locked_start: Option<f64>,
    /// Fallback identity for the defensive case of a segment without a
    /// parseable `start`: dedup against the last locked text.
    last_locked_text: String,
}

/// A segment's `start` time, accepting both wire shapes.
///
/// WhisperLive's faster-whisper backend serializes it as a formatted **string**
/// (`"1.230"`), but the TensorRT backend and community forks emit a plain JSON
/// **number**. Reading only the string form silently yielded `None` for those
/// servers, which degraded dedup to text identity — and text identity then
/// discards a legitimately *repeated* segment (a speaker saying "嗯 … 嗯" locks
/// the first and drops the second).
fn segment_start(seg: &serde_json::Value) -> Option<f64> {
    let v = seg.get("start")?;
    v.as_f64()
        .or_else(|| v.as_str().and_then(|s| s.trim().parse::<f64>().ok()))
}

impl WhisperLiveDecoder {
    /// Lock `text` into `committed` if it is a segment we have not seen.
    fn lock_segment(&mut self, start: Option<f64>, text: &str) {
        let is_new = match (start, self.last_locked_start) {
            (Some(s), Some(prev)) => s > prev + 1e-6,
            (Some(_), None) => true,
            // No parseable timestamp (off-spec server): fall back to text
            // identity so a window resend isn't double-appended.
            (None, _) => text != self.last_locked_text,
        };
        if !is_new {
            return;
        }
        push_joined(&mut self.committed, text);
        if start.is_some() {
            self.last_locked_start = start;
        }
        self.last_locked_text = text.to_string();
    }

    pub fn push(&mut self, msg: &serde_json::Value) -> Option<TranscriptDelta> {
        // Server status / control messages. WAIT (admission queue full) and
        // ERROR are fatal for this session — surface them so the client can
        // fall back to batch instead of listening to a dead stream. The
        // SERVER_READY handshake ack and other infos fall through harmlessly.
        if let Some(status) = msg.get("status").and_then(|s| s.as_str()) {
            // `message` is a float for WAIT (queue minutes) and a string for
            // ERROR — render strings unquoted, everything else via JSON.
            let detail = msg.get("message").map_or_else(String::new, |m| {
                m.as_str().map_or_else(|| m.to_string(), str::to_string)
            });
            let error = match status {
                "WAIT" => format!("ASR server busy (est. wait {detail} min)"),
                "ERROR" => format!("ASR server error: {detail}"),
                _ => return None,
            };
            return Some(TranscriptDelta {
                committed: self.committed.clone(),
                error: Some(error),
                ..TranscriptDelta::default()
            });
        }
        if msg.get("message").and_then(|m| m.as_str()) == Some("DISCONNECT") {
            return Some(TranscriptDelta {
                committed: self.committed.clone(),
                error: Some("ASR server closed the session (max connection time)".into()),
                ..TranscriptDelta::default()
            });
        }

        let segs = msg.get("segments").and_then(|s| s.as_array())?;
        let mut interim = String::new();
        let mut any_completed = false;
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
                any_completed = true;
                self.lock_segment(segment_start(s), &text);
            } else {
                // Last interim wins; hold back replacement-glyph noise from a
                // decode that split a multibyte char (committed text is final
                // by contract and left untouched).
                interim = text.chars().filter(|c| *c != '\u{FFFD}').collect();
            }
        }
        if !any_completed && interim.is_empty() {
            return None;
        }
        Some(TranscriptDelta {
            committed: self.committed.clone(),
            interim,
            ..TranscriptDelta::default()
        })
    }
}

/// Build the WhisperLive config handshake (the connection's first message).
///
/// `audio_format=int16` so s16le frames forward verbatim. The protocol has no
/// auth field — `[voice.streaming] api_key` is meaningless for this dialect.
///
/// `vocabulary` (when set) goes out as both `hotwords` and `initial_prompt`, the
/// two fields WhisperLive's own client documents as "domain vocabulary or names"
/// and forwards to faster-whisper — so the user's proper nouns get *recognized*
/// rather than find-and-replaced after the fact. Absent keys keep the server's
/// own defaults, so an unset vocabulary changes the wire not at all.
///
/// Pure so the wire shape is host-testable without a WebSocket.
fn build_handshake(model: &str, language: Option<String>, vocabulary: &str) -> serde_json::Value {
    let model = if model.trim().is_empty() {
        DEFAULT_MODEL
    } else {
        model.trim()
    };
    let mut handshake = serde_json::json!({
        "uid": uuid::Uuid::new_v4().to_string(),
        "language": language,
        "task": "transcribe",
        "model": model,
        "use_vad": true,
        "send_last_n_segments": 10,
        "audio_format": "int16"
    });
    let vocabulary = vocabulary.trim();
    if !vocabulary.is_empty() {
        handshake["hotwords"] = serde_json::Value::String(vocabulary.to_string());
        handshake["initial_prompt"] = serde_json::Value::String(vocabulary.to_string());
    }
    handshake
}

/// WS-connecting adapter speaking the native WhisperLive protocol.
pub struct WhisperLiveStream {
    target: StreamingTarget,
}

impl WhisperLiveStream {
    #[must_use]
    pub fn new(target: StreamingTarget) -> Self {
        Self { target }
    }
}

#[async_trait]
impl StreamingTranscriber for WhisperLiveStream {
    async fn open(&self, cfg: StreamConfig) -> anyhow::Result<StreamHandles> {
        let url = self
            .target
            .base_url
            .replace("https://", "wss://")
            .replace("http://", "ws://");
        let (ws, _) = tokio_tungstenite::connect_async(url).await?;
        let (mut sink, mut stream) = ws.split();
        let handshake = build_handshake(
            &self.target.model,
            cfg.language.or_else(|| self.target.language.clone()),
            &self.target.vocabulary,
        );
        sink.send(Message::Text(handshake.to_string().into()))
            .await?;

        let (audio_tx, mut audio_rx) = mpsc::channel::<Vec<u8>>(64);
        let (delta_tx, delta_rx) = mpsc::channel::<TranscriptDelta>(64);
        tokio::spawn(async move {
            let mut dec = WhisperLiveDecoder::default();
            loop {
                tokio::select! {
                    frame = audio_rx.recv() => match frame {
                        Some(bytes) => {
                            if sink.send(Message::Binary(bytes.into())).await.is_err() {
                                break;
                            }
                        }
                        None => {
                            let _ = sink.send(Message::Binary(b"END_OF_AUDIO".to_vec().into())).await;
                            break;
                        }
                    },
                    msg = stream.next() => match msg {
                        Some(Ok(Message::Text(t))) => {
                            if let Ok(v) = serde_json::from_str::<serde_json::Value>(t.as_str()) {
                                if let Some(d) = dec.push(&v) {
                                    let fatal = d.error.is_some();
                                    if delta_tx.send(d).await.is_err() || fatal {
                                        // Fatal server status: stop bridging so the
                                        // relay publishes its terminal closed event.
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

    fn seg_at(start: f64, text: &str, completed: bool) -> serde_json::Value {
        serde_json::json!({
            "start": format!("{start:.3}"),
            "end": format!("{:.3}", start + 1.0),
            "text": text,
            "completed": completed
        })
    }
    /// Segment WITHOUT a `start` timestamp — exercises the text-identity
    /// fallback for off-spec servers.
    fn seg(text: &str, completed: bool) -> serde_json::Value {
        serde_json::json!({ "text": text, "completed": completed })
    }
    fn msg(segs: Vec<serde_json::Value>) -> serde_json::Value {
        serde_json::json!({ "segments": segs })
    }

    #[test]
    fn completed_segments_lock_into_committed_last_floats() {
        let mut dec = WhisperLiveDecoder::default();
        // two completed + one trailing interim
        let d = dec
            .push(&msg(vec![
                seg_at(0.0, "你好", true),
                seg_at(1.0, "世界", true),
                seg_at(2.0, "现", false),
            ]))
            .unwrap();
        assert_eq!(d.committed, "你好世界");
        assert_eq!(d.interim, "现");
    }

    #[test]
    fn committed_dedup_does_not_double_count_across_messages() {
        let mut dec = WhisperLiveDecoder::default();
        let _ = dec.push(&msg(vec![
            seg_at(0.0, "你好", true),
            seg_at(1.0, "在", false),
        ]));
        // next message re-sends the same completed segment (send_last_n_segments window)
        let d = dec
            .push(&msg(vec![
                seg_at(0.0, "你好", true),
                seg_at(1.0, "世界", true),
                seg_at(2.0, "吗", false),
            ]))
            .unwrap();
        assert_eq!(d.committed, "你好世界");
        assert_eq!(d.interim, "吗");
    }

    #[test]
    fn sliding_window_does_not_drop_new_segments() {
        // Regression: with `send_last_n_segments = N`, once more than N
        // segments complete the server resends only the trailing N. The old
        // positional dedup compared window index against the absolute locked
        // list and dropped every later segment. Timestamp identity must keep
        // locking new segments after the window slides.
        let mut dec = WhisperLiveDecoder::default();
        // Segments 0..3 complete (window of 3 for brevity).
        let _ = dec.push(&msg(vec![
            seg_at(0.0, "一", true),
            seg_at(1.0, "二", true),
            seg_at(2.0, "三", true),
        ]));
        // Window slid: resends 1..3 plus the NEW segment 4.
        let d = dec
            .push(&msg(vec![
                seg_at(1.0, "二", true),
                seg_at(2.0, "三", true),
                seg_at(3.0, "四", true),
            ]))
            .unwrap();
        assert_eq!(d.committed, "一二三四", "segment after the slide must lock");
        // Another slide with two new segments.
        let d = dec
            .push(&msg(vec![
                seg_at(3.0, "四", true),
                seg_at(4.0, "五", true),
                seg_at(5.0, "六", true),
            ]))
            .unwrap();
        assert_eq!(d.committed, "一二三四五六");
    }

    #[test]
    fn english_segments_join_with_spaces() {
        let mut dec = WhisperLiveDecoder::default();
        let d = dec
            .push(&msg(vec![
                seg_at(0.0, "Hello there.", true),
                seg_at(1.0, "How are you", true),
            ]))
            .unwrap();
        assert_eq!(d.committed, "Hello there. How are you");
    }

    #[test]
    fn server_ready_message_is_ignored() {
        let mut dec = WhisperLiveDecoder::default();
        let ready = serde_json::json!({ "message": "SERVER_READY", "backend": "faster_whisper" });
        assert!(dec.push(&ready).is_none());
    }

    #[test]
    fn wait_status_surfaces_as_error() {
        let mut dec = WhisperLiveDecoder::default();
        let wait = serde_json::json!({ "uid": "u", "status": "WAIT", "message": 2.5 });
        let d = dec.push(&wait).unwrap();
        assert!(d.error.as_deref().unwrap_or("").contains("busy"));
    }

    #[test]
    fn error_status_and_disconnect_surface_as_error() {
        let mut dec = WhisperLiveDecoder::default();
        let err = serde_json::json!({ "uid": "u", "status": "ERROR", "message": "boom" });
        assert!(dec.push(&err).unwrap().error.is_some());
        let bye = serde_json::json!({ "uid": "u", "message": "DISCONNECT" });
        assert!(dec.push(&bye).unwrap().error.is_some());
    }

    #[test]
    fn all_completed_no_interim_still_emits() {
        let mut dec = WhisperLiveDecoder::default();
        let d = dec
            .push(&msg(vec![
                seg_at(0.0, "你好", true),
                seg_at(1.0, "世界", true),
            ]))
            .unwrap();
        assert_eq!(d.committed, "你好世界");
        assert_eq!(d.interim, "");
        assert!(!d.utterance_end);
    }

    #[test]
    fn missing_start_falls_back_to_text_identity() {
        let mut dec = WhisperLiveDecoder::default();
        let _ = dec.push(&msg(vec![seg("你好", true)]));
        // Resend of the same (start-less) segment must not double-append…
        let d = dec.push(&msg(vec![seg("你好", true)])).unwrap();
        assert_eq!(d.committed, "你好");
        // …while different text still locks.
        let d = dec.push(&msg(vec![seg("世界", true)])).unwrap();
        assert_eq!(d.committed, "你好世界");
    }

    /// Numeric `start` (TensorRT backend / forks) must be read as a timestamp,
    /// not fall through to text identity — which would drop the second of two
    /// segments that happen to carry the same words.
    #[test]
    fn numeric_start_is_a_valid_timestamp() {
        let numeric = |start: f64, text: &str| serde_json::json!({ "start": start, "end": start + 1.0, "text": text, "completed": true });
        assert_eq!(segment_start(&numeric(1.5, "x")), Some(1.5));
        assert_eq!(segment_start(&seg_at(2.25, "x", true)), Some(2.25));
        assert_eq!(segment_start(&seg("x", true)), None);

        let mut dec = WhisperLiveDecoder::default();
        let _ = dec.push(&msg(vec![numeric(0.0, "嗯")]));
        // Same words, later timestamp: a genuine repetition, must lock again.
        let d = dec.push(&msg(vec![numeric(3.0, "嗯")])).unwrap();
        assert_eq!(d.committed, "嗯嗯");
        // Window resend of an already-locked segment still dedups.
        let d = dec.push(&msg(vec![numeric(3.0, "嗯")])).unwrap();
        assert_eq!(d.committed, "嗯嗯");
    }

    #[test]
    fn handshake_carries_the_wire_contract() {
        let h = build_handshake("", Some("zh".into()), "");
        // audio_format=int16 is what lets us forward s16le frames verbatim.
        assert_eq!(h["audio_format"], "int16");
        assert_eq!(h["task"], "transcribe");
        assert_eq!(h["language"], "zh");
        assert_eq!(h["use_vad"], true);
        // Empty model → the modest default, never an empty string on the wire.
        assert_eq!(h["model"], DEFAULT_MODEL);
        assert_eq!(
            build_handshake("  large-v3 ", None, "")["model"],
            "large-v3"
        );
        assert!(h["language"].is_string());
        assert!(build_handshake("", None, "")["language"].is_null());
    }

    #[test]
    fn vocabulary_rides_the_handshake_only_when_configured() {
        // Absent keys keep the server's own defaults — an unset vocabulary must
        // not change the wire at all.
        let bare = build_handshake("", None, "   ");
        assert!(bare.get("hotwords").is_none());
        assert!(bare.get("initial_prompt").is_none());

        // Both fields, because WhisperLive forwards them to faster-whisper for
        // two different purposes (hotword boost vs decoder priming).
        let biased = build_handshake("", None, "Aleph, Leptos");
        assert_eq!(biased["hotwords"], "Aleph, Leptos");
        assert_eq!(biased["initial_prompt"], "Aleph, Leptos");
    }

    #[test]
    fn interim_replacement_glyphs_are_held_back() {
        let mut dec = WhisperLiveDecoder::default();
        let d = dec
            .push(&msg(vec![seg_at(0.0, "你\u{FFFD}好\u{FFFD}", false)]))
            .unwrap();
        assert_eq!(d.interim, "你好");
    }

    // -----------------------------------------------------------------------
    // Empty-bytes defense pin (2026-08-16 round)
    //
    // Pins the existing defensive code against the WLK "empty bytes trigger
    // end-of-audio" anti-pattern and serde-parse failures. These are NOT new
    // code paths — the decoder already handles them safely — but adding the
    // tests prevents future edits from regressing the contract.
    // -----------------------------------------------------------------------

    #[test]
    fn empty_segments_array_emits_no_delta() {
        let mut dec = WhisperLiveDecoder::default();
        // The decoder returns `None` when there are no segments to lock — the
        // stream task's `if let Some(d) = dec.push(&v)` arm treats it as "no
        // delta to publish", neither committing nor erroring.
        let d = dec.push(&serde_json::json!({
            "uid": "u",
            "segments": [],
            "is_final": true
        }));
        assert!(d.is_none(), "empty segments must yield no delta (no error)");
    }

    #[test]
    fn null_envelope_emits_no_delta() {
        let mut dec = WhisperLiveDecoder::default();
        // serde_json::Value::Null has no `status`, `message`, or `segments`
        // keys — the decoder's `?` on `.as_array()` returns None, which the
        // stream task treats as "no delta to publish" (no error, no row).
        let d = dec.push(&serde_json::Value::Null);
        assert!(d.is_none(), "null envelope must yield no delta");
    }

    #[test]
    fn empty_string_text_is_skipped() {
        let mut dec = WhisperLiveDecoder::default();
        let d = dec
            .push(&msg(vec![
                seg_at(0.0, "", true),     // empty completed
                seg_at(1.0, "你好", true), // real followed by empty
            ]))
            .unwrap();
        assert_eq!(d.committed, "你好");
    }
}
