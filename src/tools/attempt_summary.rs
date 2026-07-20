//! Per-run tool-failure aggregator — renders a compact "you've already tried"
//! summary that the prompt builder injects ahead of the next Think call.
//!
//! ## Why this exists
//!
//! Each individual `SessionEvent::ToolError` already carries a routing hint
//! (see `tools::fallback_registry::render_persistence_hint`) appended at
//! emit time. That hint is single-failure scoped: "this 401 on `web_fetch` →
//! try alternate source / autocli". A model that keeps banging on the same
//! ladder rung still sees only one hint per attempt and can lose sight of
//! the broader pattern.
//!
//! When the same run accumulates ≥ [`SUMMARY_THRESHOLD`] tool errors, the
//! aggregate view becomes the more useful signal: "you've burned 3 search
//! calls and 2 `web_fetch` calls on `rate_limited` — escalate to browser
//! tooling, don't retry the same family again."
//!
//! ## What it is NOT
//!
//! - Not a [`PromptLayer`](crate::thinker::prompt_layer::PromptLayer):
//!   layers live in the cached system prompt prefix, which is built once
//!   at harness construction. Per-turn dynamic aggregation belongs in the
//!   per-turn message vector path ([`crate::harness::agent::prompt`]).
//! - Not a persistence change: the summary is computed fresh from
//!   `events[tail_start..]` on every turn and never written back to the
//!   session log. The log stays a faithful record of what actually happened.
//! - Not a control-flow signal: harness never branches on the summary;
//!   it only renders text the LLM reads. R7 / R9 keep tool selection in
//!   the model's hands.
//!
//! ## Output shape
//!
//! Wrapped in `<system-reminder>` so the model recognises it as harness
//! chatter, not a user instruction. Body lists per-`(tool, kind)` groups
//! sorted by count descending, then alphabetical:
//!
//! ```text
//! <system-reminder>
//! Tool attempt summary for this run (5 failures):
//! - search × 3 (rate_limited)
//! - web_fetch × 2 (unauthorized)
//! Doctrine: when ≥3 failures of the same kind, climb the ladder
//!   (search → web_fetch → autocli/browser tooling) instead of retrying
//!   the same family. Choose a method that is structurally different
//!   from the ones you have already tried.
//! </system-reminder>
//! ```

use std::collections::HashMap;

use crate::session::events::{SessionEvent, SessionEventRecord};
use crate::tools::error_kind::{classify_error_str, ToolErrorKind};

/// Minimum total `ToolError` count in the visible event window before the
/// summary is rendered. Below this threshold each individual error's hint
/// (from `fallback_registry::render_persistence_hint`) is enough; the
/// aggregate adds noise.
///
/// Two-strike-plus heuristic: once a `(tool, kind)` pair has failed this many
/// times the aggregate nudges the model to switch family. The old guidelines.rs
/// "2-strike" prompt rule was removed (§1.1 prune-the-prompt); this runtime
/// signal, surfaced in the error hint, is now the LLM-visible form of it.
pub const SUMMARY_THRESHOLD: usize = 3;

/// One row of the aggregate: how many times a given `(tool, kind)` pair
/// failed in the current event window.
///
/// Eq / Hash so callers can de-duplicate or sort deterministically — the
/// renderer relies on stable ordering.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ToolFailureGroup {
    pub tool: String,
    pub kind: ToolErrorKind,
    pub count: usize,
}

/// Walk `events` and tally `ToolError` records grouped by
/// `(tool_name, error_kind)`. Returns rows sorted by `count` descending,
/// then `(tool, kind.label())` ascending so the output is deterministic
/// for snapshot tests.
///
/// Tool name resolution mirrors
/// [`crate::harness::agent::prompt::build_prompt`]'s lookup: walk
/// backwards from the `ToolError` to find the most recent
/// `ToolCallRequested` with a matching `call_id`. If no matching request
/// is found (truncated tail, replay edge case) the call is recorded
/// under `"unknown"` so the LLM still sees the kind tally.
#[must_use]
pub fn aggregate_failures(events: &[SessionEventRecord]) -> Vec<ToolFailureGroup> {
    let mut counts: HashMap<(String, ToolErrorKind), usize> = HashMap::new();

    for (idx, record) in events.iter().enumerate() {
        let SessionEvent::ToolError { call_id, error, .. } = &record.event else {
            continue;
        };
        let kind = classify_error_str(error);
        let tool = resolve_tool_name(events, idx, call_id).unwrap_or("unknown");
        *counts.entry((tool.to_string(), kind)).or_insert(0) += 1;
    }

    let mut groups: Vec<ToolFailureGroup> = counts
        .into_iter()
        .map(|((tool, kind), count)| ToolFailureGroup { tool, kind, count })
        .collect();

    groups.sort_by(|a, b| {
        b.count
            .cmp(&a.count)
            .then_with(|| a.tool.cmp(&b.tool))
            .then_with(|| a.kind.label().cmp(b.kind.label()))
    });

    groups
}

/// Render the run-level failure summary when total errors ≥ [`SUMMARY_THRESHOLD`].
///
/// Returns `None` when no aggregation is warranted yet, so callers can
/// keep their hot path branch-free (`if let Some(text) = render_run_summary(...)`).
///
/// The output is wrapped in `<system-reminder>` matching the rest of
/// Aleph's harness-injected channel (see opencode-parity G2 in
/// [`crate::harness::agent::prompt`]).
#[must_use]
pub fn render_run_summary(events: &[SessionEventRecord]) -> Option<String> {
    let groups = aggregate_failures(events);
    let total: usize = groups.iter().map(|g| g.count).sum();
    if total < SUMMARY_THRESHOLD {
        return None;
    }

    let mut out = String::with_capacity(256);
    out.push_str("<system-reminder>\n");
    out.push_str("Tool attempt summary for this run (");
    push_usize(&mut out, total);
    out.push_str(" failures):\n");

    for g in &groups {
        out.push_str("- ");
        out.push_str(&g.tool);
        out.push_str(" × ");
        push_usize(&mut out, g.count);
        out.push_str(" (");
        out.push_str(g.kind.label());
        out.push_str(")\n");
    }

    out.push_str(
        "Doctrine: when ≥3 failures of the same kind, climb the ladder \
         (search → web_fetch → autocli/browser tooling) instead of retrying \
         the same family. Choose a method that is structurally different \
         from the ones you have already tried.\n",
    );
    out.push_str("</system-reminder>");
    Some(out)
}

fn push_usize(out: &mut String, n: usize) {
    use std::fmt::Write;
    let _ = write!(out, "{n}");
}

/// Walk backwards from `error_idx` looking for the most recent
/// `ToolCallRequested` whose `call_id` matches. O(n) per error but n is
/// the tail-slice length (≲ harness `max_iterations` × ~2), not the full
/// session log.
fn resolve_tool_name<'a>(
    events: &'a [SessionEventRecord],
    error_idx: usize,
    call_id: &str,
) -> Option<&'a str> {
    let upper = error_idx.min(events.len());
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
    use crate::session::events::{now_ms, MessageContent, SessionEvent, ToolOutput, TurnTrigger};
    use std::sync::atomic::{AtomicU64, Ordering};

    fn mk(event: SessionEvent) -> SessionEventRecord {
        static SEQ: AtomicU64 = AtomicU64::new(1);
        SessionEventRecord {
            seq: SEQ.fetch_add(1, Ordering::Relaxed),
            event,
            created_at_ms: now_ms(),
        }
    }

    fn tcr(call_id: &str, name: &str) -> SessionEventRecord {
        mk(SessionEvent::ToolCallRequested {
            turn_id: uuid::Uuid::nil(),
            call_id: call_id.to_string(),
            name: name.to_string(),
            input: serde_json::json!({}),
            at: now_ms(),
        })
    }

    fn terr(call_id: &str, msg: &str) -> SessionEventRecord {
        mk(SessionEvent::ToolError {
            turn_id: uuid::Uuid::nil(),
            call_id: call_id.to_string(),
            error: msg.to_string(),
            at: now_ms(),
        })
    }

    #[test]
    fn below_threshold_returns_none() {
        let events = vec![
            tcr("c1", "search"),
            terr("c1", "HTTP 429 rate limit"),
            tcr("c2", "search"),
            terr("c2", "HTTP 429 rate limit"),
        ];
        assert!(
            render_run_summary(&events).is_none(),
            "2 failures should not trigger summary (threshold 3)",
        );
    }

    #[test]
    fn at_threshold_emits_summary() {
        let events = vec![
            tcr("c1", "search"),
            terr("c1", "HTTP 429 rate limit exceeded"),
            tcr("c2", "search"),
            terr("c2", "HTTP 429 rate limit exceeded"),
            tcr("c3", "search"),
            terr("c3", "HTTP 429 rate limit exceeded"),
        ];
        let s = render_run_summary(&events).expect("3 failures must render");
        assert!(s.starts_with("<system-reminder>"));
        assert!(s.contains("(3 failures)"));
        assert!(s.contains("search × 3 (rate_limited)"));
        assert!(s.contains("Doctrine"));
        assert!(s.ends_with("</system-reminder>"));
    }

    #[test]
    fn groups_by_tool_and_kind_independently() {
        let events = vec![
            tcr("c1", "search"),
            terr("c1", "HTTP 429 rate limit"),
            tcr("c2", "search"),
            terr("c2", "HTTP 429 rate limit"),
            tcr("c3", "web_fetch"),
            terr("c3", "HTTP 401 Unauthorized"),
            tcr("c4", "web_fetch"),
            terr("c4", "HTTP 401 Unauthorized"),
        ];
        let groups = aggregate_failures(&events);
        assert_eq!(groups.len(), 2);
        // count-descending sort; ties broken by tool name ASC
        assert_eq!(groups[0].count, 2);
        assert_eq!(groups[1].count, 2);
        let tools: Vec<&str> = groups.iter().map(|g| g.tool.as_str()).collect();
        assert_eq!(tools, vec!["search", "web_fetch"]);
    }

    #[test]
    fn sort_is_count_desc_then_tool_asc() {
        let events = vec![
            tcr("c1", "web_fetch"),
            terr("c1", "HTTP 401"),
            tcr("c2", "search"),
            terr("c2", "HTTP 429"),
            tcr("c3", "search"),
            terr("c3", "HTTP 429"),
            tcr("c4", "search"),
            terr("c4", "HTTP 429"),
        ];
        let groups = aggregate_failures(&events);
        assert_eq!(groups[0].tool, "search");
        assert_eq!(groups[0].count, 3);
        assert_eq!(groups[1].tool, "web_fetch");
        assert_eq!(groups[1].count, 1);
    }

    #[test]
    fn resolves_tool_name_via_preceding_request() {
        let events = vec![tcr("call_a", "search"), terr("call_a", "HTTP 429")];
        let groups = aggregate_failures(&events);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].tool, "search");
    }

    #[test]
    fn orphan_error_without_matching_request_falls_back_to_unknown() {
        let events = vec![terr("ghost_id", "HTTP 500")];
        let groups = aggregate_failures(&events);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].tool, "unknown");
    }

    #[test]
    fn non_error_events_are_ignored() {
        let events = vec![
            mk(SessionEvent::TurnStarted {
                turn_id: uuid::Uuid::nil(),
                trigger: TurnTrigger::UserMessage,
                at: now_ms(),
            }),
            mk(SessionEvent::UserMessage {
                turn_id: uuid::Uuid::nil(),
                content: MessageContent {
                    text: "hi".into(),
                    blocks: vec![],
                    thinking: None,
                    thinking_signature: None,
                },
                at: now_ms(),
                synthetic: false,
            }),
            tcr("c1", "search"),
            mk(SessionEvent::ToolResult {
                turn_id: uuid::Uuid::nil(),
                call_id: "c1".into(),
                output: ToolOutput {
                    value: serde_json::json!("ok"),
                    metadata: Default::default(),
                },
                at: now_ms(),
            }),
        ];
        let groups = aggregate_failures(&events);
        assert!(groups.is_empty(), "no ToolError → no groups");
    }

    #[test]
    fn rendered_text_is_compact_under_2kb() {
        // Worst-case: many distinct (tool, kind) pairs. Render must stay
        // well under typical LLM context budgets — the summary is a hint,
        // not a transcript.
        let mut events = Vec::new();
        for i in 0..20 {
            let cid = format!("c{i}");
            let tool = if i % 2 == 0 { "search" } else { "web_fetch" };
            events.push(tcr(&cid, tool));
            events.push(terr(&cid, "HTTP 429 rate limit"));
        }
        let s = render_run_summary(&events).expect("20 errors render");
        assert!(s.len() < 2048, "summary length {}", s.len());
    }

    #[test]
    fn different_error_strings_classify_to_same_kind_aggregate() {
        let events = vec![
            tcr("c1", "search"),
            terr("c1", "HTTP 429 rate limit exceeded"),
            tcr("c2", "search"),
            terr("c2", "quota exceeded; please retry later"),
            tcr("c3", "search"),
            terr("c3", "Too Many Requests"),
        ];
        let groups = aggregate_failures(&events);
        // All three classify as RateLimited → single row, count 3.
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].count, 3);
        assert_eq!(groups[0].kind, ToolErrorKind::RateLimited);
    }

    #[test]
    fn default_default_for_threshold_constant() {
        // Sanity guard: SUMMARY_THRESHOLD is the 2-strike switch-family
        // threshold surfaced in the runtime error hint (the prompt-side rule
        // was removed under §1.1 prune-the-prompt; the signal carries it now).
        assert_eq!(SUMMARY_THRESHOLD, 3);
    }
}
