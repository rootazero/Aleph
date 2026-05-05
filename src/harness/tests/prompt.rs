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

#[cfg(test)]
mod prop {
    use super::*;
    use proptest::prelude::*;

    /// Property: regardless of the order/content of UserMessage events
    /// before the tail boundary, `DefaultPromptBuilder` never panics
    /// and always produces a `Vec<UnifiedMessage>` whose length is
    /// `<= events.len() + 1` (the +1 accounts for the optionally
    /// reconstructed assistant turn — irrelevant here since this case
    /// has no AssistantMessage events, but kept as the upper bound
    /// invariant of the assemble function).
    proptest! {
        #![proptest_config(ProptestConfig::with_cases(64))]
        #[test]
        fn assemble_is_total_for_user_only_logs(
            texts in proptest::collection::vec("[a-z ]{0,40}", 0..16),
        ) {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();

            let events: Vec<SessionEventRecord> = texts
                .iter()
                .map(|t| user_msg(t))
                .collect();

            // tail_start = 0 for user-only logs (no assistant message).
            let ctx = TurnContext::new(&events, 0);
            let out = rt
                .block_on(DefaultPromptBuilder.assemble(&ctx))
                .expect("assemble must not error on user-only logs");

            prop_assert!(out.len() <= events.len() + 1);
            // Every output for user-only logs must itself be a User msg
            // since there's no assistant turn to reconstruct.
            for msg in &out {
                let is_user = matches!(msg, UnifiedMessage::User { .. });
                prop_assert!(is_user);
            }
        }
    }
}

/// Sanity benchmark — not an assertion; run with
/// `cargo test -p alephcore --lib harness::tests::prompt::perf_dispatch_overhead_documented -- --ignored --nocapture`
/// to print timings. We document this rather than assert because trait
/// dispatch is one vtable jump and any measurable regression would show
/// up in the broader gateway-level perf suite.
#[tokio::test]
#[ignore]
async fn perf_dispatch_overhead_documented() {
    use std::time::Instant;
    let events: Vec<SessionEventRecord> =
        (0..1000).map(|i| user_msg(&format!("m{i}"))).collect();
    let ctx = TurnContext::new(&events, 0);

    let start = Instant::now();
    for _ in 0..1000 {
        let _ = DefaultPromptBuilder.assemble(&ctx).await;
    }
    let elapsed = start.elapsed();
    eprintln!("1000 × assemble(1000 events) = {elapsed:?}");
}
