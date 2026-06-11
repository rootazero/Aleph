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
use crate::session::events::{RunOutcome, SessionEvent, SessionEventRecord};

/// Replayed at the position of a user-cancelled run's terminal marker
/// (codex `<turn_aborted>` parity). Without it the interruption is invisible:
/// the cancelled run's orphan `tool_use` blocks are dropped during replay
/// (Anthropic rejects them with HTTP 400), so the model sees a turn that
/// simply stops and may assume its in-flight tool calls completed. Steering
/// messages the cancelled run never answered stay in the log right before
/// this note — the model judges whether they still apply (R7); the harness
/// never deletes them.
const INTERRUPTION_NOTE: &str = "<system-reminder>\n\
    The previous run was interrupted by the user before it finished. Tool \
    calls still in flight were aborted and produced no results — do not \
    assume they completed. Re-evaluate any earlier unanswered instructions \
    in light of the interruption before continuing.\n\
    </system-reminder>";

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
/// messages (verifier vetoes, `MAX_STEPS` hints) pass through unwrapped, and
/// the conversation-opening user message (`tail_start == 0`) is never
/// wrapped.
pub(crate) fn build_prompt(
    events: &[SessionEventRecord],
    tail_start: usize,
) -> Vec<UnifiedMessage> {
    let mut messages = Vec::new();
    // Whether an assistant turn has already been emitted. Drives the G2
    // wrap decision: the conversation-opening user message (before any
    // assistant turn) is never wrapped; later real user messages are.
    let mut assistant_emitted = false;

    // Walk the FULL conversation in order, emitting one message per event.
    //
    // History must be complete: in an autonomous tool loop (cron / subagent),
    // the turn after a tool round has NO new user message — the loop continues
    // on tool results alone. Emitting only a window from the last assistant
    // turn would drop the original user task, so the model would see only its
    // own tool_use + results with no instruction (Claude replies "you didn't
    // send anything"; weaker models loop and re-invent inputs). Token pressure
    // on long histories is handled downstream by the context compactor, not by
    // truncating the prompt here.
    for (idx, record) in events.iter().enumerate() {
        match &record.event {
            SessionEvent::AssistantMessage { content, .. } => {
                // call_ids whose ToolResult/ToolError appears after this turn.
                // tool_use blocks without one are "orphans" (the turn was
                // interrupted before `act()` persisted a result) and Anthropic-
                // compatible backends reject them with HTTP 400 — drop them.
                let resolved: std::collections::HashSet<&str> = events[idx + 1..]
                    .iter()
                    .filter_map(|r| match &r.event {
                        SessionEvent::ToolResult { call_id, .. }
                        | SessionEvent::ToolError { call_id, .. } => Some(call_id.as_str()),
                        _ => None,
                    })
                    .collect();
                if let Some(blocks) = reconstruct_assistant_blocks(content, &resolved) {
                    messages.push(UnifiedMessage::Assistant { content: blocks });
                    assistant_emitted = true;
                }
            }
            SessionEvent::UserMessage {
                content, synthetic, ..
            } => {
                // G2 (opencode parity): wrap real mid-loop user messages in
                // `<system-reminder>` so the model recognises them as genuine
                // user interjections rather than synthetic harness chatter.
                // The opening user message (no assistant turn yet) and
                // synthetic messages (verifier vetoes, MAX_STEPS hints) pass
                // through unwrapped.
                let wrapped;
                let text: &str = if !*synthetic && assistant_emitted {
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
                let tool_name = resolve_tool_name(events, idx, call_id).unwrap_or("unknown");
                messages.push(UnifiedMessage::tool_result_json(
                    call_id.clone(),
                    tool_name.to_string(),
                    output.value.clone(),
                    false,
                ));
            }
            SessionEvent::ToolError { call_id, error, .. } => {
                let tool_name = resolve_tool_name(events, idx, call_id).unwrap_or("unknown");
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
            SessionEvent::RunFinished {
                outcome: RunOutcome::Cancelled,
                ..
            } => {
                // User interruption marker — see `INTERRUPTION_NOTE`. Other
                // outcomes stay invisible: Completed is the normal seam
                // between turns, and Errored already left its error in the
                // visible tool/assistant trail.
                messages.push(UnifiedMessage::user(INTERRUPTION_NOTE));
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
    if let Some(summary) = crate::tools::attempt_summary::render_run_summary(&events[tail_start..])
    {
        messages.push(UnifiedMessage::user(&summary));
    }

    // P2 (sibling of the failure summary above): idempotent-tool no-progress
    // loop detector. Fires only when a pure-read tool returned the identical
    // result on the same args ≥ NO_PROGRESS_THRESHOLD times in the visible
    // window — the success-loop blind spot the failure summary cannot see.
    // Same `<system-reminder>` channel; a nudge, never a control-flow branch
    // (R7/R10). Scoped to `events[tail_start..]`, recomputed fresh every Think.
    if let Some(notice) =
        crate::tools::no_progress::render_no_progress_notice(&events[tail_start..])
    {
        messages.push(UnifiedMessage::user(&notice));
    }

    // P2 (sibling of the two above): gather-volume budget. The failure summary
    // sees only errors and no_progress sees only byte-identical idempotent
    // repeats; neither catches a model making 150+ *successful* varied-query
    // search/web_fetch calls that never converge on the deliverable (the runaway
    // the interactive max_iterations cap force-summarises). Fires when gather
    // calls in the visible window cross GATHER_BUDGET_THRESHOLD, surfacing the
    // live count so the convergence nudge is harder to tune out than the static
    // guideline #15. Same `<system-reminder>` channel; a soft nudge, never a
    // control-flow branch (R7/R10). Scoped to the tail, recomputed every Think.
    if let Some(notice) = crate::tools::gather_budget::render_gather_notice(&events[tail_start..]) {
        messages.push(UnifiedMessage::user(&notice));
    }

    // P2 (4th sibling): redundant side-effecting call loop. no_progress catches
    // identical-result loops only on *idempotent* reads; this is its complement,
    // covering non-idempotent tools (bash/code_exec/writes/sends/MCP) when the
    // exact same (tool, args) call keeps returning the byte-identical result.
    // Order-independent, so it catches the interleaved code_exec↔bash alternation
    // that defeats the consecutive-run tool_loop_verifier and slips past
    // attempt_summary (those failures are Ok results, not ToolError). Same
    // `<system-reminder>` channel; a soft nudge, never a control-flow branch
    // (R7/R10). Scoped to the tail, recomputed every Think.
    if let Some(notice) =
        crate::tools::redundant_calls::render_redundant_call_notice(&events[tail_start..])
    {
        messages.push(UnifiedMessage::user(&notice));
    }

    messages
}

/// Reconstruct one persisted assistant turn into provider content blocks.
///
/// `resolved` is the set of tool-call ids whose `ToolResult`/`ToolError`
/// appears later in the log; `tool_use` blocks without one are orphans (an
/// interrupted turn) and are dropped — Anthropic-compatible backends reject
/// orphan `tool_use` with HTTP 400. The signed thinking block is replayed only
/// alongside a surviving `tool_use` (a lone thinking block is also rejected).
/// Returns `None` when nothing survives (no text and only orphan `tool_use`),
/// so the caller emits no empty assistant placeholder.
fn reconstruct_assistant_blocks(
    content: &crate::session::events::MessageContent,
    resolved: &std::collections::HashSet<&str>,
) -> Option<Vec<ContentBlock>> {
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
    // Signed thinking goes first so following tool_use blocks receive
    // reasoning_content in convert_messages. Only with a surviving tool_use.
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
    if blocks.is_empty() {
        None
    } else {
        Some(blocks)
    }
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
        now_ms, MessageContent, RunOutcome, SessionEvent, SessionEventRecord, ToolOutput,
        ToolOutputMetadata, TurnTrigger,
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

    /// REGRESSION: in an autonomous tool loop (cron / subagent), the turn that
    /// follows a tool round has NO new user message in the tail — the loop
    /// continues on tool results alone. The provider must still see the
    /// ORIGINAL user task, or the model loses what it was asked to do (Claude
    /// replies "you didn't send any content"; weaker models loop, re-inventing
    /// inputs). With `tail_start` = "just after the last assistant turn", the
    /// opening task is windowed out. The full conversation must be emitted.
    #[test]
    fn autonomous_continuation_preserves_original_task() {
        let turn = uuid::Uuid::new_v4();
        let events: Vec<SessionEventRecord> = vec![
            mk_record(SessionEvent::UserMessage {
                turn_id: turn,
                content: MessageContent {
                    text: "TASK: summarize today's Iran war news".into(),
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
                    text: String::new(),
                    blocks: vec![
                        json!({"type": "tool_use", "id": "c1", "name": "web_fetch", "input": {"url": "https://bbc.com"}}),
                    ],
                    thinking: None,
                    thinking_signature: None,
                },
                at: now_ms(),
            }),
            mk_record(SessionEvent::ToolCallRequested {
                turn_id: turn,
                call_id: "c1".into(),
                name: "web_fetch".into(),
                input: json!({"url": "https://bbc.com"}),
                at: now_ms(),
            }),
            mk_record(SessionEvent::ToolResult {
                turn_id: turn,
                call_id: "c1".into(),
                output: ToolOutput {
                    value: json!("<headlines>"),
                    metadata: ToolOutputMetadata::default(),
                },
                at: now_ms(),
            }),
        ];

        let tail_start = crate::harness::agent::tail_start_index(&events);
        let messages = build_prompt(&events, tail_start);

        let has_task = messages.iter().any(|m| match m {
            UnifiedMessage::User { content } => content
                .iter()
                .any(|b| matches!(b, ContentBlock::Text { text, .. } if text.contains("TASK: summarize today's Iran war news"))),
            _ => false,
        });
        assert!(
            has_task,
            "autonomous continuation dropped the original user task; messages={messages:#?}",
        );
    }

    /// codex `<turn_aborted>` parity: a user-cancelled run must leave a
    /// visible interruption note at its position in the replayed
    /// conversation; Completed/Errored terminal markers stay invisible.
    #[test]
    fn cancelled_run_marker_surfaces_interruption_note() {
        let turn = uuid::Uuid::new_v4();
        let user_msg = |text: &str| {
            mk_record(SessionEvent::UserMessage {
                turn_id: turn,
                content: MessageContent {
                    text: text.into(),
                    blocks: vec![],
                    thinking: None,
                    thinking_signature: None,
                },
                at: now_ms(),
                synthetic: false,
            })
        };
        let finished = |outcome: RunOutcome| {
            mk_record(SessionEvent::RunFinished {
                run_id: "r1".into(),
                outcome,
                at: now_ms(),
            })
        };
        let assistant = mk_record(SessionEvent::AssistantMessage {
            turn_id: turn,
            content: MessageContent {
                text: "working on the big task".into(),
                blocks: vec![],
                thinking: None,
                thinking_signature: None,
            },
            at: now_ms(),
        });

        let note_present = |events: &[SessionEventRecord]| {
            build_prompt(events, 0).iter().any(|m| match m {
                UnifiedMessage::User { content } => content.iter().any(|b| {
                    matches!(b, ContentBlock::Text { text, .. }
                        if text.contains("interrupted by the user"))
                }),
                _ => false,
            })
        };

        // Interrupt-mode restart shape: task → partial answer → user /stop →
        // replacement instruction. The new run must see the interruption.
        let cancelled = vec![
            user_msg("do the big task"),
            assistant.clone(),
            finished(RunOutcome::Cancelled),
            user_msg("actually, do something else"),
        ];
        assert!(
            note_present(&cancelled),
            "cancelled run left no interruption note in the replayed prompt",
        );

        // A normally-completed run is the ordinary seam between turns — no note.
        let completed = vec![
            user_msg("do the big task"),
            assistant,
            finished(RunOutcome::Completed),
            user_msg("thanks, next topic"),
        ];
        assert!(
            !note_present(&completed),
            "completed run must not fabricate an interruption note",
        );
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
            !messages.iter().any(|m| matches!(
                m,
                crate::providers::message::UnifiedMessage::Assistant { .. }
            )),
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
