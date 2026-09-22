//! Per-phase run tallies used by the `status` rendering in
//! [`summarize_phases`](crate::builtin_tools::workflow_tool::summarize_phases).
//!
//! This is an internal accumulator — never exposed on the wire. Kept separate
//! from the wire DTOs ([`super::dto`]) because nothing outside this file
//! should construct or inspect it.

/// Per-phase tallies for one run. `done + failed + skipped` need not equal
/// `settled`: a status can be settled without being any of the three (there
/// is no such status today, and writing the marker off `settled` rather than
/// off a sum keeps that true tomorrow).
#[derive(Default, Clone, Copy)]
pub(crate) struct PhaseTally {
    pub(crate) done: usize,
    pub(crate) failed: usize,
    pub(crate) skipped: usize,
    pub(crate) settled: usize,
    pub(crate) total: usize,
}

impl PhaseTally {
    /// One character for the whole phase: has it stopped, and if so, how.
    ///
    /// The three predicates are deliberately distinct: "produced a result"
    /// (`Completed`), "stopped badly" (`Failed`/`Cancelled`/`Unsatisfiable`),
    /// and "stopped at all" (`CoordTaskStatus::is_settled`, whose own doc
    /// names it the honest completion predicate for a workflow run). An
    /// earlier draft of this had only the first two and inferred the third
    /// from `done == total`, which renders a phase whose step was
    /// deliberately `Skipped` as `0/1 ▶` — running, forever, when it had in
    /// fact finished. That is the same "stopped is not succeeded" mistake as
    /// counting a cancelled step as done, pointed the other way.
    /// `✗` is reserved for a phase that has actually STOPPED: a failure inside
    /// a phase whose other steps are still executing used to short-circuit the
    /// settled check, so a four-step phase with one failure and three runs in
    /// flight rendered `Analyze 0/4 ✗` — "stopped badly" about work that had
    /// not stopped. The two axes are now read in order: has it stopped, and
    /// then did anything fail. The failure stays visible while running via
    /// [`Self::failed_note`], not by borrowing the terminal marker.
    pub(crate) const fn marker(self) -> &'static str {
        if self.settled != self.total {
            "▶"
        } else if self.failed > 0 {
            "✗"
        } else {
            "✓"
        }
    }

    /// The failure count, spelled out while the phase is still running — the
    /// marker cannot carry it there without lying about whether the phase has
    /// stopped. Empty once settled: `✗` already says it.
    pub(crate) fn failed_note(self) -> String {
        if self.failed > 0 && self.settled != self.total {
            format!(" ({} failed)", self.failed)
        } else {
            String::new()
        }
    }
}