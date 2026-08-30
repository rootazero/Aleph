use std::pin::Pin;
use std::time::Duration;

use futures::Stream;

use crate::a2a::domain::{
    A2AError, Artifact, Part, TaskArtifactUpdateEvent, TaskState, TaskStatusUpdateEvent,
    UpdateEvent,
};
use crate::a2a::port::A2AResult;

/// Parse an SSE HTTP response body into a stream of `UpdateEvent`s.
///
/// `idle` bounds the time between byte chunks: any silence longer than `idle`
/// (including the absence of SSE keep-alive comments) ends the stream with
/// `A2AError::Timeout`.
///
/// SSE format:
/// ```text
/// event: status-update
/// data: {"jsonrpc":"2.0","id":1,"result":{...}}
///
/// event: artifact-update
/// data: {"jsonrpc":"2.0","id":1,"result":{...}}
/// ```
#[must_use]
pub fn parse_sse_response(
    response: reqwest::Response,
    idle: Duration,
) -> Pin<Box<dyn Stream<Item = A2AResult<UpdateEvent>> + Send>> {
    parse_sse_byte_stream(response.bytes_stream(), idle)
}

/// Hard upper bounds for the SSE parser's buffers.
///
/// SSE has no per-line / per-event length cap in the spec; without these
/// limits a malicious or buggy peer can grow `line_buf` / `data_buf`
/// without bound (no newline ever sent) and OOM the process.
const MAX_LINE_BYTES: usize = 64 * 1024;
const MAX_DATA_BYTES: usize = 1024 * 1024;

/// Idle-timeout-wrapped SSE parser over a raw byte stream.
///
/// Generic over the chunk type so tests can drive it with synthetic
/// `Vec<u8>` chunks without a live HTTP connection.
pub(crate) fn parse_sse_byte_stream<S, C>(
    byte_stream: S,
    idle: Duration,
) -> Pin<Box<dyn Stream<Item = A2AResult<UpdateEvent>> + Send>>
where
    S: Stream<Item = reqwest::Result<C>> + Send + 'static,
    C: AsRef<[u8]> + Send + 'static,
{
    let stream = async_stream::stream! {
        use futures::StreamExt;
        let mut event_type = String::new();
        let mut data_buf = String::new();
        let mut line_buf = String::new();
        // Carry buffer for incomplete UTF-8 trailing bytes across chunks
        let mut carry: Vec<u8> = Vec::new();

        tokio::pin!(byte_stream);

        loop {
            let next = tokio::time::timeout(idle, byte_stream.next()).await;
            let chunk = match next {
                Ok(Some(Ok(c))) => c,
                Ok(Some(Err(e))) => {
                    yield Err(A2AError::InternalError(e.to_string()));
                    break;
                }
                Ok(None) => break,
                Err(_elapsed) => {
                    yield Err(A2AError::Timeout(idle));
                    break;
                }
            };

            // `line_buf` already holds the valid UTF-8 decoded so far, which is
            // the earliest content in wire order. The carried incomplete bytes
            // from the previous chunk come AFTER `line_buf` and BEFORE the new
            // chunk. Decode only `carry + chunk` and append the result to
            // `line_buf` to preserve wire order. (Prepending `carry` ahead of
            // `line_buf` would splice a partial line in front of the bytes that
            // complete a chunk-split multi-byte character — e.g. CJK text —
            // permanently corrupting the decode and dropping all later events.)
            let mut raw_bytes: Vec<u8> = std::mem::take(&mut carry);
            raw_bytes.extend_from_slice(chunk.as_ref());
            // Decode as much valid UTF-8 as possible, keeping incomplete trailing bytes
            match String::from_utf8(raw_bytes) {
                Ok(s) => line_buf.push_str(&s),
                Err(e) => {
                    let valid_up_to = e.utf8_error().valid_up_to();
                    let bytes = e.into_bytes();
                    // Safe: valid_up_to is guaranteed to be a valid UTF-8 boundary
                    let valid_bytes = bytes
                        .get(..valid_up_to)
                        .expect("invariant: valid_up_to is a valid UTF-8 boundary");
                    line_buf.push_str(std::str::from_utf8(valid_bytes).unwrap_or_default());
                    // Store incomplete trailing bytes for the next chunk
                    carry = bytes
                        .get(valid_up_to..)
                        .expect("invariant: valid_up_to is a valid UTF-8 boundary")
                        .to_vec();
                }
            }

            while let Some(newline_pos) = line_buf.find('\n') {
                let line = line_buf[..newline_pos].trim_end_matches('\r').to_string();
                line_buf = line_buf[newline_pos + 1..].to_string();

                if line.len() > MAX_LINE_BYTES {
                    yield Err(A2AError::ParseError(format!(
                        "SSE line exceeded {MAX_LINE_BYTES} bytes — refusing to buffer further"
                    )));
                    return;
                }

                if line.is_empty() {
                    // Empty line = end of event
                    if !data_buf.is_empty() {
                        if let Some(err) = parse_error_frame(&data_buf) {
                            yield Err(err);
                            return;
                        }
                        if let Some(event) = parse_event(&event_type, &data_buf) {
                            yield Ok(event);
                        }
                        event_type.clear();
                        data_buf.clear();
                    }
                } else if let Some(value) = line.strip_prefix("event:") {
                    event_type = value.trim().to_string();
                } else if let Some(value) = line.strip_prefix("data:") {
                    if data_buf.len() + value.trim().len() + 1 > MAX_DATA_BYTES {
                        yield Err(A2AError::ParseError(format!(
                            "SSE data buffer exceeded {MAX_DATA_BYTES} bytes — refusing to buffer further"
                        )));
                        return;
                    }
                    if !data_buf.is_empty() {
                        data_buf.push('\n');
                    }
                    data_buf.push_str(value.trim());
                }
                // Ignore other fields (id:, retry:, comments)
            }
            // If a single chunk contained no newline but is itself too big,
            // reject before the next iteration can grow `line_buf` further.
            if line_buf.len() > MAX_LINE_BYTES {
                yield Err(A2AError::ParseError(format!(
                    "SSE line buffer exceeded {MAX_LINE_BYTES} bytes — refusing to buffer further"
                )));
                return;
            }
        }

        // Handle any remaining buffered event (no trailing newline)
        if !data_buf.is_empty() {
            if let Some(err) = parse_error_frame(&data_buf) {
                yield Err(err);
            } else if let Some(event) = parse_event(&event_type, &data_buf) {
                yield Ok(event);
            }
        }
    };

    Box::pin(stream)
}

/// Detect a JSON-RPC error envelope in an SSE `data` payload.
///
/// Returns `Some` only when the payload carries a non-null `error` object.
fn parse_error_frame(data: &str) -> Option<A2AError> {
    let value: serde_json::Value = serde_json::from_str(data).ok()?;
    let error = value.get("error")?;
    if error.is_null() {
        return None;
    }
    let message = error
        .get("message")
        .and_then(|m| m.as_str())
        .unwrap_or("remote A2A streaming error");
    Some(A2AError::InternalError(message.to_string()))
}

/// Parse one SSE `data` payload into an `UpdateEvent`.
///
/// A2A SSE streams carry a JSON-RPC Response envelope per event
/// (`{"jsonrpc","id","result":<event>}`). The `result` is unwrapped first; a
/// bare event (no envelope) is tolerated for interop with non-Aleph agents.
/// Aleph's server sends a `kind`-tagged `UpdateEvent`, so that is tried
/// directly; spec agents send a bare event, disambiguated via the SSE
/// `event:` line.
fn parse_event(event_type: &str, data: &str) -> Option<UpdateEvent> {
    let value: serde_json::Value = serde_json::from_str(data).ok()?;
    let payload = value.get("result").cloned().unwrap_or(value);

    if let Ok(event) = serde_json::from_value::<UpdateEvent>(payload.clone()) {
        return Some(event);
    }

    match event_type {
        "status-update" | "status_update" => {
            serde_json::from_value::<TaskStatusUpdateEvent>(payload)
                .ok()
                .map(UpdateEvent::StatusUpdate)
        }
        "artifact-update" | "artifact_update" => {
            serde_json::from_value::<TaskArtifactUpdateEvent>(payload)
                .ok()
                .map(UpdateEvent::ArtifactUpdate)
        }
        _ => None, // Skip keep-alive, unknown events
    }
}

/// Result of folding an A2A `UpdateEvent` stream into a delegation outcome.
#[derive(Debug, Clone)]
pub struct FoldedOutcome {
    /// The remote agent's response text (artifacts, else the last status message).
    pub summary: String,
    /// Whether the remote task completed successfully.
    pub success: bool,
    /// Failure reason when `success` is false.
    pub error: Option<String>,
}

/// Concatenate the text parts of an artifact, newline-separated.
fn artifact_text(artifact: &Artifact) -> String {
    artifact
        .parts
        .iter()
        .filter_map(|p| match p {
            Part::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Consume an A2A `UpdateEvent` stream into a [`FoldedOutcome`].
///
/// Accumulates artifact text and the last status message; `on_chunk` is fired
/// with each new text fragment so callers can surface live progress. A stream
/// `Err` or a terminal `Failed`/`Rejected`/`Canceled` state yields
/// `success = false`.
pub async fn fold_stream<S, F>(stream: S, mut on_chunk: F) -> FoldedOutcome
where
    S: Stream<Item = A2AResult<UpdateEvent>> + Send,
    F: FnMut(&str),
{
    use futures::StreamExt;

    tokio::pin!(stream);
    let mut artifacts: Vec<String> = Vec::new();
    let mut last_status_text: Option<String> = None;
    let mut final_state: Option<TaskState> = None;
    let mut stream_error: Option<String> = None;

    while let Some(item) = stream.next().await {
        match item {
            Ok(UpdateEvent::StatusUpdate(ev)) => {
                if let Some(msg) = &ev.status.message {
                    let text = msg.text_content();
                    if !text.is_empty() {
                        on_chunk(&text);
                        last_status_text = Some(text);
                    }
                }
                if ev.is_final || ev.status.state.is_terminal() {
                    final_state = Some(ev.status.state);
                }
            }
            Ok(UpdateEvent::ArtifactUpdate(ev)) => {
                let text = artifact_text(&ev.artifact);
                if !text.is_empty() {
                    on_chunk(&text);
                    artifacts.push(text);
                }
            }
            Err(e) => {
                stream_error = Some(e.to_string());
                break;
            }
        }
    }

    let got_terminal = final_state.is_some();
    let failed = matches!(
        final_state,
        Some(TaskState::Failed | TaskState::Rejected | TaskState::Canceled)
    );
    let success = stream_error.is_none() && !failed && got_terminal;

    let body = if !artifacts.is_empty() {
        artifacts.join("\n")
    } else {
        last_status_text.unwrap_or_default()
    };

    let (summary, error) = if let Some(e) = stream_error {
        let summary = if body.is_empty() { e.clone() } else { body };
        (summary, Some(e))
    } else if failed {
        let msg = if body.is_empty() {
            format!("Remote A2A task ended in state {final_state:?}")
        } else {
            body
        };
        (msg.clone(), Some(msg))
    } else {
        let summary = if body.is_empty() {
            "Remote A2A task completed with no textual output".to_string()
        } else {
            body
        };
        (summary, None)
    };

    FoldedOutcome {
        summary,
        success,
        error,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::a2a::domain::message::{A2AMessage, A2ARole, Artifact, Part};
    use crate::a2a::domain::task::{TaskState, TaskStatus};
    use chrono::Utc;

    fn make_status_json() -> String {
        let event = TaskStatusUpdateEvent {
            task_id: "task-1".to_string(),
            context_id: "ctx-1".to_string(),
            status: TaskStatus {
                state: TaskState::Working,
                message: None,
                timestamp: Utc::now(),
            },
            is_final: false,
            metadata: None,
        };
        serde_json::to_string(&event).unwrap()
    }

    fn make_artifact_json() -> String {
        let event = TaskArtifactUpdateEvent {
            task_id: "task-2".to_string(),
            context_id: "ctx-2".to_string(),
            artifact: Artifact {
                artifact_id: "art-1".to_string(),
                kind: "code".to_string(),
                parts: vec![Part::Text {
                    text: "fn main() {}".to_string(),
                    metadata: None,
                }],
                metadata: None,
            },
            append: false,
            last_chunk: true,
            metadata: None,
        };
        serde_json::to_string(&event).unwrap()
    }

    #[test]
    fn parse_event_status_update() {
        let json = make_status_json();
        let result = parse_event("status-update", &json);
        assert!(matches!(result, Some(UpdateEvent::StatusUpdate(_))));
    }

    #[test]
    fn parse_event_status_update_underscore() {
        let json = make_status_json();
        let result = parse_event("status_update", &json);
        assert!(matches!(result, Some(UpdateEvent::StatusUpdate(_))));
    }

    #[test]
    fn parse_event_artifact_update() {
        let json = make_artifact_json();
        let result = parse_event("artifact-update", &json);
        assert!(matches!(result, Some(UpdateEvent::ArtifactUpdate(_))));
    }

    #[test]
    fn parse_event_artifact_update_underscore() {
        let json = make_artifact_json();
        let result = parse_event("artifact_update", &json);
        assert!(matches!(result, Some(UpdateEvent::ArtifactUpdate(_))));
    }

    #[test]
    fn parse_event_unknown_type_returns_none() {
        let json = make_status_json();
        assert!(parse_event("keep-alive", &json).is_none());
        assert!(parse_event("", &json).is_none());
        assert!(parse_event("unknown", &json).is_none());
    }

    #[test]
    fn parse_event_invalid_json_returns_none() {
        assert!(parse_event("status-update", "not json").is_none());
        assert!(parse_event("artifact-update", "{broken").is_none());
    }

    #[test]
    fn parse_event_unwraps_jsonrpc_envelope_status() {
        let ev: TaskStatusUpdateEvent = serde_json::from_str(&make_status_json()).unwrap();
        let envelope = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": UpdateEvent::StatusUpdate(ev),
        })
        .to_string();
        let result = parse_event("status-update", &envelope);
        assert!(matches!(result, Some(UpdateEvent::StatusUpdate(_))));
    }

    #[test]
    fn parse_event_unwraps_jsonrpc_envelope_artifact() {
        let ev: TaskArtifactUpdateEvent = serde_json::from_str(&make_artifact_json()).unwrap();
        let envelope = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": UpdateEvent::ArtifactUpdate(ev),
        })
        .to_string();
        let result = parse_event("artifact-update", &envelope);
        assert!(matches!(result, Some(UpdateEvent::ArtifactUpdate(_))));
    }

    #[test]
    fn parse_event_extracts_correct_fields() {
        let json = make_status_json();
        if let Some(UpdateEvent::StatusUpdate(ev)) = parse_event("status-update", &json) {
            assert_eq!(ev.task_id, "task-1");
            assert_eq!(ev.context_id, "ctx-1");
            assert!(!ev.is_final);
        } else {
            panic!("Expected StatusUpdate");
        }

        let json = make_artifact_json();
        if let Some(UpdateEvent::ArtifactUpdate(ev)) = parse_event("artifact-update", &json) {
            assert_eq!(ev.task_id, "task-2");
            assert!(ev.last_chunk);
            assert!(!ev.append);
        } else {
            panic!("Expected ArtifactUpdate");
        }
    }

    #[test]
    fn parse_error_frame_detects_jsonrpc_error() {
        let frame =
            r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32011,"message":"no matching agent"}}"#;
        let err = parse_error_frame(frame);
        assert!(err.is_some());
        assert!(err.unwrap().to_string().contains("no matching agent"));
    }

    #[test]
    fn parse_error_frame_ignores_success_envelope() {
        let frame = r#"{"jsonrpc":"2.0","id":1,"result":{"kind":"status-update"}}"#;
        assert!(parse_error_frame(frame).is_none());
        assert!(parse_error_frame("not json").is_none());
    }

    #[tokio::test]
    async fn parse_sse_byte_stream_idle_timeout_yields_timeout_error() {
        use futures::StreamExt;

        let first_chunk = format!("event: status-update\ndata: {}\n\n", make_status_json());
        let byte_stream = futures::stream::iter(vec![Ok::<Vec<u8>, reqwest::Error>(
            first_chunk.into_bytes(),
        )])
        .chain(futures::stream::pending::<Result<Vec<u8>, reqwest::Error>>());

        let parsed = parse_sse_byte_stream(byte_stream, std::time::Duration::from_millis(80));
        tokio::pin!(parsed);

        let first = parsed.next().await;
        assert!(matches!(first, Some(Ok(UpdateEvent::StatusUpdate(_)))));

        let second = parsed.next().await;
        assert!(matches!(second, Some(Err(A2AError::Timeout(_)))));

        assert!(parsed.next().await.is_none());
    }

    #[tokio::test]
    async fn parse_sse_byte_stream_multibyte_char_split_across_chunks() {
        // Regression: a chunk boundary that lands BOTH mid-multi-byte-char AND
        // with a pending partial line in `line_buf` must not corrupt the decode.
        // CJK message text exercises the multi-byte path (Aleph is CJK-first).
        use futures::StreamExt;

        let event = TaskStatusUpdateEvent {
            task_id: "任务-1".to_string(),
            context_id: "上下文-1".to_string(),
            status: TaskStatus {
                state: TaskState::Completed,
                message: Some(A2AMessage::text(A2ARole::Agent, "分析完成：黄金走势看涨")),
                timestamp: Utc::now(),
            },
            is_final: true,
            metadata: None,
        };
        let json = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": UpdateEvent::StatusUpdate(event),
        })
        .to_string();
        let frame = format!("event: status-update\ndata: {}\n\n", json);
        let bytes = frame.as_bytes();

        // Split at the first byte index that is NOT a char boundary AND falls
        // after the first '\n' (so "event: status-update" is already a complete
        // line and "data: {partial" sits in line_buf when the split char arrives).
        let split = (1..bytes.len())
            .find(|&i| !frame.is_char_boundary(i) && bytes[..i].contains(&b'\n'))
            .expect("frame must contain a multi-byte char after a newline");
        let chunk1 = bytes[..split].to_vec();
        let chunk2 = bytes[split..].to_vec();

        let byte_stream = futures::stream::iter(vec![
            Ok::<Vec<u8>, reqwest::Error>(chunk1),
            Ok::<Vec<u8>, reqwest::Error>(chunk2),
        ]);
        let parsed = parse_sse_byte_stream(byte_stream, std::time::Duration::from_secs(5));
        let events: Vec<_> = parsed.collect().await;

        assert_eq!(events.len(), 1, "exactly one event must survive the split");
        match &events[0] {
            Ok(UpdateEvent::StatusUpdate(e)) => {
                assert_eq!(e.task_id, "任务-1");
                assert_eq!(
                    e.status.message.as_ref().unwrap().text_content(),
                    "分析完成：黄金走势看涨",
                    "multi-byte message text must be reconstructed intact"
                );
            }
            other => panic!("expected one StatusUpdate, got {:?}", other),
        }
    }

    fn status_event(state: TaskState, msg: Option<&str>, is_final: bool) -> UpdateEvent {
        UpdateEvent::StatusUpdate(TaskStatusUpdateEvent {
            task_id: "t".to_string(),
            context_id: "c".to_string(),
            status: TaskStatus {
                state,
                message: msg.map(|m| A2AMessage::text(A2ARole::Agent, m)),
                timestamp: Utc::now(),
            },
            is_final,
            metadata: None,
        })
    }

    #[tokio::test]
    async fn fold_stream_success_uses_final_message() {
        let events: Vec<A2AResult<UpdateEvent>> = vec![
            Ok(status_event(TaskState::Working, None, false)),
            Ok(status_event(
                TaskState::Completed,
                Some("final answer"),
                true,
            )),
        ];
        let mut chunks: Vec<String> = Vec::new();
        let outcome = fold_stream(futures::stream::iter(events), |c| {
            chunks.push(c.to_string())
        })
        .await;
        assert!(outcome.success);
        assert_eq!(outcome.summary, "final answer");
        assert!(outcome.error.is_none());
        assert_eq!(chunks, vec!["final answer".to_string()]);
    }

    #[tokio::test]
    async fn fold_stream_failed_state_is_unsuccessful() {
        let events: Vec<A2AResult<UpdateEvent>> =
            vec![Ok(status_event(TaskState::Failed, Some("boom"), true))];
        let outcome = fold_stream(futures::stream::iter(events), |_| {}).await;
        assert!(!outcome.success);
        assert_eq!(outcome.error.as_deref(), Some("boom"));
    }

    #[tokio::test]
    async fn fold_stream_artifact_accumulates_text() {
        let artifact_ev = UpdateEvent::ArtifactUpdate(TaskArtifactUpdateEvent {
            task_id: "t".to_string(),
            context_id: "c".to_string(),
            artifact: Artifact {
                artifact_id: "a".to_string(),
                kind: "text".to_string(),
                parts: vec![Part::Text {
                    text: "part one".to_string(),
                    metadata: None,
                }],
                metadata: None,
            },
            append: false,
            last_chunk: true,
            metadata: None,
        });
        let events: Vec<A2AResult<UpdateEvent>> = vec![
            Ok(artifact_ev),
            Ok(status_event(TaskState::Completed, None, true)),
        ];
        let outcome = fold_stream(futures::stream::iter(events), |_| {}).await;
        assert!(outcome.success);
        assert_eq!(outcome.summary, "part one");
    }

    #[tokio::test]
    async fn fold_stream_error_item_fails() {
        let events: Vec<A2AResult<UpdateEvent>> =
            vec![Err(A2AError::Timeout(std::time::Duration::from_secs(1)))];
        let outcome = fold_stream(futures::stream::iter(events), |_| {}).await;
        assert!(!outcome.success);
        assert!(outcome.error.is_some());
    }
}
