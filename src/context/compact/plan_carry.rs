//! Carry the model's own execution list across context compaction.
//!
//! The `scratchpad` tool echoes the updated checklist in every mutating
//! result, which is how the model keeps sight of its plan inside a run. That
//! echo is a tool-result message — so the moment compaction drains the window,
//! the checklist is summarized into prose and the model loses the one
//! structural record of what it already finished. `<execution_plan>` does not
//! cover this: it is resolved once per run into the frozen system prompt, so
//! after a mid-run compaction it shows the turn-0 snapshot, not the current
//! one.
//!
//! Port of hermes-agent's `TodoStore.format_for_injection()` (re-injected as a
//! synthetic message right after the compressed history), with one deliberate
//! divergence: hermes filters to *active* items because its flat text list made
//! the model re-do finished work. Aleph's render is checkbox-explicit (`[x]` /
//! `[~]` / `[ ]`) and its items are **index-addressed** by `start_item` /
//! `complete_item`, so dropping the finished ones would corrupt every index the
//! model is about to use. We carry the full list and let the glyphs do the
//! disambiguation.
//!
//! Pure — no I/O, no session key, no store lookup. Everything needed is in the
//! messages being drained.

use crate::memory::scratchpad::{PlanItem, PlanItemStatus, ScratchpadSnapshot};
use crate::providers::message::{ContentBlock, UnifiedMessage};

/// Tool whose results carry the execution list.
const SCRATCHPAD_TOOL: &str = "scratchpad";

/// Stable sentinel opening a carried-over execution list. Recognised on the
/// way back in so a second compaction pass re-carries a list whose original
/// tool result was already drained by the first one.
const CARRY_MARKER: &str = "[Execution list preserved across context compaction]";

/// Render the carry message for a compaction window, or `None` when the window
/// holds no execution list (the overwhelmingly common case — calm runs pay
/// nothing).
///
/// Returns `None` for a finished list too: a fully-checked plan needs no
/// reminder, and re-injecting one invites the model to re-open closed work.
pub(crate) fn plan_carry_message(window: &[UnifiedMessage]) -> Option<UnifiedMessage> {
    let snapshot = latest_plan(window)?;
    if !snapshot.items.iter().any(|i| !i.is_done()) {
        return None;
    }
    Some(UnifiedMessage::User {
        content: vec![ContentBlock::Text {
            text: render_carry(&snapshot),
            cache_control: None,
        }],
    })
}

fn render_carry(snapshot: &ScratchpadSnapshot) -> String {
    format!(
        "<system-reminder>\nReference data, not user input.\n{CARRY_MARKER}\n{}\n\
         Keep working it with the `scratchpad` tool (start_item / complete_item use the \
         0-based index of this list).\n</system-reminder>",
        snapshot.render_progress()
    )
}

/// Newest execution list in the window: the last `scratchpad` tool result that
/// carried a snapshot, or — when a previous pass already drained those — the
/// last carry message this module itself emitted.
fn latest_plan(window: &[UnifiedMessage]) -> Option<ScratchpadSnapshot> {
    window.iter().rev().find_map(|msg| match msg {
        UnifiedMessage::ToolResult {
            tool_name, content, ..
        } if tool_name == SCRATCHPAD_TOOL => content.iter().find_map(snapshot_from_block),
        UnifiedMessage::User { content } => content.iter().find_map(|block| match block {
            ContentBlock::Text { text, .. } if text.contains(CARRY_MARKER) => {
                parse_progress(text).filter(|s| !s.items.is_empty())
            }
            _ => None,
        }),
        _ => None,
    })
}

/// Structured read of the `snapshot` field the scratchpad tool attaches to its
/// JSON output — exact, no text parsing.
fn snapshot_from_block(block: &ContentBlock) -> Option<ScratchpadSnapshot> {
    let ContentBlock::Json { value } = block else {
        return None;
    };
    let snapshot = value.get("snapshot")?.as_object()?;
    let items = snapshot
        .get("items")?
        .as_array()?
        .iter()
        .filter_map(|item| {
            let text = item.get("text")?.as_str()?.to_string();
            let status = match item.get("status").and_then(serde_json::Value::as_str) {
                Some("completed") => PlanItemStatus::Done,
                Some("in_progress") => PlanItemStatus::InProgress,
                _ => PlanItemStatus::Pending,
            };
            Some(PlanItem { text, status })
        })
        .collect::<Vec<_>>();
    if items.is_empty() {
        return None;
    }
    Some(ScratchpadSnapshot {
        objective: snapshot
            .get("objective")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string),
        items,
    })
}

/// Read back a `ScratchpadSnapshot::render_progress` block — the format this
/// module emits, so a carry survives repeated compaction passes.
fn parse_progress(text: &str) -> Option<ScratchpadSnapshot> {
    let mut objective = None;
    let mut items = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("Objective: ") {
            objective = Some(rest.trim().to_string());
        } else if let Some(rest) = line.strip_prefix("- [x] ") {
            items.push(PlanItem {
                text: rest.trim().to_string(),
                status: PlanItemStatus::Done,
            });
        } else if let Some(rest) = line.strip_prefix("- [~] ") {
            items.push(PlanItem {
                text: rest.trim().to_string(),
                status: PlanItemStatus::InProgress,
            });
        } else if let Some(rest) = line.strip_prefix("- [ ] ") {
            items.push(PlanItem::pending(rest.trim()));
        }
    }
    Some(ScratchpadSnapshot { objective, items })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn scratchpad_result(items: &[(&str, &str)], objective: Option<&str>) -> UnifiedMessage {
        UnifiedMessage::ToolResult {
            tool_call_id: "c1".into(),
            tool_name: SCRATCHPAD_TOOL.into(),
            content: vec![ContentBlock::Json {
                value: json!({
                    "success": true,
                    "message": "ok",
                    "snapshot": {
                        "objective": objective,
                        "complete": false,
                        "items": items.iter()
                            .map(|(t, s)| json!({"text": t, "status": s}))
                            .collect::<Vec<_>>(),
                    }
                }),
            }],
            is_error: false,
        }
    }

    fn user(text: &str) -> UnifiedMessage {
        UnifiedMessage::User {
            content: vec![ContentBlock::Text {
                text: text.into(),
                cache_control: None,
            }],
        }
    }

    fn carry_text(msg: &UnifiedMessage) -> String {
        match msg {
            UnifiedMessage::User { content } => match &content[0] {
                ContentBlock::Text { text, .. } => text.clone(),
                _ => panic!("expected text"),
            },
            _ => panic!("expected user message"),
        }
    }

    #[test]
    fn window_without_a_plan_costs_nothing() {
        let window = vec![user("hello"), user("world")];
        assert!(plan_carry_message(&window).is_none());
    }

    #[test]
    fn carries_the_newest_list_with_every_status_intact() {
        let window = vec![
            scratchpad_result(&[("a", "pending"), ("b", "pending")], Some("Ship")),
            user("keep going"),
            scratchpad_result(
                &[("a", "completed"), ("b", "in_progress"), ("c", "pending")],
                Some("Ship"),
            ),
        ];
        let text = carry_text(&plan_carry_message(&window).expect("a carry"));
        assert!(text.contains(CARRY_MARKER));
        assert!(text.contains("Objective: Ship"));
        assert!(
            text.contains("- [x] a"),
            "finished steps stay visible so \
                start_item/complete_item indices remain correct: {text}"
        );
        assert!(text.contains("- [~] b"));
        assert!(text.contains("- [ ] c"));
        assert!(text.contains("Progress: 1/3 done"));
    }

    #[test]
    fn a_finished_list_is_not_carried() {
        let window = vec![scratchpad_result(
            &[("a", "completed"), ("b", "completed")],
            Some("Ship"),
        )];
        assert!(plan_carry_message(&window).is_none());
    }

    #[test]
    fn a_previous_carry_is_re_carried_when_the_tool_result_is_already_gone() {
        // Second compaction pass: the original scratchpad result was drained by
        // the first pass, leaving only the carry message.
        let first = plan_carry_message(&[scratchpad_result(
            &[("a", "completed"), ("b", "in_progress")],
            Some("Ship"),
        )])
        .expect("first carry");
        let window = vec![user("older turn"), first];
        let text = carry_text(&plan_carry_message(&window).expect("re-carry"));
        assert!(text.contains("- [x] a"));
        assert!(text.contains("- [~] b"));
        assert!(text.contains("Objective: Ship"));
    }

    #[test]
    fn a_tool_result_wins_over_an_older_carry() {
        let stale = plan_carry_message(&[scratchpad_result(
            &[("a", "pending"), ("b", "pending")],
            Some("Ship"),
        )])
        .expect("stale carry");
        let window = vec![
            stale,
            scratchpad_result(&[("a", "completed"), ("b", "in_progress")], Some("Ship")),
        ];
        let text = carry_text(&plan_carry_message(&window).expect("carry"));
        assert!(text.contains("- [x] a"), "newest wins: {text}");
    }

    #[test]
    fn a_non_scratchpad_tool_result_is_ignored_even_with_a_snapshot_key() {
        let window = vec![UnifiedMessage::ToolResult {
            tool_call_id: "c1".into(),
            tool_name: "note_manage".into(),
            content: vec![ContentBlock::Json {
                value: json!({"snapshot": {"items": [{"text": "x", "status": "pending"}]}}),
            }],
            is_error: false,
        }];
        assert!(plan_carry_message(&window).is_none());
    }
}
