//! Per-turn prompt assembly — sync helper for `think.rs`.
//!
//! Round-2 inlining of the former `PromptBuilder` trait + `DefaultPromptBuilder`
//! struct (deleted from `src/harness/prompt.rs`). The trait had exactly one
//! impl with one production consumer (`subagent_spawner`); per R10 撤回模式
//! a zero-real-consumer abstraction is deleted, not preserved.
//!
//! The body never returned an error (the old `Result<_, HarnessError>` was
//! always `Ok`), so this is now an infallible sync fn.

use crate::providers::message::{ContentBlock, UnifiedMessage};
use crate::session::events::{SessionEvent, SessionEventRecord};

/// Build the per-turn message vector handed to the provider. Walks the
/// session log slice and:
///   * Reconstructs the preceding assistant turn (including signed thinking)
///     when `tail_start > 0`, dropping orphan `tool_use` blocks whose
///     matching `ToolResult` / `ToolError` is missing — Anthropic-compatible
///     backends reject orphans with HTTP 400.
///   * Walks the tail emitting `UserMessage` / `ToolResult` / `ToolError`.
///
/// G2 (opencode parity): wraps real mid-loop user messages in
/// `<system-reminder>` so the model recognises them as genuine user
/// interjections rather than synthetic harness chatter. Synthetic user
/// messages (verifier vetoes, MAX_STEPS hints) pass through unwrapped, and
/// the conversation-opening user message (`tail_start == 0`) is never
/// wrapped.
pub(crate) fn build_prompt(
    events: &[SessionEventRecord],
    tail_start: usize,
) -> Vec<UnifiedMessage> {
    let mut messages = Vec::new();

    // Reconstruct the preceding assistant turn (if any) so the model sees
    // its own tool_use request in context.
    if tail_start > 0 {
        if let SessionEvent::AssistantMessage { content, .. } = &events[tail_start - 1].event {
            // Pre-compute the set of call_ids that have a matching
            // ToolResult or ToolError in the tail. Tool_use blocks without
            // one are "orphans" — typically caused by the previous turn
            // being interrupted (turn timeout, cancel, crash) before
            // `act()` could persist a result. Drop them so the next turn
            // can proceed cleanly.
            let resolved: std::collections::HashSet<&str> = events[tail_start..]
                .iter()
                .filter_map(|r| match &r.event {
                    SessionEvent::ToolResult { call_id, .. }
                    | SessionEvent::ToolError { call_id, .. } => Some(call_id.as_str()),
                    _ => None,
                })
                .collect();

            // Partition tool_use blocks into kept vs. orphan first, so we
            // can decide whether the paired thinking block should be
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
                // chatter. Only fires when an assistant turn already
                // exists (`tail_start > 0`).
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

    // P2: run-level tool-failure aggregator. Returns `Some(text)` only
    // when ≥ SUMMARY_THRESHOLD failures have accumulated in the tail
    // window — below that, each error's own persistence_hint is enough
    // and the aggregate would be noise. Wrapped in `<system-reminder>`
    // by the renderer, so this looks like a synthetic harness signal to
    // the model rather than a fresh user turn.
    //
    // Computed from `events[tail_start..]` so it stays scoped to what
    // the model already sees in this turn's context — never persisted,
    // recomputed fresh every Think.
    if let Some(summary) =
        crate::tools::attempt_summary::render_run_summary(&events[tail_start..])
    {
        messages.push(UnifiedMessage::user(&summary));
    }

    messages
}

/// Parse a previously persisted `tool_use` JSON block back into a
/// `ContentBlock::ToolCall`. Returns `None` for blocks that don't match
/// the shape written by `tool_use_blocks`.
///
/// `pub(crate)` so the round-trip test in `harness::tests::act` can exercise
/// the writer/reader pair through `crate::harness::agent::prompt::parse_tool_use_block`.
pub(crate) fn parse_tool_use_block(
    block: &serde_json::Value,
) -> Option<crate::providers::message::ContentBlock> {
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

    #[test]
    fn build_prompt_compiles_and_runs() {
        let events: Vec<SessionEventRecord> = Vec::new();
        let out = build_prompt(&events, 0);
        assert!(out.is_empty(), "empty events → empty output");
    }

    /// Regression: when the previous assistant turn emitted tool_use blocks
    /// but `act()` was interrupted before persisting a ToolResult/ToolError
    /// for one of them, `build_prompt` must drop the orphan. Anthropic
    /// (and Anthropic-compatible) APIs reject orphan tool_use blocks with
    /// HTTP 400 ("tool_call_ids did not have response messages"), and the
    /// orphan is otherwise replayed on every subsequent turn — bricking the
    /// session.
    #[test]
    fn drops_orphan_tool_use_blocks_from_replayed_assistant() {
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
        let messages = build_prompt(&events, 3);

        let assistant_blocks = messages
            .iter()
            .find_map(|m| match m {
                crate::providers::message::UnifiedMessage::Assistant { content } => Some(content),
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
    #[test]
    fn drops_entire_assistant_when_only_orphans_remain() {
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

        let messages = build_prompt(&events, 1);
        assert!(
            !messages
                .iter()
                .any(|m| matches!(m, crate::providers::message::UnifiedMessage::Assistant { .. })),
            "no assistant message should be emitted when only orphans + thinking remain",
        );
        assert_eq!(messages.len(), 1, "expected only the new user message");
    }

    /// G2 (opencode parity): a real mid-loop user message — synthetic=false
    /// AND tail_start > 0 — must be wrapped in `<system-reminder>` so the
    /// model recognises it as a user interjection. The conversation-opening
    /// user message (tail_start == 0) is never wrapped.
    #[test]
    fn g2_wraps_real_midloop_user_message_in_system_reminder() {
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
        let messages = build_prompt(&events, 1);
        let user_text = messages
            .iter()
            .find_map(|m| match m {
                crate::providers::message::UnifiedMessage::User { content } => {
                    content.first().and_then(|b| match b {
                        ContentBlock::Text { text, .. } => Some(text.clone()),
                        _ => None,
                    })
                }
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
    #[test]
    fn g2_does_not_wrap_synthetic_user_message() {
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
        let messages = build_prompt(&events, 1);
        let user_text = messages
            .iter()
            .find_map(|m| match m {
                crate::providers::message::UnifiedMessage::User { content } => {
                    content.first().and_then(|b| match b {
                        ContentBlock::Text { text, .. } => Some(text.clone()),
                        _ => None,
                    })
                }
                _ => None,
            })
            .expect("user message present");
        assert!(
            !user_text.contains("<system-reminder>"),
            "synthetic user message must not be re-wrapped; got: {user_text}",
        );
        assert_eq!(user_text, "[verifier veto] something");
    }

    /// P2: when ≥3 tool errors accumulate in the tail window,
    /// `build_prompt` MUST append a trailing user message carrying the
    /// run-level failure summary (wrapped in `<system-reminder>`). This
    /// is computed transient from session events — never persisted, so
    /// the session log stays a faithful record while the LLM still sees
    /// aggregate guidance ahead of its next Think.
    #[test]
    fn p2_appends_attempt_summary_when_threshold_crossed() {
        let turn = uuid::Uuid::new_v4();
        let mut events: Vec<SessionEventRecord> = vec![mk_record(SessionEvent::UserMessage {
            turn_id: turn,
            content: MessageContent {
                text: "find news on X".into(),
                blocks: vec![],
                thinking: None,
                thinking_signature: None,
            },
            at: now_ms(),
            synthetic: false,
        })];
        // 3 search calls all rate-limited
        for i in 0..3 {
            let cid = format!("call_{i}");
            events.push(mk_record(SessionEvent::ToolCallRequested {
                turn_id: turn,
                call_id: cid.clone(),
                name: "search".into(),
                input: json!({"q": "X"}),
                at: now_ms(),
            }));
            events.push(mk_record(SessionEvent::ToolError {
                turn_id: turn,
                call_id: cid,
                error: "HTTP 429 rate limit exceeded".into(),
                at: now_ms(),
            }));
        }

        let messages = build_prompt(&events, 0);

        // Last message must be the synthetic summary user message.
        let last = messages.last().expect("at least one message");
        let last_text = match last {
            crate::providers::message::UnifiedMessage::User { content } => {
                content.first().and_then(|b| match b {
                    ContentBlock::Text { text, .. } => Some(text.clone()),
                    _ => None,
                })
            }
            _ => None,
        }
        .expect("trailing user text present");
        assert!(
            last_text.contains("<system-reminder>"),
            "summary must be wrapped in system-reminder; got: {last_text}",
        );
        assert!(
            last_text.contains("(3 failures)"),
            "summary must report total count; got: {last_text}",
        );
        assert!(
            last_text.contains("search × 3 (rate_limited)"),
            "summary must group by (tool, kind); got: {last_text}",
        );
    }

    /// P2: below the threshold (2 failures), no summary is appended —
    /// each error's own persistence_hint is sufficient and the aggregate
    /// would be noise.
    #[test]
    fn p2_no_summary_below_threshold() {
        let turn = uuid::Uuid::new_v4();
        let mut events: Vec<SessionEventRecord> = vec![mk_record(SessionEvent::UserMessage {
            turn_id: turn,
            content: MessageContent {
                text: "first".into(),
                blocks: vec![],
                thinking: None,
                thinking_signature: None,
            },
            at: now_ms(),
            synthetic: false,
        })];
        for i in 0..2 {
            let cid = format!("call_{i}");
            events.push(mk_record(SessionEvent::ToolCallRequested {
                turn_id: turn,
                call_id: cid.clone(),
                name: "search".into(),
                input: json!({}),
                at: now_ms(),
            }));
            events.push(mk_record(SessionEvent::ToolError {
                turn_id: turn,
                call_id: cid,
                error: "HTTP 429".into(),
                at: now_ms(),
            }));
        }

        let messages = build_prompt(&events, 0);
        let trailing_summary = messages.iter().rev().any(|m| match m {
            crate::providers::message::UnifiedMessage::User { content } => content.iter().any(|b| {
                matches!(b, ContentBlock::Text { text, .. } if text.contains("<system-reminder>") && text.contains("Tool attempt summary"))
            }),
            _ => false,
        });
        assert!(
            !trailing_summary,
            "no summary should be appended when failures < threshold",
        );
    }

    /// G2: the conversation-opening user message (tail_start == 0) is the
    /// genuine first prompt and must NOT be wrapped — wrapping would
    /// confuse the model on turn 1.
    #[test]
    fn g2_does_not_wrap_opening_user_message() {
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
        let messages = build_prompt(&events, 0);
        let user_text = messages
            .iter()
            .find_map(|m| match m {
                crate::providers::message::UnifiedMessage::User { content } => {
                    content.first().and_then(|b| match b {
                        ContentBlock::Text { text, .. } => Some(text.clone()),
                        _ => None,
                    })
                }
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
