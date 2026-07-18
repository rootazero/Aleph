# A2A Card Refresh + Streaming Outbound Delegation — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Refresh config-declared A2A agents' placeholder cards at startup, and make outbound A2A delegation consume the remote agent's SSE stream instead of the flat-timeout sync endpoint.

**Architecture:** Item 1 adds a one-shot background task (`card_refresh.rs`) that fetches each registered agent's real Agent Card and upserts it. Item 2 fixes the broken `sse_stream.rs` parser (JSON-RPC envelope unwrap), adds an idle-timeout, a `fold_stream` consumer, and an `A2AClient::send_message_stream` method; `A2ASubAgent::dispatch` becomes streaming-first with transparent sync fallback. A 2-line server-side `Sse::keep_alive` makes the idle-timeout reliable.

**Tech Stack:** Rust, tokio, reqwest (`stream` feature), axum SSE, `async_stream`, `wiremock` (dev) for HTTP test stubs.

---

## Setup (before Task 1)

Create the implementation worktree using the `superpowers:using-git-worktrees` skill (or manually):
branch `a2a-card-refresh-streaming` off `main` (HEAD `f334bd176`).

After the worktree exists, copy the design + plan docs into it and commit them as the first commit:

```bash
# from inside the worktree
mkdir -p docs/superpowers/specs docs/superpowers/plans
cp <main-repo>/docs/superpowers/specs/2026-05-21-a2a-card-refresh-streaming-design.md docs/superpowers/specs/
cp <main-repo>/docs/superpowers/plans/2026-05-21-a2a-card-refresh-streaming.md docs/superpowers/plans/
git add docs/superpowers/
git commit -m "docs: A2A card-refresh + streaming design and plan"
```

Then delete the two untracked copies from the main repo working dir so `main` stays clean.

## Cargo Concurrency Rule (applies to EVERY cargo command in this plan)

Before running any `cargo check` / `cargo test` / `cargo build` / `cargo clippy`:

```bash
ps aux | grep -E 'cargo|rustc' | grep -v grep | wc -l
```

If the count is **≥ 3**, wait and re-check until it drops below 3, then run your cargo command. The machine allows at most 3 concurrent cargo compiles.

---

## Task 1: Fix `parse_event` — unwrap the JSON-RPC envelope

Aleph's A2A server sends each SSE `data` line as a JSON-RPC Response envelope
(`{"jsonrpc","id","result":<event>}`), but `parse_event` deserializes `data`
directly as a bare event — so it cannot parse Aleph's own stream. Fix the
internals; the signature and behavior on bare events are preserved.

**Files:**
- Modify: `src/a2a/adapter/client/sse_stream.rs:96-108` (the `parse_event` fn)
- Test: `src/a2a/adapter/client/sse_stream.rs` (`#[cfg(test)] mod tests`)

- [ ] **Step 1: Write the failing tests**

Add these tests inside `mod tests` in `src/a2a/adapter/client/sse_stream.rs`
(after `parse_event_invalid_json_returns_none`):

```rust
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
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p alephcore --lib -- a2a::adapter::client::sse_stream`
Expected: the two new tests FAIL (the current `parse_event` tries
`from_str::<TaskStatusUpdateEvent>` on the envelope and gets `None`).

- [ ] **Step 3: Rewrite `parse_event`**

Replace the entire `parse_event` function (`src/a2a/adapter/client/sse_stream.rs:96-108`)
with:

```rust
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
        _ => None,
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p alephcore --lib -- a2a::adapter::client::sse_stream`
Expected: PASS — all sse_stream tests green (the 7 original + 2 new). The
original 7 still pass because a bare event has no `result` key, so `payload`
falls back to the whole value and the `event_type` match handles it.

- [ ] **Step 5: Commit**

```bash
git add src/a2a/adapter/client/sse_stream.rs
git commit -m "a2a: fix sse_stream parse_event to unwrap JSON-RPC envelope"
```

---

## Task 2: Split `parse_sse_response`, add idle-timeout + error-frame detection

`parse_sse_response` becomes a thin wrapper over a generic, testable
`parse_sse_byte_stream` that applies a per-chunk idle-timeout. A JSON-RPC
error frame in the stream is surfaced as `Err` instead of being silently
skipped.

**Files:**
- Modify: `src/a2a/adapter/client/sse_stream.rs` (imports, `parse_sse_response`, new `parse_sse_byte_stream`, new `parse_error_frame`)
- Test: `src/a2a/adapter/client/sse_stream.rs` (`mod tests`)

- [ ] **Step 1: Write the failing tests**

Add to `mod tests` in `src/a2a/adapter/client/sse_stream.rs`:

```rust
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
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p alephcore --lib -- a2a::adapter::client::sse_stream`
Expected: FAIL to compile — `parse_error_frame` and `parse_sse_byte_stream`
do not exist yet.

- [ ] **Step 3: Add the `Duration` import**

At the top of `src/a2a/adapter/client/sse_stream.rs`, add after `use std::pin::Pin;`:

```rust
use std::time::Duration;
```

- [ ] **Step 4: Replace `parse_sse_response` with the split + idle-timeout version**

Replace the entire `parse_sse_response` function (`src/a2a/adapter/client/sse_stream.rs:18-94`)
with the following two functions:

```rust
/// Parse an SSE HTTP response body into a stream of `UpdateEvent`s.
///
/// `idle` bounds the time between byte chunks: any silence longer than `idle`
/// (including the absence of SSE keep-alive comments) ends the stream with
/// `A2AError::Timeout`.
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
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p alephcore --lib -- a2a::adapter::client::sse_stream`
Expected: PASS — all sse_stream tests green, including the 3 new ones.

- [ ] **Step 6: Commit**

```bash
git add src/a2a/adapter/client/sse_stream.rs
git commit -m "a2a: add SSE idle-timeout + JSON-RPC error-frame detection"
```

---

## Task 3: Add `fold_stream` + `FoldedOutcome`

`fold_stream` folds an `UpdateEvent` stream into a delegation outcome
(summary / success / error), firing a callback for live progress. Pure — no
dependency on `agents` or `builtin_tools` types.

**Files:**
- Modify: `src/a2a/adapter/client/sse_stream.rs` (add `FoldedOutcome`, `fold_stream`, `artifact_text`)
- Modify: `src/a2a/adapter/client/mod.rs` (re-export)
- Test: `src/a2a/adapter/client/sse_stream.rs` (`mod tests`)

- [ ] **Step 1: Write the failing tests**

Add to `mod tests` in `src/a2a/adapter/client/sse_stream.rs`:

```rust
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
            Ok(status_event(TaskState::Completed, Some("final answer"), true)),
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
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p alephcore --lib -- a2a::adapter::client::sse_stream`
Expected: FAIL to compile — `fold_stream` and `FoldedOutcome` do not exist.

- [ ] **Step 3: Add `FoldedOutcome`, `fold_stream`, `artifact_text`**

Append to `src/a2a/adapter/client/sse_stream.rs`, after `parse_error_frame`
and before `#[cfg(test)]`:

```rust
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

    let failed = matches!(
        final_state,
        Some(TaskState::Failed | TaskState::Rejected | TaskState::Canceled)
    );
    let success = stream_error.is_none() && !failed;

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
            format!("Remote A2A task ended in state {:?}", final_state)
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
```

- [ ] **Step 4: Re-export from the client module**

In `src/a2a/adapter/client/mod.rs`, change:

```rust
pub use sse_stream::parse_sse_response;
```

to:

```rust
pub use sse_stream::{fold_stream, parse_sse_response, FoldedOutcome};
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p alephcore --lib -- a2a::adapter::client::sse_stream`
Expected: PASS — all sse_stream tests green, including the 4 new fold tests.

- [ ] **Step 6: Commit**

```bash
git add src/a2a/adapter/client/sse_stream.rs src/a2a/adapter/client/mod.rs
git commit -m "a2a: add fold_stream to consume SSE UpdateEvent streams"
```

---

## Task 4: Add `A2AClient::send_message_stream`

POSTs `message/send` to `{base_url}/a2a/stream` with `Accept: text/event-stream`
and returns the parsed SSE stream. Non-2xx → `Err` so callers fall back to sync.

**Files:**
- Modify: `src/a2a/adapter/client/http_client.rs` (imports, 2 consts, new method)
- Test: `src/a2a/adapter/client/http_client.rs` (`mod tests`)

- [ ] **Step 1: Write the failing tests**

Add to `mod tests` in `src/a2a/adapter/client/http_client.rs`:

```rust
    use crate::a2a::domain::{A2AMessage, A2ARole, TaskState, TaskStatus, TaskStatusUpdateEvent, UpdateEvent};
    use chrono::Utc;
    use futures::StreamExt;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// Build an SSE body of two enveloped status-update events.
    fn two_event_sse_body() -> String {
        let working = UpdateEvent::StatusUpdate(TaskStatusUpdateEvent {
            task_id: "t".to_string(),
            context_id: "c".to_string(),
            status: TaskStatus {
                state: TaskState::Working,
                message: None,
                timestamp: Utc::now(),
            },
            is_final: false,
            metadata: None,
        });
        let completed = UpdateEvent::StatusUpdate(TaskStatusUpdateEvent {
            task_id: "t".to_string(),
            context_id: "c".to_string(),
            status: TaskStatus {
                state: TaskState::Completed,
                message: Some(A2AMessage::text(A2ARole::Agent, "done")),
                timestamp: Utc::now(),
            },
            is_final: true,
            metadata: None,
        });
        let env = |ev: &UpdateEvent| {
            serde_json::json!({"jsonrpc": "2.0", "id": 1, "result": ev}).to_string()
        };
        format!(
            "event: status-update\ndata: {}\n\nevent: status-update\ndata: {}\n\n",
            env(&working),
            env(&completed)
        )
    }

    #[tokio::test]
    async fn send_message_stream_parses_two_events() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/a2a/stream"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_raw(two_event_sse_body(), "text/event-stream"),
            )
            .mount(&server)
            .await;

        let client = A2AClient::new(server.uri());
        let msg = A2AMessage::text(A2ARole::User, "hi");
        let stream = client
            .send_message_stream("task-1", &msg, None)
            .await
            .expect("stream should open");
        let events: Vec<_> = stream.collect().await;
        assert_eq!(events.len(), 2);
        assert!(events.iter().all(|e| e.is_ok()));
    }

    #[tokio::test]
    async fn send_message_stream_non_2xx_errors() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/a2a/stream"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;

        let client = A2AClient::new(server.uri());
        let msg = A2AMessage::text(A2ARole::User, "hi");
        let err = client
            .send_message_stream("task-1", &msg, None)
            .await
            .unwrap_err();
        assert!(matches!(err, A2AError::AgentUnreachable(_)));
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p alephcore --lib -- a2a::adapter::client::http_client`
Expected: FAIL to compile — `send_message_stream` does not exist.

- [ ] **Step 3: Add imports and constants**

At the top of `src/a2a/adapter/client/http_client.rs`, after `use std::time::Duration;`,
add:

```rust
use std::pin::Pin;

use futures::Stream;
```

After the existing `const DEFAULT_TIMEOUT_SECS: u64 = 120;`, add:

```rust
/// Idle-timeout for the streaming delegation path — silence longer than this
/// (no bytes, no SSE keep-alive) ends the stream.
const STREAM_IDLE_TIMEOUT: Duration = Duration::from_secs(90);

/// Bound on opening the streaming connection (TCP connect + response headers).
const STREAM_OPEN_TIMEOUT: Duration = Duration::from_secs(15);
```

- [ ] **Step 4: Add the `send_message_stream` method**

In `src/a2a/adapter/client/http_client.rs`, inside `impl A2AClient`, add this
method immediately after `send_message` (after line 158):

```rust
    /// Send a message to a remote agent over the streaming endpoint.
    ///
    /// POSTs `message/send` to `{base_url}/a2a/stream` and returns the parsed
    /// SSE stream of `UpdateEvent`s. A non-2xx status (e.g. an agent with no
    /// streaming route) is returned as `Err` so the caller can fall back to
    /// the synchronous `send_message`.
    pub async fn send_message_stream(
        &self,
        task_id: &str,
        message: &A2AMessage,
        session_id: Option<&str>,
    ) -> A2AResult<Pin<Box<dyn Stream<Item = A2AResult<UpdateEvent>> + Send>>> {
        let mut params = json!({
            "taskId": task_id,
            "message": message,
        });
        if let Some(sid) = session_id {
            params["sessionId"] = json!(sid);
        }
        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: uuid::Uuid::new_v4().to_string(),
            method: "message/send".to_string(),
            params,
        };

        let url = format!("{}/a2a/stream", self.base_url);
        let mut builder = self
            .http
            .post(&url)
            .json(&request)
            .header(reqwest::header::ACCEPT, "text/event-stream");
        if let Some(ref token) = self.auth_token {
            builder = builder.bearer_auth(token);
        }

        // Bound connection establishment only — the stream body itself is
        // governed by the per-chunk idle-timeout, not a total timeout.
        let response = tokio::time::timeout(STREAM_OPEN_TIMEOUT, builder.send())
            .await
            .map_err(|_| A2AError::Timeout(STREAM_OPEN_TIMEOUT))?
            .map_err(|e| {
                if e.is_timeout() {
                    A2AError::Timeout(STREAM_OPEN_TIMEOUT)
                } else {
                    A2AError::AgentUnreachable(e.to_string())
                }
            })?;

        if !response.status().is_success() {
            return Err(A2AError::AgentUnreachable(format!(
                "A2A stream endpoint returned HTTP {}",
                response.status()
            )));
        }

        Ok(crate::a2a::adapter::client::parse_sse_response(
            response,
            STREAM_IDLE_TIMEOUT,
        ))
    }
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p alephcore --lib -- a2a::adapter::client::http_client`
Expected: PASS — all http_client tests green, including the 2 new ones.

- [ ] **Step 6: Commit**

```bash
git add src/a2a/adapter/client/http_client.rs
git commit -m "a2a: add A2AClient::send_message_stream for SSE delegation"
```

---

## Task 5: Make `A2ASubAgent::dispatch` streaming-first

`dispatch` tries the streaming endpoint and consumes it via `fold_stream`;
on a stream-open error it falls back to `dispatch_sync` (today's exact body).

**Files:**
- Modify: `src/a2a/sub_agent.rs` (imports, `dispatch`, new `dispatch_sync`)
- Test: `src/a2a/sub_agent.rs` (`mod tests`)

- [ ] **Step 1: Write the failing tests**

Add to `mod tests` in `src/a2a/sub_agent.rs` (after
`execute_delegation_explicit_unknown_agent_reports_name`):

```rust
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn streaming_registered(name: &str, url: &str) -> RegisteredAgent {
        RegisteredAgent {
            card: AgentCard {
                id: "streamer".to_string(),
                name: name.to_string(),
                version: "1.0".to_string(),
                description: None,
                provider: None,
                documentation_url: None,
                interfaces: vec![],
                skills: vec![],
                security: vec![],
                extensions: vec![],
                default_input_modes: vec![],
                default_output_modes: vec![],
            },
            trust_level: TrustLevel::Trusted,
            base_url: url.to_string(),
            last_seen: chrono::Utc::now(),
            health: AgentHealth::Healthy,
            auth_token: None,
        }
    }

    fn sse_completed_body(answer: &str) -> String {
        let completed = UpdateEvent::StatusUpdate(TaskStatusUpdateEvent {
            task_id: "t".to_string(),
            context_id: "c".to_string(),
            status: TaskStatus {
                state: TaskState::Completed,
                message: Some(A2AMessage::text(A2ARole::Agent, answer)),
                timestamp: chrono::Utc::now(),
            },
            is_final: true,
            metadata: None,
        });
        let env =
            serde_json::json!({"jsonrpc": "2.0", "id": 1, "result": completed}).to_string();
        format!("event: status-update\ndata: {}\n\n", env)
    }

    #[tokio::test]
    async fn execute_delegation_streams_remote_result() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/a2a/stream"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_raw(sse_completed_body("streamed answer 42"), "text/event-stream"),
            )
            .mount(&server)
            .await;

        let sub = build_sub_agent(vec![streaming_registered("Streamer", &server.uri())]);
        let outcome = sub
            .execute_delegation("do the thing", Some("Streamer"))
            .await
            .unwrap();
        assert!(outcome.result.success, "got: {:?}", outcome.result);
        assert_eq!(outcome.agent.as_deref(), Some("Streamer"));
        assert!(outcome.result.summary.contains("streamed answer 42"));
    }

    #[tokio::test]
    async fn execute_delegation_falls_back_to_sync_when_no_stream_route() {
        let server = MockServer::start().await;
        // No streaming endpoint.
        Mock::given(method("POST"))
            .and(path("/a2a/stream"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;
        // Synchronous JSON-RPC endpoint returns a completed task.
        let mut task = A2ATask::new("t", "c");
        task.status.state = TaskState::Completed;
        task.history.push(A2AMessage::text(A2ARole::Agent, "sync answer 99"));
        let rpc = serde_json::json!({"jsonrpc": "2.0", "id": "x", "result": task});
        Mock::given(method("POST"))
            .and(path("/a2a"))
            .respond_with(ResponseTemplate::new(200).set_body_json(rpc))
            .mount(&server)
            .await;

        let sub = build_sub_agent(vec![streaming_registered("Streamer", &server.uri())]);
        let outcome = sub
            .execute_delegation("do it", Some("Streamer"))
            .await
            .unwrap();
        assert!(outcome.result.success, "got: {:?}", outcome.result);
        assert!(outcome.result.summary.contains("sync answer 99"));
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p alephcore --lib -- a2a::sub_agent`
Expected: FAIL to compile — `wiremock` symbols unused-then-needed and the
streaming behavior is not yet implemented (the tests will not compile until
the imports resolve; functionally they exercise the new `dispatch`).

- [ ] **Step 3: Update the client import**

In `src/a2a/sub_agent.rs`, change line 8:

```rust
use crate::a2a::adapter::client::A2AClientPool;
```

to:

```rust
use crate::a2a::adapter::client::{fold_stream, A2AClient, A2AClientPool};
```

- [ ] **Step 4: Replace `dispatch` and add `dispatch_sync`**

Replace the entire `dispatch` method (`src/a2a/sub_agent.rs:130-185`) with the
following two methods:

```rust
    /// Send a delegation request to an already-resolved remote agent.
    ///
    /// Streaming-first: consumes the remote agent's SSE stream (idle-timeout
    /// liveness + live progress). When the remote has no `/a2a/stream` route
    /// (non-Aleph agents), transparently falls back to [`Self::dispatch_sync`].
    /// Shared by [`Self::execute`] and [`Self::execute_delegation`].
    async fn dispatch(
        &self,
        agent: &RegisteredAgent,
        request: &SubAgentRequest,
    ) -> crate::error::Result<SubAgentResult> {
        let client = self.client_pool.get_or_create(agent).await.map_err(|e| {
            crate::error::AlephError::other(format!("A2A client creation failed: {}", e))
        })?;

        let message = A2AMessage::text(A2ARole::User, &request.prompt);
        let task_id = uuid::Uuid::new_v4().to_string();

        match client.send_message_stream(&task_id, &message, None).await {
            Ok(stream) => {
                let outcome = fold_stream(stream, |chunk| {
                    crate::builtin_tools::notify_tool_streaming_chunk("a2a_delegate", chunk);
                })
                .await;

                let result = if outcome.success {
                    SubAgentResult::success(request.id.clone(), outcome.summary)
                } else {
                    SubAgentResult::failure(
                        request.id.clone(),
                        outcome
                            .error
                            .unwrap_or_else(|| "A2A streaming delegation failed".to_string()),
                    )
                };

                // Spec 1 G2: record a successful delegation for parent-agent memory.
                if result.success {
                    if let Some(w) = self.raw_memory_writer.clone() {
                        emit_delegation_raw_with_registry(
                            w,
                            request,
                            &result,
                            &agent.card.id,
                            self.capture_registry.clone(),
                        );
                    }
                }
                Ok(result)
            }
            Err(e) => {
                tracing::info!(
                    error = %e,
                    "A2A streaming unavailable; falling back to sync send_message"
                );
                self.dispatch_sync(&client, agent, request, &task_id, &message)
                    .await
            }
        }
    }

    /// Synchronous delegation — POSTs `message/send` and waits for the full
    /// task. Fallback for remote agents without a streaming endpoint.
    async fn dispatch_sync(
        &self,
        client: &A2AClient,
        agent: &RegisteredAgent,
        request: &SubAgentRequest,
        task_id: &str,
        message: &A2AMessage,
    ) -> crate::error::Result<SubAgentResult> {
        match client.send_message(task_id, message, None).await {
            Ok(task) => {
                let summary = if !task.history.is_empty() {
                    task.history
                        .iter()
                        .rev()
                        .find(|m| m.role == A2ARole::Agent)
                        .map(|m| m.text_content())
                        .unwrap_or_else(|| format!("Task {} completed", task.id))
                } else if let Some(ref msg) = task.status.message {
                    msg.text_content()
                } else {
                    format!(
                        "Task {} completed with state: {:?}",
                        task.id, task.status.state
                    )
                };

                let output = serde_json::to_value(&task).unwrap_or_else(|e| {
                    tracing::warn!("Failed to serialize A2ATask: {}", e);
                    serde_json::Value::Null
                });
                let result =
                    SubAgentResult::success(request.id.clone(), summary).with_output(output);

                // Spec 1 G2: record delegation outcome for parent-agent memory.
                if let Some(w) = self.raw_memory_writer.clone() {
                    emit_delegation_raw_with_registry(
                        w,
                        request,
                        &result,
                        &agent.card.id,
                        self.capture_registry.clone(),
                    );
                }

                Ok(result)
            }
            Err(e) => Ok(SubAgentResult::failure(
                request.id.clone(),
                format!("A2A call failed: {}", e),
            )),
        }
    }
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p alephcore --lib -- a2a::sub_agent`
Expected: PASS — all sub_agent tests green, including the 2 new streaming
tests; the existing `execute_*` tests are unaffected (they hit the no-agent
path before `dispatch`).

- [ ] **Step 6: Commit**

```bash
git add src/a2a/sub_agent.rs
git commit -m "a2a: make dispatch streaming-first with sync fallback"
```

---

## Task 6: Add server-side SSE keep-alive

Aleph's `/a2a/stream` handler goes silent between `Working` and the final
event. `Sse::keep_alive` emits comment heartbeats so a streaming client's
idle-timeout stays correct.

**Files:**
- Modify: `src/a2a/adapter/server/routes.rs:7` (import), `:201` and `:208` (the two `Sse::new`)

- [ ] **Step 1: Add the `KeepAlive` import**

In `src/a2a/adapter/server/routes.rs`, change line 7:

```rust
use axum::response::sse::{Event, Sse};
```

to:

```rust
use axum::response::sse::{Event, KeepAlive, Sse};
```

- [ ] **Step 2: Add keep-alive to `a2a_stream_handler`**

In `src/a2a/adapter/server/routes.rs`, change the last line of
`a2a_stream_handler` (line 201):

```rust
    Sse::new(sse_stream).into_response()
```

to:

```rust
    Sse::new(sse_stream)
        .keep_alive(KeepAlive::new())
        .into_response()
```

- [ ] **Step 3: Add keep-alive to `sse_error`**

In `src/a2a/adapter/server/routes.rs`, in the `sse_error` function, change:

```rust
    Sse::new(stream).into_response()
```

to:

```rust
    Sse::new(stream)
        .keep_alive(KeepAlive::new())
        .into_response()
```

- [ ] **Step 4: Verify compilation and existing routes tests**

Run: `cargo test -p alephcore --lib -- a2a::`
Expected: PASS — A2A tests green. Note: `a2a::tests::test_routes_jsonrpc_sync_endpoint`
is a pre-existing baseline failure (unrelated to this change — it fails
identically on `main`); all other a2a tests must pass.

- [ ] **Step 5: Commit**

```bash
git add src/a2a/adapter/server/routes.rs
git commit -m "a2a: emit SSE keep-alive heartbeats on the streaming route"
```

---

## Task 7: Add the startup card-refresh module

A one-shot background task fetches each registered agent's real Agent Card
and upserts it, replacing the config placeholder.

**Files:**
- Create: `src/a2a/service/card_refresh.rs`
- Modify: `src/a2a/service/mod.rs`
- Test: `src/a2a/service/card_refresh.rs` (`mod tests`)

- [ ] **Step 1: Create `card_refresh.rs` with the implementation and failing tests**

Create `src/a2a/service/card_refresh.rs` with:

```rust
//! One-shot startup refresh of placeholder Agent Cards.
//!
//! `CardRegistry::load_from_config` seeds config-declared agents with
//! placeholder cards (no skills/description, `version = "unknown"`). This
//! module fetches each agent's real Agent Card once at startup and upserts it,
//! so smart routing and `a2a_agents list` see real skill data.

use crate::sync_primitives::Arc;

use super::card_registry::CardRegistry;
use crate::a2a::adapter::client::A2AClient;
use crate::a2a::port::{AgentHealth, AgentResolver, RegisteredAgent};
use crate::a2a::sub_agent::A2ASubAgent;

/// Fetch the real Agent Card for every registered agent and upsert it.
///
/// Agents whose card cannot be fetched keep their placeholder entry (still
/// routable by name). Returns the number of cards successfully refreshed.
pub async fn refresh_all_cards(registry: &CardRegistry) -> usize {
    let agents = match registry.list_agents().await {
        Ok(a) => a,
        Err(e) => {
            tracing::warn!(error = %e, "A2A card refresh: failed to list agents");
            return 0;
        }
    };

    let mut refreshed = 0usize;
    for agent in agents {
        let client = match &agent.auth_token {
            Some(token) => A2AClient::with_auth(&agent.base_url, token),
            None => A2AClient::new(&agent.base_url),
        };
        match client.fetch_agent_card().await {
            Ok(card) => {
                registry
                    .upsert(RegisteredAgent {
                        card,
                        trust_level: agent.trust_level,
                        base_url: agent.base_url.clone(),
                        last_seen: chrono::Utc::now(),
                        health: AgentHealth::Healthy,
                        auth_token: agent.auth_token.clone(),
                    })
                    .await;
                refreshed += 1;
            }
            Err(e) => {
                tracing::warn!(
                    agent = %agent.card.name,
                    url = %agent.base_url,
                    error = %e,
                    "A2A card refresh: keeping placeholder card"
                );
            }
        }
    }
    refreshed
}

/// Spawn a background task that runs one card-refresh pass at startup.
///
/// After the pass, refreshes the `A2ASubAgent` name cache so `can_handle`
/// matches newly-discovered skill names and aliases. Non-blocking.
pub fn spawn_card_refresh(registry: Arc<CardRegistry>, sub_agent: Arc<A2ASubAgent>) {
    tokio::spawn(async move {
        let n = refresh_all_cards(&registry).await;
        if n > 0 {
            sub_agent.refresh_agent_names().await;
        }
        tracing::info!(refreshed = n, "A2A startup card refresh complete");
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::a2a::config::{A2AAgentEntry, A2AConfig};
    use crate::a2a::domain::AgentCard;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn config_with_agent(name: &str, url: &str) -> A2AConfig {
        A2AConfig {
            enabled: true,
            agents: vec![A2AAgentEntry {
                name: name.to_string(),
                url: url.to_string(),
                trust_level: None,
                token: None,
            }],
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn refresh_empty_registry_returns_zero() {
        let registry = CardRegistry::new();
        assert_eq!(refresh_all_cards(&registry).await, 0);
    }

    #[tokio::test]
    async fn refresh_unreachable_keeps_placeholder() {
        let registry = CardRegistry::new();
        // 127.0.0.1:1 — nothing listens there.
        registry
            .load_from_config(&config_with_agent("Ghost", "http://127.0.0.1:1"))
            .await;

        assert_eq!(refresh_all_cards(&registry).await, 0);

        let agents = registry.list_agents().await.unwrap();
        assert_eq!(agents.len(), 1);
        assert_eq!(agents[0].card.version, "unknown");
    }

    #[tokio::test]
    async fn refresh_success_replaces_placeholder() {
        let server = MockServer::start().await;
        let real_card = AgentCard {
            id: "real-helper".to_string(),
            name: "Helper".to_string(),
            version: "2.3.1".to_string(),
            description: Some("Real card".to_string()),
            provider: None,
            documentation_url: None,
            interfaces: vec![],
            skills: vec![],
            security: vec![],
            extensions: vec![],
            default_input_modes: vec!["text".to_string()],
            default_output_modes: vec!["text".to_string()],
        };
        Mock::given(method("GET"))
            .and(path("/.well-known/agent-card.json"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&real_card))
            .mount(&server)
            .await;

        let registry = CardRegistry::new();
        registry
            .load_from_config(&config_with_agent("Helper", &server.uri()))
            .await;
        // Placeholder before refresh.
        assert_eq!(
            registry.list_agents().await.unwrap()[0].card.version,
            "unknown"
        );

        let n = refresh_all_cards(&registry).await;
        assert_eq!(n, 1);

        let agents = registry.list_agents().await.unwrap();
        assert_eq!(agents.len(), 1);
        assert_eq!(agents[0].card.version, "2.3.1");
        assert_eq!(agents[0].card.description.as_deref(), Some("Real card"));
    }
}
```

- [ ] **Step 2: Register the module and re-export**

In `src/a2a/service/mod.rs`, add `pub mod card_refresh;` after
`pub mod card_builder;`:

```rust
pub mod card_builder;
pub mod card_refresh;
pub mod card_registry;
```

and add the re-export after `pub use card_builder::CardBuilder;`:

```rust
pub use card_builder::CardBuilder;
pub use card_refresh::{refresh_all_cards, spawn_card_refresh};
```

- [ ] **Step 3: Run the tests to verify they pass**

Run: `cargo test -p alephcore --lib -- a2a::service::card_refresh`
Expected: PASS — all 3 card_refresh tests green.

- [ ] **Step 4: Commit**

```bash
git add src/a2a/service/card_refresh.rs src/a2a/service/mod.rs
git commit -m "a2a: add one-shot startup Agent Card refresh"
```

---

## Task 8: Wire `spawn_card_refresh` into server startup

**Files:**
- Modify: `src/bin/aleph-server/commands/start/mod.rs` (~line 1409)

- [ ] **Step 1: Add the `spawn_card_refresh` call**

In `src/bin/aleph-server/commands/start/mod.rs`, find the A2A init block. After
the `if let Some(ref handle) = a2a_tool_handle { ... } else { ... }` block ends
(the closing `}` near line 1409) and before the `if !args.daemon {` line that
prints "A2A protocol: enabled" (line 1411), insert:

```rust
                // 11. One-shot startup card refresh: upgrade config agents'
                // placeholder cards to their real Agent Cards in the
                // background. Non-blocking — never delays startup.
                alephcore::a2a::service::spawn_card_refresh(
                    card_registry.clone(),
                    a2a_sub_agent.clone(),
                );
```

(`card_registry` and `a2a_sub_agent` are both `Arc<...>` already in scope at
this point — defined earlier in the same block.)

- [ ] **Step 2: Verify compilation**

Run: `cargo check --bin aleph-server`
Expected: PASS — compiles with no errors. (Pre-existing unrelated warnings
from other modules are acceptable; the new code must add none.)

- [ ] **Step 3: Commit**

```bash
git add src/bin/aleph-server/commands/start/mod.rs
git commit -m "a2a: spawn startup card refresh in server init"
```

---

## Task 9: Documentation + full verification

**Files:**
- Modify: `docs/reference/MULTI_AGENT_SYSTEM.md`

- [ ] **Step 1: Update `MULTI_AGENT_SYSTEM.md`**

Open `docs/reference/MULTI_AGENT_SYSTEM.md`, locate the `## Mode 4: A2A (Remote
Agent Delegation)` section, and append this paragraph at the end of that
section (before the next `##` heading):

```markdown
### Outbound transport

Outbound delegation is streaming-first: `a2a_delegate` POSTs to the remote
agent's `/a2a/stream` (SSE) endpoint, consuming `status-update` /
`artifact-update` events with a 90s idle-timeout for liveness and live
progress notifications. A remote agent without a streaming route (non-Aleph
A2A agents) is handled by a transparent fallback to the synchronous
`message/send` endpoint.

Config-declared agents (`[[a2a.agents]]`) start with a placeholder Agent Card.
A one-shot background task at server startup fetches each agent's real Agent
Card (skills, description, version) and replaces the placeholder, so smart
routing and `a2a_agents list` see real skill data.
```

- [ ] **Step 2: Commit the docs**

```bash
git add docs/reference/MULTI_AGENT_SYSTEM.md
git commit -m "docs: note A2A streaming delegation + startup card refresh"
```

- [ ] **Step 3: Full A2A test suite**

Run: `cargo test -p alephcore --lib -- a2a`
Expected: PASS — all A2A tests green **except** the pre-existing baseline
failure `a2a::tests::test_routes_jsonrpc_sync_endpoint` (it fails identically
on `main` `f334bd176`; it is unrelated to this cycle). No *other* failures.

- [ ] **Step 4: Clippy on the touched files**

Run: `cargo clippy -p alephcore --lib 2>&1 | grep -E 'a2a/(adapter/client|service|sub_agent)' || echo "no clippy warnings in touched a2a files"`
Expected: no clippy warnings originating in the files this cycle modified.
(The project has pre-existing clippy warnings elsewhere — ignore those.)

- [ ] **Step 5: Final commit (if clippy required fixes)**

If Step 4 surfaced warnings in the touched files, fix them and commit:

```bash
git add -A
git commit -m "a2a: clippy cleanup for card-refresh + streaming"
```

If no fixes were needed, skip this step.

---

## Self-Review

**Spec coverage:**
- Item 1 `card_refresh.rs` + `spawn_card_refresh` + wiring → Tasks 7, 8. ✓
- Item 1 `mod.rs` exports → Task 7. ✓
- Item 2 `parse_event` envelope bug → Task 1. ✓
- Item 2 idle-timeout + error-frame → Task 2. ✓
- Item 2 `fold_stream` → Task 3. ✓
- Item 2 `send_message_stream` → Task 4. ✓
- Item 2 streaming-first `dispatch` + `dispatch_sync` → Task 5. ✓
- Item 2 server `Sse::keep_alive` → Task 6. ✓
- Docs note → Task 9. ✓
- Testing strategy (every new unit covered) → Tasks 1-7 each ship tests. ✓

**Type consistency:** `parse_sse_response(response, idle)`, `parse_sse_byte_stream<S,C>(byte_stream, idle)`, `fold_stream(stream, on_chunk) -> FoldedOutcome { summary, success, error }`, `A2AClient::send_message_stream(task_id, message, session_id)`, `refresh_all_cards(&CardRegistry) -> usize`, `spawn_card_refresh(Arc<CardRegistry>, Arc<A2ASubAgent>)` — names and signatures are identical across every task that references them.

**Placeholder scan:** No TBD/TODO; every code step shows complete code; every test step shows full test bodies.

**Out of scope (carried forward, per spec):** periodic refresh, `CardRegistry::fetch_card`/`resolve_by_intent` stubs, spec `message/stream`-on-`/a2a` interop.
