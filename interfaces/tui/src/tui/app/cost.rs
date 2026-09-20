//! What this conversation has cost, and how much of that this screen can
//! actually vouch for.
//!
//! Two carriers, and they overlap in time:
//!
//! * `SessionSnapshot.estimated_cost_usd` — the `sessions` row as the server
//!   priced it, correct as of the attach that delivered it.
//! * `RunSummary.estimated_cost_usd` — one run, delivered at `RunComplete`.
//!   `Option`, and `None` means *the pricing module had no rate for this
//!   provider/model*, not zero.
//!
//! So the tally is a base plus the runs observed since that base, and a
//! re-attach REBASES rather than adds — the snapshot it brings already
//! contains the runs this screen watched. That is the whole reason this is a
//! type and not two `f64` fields next to each other: the reset and the base
//! have to move together, and a caller that remembers one of them is a caller
//! who will eventually double-count a conversation's spend.
//!
//! `None` is never spent as `0` (判据 §8). An unpriced run does not vanish
//! into the total; it turns the total into a floor.

/// What the status line is allowed to claim.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CostView {
    /// Nothing priced has ever been reported for this conversation — no
    /// snapshot yet, or every contribution came back unpriced. Renders `$?`.
    Unknown,
    /// Every contribution carried a price.
    Exact(f64),
    /// A floor: this much is known, and at least one run was unpriced.
    AtLeast(f64),
}

impl CostView {
    /// The status-line text. `$?` for unknown — never `$0.000`, which reads
    /// as "this was free" and is the one wrong answer that looks like a fact.
    #[must_use]
    pub fn label(self) -> String {
        match self {
            Self::Unknown => "$?".to_string(),
            Self::Exact(v) => format!("${v:.3}"),
            Self::AtLeast(v) => format!("${v:.3}+?"),
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct CostTally {
    /// The server's number at the last attach. `None` = never told.
    base_usd: Option<f64>,
    /// Runs this screen has watched complete since that attach.
    runs_usd: f64,
    /// At least one of those runs reported no price.
    unpriced_run: bool,
}

impl CostTally {
    /// A snapshot arrived: it already accounts for every run before it, so
    /// the per-run accumulator starts over.
    pub fn rebase(&mut self, base_usd: Option<f64>) {
        self.base_usd = base_usd;
        self.runs_usd = 0.0;
        self.unpriced_run = false;
    }

    /// One run finished. `None` is recorded as "unknown", not as nothing.
    pub fn add_run(&mut self, cost_usd: Option<f64>) {
        match cost_usd {
            // A negative or non-finite number is not a price. Treat it the
            // way an absent one is treated rather than letting it walk into
            // the sum.
            Some(v) if v.is_finite() && v >= 0.0 => self.runs_usd += v,
            _ => self.unpriced_run = true,
        }
    }

    #[must_use]
    pub fn view(self) -> CostView {
        let known = self.base_usd.map_or(self.runs_usd, |b| b + self.runs_usd);
        if self.base_usd.is_none() && self.runs_usd == 0.0 {
            // Nothing priced has landed at all. Whether an unpriced run went
            // by or not, the honest answer is the same one.
            return CostView::Unknown;
        }
        if self.unpriced_run {
            CostView::AtLeast(known)
        } else {
            CostView::Exact(known)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_reported_is_unknown_not_free() {
        assert_eq!(CostTally::default().view(), CostView::Unknown);
        assert_eq!(CostView::Unknown.label(), "$?");
    }

    /// The headline arithmetic: attach, then watch two runs.
    #[test]
    fn the_total_is_the_snapshot_plus_the_runs_since_it() {
        let mut t = CostTally::default();
        t.rebase(Some(0.100));
        t.add_run(Some(0.020));
        t.add_run(Some(0.002));
        // Compared through the label: three decimals is what a reader sees,
        // and `0.1 + 0.02 + 0.002` is not bit-equal to `0.122`.
        assert!(matches!(t.view(), CostView::Exact(_)));
        assert_eq!(t.view().label(), "$0.122");
    }

    /// The reason this is one type: the second attach's snapshot already
    /// contains the run the first attach's screen watched, so adding instead
    /// of rebasing bills it twice.
    #[test]
    fn re_attaching_rebases_rather_than_doubles() {
        let mut t = CostTally::default();
        t.rebase(Some(0.100));
        t.add_run(Some(0.020));
        // The server now says 0.120 — the same 0.020, already inside.
        t.rebase(Some(0.120));
        assert_eq!(t.view(), CostView::Exact(0.120));
    }

    /// 判据 §8: an unpriced run may say "I don't know". It may not say "0".
    #[test]
    fn an_unpriced_run_turns_the_total_into_a_floor() {
        let mut t = CostTally::default();
        t.rebase(Some(0.100));
        t.add_run(None);
        assert_eq!(t.view(), CostView::AtLeast(0.100));
        assert_eq!(t.view().label(), "$0.100+?");

        // With nothing priced at all, a floor of zero is no information —
        // and `$0.000+?` reads like a price. Say unknown.
        let mut t = CostTally::default();
        t.add_run(None);
        assert_eq!(t.view(), CostView::Unknown);
    }

    /// A run that reports a price after an unpriced one does not erase the
    /// doubt: the total is still missing a term.
    #[test]
    fn doubt_survives_a_later_priced_run() {
        let mut t = CostTally::default();
        t.rebase(Some(0.0));
        t.add_run(None);
        t.add_run(Some(0.030));
        assert_eq!(t.view(), CostView::AtLeast(0.030));
    }

    /// Garbage off the wire is treated as absence, not summed.
    #[test]
    fn a_non_finite_or_negative_price_is_not_a_price() {
        for bad in [f64::NAN, f64::INFINITY, -1.0] {
            let mut t = CostTally::default();
            t.rebase(Some(0.050));
            t.add_run(Some(bad));
            assert_eq!(
                t.view(),
                CostView::AtLeast(0.050),
                "{bad} must not enter the sum"
            );
        }
    }

    /// A session with no history and no snapshot yet, then its first priced
    /// run: the base stays unknown but the run is real, so the total is the
    /// run. (`rebase(None)` is what a snapshot-less attach path leaves.)
    #[test]
    fn runs_count_even_before_a_snapshot_arrives() {
        let mut t = CostTally::default();
        t.add_run(Some(0.007));
        assert_eq!(t.view(), CostView::Exact(0.007));
    }
}
