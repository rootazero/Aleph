//! Memory evolution gate — SkillOpt-inspired discipline for the dream daemon.
//!
//! The note-layer dream pipeline mutates the corpus every cycle (merge,
//! synthesise, weave, distill). Historically those edits were applied blind:
//! an LLM proposed them, a static well-formedness gate ([`super::skill_gate`])
//! checked their *shape*, and they landed unconditionally. Nothing checked
//! whether an edit actually *improved* memory, and nothing bounded how much a
//! cycle could rewrite.
//!
//! This module ports SkillOpt's (arXiv 2605.23904) training discipline:
//!
//! * [`gate`] — a pure accept/reject decision that keeps an edit only when it
//!   strictly improves a score (`evaluate_gate`), tracking the best-ever score.
//! * [`score`] — the deterministic memory-health score the gate compares
//!   against, reusing the existing [`super::signals::SignalSnapshot`], plus a
//!   local quality score for individual merge candidates.
//! * [`budget`] — a per-cycle edit budget ("textual learning rate") that bounds
//!   nightly churn.
//!
//! Boundary with the sibling gates (R7 / no overlap):
//! * The daemon's window/idle/once-per-day checks (`super::DreamDaemon`) decide
//!   *whether a cycle runs at all* — a pre-condition, orthogonal to this module.
//! * [`super::mutation_gate::MutationGate`] detects *churn pathologies*
//!   (synthesis churn, repeated merges) and enforces cooldown — reactive.
//! * This module prevents *degrading* edits and provides monotonic-improvement
//!   pressure — proactive. The three compose; none subsumes another.

pub mod budget;
pub mod evidence;
pub mod gate;
pub mod score;

pub use budget::EditBudget;
pub use evidence::{action_fingerprint, gate_supersede_evidence, EvidenceDecision};
pub use gate::{evaluate_gate, GateOutcome};
pub use score::{memory_health_score, score_merge_candidate};

use serde::{Deserialize, Serialize};

/// Cycle-level evolution gate result, recorded on the `DreamReport` and surfaced
/// to the LLM via the `note_manage(action="evolution")` tool.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct EvolutionOutcome {
    /// Memory-health score before this cycle's edits.
    pub baseline: f64,
    /// Memory-health score after this cycle's edits.
    pub candidate: f64,
    /// Best-ever health score known at decision time.
    pub best: f64,
    /// Gate verdict comparing `candidate` to `baseline`/`best`.
    pub outcome: GateOutcome,
    /// Number of proposed merges the per-edit gate rejected this cycle.
    pub merges_rejected: u32,
}

impl EvolutionOutcome {
    /// Did this cycle make memory *worse*?
    ///
    /// A rejection alone is not a pathology — the gate also rejects ties and
    /// candidates that merely fail to beat the historical best. Only a
    /// rejection whose candidate scored below the cycle's own baseline means
    /// the night's edits actively degraded the corpus, which is what arms
    /// [`super::mutation_gate::MutationGate`]'s conserve cooldown. Single
    /// source: the daemon's operator warning and the gate's cooldown read the
    /// same predicate.
    #[must_use]
    pub fn degraded(&self) -> bool {
        self.outcome == GateOutcome::Reject && self.candidate < self.baseline
    }
}

/// Minimum local merge-safety score for a merge to be applied. Merges scoring
/// below this fuse too much distinct knowledge and are skipped by the gate.
pub const MERGE_ACCEPT_THRESHOLD: f64 = 0.35;

/// Default epsilon for the cycle-level health gate — the minimum health gain
/// that counts as improvement (guards against float noise / trivial deltas).
pub const HEALTH_GATE_EPSILON: f64 = 0.01;
