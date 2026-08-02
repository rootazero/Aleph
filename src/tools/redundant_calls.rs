//! Per-run **redundant side-effecting call loop** detector — renders a compact
//! "you keep re-issuing this exact call and getting the identical result" nudge
//! the prompt builder injects ahead of the next Think call.
//!
//! ## Why this exists
//!
//! [`no_progress`](crate::tools::no_progress) is the closest sibling: it flags
//! an *idempotent* read tool that returns the identical payload on identical
//! args. It deliberately **excludes non-idempotent tools** (bash, `code_exec`,
//! writes, sends) on the reasoning that their results are *meant* to change
//! between calls, so flagging them would false-positive a legitimately
//! progressing loop.
//!
//! That exclusion leaves a real gap, the one that produced the worst observed
//! runaway: a model alternating `code_exec` ↔ `bash`, re-issuing the **exact
//! same** command 89 times each (`python3 -m http.server …`), every call
//! returning the **byte-identical** failure payload, with a sentence of
//! narration between each. Every existing guard was blind to it:
//!
//! - [`crate::verification::tool_loop_verifier`] keys on *consecutive*
//!   `(name, args_hash)` — perfect A/B alternation keeps the trailing run at 1,
//!   and its Tier-2 same-name fallback is gated on the *absence* of narration.
//! - [`crate::tools::attempt_summary`] counts only `SessionEvent::ToolError`;
//!   these failures are `Ok` `ToolResult`s carrying `success:false` in the
//!   payload, so the failure aggregator never sees them.
//! - [`no_progress`](crate::tools::no_progress) excludes `bash/code_exec`.
//! - [`gather_budget`](crate::tools::gather_budget) counts only `search/web_fetch`.
//!
//! This module closes that gap as the exact **complement** of `no_progress`:
//!
//! - **Covers non-idempotent tools** (`!is_idempotent_builtin_name`) — bash,
//!   `code_exec`, writes, sends, MCP/extension tools. Idempotent reads stay with
//!   `no_progress`, so the two partition the tool space with no double-nudge.
//! - **Gated on result identity** — the safety that lets it include
//!   side-effecting tools without false-positiving a progressing loop. A tool
//!   whose result is *meant* to change (a `cargo test` re-run after an edit, a
//!   poll that advances) returns *different* bytes, so it is never flagged. Only
//!   a call that re-runs identically **and** keeps producing the same bytes is
//!   surfaced — that is stuck regardless of idempotency.
//! - **Order-independent.** Like `no_progress`, it buckets by `(tool, args)`
//!   over the visible window, so interleaving (the A/B alternation that defeats
//!   the consecutive-run verifier) does not hide the repetition.
//! - **R7 / R9 / R10 safe — a nudge, never a block.** It only renders text the
//!   LLM reads; the harness never branches on it and never refuses a call. Tool
//!   selection stays sovereign, same as the three sibling notices.
//!
//! ## What it is NOT
//!
//! - Not a persistence change: computed fresh from the visible event tail every
//!   turn, never written back to the session log.
//! - Not a `PromptLayer`: per-turn dynamic aggregation belongs in the per-turn
//!   message vector path ([`crate::harness::agent::prompt`]), same as the other
//!   three notices.
//! - Not a completion judgment: it states a resource fact (this exact call,
//!   re-run N times, is not changing its result) and a doctrine; the model
//!   decides what to do (R7).
//!
//! ## Output shape
//!
//! ```text
//! <system-reminder>
//! Redundant call loop detected this run: you re-issued the same call and got
//! the identical result every time.
//! - code_exec × 89 (same args, same result)
//! - bash × 89 (same args, same result)
//! Re-running an identical call that keeps returning the identical result makes
//! no progress — this holds even across different tools and even when you narrate
//! between attempts. Stop repeating it: if the result is a failure, the method is
//! blocked, so take a structurally different approach or deliver with what you
//! have and state the blocker plainly (rule 13); if it already succeeded, act on
//! the result you have instead of fetching it again.
//! </system-reminder>
//! ```

use std::collections::HashMap;

use crate::harness::agent::canonical_json_string;
use crate::session::events::{SessionEvent, SessionEventRecord};
use crate::tools::retry::is_idempotent_builtin_name;

/// Number of identical `(tool, args)` calls returning the byte-identical result
/// before the redundant-loop nudge fires.
///
/// Matches [`no_progress::NO_PROGRESS_THRESHOLD`] (also 3) and
/// [`attempt_summary::SUMMARY_THRESHOLD`] so all the harness nudges trip on the
/// same cadence: a third byte-identical re-run is where redundancy is no longer
/// plausibly intentional.
///
/// [`no_progress::NO_PROGRESS_THRESHOLD`]: crate::tools::no_progress::NO_PROGRESS_THRESHOLD
/// [`attempt_summary::SUMMARY_THRESHOLD`]: crate::tools::attempt_summary::SUMMARY_THRESHOLD
pub const REDUNDANT_CALL_THRESHOLD: usize = 3;

/// One flagged group: a non-idempotent tool whose `(tool, args)` produced a
/// byte-identical result `count` times in the visible window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RedundantCallGroup {
    pub tool: String,
    pub count: usize,
}

/// Mutable tally accumulated per `(tool, canonical_args)` key while walking the
/// event window. Mirrors [`no_progress`](crate::tools::no_progress)'s tally.
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

/// Walk `events` and surface **non-idempotent** `(tool, args)` keys whose
/// successful result repeated, byte-identical, at least
/// [`REDUNDANT_CALL_THRESHOLD`] times.
///
/// Tool/args resolution mirrors
/// [`no_progress::aggregate_no_progress`](crate::tools::no_progress::aggregate_no_progress):
/// walk backwards from each `ToolResult` to the most recent `ToolCallRequested`
/// with a matching `call_id`. A result whose request is no longer in the visible
/// window (truncated tail) is skipped — we cannot key it.
///
/// Idempotent read tools are skipped here on purpose: they are
/// [`no_progress`](crate::tools::no_progress)'s territory, so the two notices
/// never double-fire on the same key.
///
/// Returns rows sorted by `count` descending, then `tool` ascending, so the
/// render is deterministic for snapshot tests.
pub fn aggregate_redundant_calls(events: &[SessionEventRecord]) -> Vec<RedundantCallGroup> {
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
        // Complement of no_progress: idempotent reads are handled there.
        if is_idempotent_builtin_name(name) {
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

    let mut groups: Vec<RedundantCallGroup> = tallies
        .into_iter()
        .filter(|(_, t)| t.count >= REDUNDANT_CALL_THRESHOLD && t.all_identical)
        .map(|((tool, _args), t)| RedundantCallGroup {
            tool,
            count: t.count,
        })
        .collect();

    groups.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.tool.cmp(&b.tool)));
    groups
}

/// Render the redundant-call nudge, or `None` when no non-idempotent key has
/// looped to threshold yet (so callers keep a branch-free hot path).
///
/// Wrapped in `<system-reminder>` to match the rest of Aleph's harness-injected
/// channel — the model reads it as harness chatter, not a fresh user instruction.
#[must_use]
pub fn render_redundant_call_notice(events: &[SessionEventRecord]) -> Option<String> {
    let groups = aggregate_redundant_calls(events);
    if groups.is_empty() {
        return None;
    }

    let mut out = String::with_capacity(384);
    out.push_str("<system-reminder>\n");
    out.push_str(
        "Redundant call loop detected this run: you re-issued the same call and got \
         the identical result every time.\n",
    );
    for g in &groups {
        out.push_str("- ");
        out.push_str(&g.tool);
        out.push_str(" × ");
        push_usize(&mut out, g.count);
        out.push_str(" (same args, same result)\n");
    }
    out.push_str(
        "Re-running an identical call that keeps returning the identical result makes \
         no progress — this holds even across different tools and even when you narrate \
         between attempts. Stop repeating it: if the result is a failure, the method is \
         blocked, so take a structurally different approach or deliver with what you have \
         and state the blocker plainly (rule 13); if it already succeeded, act on the \
         result you have instead of fetching it again.\n",
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
/// n is the tail-slice length, not the whole session log. Mirrors
/// [`no_progress`](crate::tools::no_progress)'s resolver.
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

    /// The same loop, but with results big enough to be offloaded.
    ///
    /// Over-budget results are replaced wholesale by a `[Full output persisted:
    /// …]` marker whose file name is minted per dispatch, so fingerprinting the
    /// model-facing text verbatim made every repeat compare *different* and the
    /// `all_identical` gate could never close. The detector therefore inverted:
    /// a 300-byte failure loop fired at 3, while a 20k-token `cargo test` loop —
    /// the one that actually costs money — was invisible at 40.
    #[test]
    fn an_offloaded_result_loop_still_fires() {
        let args = serde_json::json!({ "cmd": "cargo test" });
        let mut events = Vec::new();
        for i in 0..REDUNDANT_CALL_THRESHOLD {
            let id = format!("c{i}");
            events.push(req(&id, "bash", args.clone()));
            // Byte-identical output, offloaded under a per-dispatch file name.
            events.push(res(
                &id,
                serde_json::json!(format!(
                    "[Full output persisted: /tmp/tool_results/s1/{}_bash.txt (20431 tokens, bash)]",
                    uuid::Uuid::new_v4().simple()
                )),
            ));
        }
        let groups = aggregate_redundant_calls(&events);
        assert_eq!(
            groups.first().map(|g| (g.tool.as_str(), g.count)),
            Some(("bash", REDUNDANT_CALL_THRESHOLD)),
            "identical offloaded results must not be told apart by their file names"
        );
        assert!(render_redundant_call_notice(&events).is_some());
    }

    /// The stabilizer must not blind the detector to genuinely different
    /// offloaded results — a loop that is actually progressing still differs in
    /// the size the marker reports.
    #[test]
    fn offloaded_results_of_different_sizes_are_still_distinguished() {
        let args = serde_json::json!({ "cmd": "cargo test" });
        let mut events = Vec::new();
        for i in 0..REDUNDANT_CALL_THRESHOLD {
            let id = format!("c{i}");
            events.push(req(&id, "bash", args.clone()));
            events.push(res(
                &id,
                serde_json::json!(format!(
                    "[Full output persisted: /tmp/tool_results/s1/{}_bash.txt ({} tokens, bash)]",
                    uuid::Uuid::new_v4().simple(),
                    20000 + i
                )),
            ));
        }
        assert!(
            aggregate_redundant_calls(&events).is_empty(),
            "a loop whose output keeps changing size is progressing, not stuck"
        );
    }

    /// The exact incident: `code_exec` ↔ `bash` perfectly interleaved, each
    /// re-issuing the identical call and getting the byte-identical failure
    /// payload. Both non-idempotent → both flagged. This is the loop every
    /// existing guard missed.
    #[test]
    fn interleaved_exec_loop_fires_for_both_tools() {
        let code_args = serde_json::json!({ "code": "python3 -m http.server 8080" });
        let bash_args = serde_json::json!({ "cmd": "python3 -m http.server 8080" });
        // Same failure payload from both tools (they share the exec path).
        let fail = serde_json::json!({
            "exit_code": -1,
            "stderr": "code_exec: no active session context",
        });
        let mut events = Vec::new();
        for i in 0..3 {
            let cc = format!("ce{i}");
            events.push(req(&cc, "code_exec", code_args.clone()));
            events.push(res(&cc, fail.clone()));
            let bc = format!("b{i}");
            events.push(req(&bc, "bash", bash_args.clone()));
            events.push(res(&bc, fail.clone()));
        }
        let s = render_redundant_call_notice(&events)
            .expect("interleaved identical exec loop must fire");
        assert!(s.starts_with("<system-reminder>"));
        assert!(s.contains("code_exec × 3 (same args, same result)"));
        assert!(s.contains("bash × 3 (same args, same result)"));
        assert!(s.contains("rule 13"));
        assert!(s.ends_with("</system-reminder>"));
    }

    #[test]
    fn below_threshold_is_silent() {
        let args = serde_json::json!({ "cmd": "ls" });
        let out = serde_json::json!("a\nb\n");
        let events = vec![
            req("c1", "bash", args.clone()),
            res("c1", out.clone()),
            req("c2", "bash", args.clone()),
            res("c2", out.clone()),
        ];
        assert!(
            render_redundant_call_notice(&events).is_none(),
            "2 identical calls must not fire (threshold 3)",
        );
    }

    /// Safety: a non-idempotent tool re-run with identical args but whose result
    /// *changes* (the `cargo test`-after-edit pattern) is progressing, not a
    /// loop → silent. This is why the detector gates on result identity.
    #[test]
    fn changing_result_is_not_flagged() {
        let args = serde_json::json!({ "cmd": "cargo test" });
        let events = vec![
            req("c1", "bash", args.clone()),
            res("c1", serde_json::json!("3 failed")),
            req("c2", "bash", args.clone()),
            res("c2", serde_json::json!("1 failed")),
            req("c3", "bash", args.clone()),
            res("c3", serde_json::json!("0 failed; ok")),
        ];
        assert!(
            render_redundant_call_notice(&events).is_none(),
            "a re-run whose output changes is progress, not redundancy",
        );
    }

    /// Idempotent read tools are `no_progress`'s territory — this detector skips
    /// them so the two notices never double-fire on the same key.
    #[test]
    fn idempotent_tools_are_skipped() {
        let args = serde_json::json!({ "query": "rust async" });
        let out = serde_json::json!({ "hits": ["a"] });
        let events = vec![
            req("c1", "search", args.clone()),
            res("c1", out.clone()),
            req("c2", "search", args.clone()),
            res("c2", out.clone()),
            req("c3", "search", args.clone()),
            res("c3", out.clone()),
        ];
        assert!(
            render_redundant_call_notice(&events).is_none(),
            "search is idempotent — handled by no_progress, skipped here",
        );
    }

    /// Distinct args are independent keys — writing 20 *different* files is
    /// progress, never a redundant loop, even though all calls are `file_write`.
    #[test]
    fn distinct_args_are_separate_keys() {
        let out = serde_json::json!({ "ok": true });
        let mut events = Vec::new();
        for i in 0..20 {
            let cid = format!("w{i}");
            events.push(req(
                &cid,
                "file_write",
                serde_json::json!({ "path": format!("f{i}.txt"), "content": "x" }),
            ));
            events.push(res(&cid, out.clone()));
        }
        assert!(
            render_redundant_call_notice(&events).is_none(),
            "20 distinct writes are progress, not a redundant loop",
        );
    }

    /// Same non-idempotent call, identical successful result ≥3× → flagged. A
    /// model that already has the result and keeps re-fetching it makes no
    /// progress regardless of whether the result is a success or a failure.
    #[test]
    fn identical_success_repeat_is_flagged() {
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
        let s = render_redundant_call_notice(&events).expect("3 identical bash calls fire");
        assert!(s.contains("bash × 3 (same args, same result)"));
    }

    /// Different argument key orderings canonicalize equal — still one group.
    #[test]
    fn canonical_args_collapse_key_order() {
        let out = serde_json::json!("stable");
        let events = vec![
            req("c1", "bash", serde_json::json!({ "a": 1, "b": 2 })),
            res("c1", out.clone()),
            req("c2", "bash", serde_json::json!({ "b": 2, "a": 1 })),
            res("c2", out.clone()),
            req("c3", "bash", serde_json::json!({ "a": 1, "b": 2 })),
            res("c3", out.clone()),
        ];
        let groups = aggregate_redundant_calls(&events);
        assert_eq!(groups.len(), 1, "reordered keys must collapse to one group");
        assert_eq!(groups[0].count, 3);
        assert_eq!(groups[0].tool, "bash");
    }

    /// An orphan result whose request fell out of the visible window is skipped,
    /// not counted under a bogus key.
    #[test]
    fn orphan_result_without_request_is_skipped() {
        let events = vec![res("ghost", serde_json::json!("x"))];
        assert!(aggregate_redundant_calls(&events).is_empty());
    }

    #[test]
    fn render_stays_compact() {
        let mut events = Vec::new();
        for i in 0..30 {
            let cid = format!("c{i}");
            let tool = if i % 2 == 0 { "code_exec" } else { "bash" };
            let key = if i % 2 == 0 { "code" } else { "cmd" };
            events.push(req(&cid, tool, serde_json::json!({ key: "loop" })));
            events.push(res(&cid, serde_json::json!("same")));
        }
        let s = render_redundant_call_notice(&events).expect("looping tools render");
        assert!(s.len() < 2048, "notice length {}", s.len());
    }

    #[test]
    fn threshold_constant_is_three() {
        // Guard: keep in lockstep with no_progress::NO_PROGRESS_THRESHOLD and
        // attempt_summary::SUMMARY_THRESHOLD so the harness nudges trip on the
        // same cadence.
        assert_eq!(REDUNDANT_CALL_THRESHOLD, 3);
    }
}
