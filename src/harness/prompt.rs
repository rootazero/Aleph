//! Prompt Assembly Seam — Stage 3 of the 12-module harness roadmap.
//!
//! `PromptBuilder` is the single seam through which `AgentHarness` produces
//! the per-turn `Vec<UnifiedMessage>` handed to the provider. Default
//! behavior matches the legacy private `build_prompt` byte-for-byte;
//! downstream stages (#11 Subagent, #10 Verification) inject custom
//! builders that compose memory hints, chain context, or judge prompts
//! without patching `agent.rs`.

use async_trait::async_trait;

use crate::providers::message::UnifiedMessage;
use crate::session::events::SessionEventRecord;

/// Input to `PromptBuilder::assemble`. Carries the slice of session events
/// and the tail boundary computed by `tail_start_index`. Future stages may
/// extend this struct with memory hints, skill suggestions, or chain
/// context — additions must be additive (existing builders keep working).
#[derive(Debug)]
pub struct TurnContext<'a> {
    pub events: &'a [SessionEventRecord],
    pub tail_start: usize,
}

impl<'a> TurnContext<'a> {
    pub fn new(events: &'a [SessionEventRecord], tail_start: usize) -> Self {
        Self { events, tail_start }
    }
}

/// Pluggable per-turn message assembler. Implementations must be
/// `Send + Sync` so `Arc<dyn PromptBuilder>` lives in `HarnessDeps`.
#[async_trait]
pub trait PromptBuilder: Send + Sync {
    /// Produce the `Vec<UnifiedMessage>` for the next provider call.
    /// Errors propagate as `HarnessError::Session` (or future variants).
    async fn assemble(
        &self,
        ctx: &TurnContext<'_>,
    ) -> Result<Vec<UnifiedMessage>, crate::harness::trait_def::HarnessError>;
}

/// Default builder — byte-equivalent to the pre-Stage-3 private
/// `build_prompt` function (former `agent.rs:846`).
#[derive(Debug, Default, Clone)]
pub struct DefaultPromptBuilder;

#[async_trait]
impl PromptBuilder for DefaultPromptBuilder {
    async fn assemble(
        &self,
        ctx: &TurnContext<'_>,
    ) -> Result<Vec<UnifiedMessage>, crate::harness::trait_def::HarnessError> {
        use crate::providers::message::{ContentBlock, UnifiedMessage};
        use crate::session::events::SessionEvent;

        let events = ctx.events;
        let tail_start = ctx.tail_start;
        let mut messages = Vec::new();

        // Reconstruct the preceding assistant turn (if any) so the model sees
        // its own tool_use request in context.
        if tail_start > 0 {
            if let SessionEvent::AssistantMessage { content, .. } = &events[tail_start - 1].event {
                // Pre-compute the set of call_ids that have a matching
                // ToolResult or ToolError in the tail. Tool_use blocks
                // without one are "orphans" — typically caused by the
                // previous turn being interrupted (turn timeout, cancel,
                // crash) before `act()` could persist a result. Anthropic
                // and Anthropic-compatible backends reject orphans with
                // HTTP 400 ("tool_call_ids did not have response messages"),
                // and every subsequent turn replays the same broken state.
                // Drop orphans so the next turn can proceed cleanly.
                let resolved: std::collections::HashSet<&str> = events[tail_start..]
                    .iter()
                    .filter_map(|r| match &r.event {
                        SessionEvent::ToolResult { call_id, .. }
                        | SessionEvent::ToolError { call_id, .. } => Some(call_id.as_str()),
                        _ => None,
                    })
                    .collect();

                // Partition tool_use blocks into kept vs. orphan first, so
                // we can decide whether the paired thinking block should be
                // included (signed thinking only makes sense alongside a
                // surviving tool_use intent).
                let mut tool_blocks: Vec<ContentBlock> = Vec::new();
                let mut dropped_orphans: Vec<String> = Vec::new();
                for raw in &content.blocks {
                    if let Some(tc) = parse_tool_use_block(raw) {
                        if let ContentBlock::ToolCall { id, .. } = &tc {
                            if !resolved.contains(id.as_str()) {
                                dropped_orphans.push(id.clone());
                                continue;
                            }
                        }
                        tool_blocks.push(tc);
                    }
                }
                if !dropped_orphans.is_empty() {
                    tracing::warn!(
                        orphans = ?dropped_orphans,
                        "dropping orphan tool_use blocks from replayed assistant message \
                         (no matching tool_result/tool_error in session log)",
                    );
                }

                let mut blocks: Vec<ContentBlock> = Vec::new();
                // Reconstruct signed thinking block first so tool_use blocks
                // that follow it receive reasoning_content in convert_messages.
                // Skip when no tool_use survived: a lone signed thinking block
                // (without any subsequent action) is rejected by Anthropic.
                if !tool_blocks.is_empty() {
                    if let (Some(ref thinking), Some(ref sig)) =
                        (&content.thinking, &content.thinking_signature)
                    {
                        if !thinking.is_empty() {
                            blocks.push(ContentBlock::Thinking {
                                thinking: thinking.clone(),
                                signature: Some(sig.clone()),
                            });
                        }
                    }
                }
                if !content.text.is_empty() {
                    blocks.push(ContentBlock::Text {
                        text: content.text.clone(),
                        cache_control: None,
                    });
                }
                blocks.extend(tool_blocks);
                if !blocks.is_empty() {
                    messages.push(UnifiedMessage::Assistant { content: blocks });
                }
            }
        }

        // Walk the tail and emit UserMessage / ToolResult entries.
        for (offset, record) in events[tail_start..].iter().enumerate() {
            match &record.event {
                SessionEvent::UserMessage {
                    content, synthetic, ..
                } => {
                    // G2 (opencode parity): wrap real mid-loop user messages
                    // in `<system-reminder>` so the model recognises them as
                    // genuine user interjections rather than synthetic harness
                    // chatter (verifier vetoes, MAX_STEPS hints). The wrap
                    // only fires when an assistant turn already exists
                    // (`tail_start > 0`) — the conversation-opening user
                    // message is never wrapped.
                    let wrapped;
                    let text: &str = if !*synthetic && tail_start > 0 {
                        wrapped = format!(
                            "<system-reminder>\n\
                             The user sent the following message:\n\
                             {}\n\n\
                             Please address this message and continue with your tasks.\n\
                             </system-reminder>",
                            content.text,
                        );
                        &wrapped
                    } else {
                        content.text.as_str()
                    };
                    messages.push(UnifiedMessage::user(text));
                }
                SessionEvent::ToolResult {
                    call_id, output, ..
                } => {
                    let tool_result_idx = tail_start + offset;
                    let tool_name =
                        resolve_tool_name(events, tool_result_idx, call_id).unwrap_or("unknown");
                    messages.push(UnifiedMessage::tool_result_json(
                        call_id.clone(),
                        tool_name.to_string(),
                        output.value.clone(),
                        false,
                    ));
                }
                SessionEvent::ToolError { call_id, error, .. } => {
                    let tool_result_idx = tail_start + offset;
                    let tool_name =
                        resolve_tool_name(events, tool_result_idx, call_id).unwrap_or("unknown");
                    messages.push(UnifiedMessage::ToolResult {
                        tool_call_id: call_id.clone(),
                        tool_name: tool_name.to_string(),
                        content: vec![ContentBlock::Text {
                            text: error.clone(),
                            cache_control: None,
                        }],
                        is_error: true,
                    });
                }
                _ => {}
            }
        }

        Ok(messages)
    }
}

/// Parse a previously persisted `tool_use` JSON block back into a
/// `ContentBlock::ToolCall`. Returns `None` for blocks that don't match
/// the shape written by `tool_use_blocks`.
///
/// `pub(crate)` so the round-trip test in `harness::tests::act` can exercise
/// the writer/reader pair without re-exporting through `harness::agent`.
pub(crate) fn parse_tool_use_block(
    block: &serde_json::Value,
) -> Option<crate::providers::message::ContentBlock> {
    use crate::providers::message::ContentBlock;
    let obj = block.as_object()?;
    if obj.get("type").and_then(serde_json::Value::as_str) != Some("tool_use") {
        return None;
    }
    let id = obj
        .get("id")
        .and_then(serde_json::Value::as_str)?
        .to_string();
    let name = obj
        .get("name")
        .and_then(serde_json::Value::as_str)?
        .to_string();
    let arguments = obj.get("input").cloned().unwrap_or(serde_json::Value::Null);
    // Gemini 3 `thoughtSignature`, persisted by `tool_use_blocks`. Absent for
    // other providers and for sessions logged before this field existed.
    let thought_signature = obj
        .get("thought_signature")
        .and_then(serde_json::Value::as_str)
        .map(|s| s.to_string());
    Some(ContentBlock::ToolCall {
        id,
        name,
        arguments,
        thought_signature,
    })
}

/// Find the `ToolCallRequested.name` whose `call_id` matches, searching
/// strictly BEFORE `before_idx` (i.e. within `events[..before_idx]`).
fn resolve_tool_name<'a>(
    events: &'a [crate::session::events::SessionEventRecord],
    before_idx: usize,
    call_id: &str,
) -> Option<&'a str> {
    use crate::session::events::SessionEvent;
    let upper = before_idx.min(events.len());
    events[..upper].iter().rev().find_map(|r| match &r.event {
        SessionEvent::ToolCallRequested {
            call_id: id, name, ..
        } if id == call_id => Some(name.as_str()),
        _ => None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::message::ContentBlock;
    use crate::session::events::{
        now_ms, MessageContent, SessionEvent, SessionEventRecord, ToolOutput, ToolOutputMetadata,
        TurnTrigger,
    };
    use serde_json::json;

    /// Tests construct events with monotonic seq counters; the prompt
    /// builder doesn't read `seq` itself but the type requires the field.
    fn mk_record(event: SessionEvent) -> SessionEventRecord {
        use std::sync::atomic::{AtomicU64, Ordering};
        static SEQ: AtomicU64 = AtomicU64::new(1);
        SessionEventRecord {
            seq: SEQ.fetch_add(1, Ordering::Relaxed),
            event,
            created_at_ms: now_ms(),
        }
    }

    #[tokio::test]
    async fn default_builder_compiles_and_runs() {
        let events: Vec<SessionEventRecord> = Vec::new();
        let ctx = TurnContext::new(&events, 0);
        let builder = DefaultPromptBuilder;
        let out = builder.assemble(&ctx).await.expect("assemble ok");
        assert!(out.is_empty(), "empty events → empty output");
    }

    /// Regression: when the previous assistant turn emitted tool_use blocks
    /// but `act()` was interrupted before persisting a ToolResult/ToolError
    /// for one of them, the prompt builder must drop the orphan. Anthropic
    /// (and Anthropic-compatible) APIs reject orphan tool_use blocks with
    /// HTTP 400 ("tool_call_ids did not have response messages"), and the
    /// orphan is otherwise replayed on every subsequent turn — bricking the
    /// session.
    #[tokio::test]
    async fn drops_orphan_tool_use_blocks_from_replayed_assistant() {
        let turn = uuid::Uuid::new_v4();
        let events: Vec<SessionEventRecord> = vec![
            mk_record(SessionEvent::TurnStarted {
                turn_id: turn,
                trigger: TurnTrigger::UserMessage,
                at: now_ms(),
            }),
            mk_record(SessionEvent::UserMessage {
                turn_id: turn,
                content: MessageContent {
                    text: "first".into(),
                    blocks: vec![],
                    thinking: None,
                    thinking_signature: None,
                },
                at: now_ms(),
                synthetic: false,
            }),
            mk_record(SessionEvent::AssistantMessage {
                turn_id: turn,
                content: MessageContent {
                    text: "Let me check.".into(),
                    blocks: vec![
                        json!({"type": "tool_use", "id": "kept_id", "name": "tool_a", "input": {}}),
                        json!({"type": "tool_use", "id": "orphan_id", "name": "tool_b", "input": {}}),
                    ],
                    thinking: None,
                    thinking_signature: None,
                },
                at: now_ms(),
            }),
            mk_record(SessionEvent::ToolCallRequested {
                turn_id: turn,
                call_id: "kept_id".into(),
                name: "tool_a".into(),
                input: json!({}),
                at: now_ms(),
            }),
            mk_record(SessionEvent::ToolResult {
                turn_id: turn,
                call_id: "kept_id".into(),
                output: ToolOutput {
                    value: json!("ok"),
                    metadata: ToolOutputMetadata::default(),
                },
                at: now_ms(),
            }),
            // Note: NO ToolResult/ToolError for "orphan_id" — this is the bug shape.
            mk_record(SessionEvent::UserMessage {
                turn_id: turn,
                content: MessageContent {
                    text: "another query".into(),
                    blocks: vec![],
                    thinking: None,
                    thinking_signature: None,
                },
                at: now_ms(),
                synthetic: false,
            }),
        ];

        // Tail starts after the AssistantMessage (index 2 → tail_start = 3).
        let ctx = TurnContext::new(&events, 3);
        let messages = DefaultPromptBuilder
            .assemble(&ctx)
            .await
            .expect("assemble ok");

        let assistant_blocks = messages
            .iter()
            .find_map(|m| match m {
                UnifiedMessage::Assistant { content } => Some(content),
                _ => None,
            })
            .expect("assistant message present");
        let tool_use_ids: Vec<&str> = assistant_blocks
            .iter()
            .filter_map(|b| match b {
                ContentBlock::ToolCall { id, .. } => Some(id.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(
            tool_use_ids,
            vec!["kept_id"],
            "orphan tool_use must be filtered; only matched call survives",
        );
        // The text block is preserved even though one tool_use was dropped.
        assert!(assistant_blocks
            .iter()
            .any(|b| matches!(b, ContentBlock::Text { text, .. } if text == "Let me check.")));
    }

    /// Boundary case: when ALL tool_use blocks in the previous assistant
    /// turn are orphans AND the assistant has no text, the entire assistant
    /// message must be elided (no empty placeholder pushed). The signed
    /// thinking block, if any, is also dropped because it would otherwise
    /// stand alone — Anthropic rejects a thinking block without a paired
    /// action.
    #[tokio::test]
    async fn drops_entire_assistant_when_only_orphans_remain() {
        let turn = uuid::Uuid::new_v4();
        let events: Vec<SessionEventRecord> = vec![
            mk_record(SessionEvent::AssistantMessage {
                turn_id: turn,
                content: MessageContent {
                    text: String::new(),
                    blocks: vec![
                        json!({"type": "tool_use", "id": "orphan_only", "name": "tool_x", "input": {}}),
                    ],
                    thinking: Some("planning...".into()),
                    thinking_signature: Some("sig_z".into()),
                },
                at: now_ms(),
            }),
            mk_record(SessionEvent::UserMessage {
                turn_id: turn,
                content: MessageContent {
                    text: "next".into(),
                    blocks: vec![],
                    thinking: None,
                    thinking_signature: None,
                },
                at: now_ms(),
                synthetic: false,
            }),
        ];

        let ctx = TurnContext::new(&events, 1);
        let messages = DefaultPromptBuilder
            .assemble(&ctx)
            .await
            .expect("assemble ok");
        assert!(
            !messages
                .iter()
                .any(|m| matches!(m, UnifiedMessage::Assistant { .. })),
            "no assistant message should be emitted when only orphans + thinking remain",
        );
        assert_eq!(messages.len(), 1, "expected only the new user message");
    }

    /// G2 (opencode parity): a real mid-loop user message — synthetic=false
    /// AND tail_start > 0 — must be wrapped in `<system-reminder>` so the
    /// model recognises it as a user interjection. The conversation-opening
    /// user message (tail_start == 0) is never wrapped.
    #[tokio::test]
    async fn g2_wraps_real_midloop_user_message_in_system_reminder() {
        let turn = uuid::Uuid::new_v4();
        let events: Vec<SessionEventRecord> = vec![
            mk_record(SessionEvent::AssistantMessage {
                turn_id: turn,
                content: MessageContent {
                    text: "Working on it.".into(),
                    blocks: vec![],
                    thinking: None,
                    thinking_signature: None,
                },
                at: now_ms(),
            }),
            mk_record(SessionEvent::UserMessage {
                turn_id: turn,
                content: MessageContent {
                    text: "actually wait, do this instead".into(),
                    blocks: vec![],
                    thinking: None,
                    thinking_signature: None,
                },
                at: now_ms(),
                synthetic: false,
            }),
        ];
        let ctx = TurnContext::new(&events, 1);
        let messages = DefaultPromptBuilder
            .assemble(&ctx)
            .await
            .expect("assemble ok");
        let user_text = messages
            .iter()
            .find_map(|m| match m {
                UnifiedMessage::User { content } => content.first().and_then(|b| match b {
                    ContentBlock::Text { text, .. } => Some(text.clone()),
                    _ => None,
                }),
                _ => None,
            })
            .expect("user message present");
        assert!(
            user_text.contains("<system-reminder>"),
            "real mid-loop user message must be wrapped; got: {user_text}",
        );
        assert!(
            user_text.contains("actually wait, do this instead"),
            "original user text preserved inside wrap",
        );
    }

    /// G2: synthetic user messages (verifier vetoes, MAX_STEPS hints) must
    /// pass through unwrapped — the model has already been trained on the
    /// `<system-reminder>` shape; wrapping a synthetic reminder inside
    /// another reminder muddles the signal.
    #[tokio::test]
    async fn g2_does_not_wrap_synthetic_user_message() {
        let turn = uuid::Uuid::new_v4();
        let events: Vec<SessionEventRecord> = vec![
            mk_record(SessionEvent::AssistantMessage {
                turn_id: turn,
                content: MessageContent {
                    text: "Done.".into(),
                    blocks: vec![],
                    thinking: None,
                    thinking_signature: None,
                },
                at: now_ms(),
            }),
            mk_record(SessionEvent::UserMessage {
                turn_id: turn,
                content: MessageContent {
                    text: "[verifier veto] something".into(),
                    blocks: vec![],
                    thinking: None,
                    thinking_signature: None,
                },
                at: now_ms(),
                synthetic: true,
            }),
        ];
        let ctx = TurnContext::new(&events, 1);
        let messages = DefaultPromptBuilder
            .assemble(&ctx)
            .await
            .expect("assemble ok");
        let user_text = messages
            .iter()
            .find_map(|m| match m {
                UnifiedMessage::User { content } => content.first().and_then(|b| match b {
                    ContentBlock::Text { text, .. } => Some(text.clone()),
                    _ => None,
                }),
                _ => None,
            })
            .expect("user message present");
        assert!(
            !user_text.contains("<system-reminder>"),
            "synthetic user message must not be re-wrapped; got: {user_text}",
        );
        assert_eq!(user_text, "[verifier veto] something");
    }

    /// G2: the conversation-opening user message (tail_start == 0) is the
    /// genuine first prompt and must NOT be wrapped — wrapping would
    /// confuse the model on turn 1.
    #[tokio::test]
    async fn g2_does_not_wrap_opening_user_message() {
        let turn = uuid::Uuid::new_v4();
        let events: Vec<SessionEventRecord> = vec![mk_record(SessionEvent::UserMessage {
            turn_id: turn,
            content: MessageContent {
                text: "hello, help me with X".into(),
                blocks: vec![],
                thinking: None,
                thinking_signature: None,
            },
            at: now_ms(),
            synthetic: false,
        })];
        let ctx = TurnContext::new(&events, 0);
        let messages = DefaultPromptBuilder
            .assemble(&ctx)
            .await
            .expect("assemble ok");
        let user_text = messages
            .iter()
            .find_map(|m| match m {
                UnifiedMessage::User { content } => content.first().and_then(|b| match b {
                    ContentBlock::Text { text, .. } => Some(text.clone()),
                    _ => None,
                }),
                _ => None,
            })
            .expect("user message present");
        assert!(
            !user_text.contains("<system-reminder>"),
            "opening user message must not be wrapped; got: {user_text}",
        );
        assert_eq!(user_text, "hello, help me with X");
    }
}
