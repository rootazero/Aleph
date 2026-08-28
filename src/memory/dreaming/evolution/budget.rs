//! Edit budget — SkillOpt's "textual learning rate" for one dream cycle.
//!
//! Bounds how much memory a single cycle may *rewrite*, so a runaway pipeline
//! can't churn the whole corpus in one night. The budget is shared across all
//! **destructive** stages — the ones that overwrite or remove existing
//! knowledge: `NoteConsolidate` (merges), `NoteDecay` (archival), and the
//! distill `Supersede` action (`SkillDistill` / `FeedbackDistill`). Each calls
//! [`EditBudget::try_spend`] before an edit and stops when it is exhausted.
//! **Additive** growth (new synthesis notes, distill `New` / `Strengthen`, weave
//! links) is deliberately *not* budgeted, so the growth path is never starved
//! by earlier destructive stages within the same cycle.

use serde::{Deserialize, Serialize};

/// Per-cycle edit allowance. Replaces ad-hoc per-stage `cap` counters with a
/// single budget threaded through the pipeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct EditBudget {
    /// Remaining number of destructive edits (merges, synthesis writes, …).
    pub edits_remaining: u32,
    /// Remaining bytes that may be rewritten this cycle.
    pub bytes_remaining: u64,
}

impl EditBudget {
    /// Default budget: conservative enough to keep nightly churn bounded.
    pub const DEFAULT_EDITS: u32 = 32;
    pub const DEFAULT_BYTES: u64 = 256 * 1024;

    /// Minimum bytes per edit. A 1-byte supersede (or any other edit too
    /// small to be a real destructive act) is silently rejected so the
    /// budget cannot fund degenerate churn: with `bytes_remaining = 1` and
    /// `edits_remaining = 1` the budget would otherwise admit a no-op
    /// edit, the next cycle would re-distill, and the cycle-timeout shape
    /// would never converge on a stable state. Picked at 64 because the
    /// smallest meaningful destructive edit (a one-line note rewrite) is
    /// well over that floor and a degenerate 1–63 byte edit is never
    /// meaningful.
    pub const MIN_EDIT_BYTES: u64 = 64;

    #[must_use]
    pub const fn new(edits: u32, bytes: u64) -> Self {
        Self {
            edits_remaining: edits,
            bytes_remaining: bytes,
        }
    }

    /// Attempt to spend one edit of `bytes`. Returns `true` and decrements when
    /// the budget covers it; returns `false` (and leaves the budget untouched)
    /// when exhausted, signalling the caller to stop editing this cycle.
    ///
    /// The byte spend is clamped to at least [`Self::MIN_EDIT_BYTES`] so a
    /// near-exhausted budget cannot fund a degenerate 1-byte edit.
    pub fn try_spend(&mut self, bytes: u64) -> bool {
        let spend = bytes.max(Self::MIN_EDIT_BYTES);
        if self.edits_remaining == 0 || self.bytes_remaining < spend {
            return false;
        }
        self.edits_remaining -= 1;
        self.bytes_remaining -= bytes;
        true
    }

    /// Whether any edit budget remains.
    #[must_use]
    pub const fn is_exhausted(&self) -> bool {
        self.edits_remaining == 0 || self.bytes_remaining == 0
    }
}

impl Default for EditBudget {
    fn default() -> Self {
        Self::new(Self::DEFAULT_EDITS, Self::DEFAULT_BYTES)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spends_until_edit_count_exhausted() {
        let mut b = EditBudget::new(2, 10_000);
        assert!(b.try_spend(100));
        assert!(b.try_spend(100));
        assert!(!b.try_spend(100), "third edit must be refused");
        assert!(b.is_exhausted());
    }

    #[test]
    fn refuses_oversized_edit_without_spending() {
        let mut b = EditBudget::new(5, 500);
        assert!(!b.try_spend(1000), "edit larger than byte budget refused");
        assert_eq!(b.edits_remaining, 5, "refused edit must not consume budget");
        assert_eq!(b.bytes_remaining, 500);
    }

    #[test]
    fn byte_budget_can_exhaust_before_edit_count() {
        let mut b = EditBudget::new(10, 250);
        assert!(b.try_spend(200));
        assert!(!b.try_spend(200), "only 50 bytes left");
        assert_eq!(b.edits_remaining, 9);
    }
}
