//! The fold ledger: lazy per-fold payback accounting (Context Fabric, spec
//! 2026-10-01 §line-1 1c).
//!
//! The question this module answers is "did *this* fold net-save tokens, or
//! did the summary cost more than the retired span will ever repay?" — a
//! different question from `core/cache-hit-rate`'s "what share of prompt
//! tokens came from the prefix cache", and deliberately kept separate from
//! it: the two numbers move independently (a 97% hit rate says nothing about
//! a fold whose summary is larger than the span it retired).
//!
//! Everything here is a pure function over already-loaded data. There is no
//! background sweep and nothing on the hot path: the accounting inputs are
//! persisted anyway (`FoldRecorded` token estimates; the `compactor:<agent>`
//! metering channel, FL §2.18), and verdicts are computed only when someone
//! asks — today that someone is the `core/fold-economics` doctor check.

use crate::context::compact::folds::FoldRecord;

/// Whether a fold has paid back what the summarizer charged for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Payback {
    /// Observed savings already cover the summarizer's cost.
    Breakeven,
    /// Per-turn saving is positive, but the turns observed so far have not
    /// yet repaid the summarizer.
    NotYet,
    /// The summary costs as much as or more than the span it retired, so the
    /// fold saves nothing (or goes backwards) on every turn and can never
    /// pay back.
    Never,
}

/// One fold's ledger line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FoldVerdict {
    /// The fold this verdict judges.
    pub fold_id: String,
    /// `turns × (folded − summary) − summarizer_cost`: the fold's net
    /// position within the observation window. Negative means the fold is
    /// still in debt — or, for [`Payback::Never`], sinking deeper every turn.
    pub net_saved_tokens: i64,
    /// The summarizer tokens attributed to this fold.
    pub summary_cost_tokens: u64,
    /// The verdict.
    pub payback: Payback,
}

/// Judge one fold against what was observed after it.
///
/// `turns_observed` is the number of post-fold turns actually measured; it is
/// clamped to `window_turns` (the observation horizon N — the doctor check
/// uses 10), so a fold judged long after the fact is held to the same
/// yardstick as a fresh one instead of being credited with turns nobody
/// measured.
#[must_use]
pub fn judge_fold(
    fold: &FoldRecord,
    turns_observed: u32,
    summarizer_cost_tokens: u64,
    window_turns: u32,
) -> FoldVerdict {
    let turns = i64::from(turns_observed.min(window_turns));
    let per_turn_saving = fold.folded_tokens as i64 - fold.summary_tokens as i64;
    let gross_saved = per_turn_saving * turns;
    let net_saved_tokens = gross_saved - summarizer_cost_tokens as i64;
    let payback = if per_turn_saving <= 0 {
        Payback::Never
    } else if gross_saved >= summarizer_cost_tokens as i64 {
        Payback::Breakeven
    } else {
        Payback::NotYet
    };
    FoldVerdict {
        fold_id: fold.fold_id.clone(),
        net_saved_tokens,
        summary_cost_tokens: summarizer_cost_tokens,
        payback,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::compact::folds::{FoldStrategy, FoldTrigger};

    fn fold(folded_tokens: u64, summary_tokens: u64) -> FoldRecord {
        FoldRecord {
            fold_id: "fold_test_2".to_string(),
            from_seq: 2,
            to_seq: 40,
            summary_ref: "41".to_string(),
            strategy: FoldStrategy::Manual,
            trigger: FoldTrigger::ManualCommand,
            folded_tokens,
            summary_tokens,
            at: 42,
        }
    }

    #[test]
    fn judge_fold_breakeven_when_reads_dominate() {
        // 11,200 saved per turn; two observed turns repay a 20,000-token
        // summarizer bill with margin.
        let verdict = judge_fold(&fold(12_000, 800), 2, 20_000, 10);
        assert_eq!(verdict.payback, Payback::Breakeven);
        assert_eq!(verdict.net_saved_tokens, 2_400);
        assert_eq!(verdict.summary_cost_tokens, 20_000);
    }

    #[test]
    fn judge_fold_never_when_summary_exceeds_savings() {
        // The summary is *larger* than the retired span: every turn saves a
        // negative amount, so no number of observed turns can ever repay
        // anything.
        let verdict = judge_fold(&fold(800, 900), 50, 0, 10);
        assert_eq!(verdict.payback, Payback::Never);
        assert!(verdict.net_saved_tokens < 0);
    }

    #[test]
    fn judge_fold_not_yet_before_savings_repay_cost() {
        // Positive per-turn saving, but one observed turn has not yet covered
        // the 20,000-token bill.
        let verdict = judge_fold(&fold(12_000, 800), 1, 20_000, 10);
        assert_eq!(verdict.payback, Payback::NotYet);
        assert_eq!(verdict.net_saved_tokens, -8_800);
    }

    #[test]
    fn judge_fold_caps_turns_at_the_observation_window() {
        // 100 turns nominally observed, but the window is 10: the verdict is
        // computed over 10 turns, not 100.
        let verdict = judge_fold(&fold(12_000, 800), 100, 200_000, 10);
        assert_eq!(verdict.payback, Payback::NotYet);
        assert_eq!(verdict.net_saved_tokens, 10 * 11_200 - 200_000);
    }

    #[test]
    fn zero_cost_fold_is_breakeven_immediately() {
        // A free fold (session-memory reuse) with positive saving owes
        // nothing.
        let verdict = judge_fold(&fold(12_000, 800), 0, 0, 10);
        assert_eq!(verdict.payback, Payback::Breakeven);
        assert_eq!(verdict.net_saved_tokens, 0);
    }
}
