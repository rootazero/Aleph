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

/// Where this pass's list came from — which decides whether it is rendered or
/// re-emitted verbatim.
enum CarrySource {
    /// A `scratchpad` tool result still in the window: render it.
    Fresh(ScratchpadSnapshot),
    /// A carry an earlier pass already emitted: **re-emit the same bytes**.
    ///
    /// Not a micro-optimisation — a correctness requirement once the render is
    /// bounded. Parsing a bounded carry back and re-rendering it would make the
    /// second pass compute `Progress: x/y` over the items that survived the
    /// first pass's elision, reporting a total that was never true of the plan.
    /// A reduction stage that is handed the survivors of an earlier reduction
    /// cannot tell them apart from the whole; passing the text through means it
    /// is never asked to.
    Prior {
        snapshot: ScratchpadSnapshot,
        text: String,
    },
}

impl CarrySource {
    fn snapshot(&self) -> &ScratchpadSnapshot {
        match self {
            Self::Fresh(s) | Self::Prior { snapshot: s, .. } => s,
        }
    }
}

/// Render the carry message for a compaction window, or `None` when the window
/// holds no execution list (the overwhelmingly common case — calm runs pay
/// nothing).
///
/// Returns `None` for a finished list too: a fully-checked plan needs no
/// reminder, and re-injecting one invites the model to re-open closed work.
pub(crate) fn plan_carry_message(window: &[UnifiedMessage]) -> Option<UnifiedMessage> {
    let source = latest_plan(window)?;
    if !source.snapshot().items.iter().any(|i| !i.is_done()) {
        return None;
    }
    let text = match source {
        CarrySource::Fresh(s) => render_carry(&s),
        CarrySource::Prior { text, .. } => text,
    };
    Some(UnifiedMessage::User {
        content: vec![ContentBlock::Text {
            text,
            cache_control: None,
        }],
    })
}

/// Bounded, for the same reason `active_execution_plan` is: this message is
/// spliced into the compacted history by `splice_preserved`, which adds it
/// **after** the budget arithmetic is done — so its size is paid, unaccounted,
/// on every request until the next compaction, at the one moment the window was
/// already over budget. The plan reaches the prompt on two legs; a bound on one
/// of them is not a bound.
///
/// Ordinary plans render byte-identically to the unbounded form.
fn render_carry(snapshot: &ScratchpadSnapshot) -> String {
    format!(
        "<system-reminder>\nReference data, not user input.\n{CARRY_MARKER}\n{}\n\
         Keep working it with the `scratchpad` tool (start_item / complete_item use the \
         0-based index of this list).\n</system-reminder>",
        snapshot.render_progress_bounded()
    )
}

/// Newest execution list in the window: the last `scratchpad` tool result that
/// carried a snapshot, or — when a previous pass already drained those — the
/// last carry message this module itself emitted.
fn latest_plan(window: &[UnifiedMessage]) -> Option<CarrySource> {
    window.iter().rev().find_map(|msg| match msg {
        UnifiedMessage::ToolResult {
            tool_name, content, ..
        } if tool_name == SCRATCHPAD_TOOL => content
            .iter()
            .find_map(snapshot_from_block)
            .map(CarrySource::Fresh),
        UnifiedMessage::User { content } => content.iter().find_map(|block| match block {
            ContentBlock::Text { text, .. } if text.contains(CARRY_MARKER) => {
                // Parsed only to answer "is there unfinished work here?" — the
                // bytes that go back out are `text`, not a re-render of this.
                parse_progress(text)
                    .filter(|s| !s.items.is_empty())
                    .map(|snapshot| CarrySource::Prior {
                        snapshot,
                        text: text.clone(),
                    })
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

    /// A second pass over an already-carried list re-emits the FIRST pass's
    /// bytes, not a re-render of what it could parse back.
    ///
    /// With a bounded render the difference is load-bearing: re-rendering would
    /// recompute `Progress:` over the items that survived the first elision and
    /// state a total the plan never had. Passing the text through means the
    /// second stage is never handed the first stage's survivors and asked to
    /// treat them as the whole.
    #[test]
    fn a_re_carry_reuses_the_bytes_it_was_given() {
        let first = plan_carry_message(&[scratchpad_result(
            &[("a", "completed"), ("b", "in_progress")],
            Some("Ship"),
        )])
        .expect("first carry");
        let first_text = carry_text(&first);

        let second = plan_carry_message(&[user("older turn"), first]).expect("re-carry");
        assert_eq!(
            carry_text(&second),
            first_text,
            "a re-carry must be byte-identical, not a re-render"
        );
    }

    /// The carry is spliced in after the compactor's budget arithmetic, so its
    /// size is unaccounted — it needs the same ceiling the prompt layer has.
    #[test]
    fn the_carry_is_bounded_for_a_pathological_plan() {
        let items: Vec<(String, &str)> = (0..400)
            .map(|i| (format!("{}{i}", "x".repeat(800)), "pending"))
            .collect();
        let borrowed: Vec<(&str, &str)> = items.iter().map(|(t, s)| (t.as_str(), *s)).collect();
        let text =
            carry_text(&plan_carry_message(&[scratchpad_result(&borrowed, Some("Ship"))]).unwrap());
        assert!(
            text.len() < 20_000,
            "the carry rides in the compacted history unaccounted; got {} bytes",
            text.len()
        );
        assert!(text.contains("not shown here"), "elision must be disclosed");
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
