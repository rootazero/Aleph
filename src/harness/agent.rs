//! AgentHarness — the concrete Think→Act implementation.
//!
//! Task 8 implements the Think half of the loop. The Act half (Task 9) will
//! replace the `// TODO(Task 9)` stub at the end of `run_turn`.

use async_trait::async_trait;

use crate::harness::deps::HarnessDeps;
use crate::harness::trait_def::{Harness, HarnessError, TurnState};
use crate::providers::adapter::RequestPayload;
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
}

#[async_trait]
impl Harness for AgentHarness {
    async fn run_turn(&self, session_id: &SessionId) -> Result<TurnState, HarnessError> {
        // 1. Fetch event tail since the last AssistantMessage (or full log on first turn).
        let events = self.deps.session.get_events(session_id, None, None).await?;
        let tail = tail_since_last_assistant(&events);

        // 2. Build the LLM request from the tail.
        let messages = build_prompt(tail);
        let payload = RequestPayload::new(&messages);

        // 3. Call the LLM.
        let response = self.deps.llm.process(payload).await?;

        // 4. Emit AssistantMessage with the response text.
        let turn_id = current_turn_id(&events);
        let text = response.text_content();
        let assistant_event = SessionEvent::AssistantMessage {
            turn_id,
            content: MessageContent {
                text,
                blocks: Vec::new(),
            },
            at: now_ms(),
        };
        self.deps
            .session
            .emit_event(session_id, assistant_event)
            .await?;

        // 5. Decide whether to continue.
        if response.tool_calls.is_empty() {
            Ok(TurnState::Done)
        } else {
            // TODO(Task 9): dispatch Act phase — execute each tool_call via
            // `deps.tools` / `deps.sandbox`, emit ToolResult events, and
            // return `TurnState::Continue` only after the Act half completes.
            Ok(TurnState::Continue)
        }
    }
}

/// Return the slice of events after the most recent `AssistantMessage`.
///
/// If no `AssistantMessage` exists, the full tail is returned. This mirrors
/// pi-mono's "events since last assistant turn" semantics.
fn tail_since_last_assistant(events: &[SessionEventRecord]) -> &[SessionEventRecord] {
    let cut = events
        .iter()
        .rposition(|r| matches!(r.event, SessionEvent::AssistantMessage { .. }));
    match cut {
        Some(idx) => &events[idx + 1..],
        None => events,
    }
}

/// Dumb shape transform: `SessionEvent` tail → `Vec<UnifiedMessage>`.
///
/// Only `UserMessage` and `ToolResult` events are carried into the prompt.
/// The Act phase (Task 9) will also emit Assistant tool_call messages; those
/// will feed back through the session log on subsequent Think calls.
fn build_prompt(tail: &[SessionEventRecord]) -> Vec<UnifiedMessage> {
    let mut messages = Vec::with_capacity(tail.len());
    for record in tail {
        match &record.event {
            SessionEvent::UserMessage { content, .. } => {
                messages.push(UnifiedMessage::user(&content.text));
            }
            SessionEvent::ToolResult {
                call_id, output, ..
            } => {
                // NOTE: `SessionEvent::ToolResult` does not carry the tool name;
                // Task 9 should either thread it through here or resolve it from
                // the preceding `ToolCallRequested` event.
                let text = serde_json::to_string(&output.value).unwrap_or_default();
                messages.push(UnifiedMessage::ToolResult {
                    tool_call_id: call_id.clone(),
                    tool_name: "unknown".to_string(),
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
