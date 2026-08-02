//! Per-run idempotent-tool **no-progress loop** detector — renders a compact
//! "you keep getting the same result" nudge the prompt builder injects ahead
//! of the next Think call.
//!
//! ## Why this exists
//!
//! [`attempt_summary`](crate::tools::attempt_summary) is the sibling of this
//! module: it aggregates **failures** (`SessionEvent::ToolError`) so the model
//! sees "you've burned 3 search calls on `rate_limited` — climb the ladder".
//! It is blind to the *opposite* pathology: a tool that keeps **succeeding**
//! but returns the **identical** payload every time.
//!
//! A model can wedge itself re-reading the same file, re-running the same
//! search, or re-fetching the same URL across turns — each call succeeds, so
//! no `ToolError` fires and the failure summary stays silent, yet the model
//! makes zero progress. hermes-agent's `ToolCallGuardrailController` flags
//! exactly this case ("idempotent tool returns the same result N times").
//!
//! This module closes that gap, mapped onto Aleph's redlines:
//!
//! - **Precision via the idempotency SSOT.** Only tools passing
//!   [`is_idempotent_builtin_name`](crate::tools::retry::is_idempotent_builtin_name)
//!   (pure reads — no writes, sends, or remote mutation) are considered. A
//!   tool whose result is *meant* to change between calls (a write, a clock,
//!   a queue poll) is never flagged, so legitimate progressing loops are not
//!   false-positived.
//! - **Output-identity gated.** A group is only flagged when the canonical
//!   output is byte-identical across every occurrence. Repeated calls that
//!   return *different* results (a poll that is actually advancing) are left
//!   alone.
//! - **R7 / R9 / R10 safe — it is a nudge, never a block.** hermes hard-blocks
//!   the call at its `block` threshold; Aleph's thin-harness redlines forbid
//!   the loop from making completion or control-flow decisions on the model's
//!   behalf. So this only renders text the LLM reads; the harness never
//!   branches on it and never refuses a call. Tool selection stays sovereign.
//!
//! ## What it is NOT
//!
//! - Not a persistence change: computed fresh from the visible event tail on
//!   every turn, never written back to the session log.
//! - Not a `PromptLayer`: per-turn dynamic aggregation belongs in the
//!   per-turn message vector path ([`crate::harness::agent::prompt`]), same as
//!   [`attempt_summary::render_run_summary`](crate::tools::attempt_summary::render_run_summary).
//!
//! ## Output shape
//!
//! ```text
//! <system-reminder>
//! No-progress loop detected this run: an idempotent read tool returned the
//! identical result on repeated identical calls.
//! - read_file × 3 (same args, same output)
//! - search × 4 (same args, same output)
//! These calls are not changing the result. Do not repeat them — the content
//! is stable. Act on what you already have, or take a structurally different
//! step toward the goal.
//! </system-reminder>
//! ```

use std::collections::HashMap;

use crate::harness::agent::canonical_json_string;
use crate::session::events::{SessionEvent, SessionEventRecord};
use crate::tools::retry::is_idempotent_builtin_name;

/// Number of identical successful results from the same idempotent
/// `(tool, args)` before the no-progress nudge fires.
///
/// Three identical reads means the model has confirmed the content is stable
/// twice over and is now looping. Mirrors [`attempt_summary::SUMMARY_THRESHOLD`]
/// (also 3) so the two harness nudges trip on the same cadence.
///
/// [`attempt_summary::SUMMARY_THRESHOLD`]: crate::tools::attempt_summary::SUMMARY_THRESHOLD
pub const NO_PROGRESS_THRESHOLD: usize = 3;

/// One flagged group: an idempotent tool whose `(tool, args)` produced a
/// byte-identical result `count` times in the visible window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoProgressGroup {
    pub tool: String,
    pub count: usize,
}

/// Mutable tally accumulated per `(tool, canonical_args)` key while walking the
/// event window.
struct Tally {
    count: usize,
    /// Canonical fingerprint of the first result seen for this key.
    fingerprint: Option<String>,
    /// `true` while every result for this key has matched `fingerprint`.
    all_identical: bool,
}

impl Tally {
    const fn new() -> Self {
        Self {
            count: 0,
            fingerprint: None,
            all_identical: true,
        }
    }
}

/// Walk `events` and surface idempotent `(tool, args)` keys whose successful
/// result repeated, byte-identical, at least [`NO_PROGRESS_THRESHOLD`] times.
///
/// Tool/args resolution mirrors
/// [`attempt_summary::aggregate_failures`](crate::tools::attempt_summary::aggregate_failures):
/// walk backwards from each `ToolResult` to the most recent
/// `ToolCallRequested` with a matching `call_id`. A result whose request is no
/// longer in the visible window (truncated tail) is skipped — we cannot key it.
///
/// Returns rows sorted by `count` descending, then `tool` ascending, so the
/// render is deterministic for snapshot tests.
pub fn aggregate_no_progress(events: &[SessionEventRecord]) -> Vec<NoProgressGroup> {
    let mut tallies: HashMap<(String, String), Tally> = HashMap::new();

    for (idx, record) in events.iter().enumerate() {
        let SessionEvent::ToolResult {
            call_id, output, ..
        } = &record.event
        else {
            continue;
        };
        let Some((name, input)) = resolve_request(events, idx, call_id) else {
            continue;
        };
        // Only pure-read builtins can loop without progress; a tool whose
        // result is meant to change is never a no-progress signal.
        if !is_idempotent_builtin_name(name) {
            continue;
        }
        let args_key = canonical_json_string(input);
        // Fingerprint the model-facing text with the offload path folded out:
        // an over-budget result is replaced by a marker whose file name is
        // unique per dispatch, which would make every repeat of a byte-identical
        // large result compare *different* and silently disable the whole
        // detector for exactly the expensive loops it exists to catch.
        let fingerprint = match output.value.as_str() {
            Some(text) => crate::tools::result_store::stabilize_persisted_ref(text).into_owned(),
            None => canonical_json_string(&output.value),
        };

        let tally = tallies
            .entry((name.to_string(), args_key))
            .or_insert_with(Tally::new);
        tally.count += 1;
        match &tally.fingerprint {
            None => tally.fingerprint = Some(fingerprint),
            Some(prev) => {
                if *prev != fingerprint {
                    tally.all_identical = false;
                }
            }
        }
    }

    let mut groups: Vec<NoProgressGroup> = tallies
        .into_iter()
        .filter(|(_, t)| t.count >= NO_PROGRESS_THRESHOLD && t.all_identical)
        .map(|((tool, _args), t)| NoProgressGroup {
            tool,
            count: t.count,
        })
        .collect();

    groups.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.tool.cmp(&b.tool)));
    groups
}

/// Render the no-progress nudge, or `None` when no idempotent key has looped
/// to threshold yet (so callers keep a branch-free hot path).
///
/// Wrapped in `<system-reminder>` to match the rest of Aleph's
/// harness-injected channel — the model reads it as harness chatter, not a
/// fresh user instruction.
#[must_use]
pub fn render_no_progress_notice(events: &[SessionEventRecord]) -> Option<String> {
    let groups = aggregate_no_progress(events);
    if groups.is_empty() {
        return None;
    }

    let mut out = String::with_capacity(256);
    out.push_str("<system-reminder>\n");
    out.push_str(
        "No-progress loop detected this run: an idempotent read tool returned the \
         identical result on repeated identical calls.\n",
    );
    for g in &groups {
        out.push_str("- ");
        out.push_str(&g.tool);
        out.push_str(" × ");
        push_usize(&mut out, g.count);
        out.push_str(" (same args, same output)\n");
    }
    out.push_str(
        "These calls are not changing the result. Do not repeat them — the content \
         is stable. Act on what you already have, or take a structurally different \
         step toward the goal.\n",
    );
    out.push_str("</system-reminder>");
    Some(out)
}

fn push_usize(out: &mut String, n: usize) {
    use std::fmt::Write;
    let _ = write!(out, "{n}");
}

/// Walk backwards from `result_idx` to the most recent `ToolCallRequested`
/// whose `call_id` matches, returning its `(name, input)`. O(n) per result but
/// n is the tail-slice length, not the whole session log.
fn resolve_request<'a>(
    events: &'a [SessionEventRecord],
    result_idx: usize,
    call_id: &str,
) -> Option<(&'a str, &'a serde_json::Value)> {
    let upper = result_idx.min(events.len());
    events[..upper].iter().rev().find_map(|r| match &r.event {
        SessionEvent::ToolCallRequested {
            call_id: id,
            name,
            input,
            ..
        } if id == call_id => Some((name.as_str(), input)),
        _ => None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::events::{now_ms, SessionEvent, ToolOutput};
    use std::sync::atomic::{AtomicU64, Ordering};

    fn mk(event: SessionEvent) -> SessionEventRecord {
        static SEQ: AtomicU64 = AtomicU64::new(1);
        SessionEventRecord {
            seq: SEQ.fetch_add(1, Ordering::Relaxed),
            event,
            created_at_ms: now_ms(),
        }
    }

    fn req(call_id: &str, name: &str, input: serde_json::Value) -> SessionEventRecord {
        mk(SessionEvent::ToolCallRequested {
            turn_id: uuid::Uuid::nil(),
            call_id: call_id.to_string(),
            name: name.to_string(),
            input,
            at: now_ms(),
        })
    }

    fn res(call_id: &str, value: serde_json::Value) -> SessionEventRecord {
        mk(SessionEvent::ToolResult {
            turn_id: uuid::Uuid::nil(),
            call_id: call_id.to_string(),
            output: ToolOutput {
                value,
                metadata: Default::default(),
            },
            at: now_ms(),
        })
    }

    /// Three identical `search` reads (idempotent, same args, same output) →
    /// nudge fires.
    #[test]
    fn three_identical_idempotent_results_fire() {
        let args = serde_json::json!({ "query": "rust async" });
        let out = serde_json::json!({ "hits": ["a", "b"] });
        let events = vec![
            req("c1", "search", args.clone()),
            res("c1", out.clone()),
            req("c2", "search", args.clone()),
            res("c2", out.clone()),
            req("c3", "search", args.clone()),
            res("c3", out.clone()),
        ];
        let s = render_no_progress_notice(&events).expect("3 identical reads must nudge");
        assert!(s.starts_with("<system-reminder>"));
        assert!(s.contains("search × 3 (same args, same output)"));
        assert!(s.ends_with("</system-reminder>"));
    }

    #[test]
    fn below_threshold_is_silent() {
        let args = serde_json::json!({ "query": "x" });
        let out = serde_json::json!("same");
        let events = vec![
            req("c1", "search", args.clone()),
            res("c1", out.clone()),
            req("c2", "search", args.clone()),
            res("c2", out.clone()),
        ];
        assert!(
            render_no_progress_notice(&events).is_none(),
            "2 identical reads should not fire (threshold 3)",
        );
    }

    /// Same tool + same args but the output *changes* → progressing, not a
    /// loop → silent.
    #[test]
    fn changing_output_is_not_flagged() {
        let args = serde_json::json!({ "query": "queue" });
        let events = vec![
            req("c1", "search", args.clone()),
            res("c1", serde_json::json!({ "n": 1 })),
            req("c2", "search", args.clone()),
            res("c2", serde_json::json!({ "n": 2 })),
            req("c3", "search", args.clone()),
            res("c3", serde_json::json!({ "n": 3 })),
        ];
        assert!(
            render_no_progress_notice(&events).is_none(),
            "advancing results must not be treated as no-progress",
        );
    }

    /// Different argument key orderings canonicalize equal — still one group.
    #[test]
    fn canonical_args_collapse_key_order() {
        let out = serde_json::json!("stable");
        let events = vec![
            req("c1", "memory_search", serde_json::json!({ "a": 1, "b": 2 })),
            res("c1", out.clone()),
            req("c2", "memory_search", serde_json::json!({ "b": 2, "a": 1 })),
            res("c2", out.clone()),
            req("c3", "memory_search", serde_json::json!({ "a": 1, "b": 2 })),
            res("c3", out.clone()),
        ];
        let groups = aggregate_no_progress(&events);
        assert_eq!(groups.len(), 1, "reordered keys must collapse to one group");
        assert_eq!(groups[0].count, 3);
        assert_eq!(groups[0].tool, "memory_search");
    }

    /// A non-idempotent tool (bash) repeating identical output is NOT flagged —
    /// only pure reads can be no-progress loops.
    #[test]
    fn non_idempotent_tool_is_ignored() {
        let args = serde_json::json!({ "cmd": "echo hi" });
        let out = serde_json::json!("hi\n");
        let events = vec![
            req("c1", "bash", args.clone()),
            res("c1", out.clone()),
            req("c2", "bash", args.clone()),
            res("c2", out.clone()),
            req("c3", "bash", args.clone()),
            res("c3", out.clone()),
        ];
        assert!(
            render_no_progress_notice(&events).is_none(),
            "bash is not idempotent — must never be flagged",
        );
    }

    /// Same idempotent tool but *different* args are independent keys; neither
    /// reaches threshold.
    #[test]
    fn distinct_args_are_separate_keys() {
        let out = serde_json::json!("r");
        let events = vec![
            req("c1", "search", serde_json::json!({ "q": "one" })),
            res("c1", out.clone()),
            req("c2", "search", serde_json::json!({ "q": "two" })),
            res("c2", out.clone()),
            req("c3", "search", serde_json::json!({ "q": "three" })),
            res("c3", out.clone()),
        ];
        assert!(
            render_no_progress_notice(&events).is_none(),
            "three distinct queries are progress, not a loop",
        );
    }

    /// A result whose request fell out of the visible window is skipped, not
    /// counted under a bogus key.
    #[test]
    fn orphan_result_without_request_is_skipped() {
        let events = vec![res("ghost", serde_json::json!("x"))];
        assert!(aggregate_no_progress(&events).is_empty());
    }

    /// Two distinct looping tools → both rows, sorted by count desc then name.
    #[test]
    fn multiple_groups_sorted_deterministically() {
        let mut events = Vec::new();
        // web_fetch × 4 identical
        for i in 0..4 {
            let cid = format!("w{i}");
            events.push(req(&cid, "web_fetch", serde_json::json!({ "url": "u" })));
            events.push(res(&cid, serde_json::json!("page")));
        }
        // search × 3 identical
        for i in 0..3 {
            let cid = format!("s{i}");
            events.push(req(&cid, "search", serde_json::json!({ "q": "k" })));
            events.push(res(&cid, serde_json::json!("hit")));
        }
        let groups = aggregate_no_progress(&events);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].tool, "web_fetch");
        assert_eq!(groups[0].count, 4);
        assert_eq!(groups[1].tool, "search");
        assert_eq!(groups[1].count, 3);
    }

    #[test]
    fn render_stays_compact() {
        let mut events = Vec::new();
        for i in 0..30 {
            let cid = format!("c{i}");
            let tool = if i % 2 == 0 { "search" } else { "web_fetch" };
            let q = if i % 2 == 0 { "qa" } else { "qb" };
            events.push(req(&cid, tool, serde_json::json!({ "q": q })));
            events.push(res(&cid, serde_json::json!("same")));
        }
        let s = render_no_progress_notice(&events).expect("looping tools render");
        assert!(s.len() < 2048, "notice length {}", s.len());
    }

    #[test]
    fn threshold_constant_is_three() {
        // Guard: keep in lockstep with attempt_summary::SUMMARY_THRESHOLD so
        // the two harness nudges trip on the same cadence.
        assert_eq!(NO_PROGRESS_THRESHOLD, 3);
    }
}
