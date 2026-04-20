//! AgentHarness — the concrete Think→Act implementation.
//!
//! Task 8 implemented the Think half of the loop. Task 9 adds:
//!   * Act dispatch (executing tool_calls sequentially, emitting ToolResult /
//!     ToolError events).
//!   * Preservation of assistant `tool_use` intent inside `AssistantMessage`
//!     events so later Think cycles can reconstruct the conversation.
//!   * Full-history `build_prompt` that re-emits the preceding assistant
//!     tool_use turn and resolves real tool names for `ToolResult` messages.

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::harness::callback::{HarnessCallback, NoopHarnessCallback};
use crate::harness::deps::HarnessDeps;
use crate::harness::trait_def::{Harness, HarnessError, TurnState};
use crate::providers::adapter::{NativeToolCall, RequestPayload};
use crate::providers::message::{ContentBlock, UnifiedMessage};
use crate::session::events::{now_ms, MessageContent, SessionEvent, SessionEventRecord, TurnId};
use crate::session::service::SessionId;

pub struct AgentHarness {
    deps: HarnessDeps,
}

impl AgentHarness {
    pub fn new(deps: HarnessDeps) -> Self {
        Self { deps }
    }

    /// Convenience: wrap this harness as an `Arc<dyn SessionDriver>` so it
    /// can be stored in containers that don't depend on the concrete type.
    pub fn into_session_driver(self) -> std::sync::Arc<dyn crate::session::SessionDriver> {
        std::sync::Arc::new(self)
    }

    /// Act phase: execute each tool_call sequentially, emitting a
    /// `ToolCallRequested` event before every call and either a `ToolResult`
    /// or `ToolError` event after. A failure short-circuits the rest of the
    /// batch and surfaces as `HarnessError::Tool`.
    async fn act(
        &self,
        session_id: &SessionId,
        turn_id: TurnId,
        tool_calls: Vec<NativeToolCall>,
        callback: &mut dyn HarnessCallback,
    ) -> Result<(), HarnessError> {
        for call in tool_calls {
            callback.on_tool_call(&call.name);
            let requested = SessionEvent::ToolCallRequested {
                turn_id,
                call_id: call.id.clone(),
                name: call.name.clone(),
                input: call.arguments.clone(),
                at: now_ms(),
            };
            self.deps.session.emit_event(session_id, requested).await?;

            match self.deps.tools.execute(&call.name, call.arguments).await {
                Ok(output) => {
                    let result_event = SessionEvent::ToolResult {
                        turn_id,
                        call_id: call.id,
                        output,
                        at: now_ms(),
                    };
                    self.deps
                        .session
                        .emit_event(session_id, result_event)
                        .await?;
                }
                Err(e) => {
                    // Attempt to persist ToolError; log but do NOT shadow the
                    // original tool failure if the session write itself fails.
                    // Using `?` here would convert a `SessionError` into
                    // `HarnessError::Session` and hide the underlying tool
                    // error from the caller.
                    let error_event = SessionEvent::ToolError {
                        turn_id,
                        call_id: call.id.clone(),
                        error: e.to_string(),
                        at: now_ms(),
                    };
                    if let Err(emit_err) =
                        self.deps.session.emit_event(session_id, error_event).await
                    {
                        tracing::warn!(
                            ?session_id,
                            call_id = %call.id,
                            ?emit_err,
                            "failed to persist ToolError event",
                        );
                    }
                    return Err(HarnessError::Tool(e));
                }
            }
        }
        Ok(())
    }
}

#[async_trait]
impl crate::session::SessionDriver for AgentHarness {
    async fn drive(&self, session_id: &SessionId) -> crate::error::Result<()> {
        // Preserve `HarnessError` variant semantics when lifting into the
        // crate-wide `AlephError`:
        //   * `Cancelled` → `AlephError::Cancelled` (downstream UI branches
        //     on this to render "Operation cancelled." rather than a
        //     provider-failure banner).
        //   * `Llm(inner)` → unwrap: `inner` is already an `AlephError`, so
        //     re-wrapping would hide the structured `AuthenticationError`
        //     / `NetworkError` / etc. from callers that `match` on it.
        //   * `Tool` / `Session` → no better variant exists on `AlephError`;
        //     stringify through `provider` with a discriminating prefix.
        // Exhaustive match (no wildcard) so new `HarnessError` variants
        // force a review here.
        let mut cb = NoopHarnessCallback;
        self.run(session_id, &mut cb).await.map_err(|e| match e {
            HarnessError::Cancelled => crate::error::AlephError::Cancelled,
            HarnessError::Llm(inner) => inner,
            HarnessError::Tool(tool_err) => {
                crate::error::AlephError::provider(format!("harness tool error: {tool_err}"))
            }
            HarnessError::Session(sess_err) => {
                crate::error::AlephError::provider(format!("harness session error: {sess_err}"))
            }
        })
    }
}

#[async_trait]
impl Harness for AgentHarness {
    async fn run_turn(
        &self,
        session_id: &SessionId,
        callback: &mut dyn HarnessCallback,
    ) -> Result<TurnState, HarnessError> {
        // 1. Fetch full event log and compute the tail boundary.
        let events = self.deps.session.get_events(session_id, None, None).await?;
        let tail_start = tail_start_index(&events);

        // 2. Build the LLM request. `build_prompt` has access to the full log
        //    so it can reconstruct the preceding assistant tool_use turn and
        //    resolve tool names for tool_result messages.
        let messages = build_prompt(&events, tail_start);
        let payload = RequestPayload::new(&messages);

        // 3. Call the LLM.
        let response = self.deps.llm.process(payload).await?;

        // 4. Emit AssistantMessage preserving any tool_use intent in `blocks`.
        let turn_id = current_turn_id(&events);
        let text = response.text_content();
        if !text.is_empty() {
            // Non-streaming LLM layer emits one chunk per turn; the callback
            // shape permits finer chunking once `process_stream` is wired.
            callback.on_delta(&text);
        }
        let blocks = tool_use_blocks(&response.tool_calls);
        let assistant_event = SessionEvent::AssistantMessage {
            turn_id,
            content: MessageContent { text, blocks },
            at: now_ms(),
        };
        self.deps
            .session
            .emit_event(session_id, assistant_event)
            .await?;

        // 5. If the LLM produced tool_calls, run the Act phase; otherwise done.
        if response.tool_calls.is_empty() {
            Ok(TurnState::Done)
        } else {
            self.act(session_id, turn_id, response.tool_calls, callback)
                .await?;
            Ok(TurnState::Continue)
        }
    }
}

/// Index at which events "since the last AssistantMessage" begin.
///
/// Returns `events.len()` when the log ends with an AssistantMessage and
/// `0` when there is none. The caller uses this both as the start of the
/// tail slice and as the boundary for reconstructing the prior assistant
/// turn.
fn tail_start_index(events: &[SessionEventRecord]) -> usize {
    events
        .iter()
        .rposition(|r| matches!(r.event, SessionEvent::AssistantMessage { .. }))
        .map(|idx| idx + 1)
        .unwrap_or(0)
}

/// Serialize each `NativeToolCall` as a JSON `tool_use` block so the full
/// assistant intent is preserved in the session log.
///
/// `pub(crate)` so round-trip tests can exercise the writer/reader pair.
pub(crate) fn tool_use_blocks(tool_calls: &[NativeToolCall]) -> Vec<Value> {
    tool_calls
        .iter()
        .map(|c| {
            json!({
                "type": "tool_use",
                "id": c.id,
                "name": c.name,
                "input": c.arguments,
            })
        })
        .collect()
}

/// Parse a previously persisted `tool_use` JSON block back into a
/// `ContentBlock::ToolCall`. Returns `None` for blocks that don't match
/// the shape written by `tool_use_blocks`.
///
/// `pub(crate)` so round-trip tests can exercise the writer/reader pair.
pub(crate) fn parse_tool_use_block(block: &Value) -> Option<ContentBlock> {
    let obj = block.as_object()?;
    if obj.get("type").and_then(Value::as_str) != Some("tool_use") {
        return None;
    }
    let id = obj.get("id").and_then(Value::as_str)?.to_string();
    let name = obj.get("name").and_then(Value::as_str)?.to_string();
    let arguments = obj.get("input").cloned().unwrap_or(Value::Null);
    Some(ContentBlock::ToolCall {
        id,
        name,
        arguments,
    })
}

/// Build the prompt messages from the full event log.
///
/// Shape of the output:
///   1. Any `UserMessage` events that precede the last `AssistantMessage`
///      (current turn's boundary) are not replayed — only events at or after
///      the tail boundary are carried forward, EXCEPT for the immediately
///      preceding `AssistantMessage` itself which is reconstructed as an
///      `Assistant` turn so the model sees its own prior `tool_use` request.
///   2. The reconstructed assistant turn (if any) goes first.
///   3. Then the tail events (`UserMessage`, `ToolResult`) in log order.
///
/// Tool names for `ToolResult` entries are resolved by scanning backwards
/// through the full log for the matching `ToolCallRequested`. Falls back
/// to `"unknown"` only if no such event exists.
fn build_prompt(events: &[SessionEventRecord], tail_start: usize) -> Vec<UnifiedMessage> {
    let mut messages = Vec::new();

    // Reconstruct the preceding assistant turn (if any) so the model sees
    // its own tool_use request in context.
    if tail_start > 0 {
        if let SessionEvent::AssistantMessage { content, .. } = &events[tail_start - 1].event {
            let mut blocks: Vec<ContentBlock> = Vec::new();
            if !content.text.is_empty() {
                blocks.push(ContentBlock::Text {
                    text: content.text.clone(),
                    cache_control: None,
                });
            }
            for raw in &content.blocks {
                if let Some(tc) = parse_tool_use_block(raw) {
                    blocks.push(tc);
                }
            }
            if !blocks.is_empty() {
                messages.push(UnifiedMessage::Assistant { content: blocks });
            }
        }
    }

    // Walk the tail and emit UserMessage / ToolResult entries.
    for (offset, record) in events[tail_start..].iter().enumerate() {
        match &record.event {
            SessionEvent::UserMessage { content, .. } => {
                messages.push(UnifiedMessage::user(&content.text));
            }
            SessionEvent::ToolResult {
                call_id, output, ..
            } => {
                // Resolve the tool name by searching strictly BEFORE this
                // ToolResult, so matching `call_id`s in later turns cannot
                // win over the correct in-turn `ToolCallRequested`.
                let tool_result_idx = tail_start + offset;
                let tool_name =
                    resolve_tool_name(events, tool_result_idx, call_id).unwrap_or("unknown");
                let text = serde_json::to_string(&output.value).unwrap_or_default();
                messages.push(UnifiedMessage::ToolResult {
                    tool_call_id: call_id.clone(),
                    tool_name: tool_name.to_string(),
                    content: vec![ContentBlock::Text {
                        text,
                        cache_control: None,
                    }],
                    is_error: false,
                });
            }
            _ => {}
        }
    }

    messages
}

/// Find the `ToolCallRequested.name` whose `call_id` matches, searching
/// strictly BEFORE `before_idx` (i.e. within `events[..before_idx]`).
///
/// Scanning from the end of the log would return the most recent match
/// anywhere, which is wrong when two turns reuse the same `call_id`
/// (provider retries, deterministic ID schemes). Bounding the search to
/// the segment preceding the `ToolResult` guarantees we pick the matching
/// request from the same turn.
fn resolve_tool_name<'a>(
    events: &'a [SessionEventRecord],
    before_idx: usize,
    call_id: &str,
) -> Option<&'a str> {
    let upper = before_idx.min(events.len());
    events[..upper].iter().rev().find_map(|r| match &r.event {
        SessionEvent::ToolCallRequested {
            call_id: id, name, ..
        } if id == call_id => Some(name.as_str()),
        _ => None,
    })
}

/// Find the most recent `TurnStarted` id; generate a fresh one if none exists.
fn current_turn_id(events: &[SessionEventRecord]) -> TurnId {
    events
        .iter()
        .rev()
        .find_map(|r| match &r.event {
            SessionEvent::TurnStarted { turn_id, .. } => Some(*turn_id),
            _ => None,
        })
        .unwrap_or_else(uuid::Uuid::new_v4)
}
