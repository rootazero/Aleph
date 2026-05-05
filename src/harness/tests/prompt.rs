//! Shape tests for `DefaultPromptBuilder` — verify the assembler produces
//! the expected message structure for common session event patterns.
//! (UnifiedMessage and SessionEvent both lack PartialEq, so structural
//! pattern matching is the comparison vehicle.)

use crate::harness::prompt::{DefaultPromptBuilder, PromptBuilder, TurnContext};
use crate::providers::message::UnifiedMessage;
use crate::session::events::{
    now_ms, MessageContent, SessionEvent, SessionEventRecord, ToolOutput, ToolOutputMetadata,
};
use serde_json::json;
use uuid::Uuid;

fn record(event: SessionEvent) -> SessionEventRecord {
    SessionEventRecord {
        seq: 0,
        event,
        created_at_ms: now_ms(),
    }
}

fn user_msg(text: &str) -> SessionEventRecord {
    record(SessionEvent::UserMessage {
        turn_id: Uuid::nil(),
        content: MessageContent {
            text: text.to_string(),
            blocks: vec![],
        },
        at: now_ms(),
    })
}

fn assistant_with_tool_use(tool_id: &str, tool_name: &str) -> SessionEventRecord {
    record(SessionEvent::AssistantMessage {
        turn_id: Uuid::nil(),
        content: MessageContent {
            text: String::new(),
            blocks: vec![json!({
                "type": "tool_use",
                "id": tool_id,
                "name": tool_name,
                "input": {}
            })],
        },
        at: now_ms(),
    })
}

fn tool_call_requested(tool_id: &str, tool_name: &str) -> SessionEventRecord {
    record(SessionEvent::ToolCallRequested {
        turn_id: Uuid::nil(),
        call_id: tool_id.to_string(),
        name: tool_name.to_string(),
        input: json!({}),
        at: now_ms(),
    })
}

fn tool_result(call_id: &str, value: serde_json::Value) -> SessionEventRecord {
    record(SessionEvent::ToolResult {
        turn_id: Uuid::nil(),
        call_id: call_id.to_string(),
        output: ToolOutput {
            value,
            metadata: ToolOutputMetadata::default(),
        },
        at: now_ms(),
    })
}

#[tokio::test]
async fn empty_log_yields_empty_messages() {
    let events: Vec<SessionEventRecord> = Vec::new();
    let ctx = TurnContext::new(&events, 0);
    let out = DefaultPromptBuilder.assemble(&ctx).await.expect("ok");
    assert!(out.is_empty());
}

#[tokio::test]
async fn single_user_message_passes_through() {
    let events = vec![user_msg("hello")];
    let ctx = TurnContext::new(&events, 0);
    let out = DefaultPromptBuilder.assemble(&ctx).await.expect("ok");
    assert_eq!(out.len(), 1);
    match &out[0] {
        UnifiedMessage::User { content } => {
            assert_eq!(content.len(), 1);
            match &content[0] {
                crate::providers::message::ContentBlock::Text { text, .. } => {
                    assert_eq!(text, "hello");
                }
                other => panic!("expected Text block, got {other:?}"),
            }
        }
        other => panic!("expected User message, got {other:?}"),
    }
}

#[tokio::test]
async fn assistant_then_tool_result_reconstructs_prior_turn() {
    // 4-event fixture: user → tool_call_requested → assistant(tool_use) → tool_result
    let events = vec![
        user_msg("fetch the weather"),
        tool_call_requested("c1", "weather"),
        assistant_with_tool_use("c1", "weather"),
        tool_result("c1", json!({"temp": 70})),
    ];

    // tail_start = position immediately AFTER the last AssistantMessage so it
    // is reconstructed from events[tail_start - 1] and the ToolResult is
    // walked from events[tail_start..].
    let tail_start = events
        .iter()
        .rposition(|r| matches!(r.event, SessionEvent::AssistantMessage { .. }))
        .map(|i| i + 1)
        .unwrap_or(0);

    let ctx = TurnContext::new(&events, tail_start);
    let new_output = DefaultPromptBuilder.assemble(&ctx).await.expect("ok");

    // Shape: 1 reconstructed Assistant + 1 ToolResult
    assert_eq!(new_output.len(), 2);

    // First message: reconstructed Assistant turn with the tool_use block
    match &new_output[0] {
        UnifiedMessage::Assistant { content } => {
            assert_eq!(content.len(), 1);
            match &content[0] {
                crate::providers::message::ContentBlock::ToolCall { id, name, .. } => {
                    assert_eq!(id, "c1");
                    assert_eq!(name, "weather");
                }
                other => panic!("expected ToolCall, got {other:?}"),
            }
        }
        other => panic!("expected Assistant, got {other:?}"),
    }

    // Second message: ToolResult
    match &new_output[1] {
        UnifiedMessage::ToolResult { tool_call_id, tool_name, is_error, .. } => {
            assert_eq!(tool_call_id, "c1");
            assert_eq!(tool_name, "weather");
            assert!(!is_error);
        }
        other => panic!("expected ToolResult, got {other:?}"),
    }
}
