//! Per-run gather-volume budget — renders a dynamic "you have gathered
//! enough, converge now" notice that the prompt builder injects ahead of
//! the next Think call.
//!
//! ## Why this exists
//!
//! [`attempt_summary`](crate::tools::attempt_summary) catches repeated
//! *failures* ("3 search calls 429'd → climb the ladder"). [`no_progress`]
//! catches byte-identical *idempotent* repeats. Neither sees the failure
//! mode that actually burns runs: a model that makes 150+ **successful**
//! `search`/`web_fetch` calls with slightly varying queries and a sentence
//! of narration between each, never converging on the deliverable until it
//! hits `max_iterations` and is force-summarised. Every structural guard is
//! keyed by exact `(tool, args)` identity or by error, so varied-query
//! successful over-gathering is invisible to all of them — bounded only by
//! the interactive `max_iterations` cap (1000).
//!
//! Guidelines #12 ("Converge") and the persistence-doctrine "Ceiling" clause
//! already say this in the static system prompt, but a constant prose rule gets tuned
//! out over a long run. This signal is *dynamic*: it surfaces the live call
//! count ("you have made 47 `search/web_fetch` calls this run"), which is far
//! harder to ignore than always-present prose — the same rationale that makes
//! [`attempt_summary`] a per-turn message rather than a [`PromptLayer`].
//!
//! Mirrors the industry pattern: opensquilla's `max_web_search_calls_per_turn`
//! budget + its pre-cap "provide the best concise final answer from the work
//! completed so far" injection; openclaw's escalating loop-detection warn/block.
//! Aleph keeps it a SOFT nudge — the model stays sovereign over whether to
//! converge (R7/R10); the harness never branches on this, it only renders text.
//!
//! ## What it is NOT
//!
//! - Not a hard cap / circuit breaker: it never blocks a call or aborts the
//!   run. The model can keep gathering if it genuinely must (deep research);
//!   the notice just makes the cost visible.
//! - Not a [`PromptLayer`](crate::thinker::prompt_layer::PromptLayer): computed
//!   fresh from `events[tail_start..]` every Think, never persisted.
//! - Not a completion judgment: it does not decide the task is done. It states
//!   a resource fact and a doctrine; the model decides (R7).
//!
//! ## Output shape
//!
//! ```text
//! <system-reminder>
//! Gather budget: you have made 47 successful search/web_fetch calls this run.
//! This is resource accounting, not a block — the tools still work, but 47 is
//! already well past what this task needs and more gathering is very unlikely
//! to surface new information. A specific datum you cannot retrieve (a precise
//! real-time figure, a suitable image) is a GAP to note, not a reason to keep
//! searching: stop gathering now, produce the deliverable with the data you
//! already have, and state any missing datum plainly in the output (rule 13).
//! Do not let one unobtainable sub-item block the whole task.
//! </system-reminder>
//! ```

use crate::session::events::{SessionEvent, SessionEventRecord};

/// Tools whose invocation counts as "gathering". Kept tight: only the two
/// external-information tools. Escalation tools (the `browser_*` family) are
/// deliberately excluded — the doctrine pushes the model *toward* a one-shot
/// class escalation (the persistence-doctrine "Ceiling" clause), so counting
/// them would punish the right move.
const GATHER_TOOLS: &[&str] = &["search", "web_fetch"];

/// Number of gather-tool calls in the visible window before the convergence
/// notice fires. Below this, a multi-source research task is still plausibly
/// productive and the notice would be noise; at/above it, the model is almost
/// certainly over-gathering.
///
/// Generous on purpose: a legitimate multi-source report needs maybe 5–8
/// searches; 12 leaves headroom while still firing long before the 1000-turn
/// interactive `max_iterations` cap that the runaway gather run hit at ~156.
pub const GATHER_BUDGET_THRESHOLD: usize = 12;

/// Count gather-tool calls in `events` that actually **returned data** — a
/// `ToolCallRequested` naming a [`GATHER_TOOLS`] entry which was later resolved
/// by a `ToolResult` rather than a `ToolError`.
///
/// Counting bare requests instead was a lie with teeth. The notice this feeds
/// tells the model "you have gathered plenty — stop and produce the deliverable
/// with the data you already have". After twelve rate-limited `search` calls the
/// model has no data at all, and the prompt carried the contradiction in two
/// adjacent `<system-reminder>` blocks: `attempt_summary` said "climb the
/// ladder", this said "stop gathering, you have enough". A wall of failures must
/// not trip a convergence nudge.
///
/// Requests still in flight (no terminal event yet) count as zero: they have not
/// produced anything to converge on.
#[must_use]
pub fn count_successful_gather_calls(events: &[SessionEventRecord]) -> usize {
    let mut pending: std::collections::HashSet<&str> = std::collections::HashSet::new();
    let mut succeeded = 0usize;
    for record in events {
        match &record.event {
            SessionEvent::ToolCallRequested { call_id, name, .. }
                if GATHER_TOOLS.contains(&name.as_str()) =>
            {
                pending.insert(call_id.as_str());
            }
            SessionEvent::ToolResult { call_id, .. } => {
                if pending.remove(call_id.as_str()) {
                    succeeded += 1;
                }
            }
            SessionEvent::ToolError { call_id, .. } => {
                pending.remove(call_id.as_str());
            }
            _ => {}
        }
    }
    succeeded
}

/// Render the convergence notice when gather calls ≥ [`GATHER_BUDGET_THRESHOLD`].
///
/// Returns `None` when no notice is warranted yet, so callers stay branch-free
/// (`if let Some(text) = render_gather_notice(...)`). Wrapped in
/// `<system-reminder>` to match Aleph's harness-injected channel.
#[must_use]
pub fn render_gather_notice(events: &[SessionEventRecord]) -> Option<String> {
    let n = count_successful_gather_calls(events);
    if n < GATHER_BUDGET_THRESHOLD {
        return None;
    }

    use std::fmt::Write;
    let mut out = String::with_capacity(640);
    out.push_str("<system-reminder>\n");
    let _ = write!(
        out,
        "Gather budget: you have made {n} successful search/web_fetch calls this run. "
    );
    out.push_str(
        "This is resource accounting, not a block — the tools still work, but this is \
         already well past what the task needs and more gathering is very unlikely to \
         surface new information. A specific datum you cannot retrieve (a precise \
         real-time figure, a suitable image) is a GAP to note, not a reason to keep \
         searching: stop gathering now, produce the deliverable with the data you \
         already have, and state any missing datum plainly in the output (rule 13). \
         Do not let one unobtainable sub-item block the whole task.\n",
    );
    out.push_str("</system-reminder>");
    Some(out)
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

    fn tres(call_id: &str) -> SessionEventRecord {
        mk(SessionEvent::ToolResult {
            turn_id: uuid::Uuid::nil(),
            call_id: call_id.to_string(),
            output: ToolOutput {
                value: serde_json::json!({"ok": true}),
                metadata: Default::default(),
            },
            at: now_ms(),
        })
    }

    fn terr(call_id: &str) -> SessionEventRecord {
        mk(SessionEvent::ToolError {
            turn_id: uuid::Uuid::nil(),
            call_id: call_id.to_string(),
            error: "429 rate limited".to_string(),
            at: now_ms(),
        })
    }

    /// Requests that each came back with data — what the notice claims to count.
    fn gather_events(n: usize, tool: &str) -> Vec<SessionEventRecord> {
        (0..n)
            .flat_map(|i| {
                let id = format!("{tool}-ok-{i}");
                [tcr(&id, tool), tres(&id)]
            })
            .collect()
    }

    /// Requests that each failed. Same volume, zero data.
    fn failed_gather_events(n: usize, tool: &str) -> Vec<SessionEventRecord> {
        (0..n)
            .flat_map(|i| {
                let id = format!("{tool}-err-{i}");
                [tcr(&id, tool), terr(&id)]
            })
            .collect()
    }

    #[test]
    fn below_threshold_returns_none() {
        let events = gather_events(GATHER_BUDGET_THRESHOLD - 1, "search");
        assert!(
            render_gather_notice(&events).is_none(),
            "{} calls must not fire (threshold {})",
            GATHER_BUDGET_THRESHOLD - 1,
            GATHER_BUDGET_THRESHOLD,
        );
    }

    #[test]
    fn at_threshold_emits_notice_with_live_count() {
        let events = gather_events(GATHER_BUDGET_THRESHOLD, "search");
        let s = render_gather_notice(&events).expect("threshold reached must render");
        assert!(s.starts_with("<system-reminder>"));
        assert!(s.ends_with("</system-reminder>"));
        // Dynamic value-add: the live count appears verbatim.
        assert!(s.contains(&format!("made {GATHER_BUDGET_THRESHOLD} ")));
        // Doctrine: converge + note-the-gap + don't-block-whole-task.
        assert!(s.contains("stop gathering now"));
        assert!(s.contains("GAP to note"));
        assert!(s.contains("block the whole task"));
        assert!(s.contains("rule 13"));
    }

    #[test]
    fn count_reflects_actual_n_in_message() {
        let events = gather_events(47, "web_fetch");
        let s = render_gather_notice(&events).expect("47 calls render");
        assert!(s.contains("made 47 "), "live count must be the real N");
    }

    #[test]
    fn counts_both_search_and_web_fetch() {
        let mut events = gather_events(7, "search");
        events.extend(gather_events(6, "web_fetch"));
        assert_eq!(count_successful_gather_calls(&events), 13);
        assert!(render_gather_notice(&events).is_some());
    }

    #[test]
    fn non_gather_tools_do_not_count() {
        // 20 file_write/file_read calls must never trip the gather budget —
        // producing the deliverable is the opposite of over-gathering.
        let mut events = gather_events(20, "file_write");
        events.extend(gather_events(20, "file_read"));
        assert_eq!(count_successful_gather_calls(&events), 0);
        assert!(render_gather_notice(&events).is_none());
    }

    #[test]
    fn a_wall_of_failures_does_not_fire_the_converge_nudge() {
        // The notice's whole content is "you have gathered plenty — stop and
        // produce the deliverable with the data you already have". Twelve
        // rate-limited searches produce no data, and `attempt_summary` is
        // simultaneously telling the model to climb the ladder. Firing here put
        // two contradictory `<system-reminder>` blocks in one prompt and pushed
        // the model to deliver on nothing.
        let events = failed_gather_events(GATHER_BUDGET_THRESHOLD * 2, "search");
        assert_eq!(count_successful_gather_calls(&events), 0);
        assert!(render_gather_notice(&events).is_none());
    }

    #[test]
    fn in_flight_requests_are_not_counted_as_gathered() {
        // A request with no terminal event yet has produced nothing to converge
        // on. Counting it would let one concurrent batch trip the notice.
        let events: Vec<_> = (0..GATHER_BUDGET_THRESHOLD)
            .map(|i| tcr(&format!("inflight-{i}"), "search"))
            .collect();
        assert_eq!(count_successful_gather_calls(&events), 0);
        assert!(render_gather_notice(&events).is_none());
    }

    #[test]
    fn a_mixed_run_counts_only_the_calls_that_returned_data() {
        let mut events = gather_events(GATHER_BUDGET_THRESHOLD, "search");
        events.extend(failed_gather_events(30, "web_fetch"));
        assert_eq!(
            count_successful_gather_calls(&events),
            GATHER_BUDGET_THRESHOLD
        );
        let s = render_gather_notice(&events).expect("the successful half reaches the threshold");
        assert!(s.contains(&format!("made {GATHER_BUDGET_THRESHOLD} ")));
    }

    #[test]
    fn escalation_tools_excluded() {
        // Browser tooling is the *right* one-shot escalation (doctrine
        // "Ceiling" clause); counting it would punish the move it pushes toward.
        let mut events = gather_events(20, "browser_open");
        events.extend(gather_events(20, "browser_snapshot"));
        assert_eq!(count_successful_gather_calls(&events), 0);
    }

    #[test]
    fn non_request_events_ignored() {
        let events = vec![
            mk(SessionEvent::TurnStarted {
                turn_id: uuid::Uuid::nil(),
                trigger: TurnTrigger::UserMessage,
                at: now_ms(),
            }),
            mk(SessionEvent::UserMessage {
                turn_id: uuid::Uuid::nil(),
                content: MessageContent {
                    text: "search the web a lot".into(),
                    blocks: vec![],
                    thinking: None,
                    thinking_signature: None,
                },
                at: now_ms(),
                synthetic: false,
            }),
            mk(SessionEvent::ToolResult {
                turn_id: uuid::Uuid::nil(),
                call_id: "c1".into(),
                output: ToolOutput {
                    value: serde_json::json!("search results here"),
                    metadata: Default::default(),
                },
                at: now_ms(),
            }),
        ];
        // Only ToolCallRequested counts — a ToolResult or a user message that
        // merely mentions "search" must not inflate the budget.
        assert_eq!(count_successful_gather_calls(&events), 0);
    }

    #[test]
    fn rendered_text_is_compact_under_2kb() {
        let events = gather_events(999, "search");
        let s = render_gather_notice(&events).expect("renders");
        assert!(s.len() < 2048, "notice length {}", s.len());
    }
}
