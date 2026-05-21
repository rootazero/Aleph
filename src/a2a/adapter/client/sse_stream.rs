use std::pin::Pin;
use std::time::Duration;

use futures::Stream;

use crate::a2a::domain::*;
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
pub fn parse_sse_response(
    response: reqwest::Response,
    idle: Duration,
) -> Pin<Box<dyn Stream<Item = A2AResult<UpdateEvent>> + Send>> {
    parse_sse_byte_stream(response.bytes_stream(), idle)
}

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

            // Prepend any incomplete bytes from the previous chunk
            let mut raw_bytes: Vec<u8> = std::mem::take(&mut carry);
            raw_bytes.extend_from_slice(line_buf.as_bytes());
            raw_bytes.extend_from_slice(chunk.as_ref());
            line_buf.clear();
            // Decode as much valid UTF-8 as possible, keeping incomplete trailing bytes
            match String::from_utf8(raw_bytes) {
                Ok(s) => line_buf = s,
                Err(e) => {
                    let valid_up_to = e.utf8_error().valid_up_to();
                    let bytes = e.into_bytes();
                    // Safe: valid_up_to is guaranteed to be a valid UTF-8 boundary
                    line_buf = String::from_utf8(bytes[..valid_up_to].to_vec())
                        .unwrap_or_default();
                    // Store incomplete trailing bytes for the next chunk
                    carry = bytes[valid_up_to..].to_vec();
                }
            }

            while let Some(newline_pos) = line_buf.find('\n') {
                let line = line_buf[..newline_pos].trim_end_matches('\r').to_string();
                line_buf = line_buf[newline_pos + 1..].to_string();

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
                    if !data_buf.is_empty() {
                        data_buf.push('\n');
                    }
                    data_buf.push_str(value.trim());
                }
                // Ignore other fields (id:, retry:, comments)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::a2a::domain::message::{Artifact, Part};
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
}
