//! `MutationGate` — detects evolution pathologies and enforces cooldown.
//!
//! Four detections, all keyed on **identifiers** (note pairs, synthesis paths,
//! cohort counts) rather than on model prose — churn is a structural property,
//! and re-judging semantics with rules would violate R7:
//!
//! 1. Merge cycle: same note pair merged 3+ consecutive cycles
//! 2. Synthesis churn: same synthesis note rewritten to a *different* body in
//!    3+ consecutive cycles (the corpus flip-flopping on the same subject)
//! 3. Wasted distillation: mature skill-notes that never get recalled
//! 4. Degraded-health cooldown: a cycle whose evolution gate rejected a
//!    health-lowering candidate parks the loop in Conserve for two cycles
//!
//! ## State is derived, never accumulated
//!
//! The gate holds no cross-cycle state of its own. Every field below is folded
//! out of the persisted dream event log (`dream_events.jsonl`) at the start of
//! each cycle via [`MutationGate::from_reports`]. This mirrors evolver's
//! `analyzeRecentHistory(recentEvents)` — a pure function over durable events —
//! and satisfies the constitution's A3 (state reconstructible from a single
//! persistent source).
//!
//! The alternative — the in-process accumulator this type used to be — was
//! structurally unable to fire: dream cycles run at most once per day, so a
//! five-cycle detection window needed five consecutive days of daemon uptime
//! before it could *ever* trip, and the conserve cooldown armed by a
//! health-degrading cycle evaporated on the next restart. The identical history
//! was already on disk the whole time, unread.

use std::collections::{HashMap, HashSet, VecDeque};

// Re-use GateDecision from selector
pub use super::selector::GateDecision;

use super::report::DreamReport;

const MERGE_CYCLE_WINDOW: usize = 5;
const MERGE_CYCLE_THRESHOLD: usize = 3;
/// Consecutive cycles a synthesis note must be rewritten *with a different
/// body* before it counts as flip-flopping. A stable rewrite (same digest) is
/// not churn.
const SYNTHESIS_CHURN_THRESHOLD: usize = 3;
const DISTILL_WINDOW: usize = 5;
const DISTILL_MIN_RECALL_RATE: f64 = 0.1;
/// Cycles the loop stays in Conserve after the evolution gate rejected a
/// health-lowering cycle.
pub const DEGRADED_HEALTH_COOLDOWN_CYCLES: u32 = 2;

/// One cycle's churn-relevant footprint — the unit [`MutationGate`] folds over.
///
/// Derived from the persisted [`DreamReport`] so a rehydrated gate and a live
/// one see byte-identical input.
#[derive(Debug, Clone, Default)]
pub struct CycleFootprint {
    /// Note pairs merged this cycle, order-normalised.
    pub merges: HashSet<(String, String)>,
    /// Synthesis notes rewritten this cycle: path → digest of the *body*.
    pub synthesis_rewrites: HashMap<String, String>,
    /// Mature skill-note cohort `(size, recalled)`, or `None` when no mature
    /// cohort existed yet — feeding zeros would never arm the detector.
    pub distill: Option<(u32, u32)>,
    /// This cycle's evolution gate rejected a candidate that *lowered* memory
    /// health. Arms the conserve cooldown.
    pub degraded_health: bool,
}

impl CycleFootprint {
    /// Project a cycle report onto the churn signals the gate consumes.
    #[must_use]
    pub fn from_report(report: &DreamReport) -> Self {
        let merges = report
            .merged_pairs
            .iter()
            .map(|(a, b)| normalize_pair(a, b))
            .collect();
        let synthesis_rewrites = report
            .synthesis_rewrites
            .iter()
            // rust-doctor-disable-next-line excessive-clone
            .map(|(path, digest)| (path.clone(), digest.clone()))
            .collect();
        Self {
            merges,
            synthesis_rewrites,
            distill: (report.distill_produced > 0)
                .then_some((report.distill_produced, report.distill_recalled)),
            degraded_health: report
                .evolution
                .as_ref()
                .is_some_and(super::evolution::EvolutionOutcome::degraded),
        }
    }
}

fn normalize_pair(a: &str, b: &str) -> (String, String) {
    if a < b {
        (a.to_string(), b.to_string())
    } else {
        (b.to_string(), a.to_string())
    }
}

/// Tracks evolution pathology state across Dream cycles.
#[derive(Debug, Default)]
pub struct MutationGate {
    merge_history: VecDeque<HashSet<(String, String)>>,
    synthesis_history: VecDeque<HashMap<String, String>>,
    distill_history: VecDeque<(u32, u32)>,
    cooldown: u32,
}

impl MutationGate {
    /// How many past cycles [`Self::from_reports`] needs to see. The daemon
    /// reads exactly this many events, so the window can never drift out of
    /// sync with the detectors' thresholds.
    pub const HISTORY_WINDOW: usize = MERGE_CYCLE_WINDOW;

    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Rebuild the gate by folding past cycle reports **oldest first**.
    ///
    /// `reports` may be shorter than [`Self::HISTORY_WINDOW`] (fresh install,
    /// truncated log) — the detectors simply stay disarmed until enough history
    /// accumulates, exactly as they did on a cold process.
    pub fn from_reports<'a, I>(reports: I) -> Self
    where
        I: IntoIterator<Item = &'a DreamReport>,
    {
        let mut gate = Self::new();
        for report in reports {
            gate.record_cycle(&CycleFootprint::from_report(report));
        }
        gate
    }

    /// Fold one completed cycle into the sliding windows.
    ///
    /// Cooldown bookkeeping happens here (not in a separate `tick`): recording
    /// a cycle *is* the passage of a cycle, so the counter can never drift from
    /// the history it is supposed to describe. A degrading cycle re-arms the
    /// full cooldown after the decrement, so the two cycles it buys are the two
    /// that follow it.
    pub fn record_cycle(&mut self, footprint: &CycleFootprint) {
        self.cooldown = self.cooldown.saturating_sub(1);
        if footprint.degraded_health {
            self.cooldown = DEGRADED_HEALTH_COOLDOWN_CYCLES;
        }

        push_capped(
            &mut self.merge_history,
            // rust-doctor-disable-next-line excessive-clone
            footprint.merges.clone(),
            MERGE_CYCLE_WINDOW,
        );
        push_capped(
            &mut self.synthesis_history,
            // rust-doctor-disable-next-line excessive-clone
            footprint.synthesis_rewrites.clone(),
            SYNTHESIS_CHURN_THRESHOLD,
        );
        if let Some(cohort) = footprint.distill {
            push_capped(&mut self.distill_history, cohort, DISTILL_WINDOW);
        }
    }

    #[must_use]
    pub fn evaluate(&self) -> GateDecision {
        if self.cooldown > 0 {
            return GateDecision::Conserve {
                reason: "memory health degraded in a recent cycle".into(),
                cooldown_remaining: self.cooldown,
            };
        }

        let detectors: [fn(&Self) -> Option<String>; 3] = [
            Self::detect_merge_cycle,
            Self::detect_synthesis_churn,
            Self::detect_wasted_distillation,
        ];
        for detect in detectors {
            if let Some(reason) = detect(self) {
                return GateDecision::Conserve {
                    reason,
                    cooldown_remaining: 0,
                };
            }
        }

        GateDecision::Allow
    }

    fn detect_merge_cycle(&self) -> Option<String> {
        if self.merge_history.len() < MERGE_CYCLE_THRESHOLD {
            return None;
        }
        let sets: Vec<&HashSet<(String, String)>> = self.merge_history.iter().collect();

        for window in sets.windows(MERGE_CYCLE_THRESHOLD) {
            let repeated = window[0]
                .iter()
                .find(|pair| window[1..].iter().all(|set| set.contains(*pair)));
            if let Some(pair) = repeated {
                return Some(format!(
                    "merge cycle: ({}, {}) repeated {MERGE_CYCLE_THRESHOLD} consecutive cycles",
                    pair.0, pair.1
                ));
            }
        }
        None
    }

    /// A synthesis note rewritten to a *different* body in every cycle of the
    /// window is the corpus arguing with itself about the same subject.
    ///
    /// Keyed on `(path, body digest)` rather than on the prose itself: whether
    /// two syntheses *contradict* is a semantic judgement the `note_drift` LLM
    /// stage already makes (and reports as `contradictions_found`, which feeds
    /// the selector's stability veto). What this detector adds is the structural
    /// half the model cannot see: the same target churning night after night.
    fn detect_synthesis_churn(&self) -> Option<String> {
        if self.synthesis_history.len() < SYNTHESIS_CHURN_THRESHOLD {
            return None;
        }
        let cycles: Vec<&HashMap<String, String>> = self.synthesis_history.iter().collect();

        for (path, digest) in cycles[0] {
            let mut previous = digest;
            let flipped = cycles[1..].iter().all(|cycle| {
                cycle.get(path).is_some_and(|d| {
                    let changed = d != previous;
                    previous = d;
                    changed
                })
            });
            if flipped {
                return Some(format!(
                    "synthesis churn: {path} rewritten with different content \
                     {SYNTHESIS_CHURN_THRESHOLD} consecutive cycles"
                ));
            }
        }
        None
    }

    fn detect_wasted_distillation(&self) -> Option<String> {
        if self.distill_history.len() < DISTILL_WINDOW {
            return None;
        }

        let (total_produced, total_recalled): (u32, u32) = self
            .distill_history
            .iter()
            .fold((0, 0), |(p, r), (dp, dr)| (p + dp, r + dr));

        if total_produced == 0 {
            return None;
        }

        let rate = f64::from(total_recalled) / f64::from(total_produced);
        if rate < DISTILL_MIN_RECALL_RATE {
            Some(format!(
                "wasted distillation: {total_recalled}/{total_produced} recalled ({:.0}% < {:.0}%)",
                rate * 100.0,
                DISTILL_MIN_RECALL_RATE * 100.0,
            ))
        } else {
            None
        }
    }
}

fn push_capped<T>(queue: &mut VecDeque<T>, item: T, cap: usize) {
    while queue.len() >= cap {
        queue.pop_front();
    }
    queue.push_back(item);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::dreaming::evolution::{EvolutionOutcome, GateOutcome};

    fn merge_cycle(pairs: &[(&str, &str)]) -> CycleFootprint {
        CycleFootprint {
            merges: pairs.iter().map(|(a, b)| normalize_pair(a, b)).collect(),
            ..Default::default()
        }
    }

    fn synthesis_cycle(entries: &[(&str, &str)]) -> CycleFootprint {
        CycleFootprint {
            synthesis_rewrites: entries
                .iter()
                .map(|(p, d)| ((*p).to_string(), (*d).to_string()))
                .collect(),
            ..Default::default()
        }
    }

    fn distill_cycle(size: u32, recalled: u32) -> CycleFootprint {
        CycleFootprint {
            distill: Some((size, recalled)),
            ..Default::default()
        }
    }

    #[test]
    fn no_history_allows() {
        assert!(matches!(
            MutationGate::new().evaluate(),
            GateDecision::Allow
        ));
    }

    #[test]
    fn merge_cycle_detected_after_three_repeats() {
        let mut gate = MutationGate::new();
        gate.record_cycle(&merge_cycle(&[("note_a", "note_b")]));
        assert!(matches!(gate.evaluate(), GateDecision::Allow));
        gate.record_cycle(&merge_cycle(&[("note_a", "note_b")]));
        assert!(matches!(gate.evaluate(), GateDecision::Allow));
        gate.record_cycle(&merge_cycle(&[("note_a", "note_b")]));
        assert!(matches!(gate.evaluate(), GateDecision::Conserve { .. }));
    }

    #[test]
    fn merge_pair_order_does_not_matter() {
        let mut gate = MutationGate::new();
        gate.record_cycle(&merge_cycle(&[("b", "a")]));
        gate.record_cycle(&merge_cycle(&[("a", "b")]));
        gate.record_cycle(&merge_cycle(&[("b", "a")]));
        assert!(matches!(gate.evaluate(), GateDecision::Conserve { .. }));
    }

    #[test]
    fn different_pairs_do_not_trigger() {
        let mut gate = MutationGate::new();
        gate.record_cycle(&merge_cycle(&[("a", "b")]));
        gate.record_cycle(&merge_cycle(&[("c", "d")]));
        gate.record_cycle(&merge_cycle(&[("e", "f")]));
        assert!(matches!(gate.evaluate(), GateDecision::Allow));
    }

    /// The regression the old two-phase (`record_*` then `advance_cycle`)
    /// protocol hid: production always evaluated with an empty "current" buffer,
    /// so the oscillation detector short-circuited before looking at anything.
    /// A gate folded purely from past cycles must still fire.
    #[test]
    fn synthesis_churn_fires_from_history_alone() {
        let mut gate = MutationGate::new();
        gate.record_cycle(&synthesis_cycle(&[("synthesis/rust", "h1")]));
        gate.record_cycle(&synthesis_cycle(&[("synthesis/rust", "h2")]));
        assert!(matches!(gate.evaluate(), GateDecision::Allow));
        gate.record_cycle(&synthesis_cycle(&[("synthesis/rust", "h3")]));
        let decision = gate.evaluate();
        assert!(
            matches!(&decision, GateDecision::Conserve { reason, .. } if reason.contains("synthesis churn")),
            "expected synthesis churn, got {decision:?}"
        );
    }

    #[test]
    fn stable_synthesis_is_not_churn() {
        // Rewritten every cycle but always to the same body — no flip-flop.
        let mut gate = MutationGate::new();
        for _ in 0..3 {
            gate.record_cycle(&synthesis_cycle(&[("synthesis/rust", "same")]));
        }
        assert!(matches!(gate.evaluate(), GateDecision::Allow));
    }

    #[test]
    fn synthesis_churn_needs_the_same_path_every_cycle() {
        let mut gate = MutationGate::new();
        gate.record_cycle(&synthesis_cycle(&[("synthesis/rust", "h1")]));
        gate.record_cycle(&synthesis_cycle(&[("synthesis/python", "h2")]));
        gate.record_cycle(&synthesis_cycle(&[("synthesis/rust", "h3")]));
        assert!(matches!(gate.evaluate(), GateDecision::Allow));
    }

    #[test]
    fn wasted_distillation_detected() {
        let mut gate = MutationGate::new();
        for _ in 0..DISTILL_WINDOW {
            gate.record_cycle(&distill_cycle(2, 0));
        }
        assert!(matches!(gate.evaluate(), GateDecision::Conserve { .. }));
    }

    #[test]
    fn distillation_with_recalls_ok() {
        let mut gate = MutationGate::new();
        for _ in 0..DISTILL_WINDOW {
            gate.record_cycle(&distill_cycle(2, 1));
        }
        assert!(matches!(gate.evaluate(), GateDecision::Allow));
    }

    #[test]
    fn cycles_without_a_mature_cohort_do_not_arm_the_detector() {
        let mut gate = MutationGate::new();
        for _ in 0..DISTILL_WINDOW * 2 {
            gate.record_cycle(&CycleFootprint::default());
        }
        assert!(matches!(gate.evaluate(), GateDecision::Allow));
    }

    #[test]
    fn degraded_health_cools_down_for_exactly_two_cycles() {
        let degraded = CycleFootprint {
            degraded_health: true,
            ..Default::default()
        };
        let mut gate = MutationGate::new();
        gate.record_cycle(&degraded);
        assert!(matches!(
            gate.evaluate(),
            GateDecision::Conserve {
                cooldown_remaining: 2,
                ..
            }
        ));
        gate.record_cycle(&CycleFootprint::default());
        assert!(matches!(
            gate.evaluate(),
            GateDecision::Conserve {
                cooldown_remaining: 1,
                ..
            }
        ));
        gate.record_cycle(&CycleFootprint::default());
        assert!(matches!(gate.evaluate(), GateDecision::Allow));
    }

    /// The cooldown must survive a process restart, which it only can if it is
    /// derived from the persisted reports rather than accumulated in RAM.
    #[test]
    fn cooldown_survives_rehydration_from_reports() {
        let degrading = DreamReport {
            evolution: Some(EvolutionOutcome {
                baseline: 0.6,
                candidate: 0.4,
                best: 0.6,
                outcome: GateOutcome::Reject,
                merges_rejected: 0,
            }),
            ..Default::default()
        };
        let gate = MutationGate::from_reports([&degrading]);
        assert!(matches!(
            gate.evaluate(),
            GateDecision::Conserve {
                cooldown_remaining: 2,
                ..
            }
        ));
    }

    /// A rejected candidate that did *not* lower health (a tie, or a rejection
    /// against the historical best) is not a pathology — no cooldown.
    #[test]
    fn non_degrading_rejection_does_not_cool_down() {
        let flat = DreamReport {
            evolution: Some(EvolutionOutcome {
                baseline: 0.6,
                candidate: 0.6,
                best: 0.8,
                outcome: GateOutcome::Reject,
                merges_rejected: 0,
            }),
            ..Default::default()
        };
        assert!(matches!(
            MutationGate::from_reports([&flat]).evaluate(),
            GateDecision::Allow
        ));
    }

    #[test]
    fn from_reports_folds_merge_history() {
        let cycle = |a: &str, b: &str| DreamReport {
            merged_pairs: vec![(a.to_string(), b.to_string())],
            ..Default::default()
        };
        let reports = [cycle("x", "y"), cycle("x", "y"), cycle("x", "y")];
        let gate = MutationGate::from_reports(&reports);
        assert!(matches!(gate.evaluate(), GateDecision::Conserve { .. }));
    }

    #[test]
    fn serde_roundtrip_gate_decision() {
        let d = GateDecision::Conserve {
            reason: "test".into(),
            cooldown_remaining: 2,
        };
        let json = serde_json::to_string(&d).unwrap();
        let back: GateDecision = serde_json::from_str(&json).unwrap();
        assert!(matches!(
            back,
            GateDecision::Conserve {
                cooldown_remaining: 2,
                ..
            }
        ));
    }
}
