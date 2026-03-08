use std::pin::Pin;

use futures::Stream;

use crate::a2a::domain::*;
use crate::a2a::port::A2AResult;

/// Parse an SSE response body into a stream of UpdateEvents.
///
/// SSE format:
/// ```text
/// event: status-update
/// data: {"taskId":"...","status":{...}}
///
/// event: artifact-update
/// data: {"taskId":"...","artifact":{...}}
/// ```
pub fn parse_sse_response(
    response: reqwest::Response,
) -> Pin<Box<dyn Stream<Item = A2AResult<UpdateEvent>> + Send>> {
    let byte_stream = response.bytes_stream();

    let stream = async_stream::stream! {
        use futures::StreamExt;
        let mut event_type = String::new();
        let mut data_buf = String::new();
        let mut line_buf = String::new();

        tokio::pin!(byte_stream);

        while let Some(chunk_result) = byte_stream.next().await {
            let chunk = match chunk_result {
                Ok(c) => c,
                Err(e) => {
                    yield Err(A2AError::InternalError(e.to_string()));
                    break;
                }
            };

            let text = String::from_utf8_lossy(&chunk);
            line_buf.push_str(&text);

            while let Some(newline_pos) = line_buf.find('\n') {
                let line = line_buf[..newline_pos].trim_end_matches('\r').to_string();
                line_buf = line_buf[newline_pos + 1..].to_string();

                if line.is_empty() {
                    // Empty line = end of event
                    if !data_buf.is_empty() {
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
            if let Some(event) = parse_event(&event_type, &data_buf) {
                yield Ok(event);
            }
        }
    };

    Box::pin(stream)
}

fn parse_event(event_type: &str, data: &str) -> Option<UpdateEvent> {
    match event_type {
        "status-update" | "status_update" => serde_json::from_str::<TaskStatusUpdateEvent>(data)
            .ok()
            .map(UpdateEvent::StatusUpdate),
        "artifact-update" | "artifact_update" => {
            serde_json::from_str::<TaskArtifactUpdateEvent>(data)
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
}
