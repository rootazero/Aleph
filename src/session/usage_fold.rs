//! Token totals folded from `AssistantMessage.usage` — the ONE derivation of
//! "what this session / run spent" (§5.5). `AssistantRunMeta` no longer carries
//! counters; it keeps only what a fold cannot give (run_id join, occupancy,
//! cost, model). Discarded-retry calls are not in the log and are not counted
//! here; the per-call `SpendLedger` is the other fact and is untouched.
//!
//! The `sessions` row's `input_tokens` / `output_tokens` columns are a
//! materialisation of this fold, accumulated one run at a time by the
//! projector when the run's meta lands, or when a whole-session heal
//! synthesizes the stamp of a finished run whose meta never did
//! (`session_projector::bill_run_from_fold`, its one biller for both) — the
//! row is a face, not a second derivation.
use crate::session::events::{SessionEvent, SessionEventRecord};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct UsageTotals {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_creation: u64,
    pub reasoning: u64,
    /// Assistant messages whose provider reported usage.
    pub with_usage: usize,
    /// Assistant messages with `usage: None` — absent, NOT zero. A consumer that
    /// shows a total with `without_usage > 0` is showing a floor, and must say so.
    pub without_usage: usize,
}

impl UsageTotals {
    fn fold(mut self, event: &SessionEvent) -> Self {
        if let SessionEvent::AssistantMessage { usage, .. } = event {
            match usage {
                Some(u) => {
                    self.input += u64::from(u.input);
                    self.output += u64::from(u.output);
                    self.cache_read += u64::from(u.cache_read);
                    self.cache_creation += u64::from(u.cache_creation);
                    self.reasoning += u64::from(u.reasoning);
                    self.with_usage += 1;
                }
                None => self.without_usage += 1,
            }
        }
        self
    }
}

/// Totals of every assistant message in `log`, in log order. The whole-session
/// fold; [`run_usage_totals`] is this same fold over one run's tail and, so
/// far, its only caller — widen the visibility when a whole-session reader
/// appears, not before.
#[must_use]
fn session_usage_totals(log: &[SessionEventRecord]) -> UsageTotals {
    log.iter()
        .fold(UsageTotals::default(), |acc, r| acc.fold(&r.event))
}

/// Totals of the run opened by the LAST `RunStarted` in `slice`. `None` when
/// the slice holds none: an unanchored fold would bill a whole session to one run.
#[must_use]
pub fn run_usage_totals(slice: &[SessionEventRecord]) -> Option<UsageTotals> {
    let start = slice
        .iter()
        .rposition(|r| matches!(r.event, SessionEvent::RunStarted { .. }))?;
    slice.get(start..).map(session_usage_totals)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orchestrator::dispatch::TokenBreakdown;
    use crate::session::events::{MessageContent, RunOutcome, SessionEvent, SessionEventRecord};
    fn rec(seq: u64, event: SessionEvent) -> SessionEventRecord {
        SessionEventRecord {
            seq,
            event,
            created_at_ms: 0,
        }
    }
    fn asst(usage: Option<TokenBreakdown>) -> SessionEvent {
        SessionEvent::AssistantMessage {
            turn_id: uuid::Uuid::new_v4(),
            content: MessageContent {
                text: "a".into(),
                blocks: vec![],
                thinking: None,
                thinking_signature: None,
            },
            usage,
            at: 0,
        }
    }
    fn tb(input: u32, output: u32) -> TokenBreakdown {
        TokenBreakdown {
            input,
            output,
            cache_read: 0,
            cache_creation: 0,
            reasoning: 0,
        }
    }
    fn rs(id: &str) -> SessionEvent {
        SessionEvent::RunStarted {
            run_id: id.into(),
            at: 0,
            project_root: None,
            envelope: None,
        }
    }
    fn rf(id: &str) -> SessionEvent {
        SessionEvent::RunFinished {
            run_id: id.into(),
            outcome: RunOutcome::Completed,
            at: 0,
        }
    }

    #[test]
    fn session_totals_sum_every_priced_message_and_count_the_unpriced() {
        let log = [
            rec(1, rs("a")),
            rec(2, asst(Some(tb(100, 10)))),
            rec(3, asst(None)),
            rec(4, rf("a")),
            rec(5, rs("b")),
            rec(6, asst(Some(tb(5, 1)))),
        ];
        let t = session_usage_totals(&log);
        assert_eq!(
            (t.input, t.output, t.with_usage, t.without_usage),
            (105, 11, 2, 1)
        );
    }

    #[test]
    fn run_totals_anchor_on_the_last_run_started() {
        let log = [
            rec(1, rs("a")),
            rec(2, asst(Some(tb(100, 10)))),
            rec(3, rf("a")),
            rec(4, rs("b")),
            rec(5, asst(Some(tb(5, 1)))),
        ];
        assert_eq!(
            run_usage_totals(&log).map(|t| (t.input, t.output)),
            Some((5, 1))
        );
        assert_eq!(
            run_usage_totals(&log[1..3]),
            None,
            "no RunStarted in the slice ⇒ refuse rather than bill the whole slice"
        );
    }

    /// The three cache / reasoning counters ride the same fold; a message that
    /// reports them is summed on every axis, not just input / output.
    #[test]
    fn every_axis_of_the_breakdown_is_summed() {
        let priced = TokenBreakdown {
            input: 1,
            output: 2,
            cache_read: 3,
            cache_creation: 4,
            reasoning: 5,
        };
        let log = [
            rec(1, rs("a")),
            rec(2, asst(Some(priced.clone()))),
            rec(3, asst(Some(priced))),
        ];
        let t = session_usage_totals(&log);
        assert_eq!(
            (t.cache_read, t.cache_creation, t.reasoning, t.with_usage),
            (6, 8, 10, 2)
        );
        assert_eq!(
            run_usage_totals(&log),
            Some(t),
            "one run ⇒ the run fold IS the session fold"
        );
    }
}
