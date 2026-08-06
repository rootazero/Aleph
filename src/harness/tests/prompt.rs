//! Shape tests for `build_prompt` — verify the assembler produces
//! the expected message structure for common session event patterns.
//! (UnifiedMessage and SessionEvent both lack PartialEq, so structural
//! pattern matching is the comparison vehicle.)

use crate::harness::agent::prompt::build_prompt;
use crate::providers::message::{ContentBlock, UnifiedMessage};
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
            thinking: None,
            thinking_signature: None,
        },
        at: now_ms(),
        synthetic: false,
        author_user_id: None,
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
            thinking: None,
            thinking_signature: None,
        },
        usage: None,
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

#[test]
fn empty_log_yields_empty_messages() {
    let events: Vec<SessionEventRecord> = Vec::new();
    let out = build_prompt(&events, 0);
    assert!(out.is_empty());
}

#[test]
fn single_user_message_passes_through() {
    let events = vec![user_msg("hello")];
    let out = build_prompt(&events, 0);
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

#[test]
fn assistant_then_tool_result_reconstructs_prior_turn() {
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

    let new_output = build_prompt(&events, tail_start);

    // Shape: the FULL conversation — opening user task + reconstructed
    // Assistant tool_use + ToolResult. The opening user message MUST be
    // preserved (regression: it was previously windowed out, so an
    // autonomous tool loop lost its task on the continuation turn).
    assert_eq!(new_output.len(), 3);

    // First message: the original user task, preserved and unwrapped (it is
    // the conversation opener, before any assistant turn).
    match &new_output[0] {
        UnifiedMessage::User { content } => match &content[0] {
            crate::providers::message::ContentBlock::Text { text, .. } => {
                assert_eq!(text, "fetch the weather");
            }
            other => panic!("expected Text block, got {other:?}"),
        },
        other => panic!("expected User message, got {other:?}"),
    }

    // Second message: reconstructed Assistant turn with the tool_use block
    match &new_output[1] {
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

    // Third message: ToolResult
    match &new_output[2] {
        UnifiedMessage::ToolResult {
            tool_call_id,
            tool_name,
            is_error,
            ..
        } => {
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

    // Property: regardless of the order/content of UserMessage events
    // before the tail boundary, `build_prompt` never panics and always
    // produces a `Vec<UnifiedMessage>` whose length is `<= events.len() + 1`
    // (the +1 accounts for the optionally reconstructed assistant turn —
    // irrelevant here since this case has no AssistantMessage events,
    // but kept as the upper bound invariant of the assemble function).
    proptest! {
        #![proptest_config(ProptestConfig::with_cases(64))]
        #[test]
        fn assemble_is_total_for_user_only_logs(
            texts in proptest::collection::vec("[a-z ]{0,40}", 0..16),
        ) {
            let events: Vec<SessionEventRecord> = texts
                .iter()
                .map(|t| user_msg(t))
                .collect();

            // tail_start = 0 for user-only logs (no assistant message).
            let out = build_prompt(&events, 0);

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

/// REGRESSION: a Panel turn with an image attachment persists the image as a
/// serde-encoded `ContentBlock` on the user message's `blocks` (seeded via
/// `FlowInput::Multimodal`). The rebuilt prompt MUST carry it to the provider —
/// the user arm used to read only `content.text`, so the image never reached
/// the model at all.
///
/// Lives here rather than in `agent/prompt.rs`'s inline `mod tests` to follow
/// the directory convention the harness is converging on (see `448ce1c03`).
#[test]
fn user_message_replays_persisted_image_blocks() {
    let events = vec![record(SessionEvent::UserMessage {
        turn_id: Uuid::new_v4(),
        content: MessageContent {
            text: "what is in this picture?".into(),
            blocks: vec![serde_json::to_value(ContentBlock::Image {
                data: "aGk=".into(),
                mime_type: "image/png".into(),
            })
            .unwrap()],
            thinking: None,
            thinking_signature: None,
        },
        at: now_ms(),
        synthetic: false,
        author_user_id: None,
    })];

    let out = build_prompt(&events, 0);
    let UnifiedMessage::User { content } = &out[0] else {
        panic!("expected a user message, got {:?}", out[0]);
    };
    assert!(
        content.iter().any(
            |b| matches!(b, ContentBlock::Text { text, .. } if text == "what is in this picture?")
        ),
        "the prompt text must survive alongside the image"
    );
    assert!(
        content.iter().any(
            |b| matches!(b, ContentBlock::Image { mime_type, .. } if mime_type == "image/png")
        ),
        "the attached image must reach the provider message"
    );
}

fn system_msg(text: &str) -> SessionEventRecord {
    record(SessionEvent::SystemMessage {
        turn_id: Uuid::new_v4(),
        content: text.to_string(),
        at: now_ms(),
    })
}

/// REGRESSION: `build_prompt` had no `SystemMessage` arm, so the catch-all
/// `_ => {}` swallowed it. The split child's head is exactly this event
/// (`context::compact::session_split::build_summary_event`), so a session that
/// crossed the split threshold woke up with no summary AND no original task —
/// only the orphan-result downgrade note. The OpenAI-compat gateway's client
/// system messages went the same way.
#[test]
fn system_message_is_replayed() {
    // Split-child shape: `[Context Summary]` head, then the copied fresh tail
    // whose first ToolResult has no assistant turn to pair with.
    let events = vec![
        system_msg("[Context Summary]\nthe user is porting the parser to nom"),
        tool_result("c1", json!({"ok": true})),
    ];

    let out = build_prompt(&events, 0);
    assert_eq!(out.len(), 2, "summary + downgraded orphan result");
    match &out[0] {
        UnifiedMessage::User { content } => match &content[0] {
            ContentBlock::Text { text, .. } => {
                assert!(
                    text.contains("[Context Summary]") && text.contains("nom"),
                    "the split summary must reach the model, got {text:?}"
                );
            }
            other => panic!("expected Text block, got {other:?}"),
        },
        other => panic!("expected the summary as a User message, got {other:?}"),
    }
}

/// The system message goes through the same deferral buffer as a user message:
/// emitted while the turn still owes tool results it would split the
/// tool_use/tool_result pairing (HTTP 400), and a "push only when nothing is
/// owed" shortcut would drop it — the original bug, one layer down.
#[test]
fn system_message_lands_after_owed_tool_results() {
    let events = vec![
        user_msg("port the parser"),
        tool_call_requested("c1", "read_file"),
        assistant_with_tool_use("c1", "read_file"),
        system_msg("[Context Summary]\nearlier turns elided"),
        tool_result("c1", json!({"bytes": 12})),
    ];

    let out = build_prompt(&events, 0);
    assert_eq!(out.len(), 4);
    assert!(matches!(out[1], UnifiedMessage::Assistant { .. }));
    assert!(
        matches!(out[2], UnifiedMessage::ToolResult { .. }),
        "the result must immediately follow its tool_use"
    );
    match &out[3] {
        UnifiedMessage::User { content } => match &content[0] {
            ContentBlock::Text { text, .. } => {
                assert!(text.contains("[Context Summary]"), "got {text:?}");
            }
            other => panic!("expected Text block, got {other:?}"),
        },
        other => panic!("expected the deferred summary last, got {other:?}"),
    }
}

/// Sanity benchmark — not an assertion; run with
/// `cargo test -p alephcore --lib harness::tests::prompt::perf_dispatch_overhead_documented -- --ignored --nocapture`
/// to print timings.
#[test]
#[ignore]
fn perf_dispatch_overhead_documented() {
    use std::time::Instant;
    let events: Vec<SessionEventRecord> = (0..1000).map(|i| user_msg(&format!("m{i}"))).collect();

    let start = Instant::now();
    for _ in 0..1000 {
        let _ = build_prompt(&events, 0);
    }
    let elapsed = start.elapsed();
    eprintln!("1000 × build_prompt(1000 events) = {elapsed:?}");
}
