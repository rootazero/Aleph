//! Per-run workflow budget — a small, dependency-free atomic counter for the
//! workflow layer's own cost tracking.
//!
//! This module is **deliberately separate** from `teams::dispatcher::*`'s
//! budget machinery. The dispatcher already enforces a global, per-task
//! runtime budget (CPU seconds, wall-clock timeout, retry ceiling). What it
//! does NOT enforce is a per-RUN budget that adds across the steps — and the
//! workflow layer is the only layer that knows the run is the unit. A
//! 5-step run with 30-second steps could collectively exceed the
//! dispatcher's per-task budget without any single step looking expensive.
//!
//! ## Contract with `materialize`
//!
//! `WorkflowRunBudget` is **observation-only at the materialiser boundary**.
//! `materialize` does not consult it — the budget is constructed by the
//! run-start path (`workflow(action='run')`) and charged from the dispatcher's
//! settle hook (`notify_settled_workflow_runs`) as each step's true cost
//! becomes known. Wiring that the materialiser would reach into the budget
//! would couple compile time to a count field that changes after the run has
//! started, the same anti-pattern the byte-identical metadata policy exists
//! to prevent.
//!
//! ## Atomicity
//!
//! All operations use `Ordering::Relaxed`. The counter is per-run and
//! single-writer from the runtime's point of view (one settle hook per run
//! at a time), so no cross-thread ordering is required. `Relaxed` is the
//! cheapest ordering that gives monotonicity on a single counter and is the
//! default the dispatcher already uses for its own counters.

use std::sync::atomic::{AtomicU64, Ordering};

/// Per-run budget for a workflow execution.
///
/// `None` total means **unlimited** — every charge succeeds. `total = Some(0)`
/// means "the run is already over budget on construction"; `remaining()` then
/// returns `0` and every charge fails with `Exhausted`. The two cases are
/// intentionally distinct because "no cap was set" and "the cap is zero"
/// answer different questions for the user.
#[derive(Debug)]
pub struct WorkflowRunBudget {
    /// `None` = uncapped. `Some(n)` = at most `n` units may be charged
    /// across the whole run (across all its steps, summed by the caller).
    total: Option<u64>,
    /// Monotonically increasing tally of units consumed so far. Read via
    /// `snapshot()`, modified only through `try_charged`.
    spent: AtomicU64,
}

impl WorkflowRunBudget {
    /// An uncapped budget — every charge succeeds, no cap to consult. The
    /// shape every run had before per-run budgeting existed.
    #[must_use]
    pub fn uncapped() -> Self {
        Self {
            total: None,
            spent: AtomicU64::new(0),
        }
    }

    /// A budget with `total` units available. `total = 0` constructs a
    /// born-exhausted budget: `remaining()` is `0` and `try_charged(_)`
    /// always returns `Exhausted`. This is intentional — it lets the caller
    /// model "the run was over budget before it started" without a special
    /// enum case.
    #[must_use]
    pub fn capped(total: u64) -> Self {
        Self {
            total: Some(total),
            spent: AtomicU64::new(0),
        }
    }

    /// Units still available before the next charge would fail.
    ///
    /// Uncapped (`total = None`) → `u64::MAX` (saturating). A capped budget
    /// that has been overcharged returns `0`, not a negative number — the
    /// accounting layer can drive the counter above `total` only by
    /// concurrently in-flight charges (atomic loads can lag a writer by a
    /// few cycles), and "0 left" is the correct answer for "the next charge
    /// will be refused" regardless of which side of the boundary the lag
    /// lands on.
    #[must_use]
    pub fn remaining(&self) -> u64 {
        match self.total {
            None => u64::MAX,
            Some(cap) => cap.saturating_sub(self.spent.load(Ordering::Relaxed)),
        }
    }

    /// The full accounting state — `(cap, spent, remaining)`. `cap = None`
    /// means uncapped, so `remaining` is `u64::MAX` by the same saturating
    /// rule [`remaining`](Self::remaining) uses.
    #[must_use]
    pub fn snapshot(&self) -> BudgetSnapshot {
        let spent = self.spent.load(Ordering::Relaxed);
        let remaining = match self.total {
            None => u64::MAX,
            Some(cap) => cap.saturating_sub(spent),
        };
        BudgetSnapshot {
            total: self.total,
            spent,
            remaining,
        }
    }

    /// Try to charge `cost` units against the budget. Returns:
    /// - [`BudgetOutcome::Ok`] with the `remaining` balance AFTER the charge
    ///   if the charge fits (a `cost = 0` charge is always OK);
    /// - [`BudgetOutcome::Exhausted`] with the `remaining` balance — the
    ///   headroom that WAS available at the moment of refusal — if the
    ///   charge would push the run past the cap. The counter is NOT modified
    ///   on `Exhausted` — the over-budget attempt is silent on the tally, so a
    ///   follow-up `try_charged(remaining)` can still succeed if the caller
    ///   backs off.
    ///
    /// `cost == 0` is legal and always succeeds; a `cost` larger than the cap
    /// is a refusal with `remaining = headroom` (the same headroom any
    /// temporarily-impossible charge would report). The caller can detect
    /// the impossible case via `cost > out.remaining()`, which is cheaper
    /// than the budget layer branching on a third state.
    pub fn try_charged(&self, cost: u64) -> BudgetOutcome {
        // Uncapped → always OK; spend the tally regardless so a snapshot still
        // answers "how much has the run cost so far?".
        let Some(cap) = self.total else {
            self.spent.fetch_add(cost, Ordering::Relaxed);
            return BudgetOutcome::Ok {
                remaining: u64::MAX,
            };
        };
        loop {
            let current = self.spent.load(Ordering::Relaxed);
            let headroom = cap.saturating_sub(current);
            if cost > headroom {
                return BudgetOutcome::Exhausted { remaining: headroom };
            }
            // CAS: another thread may have raced us between the load and the
            // store. The relaxed ordering is sufficient — the counter is
            // monotonic, and a lost CAS just retries against a fresher value.
            if self
                .spent
                .compare_exchange(current, current + cost, Ordering::Relaxed, Ordering::Relaxed)
                .is_ok()
            {
                return BudgetOutcome::Ok {
                    remaining: cap - (current + cost),
                };
            }
        }
    }
}

/// Outcome of a single charge against a [`WorkflowRunBudget`].
///
/// `Ok { remaining }` carries the new balance so the caller does not need a
/// second atomic load to learn what happened (and the race window that load
/// would re-open is the one the budget exists to close).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BudgetOutcome {
    /// The charge succeeded; `remaining` is the balance AFTER the charge.
    Ok { remaining: u64 },
    /// The charge was refused; the counter was NOT modified. `remaining` is
    /// the headroom that WAS available at the moment of refusal (0 when the
    /// charge is bigger than the cap itself).
    Exhausted { remaining: u64 },
}

impl BudgetOutcome {
    /// True when the charge went through. The inverse of `is_exhausted()`.
    #[must_use]
    pub const fn is_ok(&self) -> bool {
        matches!(self, Self::Ok { .. })
    }

    /// True when the charge was refused. The inverse of `is_ok()`.
    #[must_use]
    pub const fn is_exhausted(&self) -> bool {
        matches!(self, Self::Exhausted { .. })
    }

    /// The remaining balance carried by this outcome. For `Ok` it is the
    /// post-charge balance; for `Exhausted` it is the pre-refusal headroom.
    #[must_use]
    pub const fn remaining(&self) -> u64 {
        match self {
            Self::Ok { remaining } | Self::Exhausted { remaining } => *remaining,
        }
    }
}

/// Snapshot of a [`WorkflowRunBudget`]'s accounting — what `snapshot()`
/// returns. Cheap to copy; intended for logging and projection only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BudgetSnapshot {
    /// The cap, or `None` for an uncapped budget.
    pub total: Option<u64>,
    /// Units charged so far. Monotonic; can briefly exceed `total` only
    /// through in-flight charges the snapshot was taken between.
    pub spent: u64,
    /// `total.saturating_sub(spent)` for a capped budget, `u64::MAX` for an
    /// uncapped one — exactly what [`WorkflowRunBudget::remaining`] returns.
    pub remaining: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uncapped_charge_always_succeeds() {
        // A budget with no cap exists so the layer can be wired before the
        // user decides on a cap. Every charge must succeed; `remaining` reads
        // as `u64::MAX` to mean "no headroom to compute".
        let b = WorkflowRunBudget::uncapped();
        assert_eq!(b.remaining(), u64::MAX);
        let out = b.try_charged(100);
        assert!(out.is_ok());
        assert_eq!(out.remaining(), u64::MAX);
        let snap = b.snapshot();
        assert_eq!(snap.total, None);
        assert_eq!(snap.spent, 100);
        assert_eq!(snap.remaining, u64::MAX);
    }

    #[test]
    fn normal_charge_decrements_remaining() {
        let b = WorkflowRunBudget::capped(100);
        assert_eq!(b.remaining(), 100);

        let out = b.try_charged(30);
        assert_eq!(out, BudgetOutcome::Ok { remaining: 70 });
        assert_eq!(b.remaining(), 70);

        let out = b.try_charged(50);
        assert_eq!(out, BudgetOutcome::Ok { remaining: 20 });
        assert_eq!(b.remaining(), 20);

        let snap = b.snapshot();
        assert_eq!(snap.total, Some(100));
        assert_eq!(snap.spent, 80);
        assert_eq!(snap.remaining, 20);
    }

    #[test]
    fn partial_charge_at_boundary_succeeds() {
        // Charging exactly the remaining headroom is legal and leaves 0.
        let b = WorkflowRunBudget::capped(10);
        let out = b.try_charged(7);
        assert_eq!(out, BudgetOutcome::Ok { remaining: 3 });
        let out = b.try_charged(3);
        assert_eq!(out, BudgetOutcome::Ok { remaining: 0 });
        // Spent is now exactly the cap.
        let snap = b.snapshot();
        assert_eq!(snap.spent, 10);
        assert_eq!(snap.total, Some(10));
        assert_eq!(snap.remaining, 0);
    }

    #[test]
    fn overbudget_charge_is_refused_and_does_not_modify_state() {
        // A charge that would push past the cap is refused, the counter is
        // NOT moved, and a follow-up charge that fits the leftover succeeds.
        // The refuse-then-shrink pattern is the failure mode the budget
        // exists to let a caller recover from: a step that tried to spend
        // 100 of a 50-unit budget must not block a 30-unit step that comes
        // after it.
        let b = WorkflowRunBudget::capped(50);
        let out = b.try_charged(20);
        assert_eq!(out, BudgetOutcome::Ok { remaining: 30 });

        let out = b.try_charged(100);
        assert!(out.is_exhausted());
        assert_eq!(out.remaining(), 30, "headroom is reported, not 0");
        // The counter is untouched on a refusal — so the next, smaller
        // charge still fits and succeeds.
        assert_eq!(b.spent.load(Ordering::Relaxed), 20);
        let out = b.try_charged(30);
        assert_eq!(out, BudgetOutcome::Ok { remaining: 0 });
    }

    #[test]
    fn zero_cost_charge_is_a_no_op() {
        // A zero-cost charge is legal — `cost = 0` is the shape a "tick"
        // uses to poll the budget without spending anything. It must not
        // spuriously exhaust.
        let b = WorkflowRunBudget::capped(0);
        let out = b.try_charged(0);
        assert!(out.is_ok());
        assert_eq!(out.remaining(), 0);
        let snap = b.snapshot();
        assert_eq!(snap.spent, 0);

        // And against a finite cap it does not consume the cap.
        let b = WorkflowRunBudget::capped(10);
        assert_eq!(b.try_charged(0), BudgetOutcome::Ok { remaining: 10 });
        assert_eq!(b.remaining(), 10);
    }

    #[test]
    fn zero_cap_is_born_exhausted() {
        // `capped(0)` is "the run is already over budget on construction".
        // It must refuse every positive charge and report zero headroom —
        // distinct from `uncapped()`, which never refuses.
        let b = WorkflowRunBudget::capped(0);
        assert_eq!(b.remaining(), 0);
        let out = b.try_charged(1);
        assert!(out.is_exhausted());
        assert_eq!(out.remaining(), 0);
        let snap = b.snapshot();
        assert_eq!(snap.total, Some(0));
        assert_eq!(snap.spent, 0);
        assert_eq!(snap.remaining, 0);
    }

    #[test]
    fn uncapped_zero_cap_zero_spent_are_distinct() {
        // The three constructors encode three different realities and must
        // not collapse into one. A confused caller who cannot tell "no cap
        // was set" from "the cap was set to zero" has no way to choose the
        // right user-facing message. `capped(0)` and a fully-spent budget
        // both report `remaining = 0`, but they answer different questions
        // (refuse on construction vs. refuse mid-run), so the snapshot
        // distinguishes them via `total` / `spent` rather than via `remaining`.
        let uncapped = WorkflowRunBudget::uncapped();
        let zero_cap = WorkflowRunBudget::capped(0);
        let spent: WorkflowRunBudget =
            WorkflowRunBudget { total: Some(5), spent: AtomicU64::new(5) };
        let uncapped_snap = uncapped.snapshot();
        let zero_cap_snap = zero_cap.snapshot();
        let spent_snap = spent.snapshot();
        assert_eq!(uncapped_snap.total, None, "uncapped has no cap");
        assert_eq!(zero_cap_snap.total, Some(0), "zero cap is a cap of zero");
        assert_eq!(spent_snap.total, Some(5), "fully-spent keeps the original cap");
        assert_ne!(uncapped_snap.spent, spent_snap.spent,
            "uncapped spent differs from a fully-spent capped budget");
        assert_eq!(uncapped_snap.remaining, u64::MAX, "uncapped is u64::MAX");
        assert_eq!(zero_cap_snap.remaining, 0, "zero cap is 0");
        assert_eq!(spent_snap.remaining, 0, "fully spent is 0");
    }

    #[test]
    fn charge_larger_than_cap_refuses_with_headroom() {
        // A single charge bigger than the whole cap must refuse — and the
        // headroom it reports is the same headroom any temporarily-
        // impossible charge would report, so the caller can compare
        // `cost > remaining` to detect "never possible" without a third
        // budget state.
        let b = WorkflowRunBudget::capped(10);
        let out = b.try_charged(11);
        assert!(out.is_exhausted());
        // headroom = cap - spent = 10 - 0 = 10 (cost > cap, but the layer
        // reports what was available — the caller checks `cost > remaining`).
        assert_eq!(out.remaining(), 10);
        assert_eq!(b.spent.load(Ordering::Relaxed), 0, "refusal does not move");
        // An impossible cost (> cap) AND a temporarily-impossible cost
        // (> headroom but <= cap) report the same headroom shape.
        let b2 = WorkflowRunBudget::capped(10);
        let _ = b2.try_charged(5); // spent = 5, headroom = 5
        let out2 = b2.try_charged(11); // cost(11) > cap(10), refuses
        assert!(out2.is_exhausted());
        assert_eq!(out2.remaining(), 5, "headroom unchanged: cap(10) - spent(5)");
        let out3 = b2.try_charged(6); // cost(6) > headroom(5) but <= cap(10)
        assert!(out3.is_exhausted());
        assert_eq!(out3.remaining(), 5, "same shape, same headroom");
    }

    #[test]
    fn budget_outcome_accessors_round_trip() {
        // The accessor trio on `BudgetOutcome` is the surface every caller
        // exercises; assert them by name so a rename / signature change is
        // a compile-visible edit here too.
        let ok = BudgetOutcome::Ok { remaining: 7 };
        assert!(ok.is_ok());
        assert!(!ok.is_exhausted());
        assert_eq!(ok.remaining(), 7);

        let ex = BudgetOutcome::Exhausted { remaining: 3 };
        assert!(!ex.is_ok());
        assert!(ex.is_exhausted());
        assert_eq!(ex.remaining(), 3);
    }

    #[test]
    fn concurrent_charges_respect_the_cap() {
        // The counter is monotonic and the CAS loop in `try_charged` is the
        // only thing standing between two threads and a double-spend. Stress
        // it: 100 threads × 100 charges of 1 against a cap of 1_000 must
        // yield exactly 1_000 successes and 9_000 refusals, with the spent
        // tally landing on exactly 1_000.
        use std::sync::Arc;
        use std::thread;

        let budget = Arc::new(WorkflowRunBudget::capped(1_000));
        let mut handles = Vec::new();
        for _ in 0..100 {
            let b = Arc::clone(&budget);
            handles.push(thread::spawn(move || {
                let mut oks = 0usize;
                let mut refused = 0usize;
                for _ in 0..100 {
                    match b.try_charged(1) {
                        BudgetOutcome::Ok { .. } => oks += 1,
                        BudgetOutcome::Exhausted { .. } => refused += 1,
                    }
                }
                (oks, refused)
            }));
        }
        let (total_ok, total_refused): (usize, usize) =
            handles.into_iter().map(|h| h.join().unwrap()).fold((0, 0), |a, b| (a.0 + b.0, a.1 + b.1));
        assert_eq!(total_ok, 1_000, "exactly the cap may be charged");
        assert_eq!(total_refused, 9_000, "every other charge is refused");
        assert_eq!(
            budget.spent.load(Ordering::Relaxed),
            1_000,
            "the spent tally lands exactly on the cap"
        );
    }
}