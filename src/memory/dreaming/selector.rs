//! Strategy Selector — deterministic signal-to-strategy mapping.
//!
//! No LLM calls. Uses composite signal scores and a sliding-window
//! personality adaptation to choose the optimal `DreamStrategy`.

use std::collections::VecDeque;

use serde::{Deserialize, Serialize};

use super::signals::SignalSnapshot;
use super::strategy::DreamStrategy;

/// Mutation gate decision (input to selector).
///
/// A third `Skip { reason }` variant existed with zero producers — the gate
/// only ever returns `Allow` or `Conserve`, and "don't run a cycle at all" is
/// decided upstream by the daemon's window/idle/once-per-day preconditions.
/// Withdrawn under R10 (no abstractions kept open for a hypothetical future).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum GateDecision {
    Allow,
    Conserve {
        reason: String,
        cooldown_remaining: u32,
    },
}

/// Output of strategy selection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SelectionDecision {
    pub strategy: DreamStrategy,
    pub rationale: String,
    pub personality_adjustment: f64,
}

pub const DEFAULT_SYNTHESIZE_THRESHOLD: f64 = 0.6;
pub const MIN_SYNTHESIZE_THRESHOLD: f64 = 0.4;
pub const MAX_SYNTHESIZE_THRESHOLD: f64 = 0.8;
const PERSONALITY_WINDOW: usize = 10;
const PERSONALITY_STEP: f64 = 0.1;
const HIGH_PASS_RATE: f64 = 0.8;
const LOW_PASS_RATE: f64 = 0.5;
const MIN_STABILITY: f64 = 0.5;

#[derive(Debug, Clone)]
struct CycleRecord {
    validation_passed: bool,
}

/// Deterministic strategy selector with sliding-window personality.
///
/// Like [`super::mutation_gate::MutationGate`], the window is **derived** from
/// the persisted dream event log at the start of each cycle rather than
/// accumulated in the process. Cycles run at most once a day, so an in-RAM
/// ten-cycle window reset the daemon's personality to neutral on every restart
/// and effectively never adapted.
pub struct StrategySelector {
    history: VecDeque<CycleRecord>,
}

impl StrategySelector {
    /// How many past cycles [`Self::from_outcomes`] needs to see.
    pub const HISTORY_WINDOW: usize = PERSONALITY_WINDOW;

    #[must_use]
    pub fn new() -> Self {
        Self {
            history: VecDeque::with_capacity(PERSONALITY_WINDOW),
        }
    }

    /// Rebuild the personality window from past cycles' validation verdicts,
    /// **oldest first**.
    #[must_use]
    pub fn from_outcomes<I: IntoIterator<Item = bool>>(validation_passed: I) -> Self {
        let mut selector = Self::new();
        for passed in validation_passed {
            selector.record_cycle_outcome(passed);
        }
        selector
    }

    /// Record outcome of a completed Dream cycle for personality adaptation.
    ///
    /// Personality keys solely on the validation pass-rate over the sliding
    /// window (see `personality_adjustment`), so only `validation_passed` is
    /// recorded — the cycle strategy and skill-recall rate are not consulted.
    pub fn record_cycle_outcome(&mut self, validation_passed: bool) {
        if self.history.len() >= PERSONALITY_WINDOW {
            self.history.pop_front();
        }
        self.history.push_back(CycleRecord { validation_passed });
    }

    /// Current synthesize threshold after personality adjustment.
    #[must_use]
    pub fn synthesize_threshold(&self) -> f64 {
        let adjustment = self.personality_adjustment();
        (DEFAULT_SYNTHESIZE_THRESHOLD + adjustment)
            .clamp(MIN_SYNTHESIZE_THRESHOLD, MAX_SYNTHESIZE_THRESHOLD)
    }

    fn personality_adjustment(&self) -> f64 {
        if self.history.is_empty() {
            return 0.0;
        }
        let pass_rate = self.history.iter().filter(|r| r.validation_passed).count() as f64
            / self.history.len() as f64;

        if pass_rate > HIGH_PASS_RATE {
            -PERSONALITY_STEP
        } else if pass_rate < LOW_PASS_RATE {
            PERSONALITY_STEP
        } else {
            0.0
        }
    }

    /// Select the best strategy given current signals and gate decision.
    #[must_use]
    pub fn select(&self, snapshot: &SignalSnapshot, gate: &GateDecision) -> SelectionDecision {
        let adjustment = self.personality_adjustment();

        match gate {
            GateDecision::Conserve { reason, .. } => {
                return SelectionDecision {
                    strategy: DreamStrategy::Conserve,
                    rationale: format!("gate forced conserve: {reason}"),
                    personality_adjustment: adjustment,
                };
            }
            GateDecision::Allow => {}
        }

        let threshold = self.synthesize_threshold();

        let growth_rate = snapshot.score("note_growth_rate");
        let skill_recall_rate = snapshot.score("skill_recall_rate");
        let growth_pressure = growth_rate * (1.0 - skill_recall_rate);

        let contradiction_rate = snapshot.score("high_contradiction_rate");
        let duplication_rate = snapshot.score("high_duplication_rate");
        let stability = 1.0 - (contradiction_rate + duplication_rate) / 2.0;

        if growth_pressure > threshold && stability > MIN_STABILITY {
            SelectionDecision {
                strategy: DreamStrategy::Synthesize,
                rationale: format!(
                    "growth_pressure={growth_pressure:.2} > threshold={threshold:.2}, stability={stability:.2}"
                ),
                personality_adjustment: adjustment,
            }
        } else {
            SelectionDecision {
                strategy: DreamStrategy::Consolidate,
                rationale: format!(
                    "growth_pressure={growth_pressure:.2} <= threshold={threshold:.2} or stability={stability:.2} <= {MIN_STABILITY}"
                ),
                personality_adjustment: adjustment,
            }
        }
    }
}

impl Default for StrategySelector {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::dreaming::signals::RawMetrics;

    fn snapshot_from(metrics: &RawMetrics) -> SignalSnapshot {
        SignalSnapshot::from_metrics(metrics)
    }

    #[test]
    fn default_metrics_select_consolidate() {
        let snapshot = snapshot_from(&RawMetrics::default());
        let decision = StrategySelector::new().select(&snapshot, &GateDecision::Allow);
        assert_eq!(decision.strategy, DreamStrategy::Consolidate);
    }

    #[test]
    fn high_growth_low_skill_recall_selects_synthesize() {
        let snapshot = snapshot_from(&RawMetrics {
            notes_added_24h: 80,
            total_notes: 100,
            skill_notes_total: 10,
            skill_notes_recalled: 0,
            ..Default::default()
        });
        let decision = StrategySelector::new().select(&snapshot, &GateDecision::Allow);
        assert_eq!(decision.strategy, DreamStrategy::Synthesize);
    }

    #[test]
    fn gate_conserve_overrides_synthesize() {
        let snapshot = snapshot_from(&RawMetrics {
            notes_added_24h: 80,
            total_notes: 100,
            ..Default::default()
        });
        let gate = GateDecision::Conserve {
            reason: "merge cycle detected".into(),
            cooldown_remaining: 3,
        };
        let decision = StrategySelector::new().select(&snapshot, &gate);
        assert_eq!(decision.strategy, DreamStrategy::Conserve);
    }

    /// Personality must be reconstructible from the persisted verdicts alone —
    /// otherwise it silently resets to neutral on every daemon restart.
    #[test]
    fn from_outcomes_reproduces_an_accumulated_window() {
        let verdicts = [true, false, true, true, true, true, true, true, true, true];
        let mut accumulated = StrategySelector::new();
        for v in verdicts {
            accumulated.record_cycle_outcome(v);
        }
        let derived = StrategySelector::from_outcomes(verdicts);
        assert!(
            (derived.synthesize_threshold() - accumulated.synthesize_threshold()).abs() < 1e-9,
            "derived personality must match the accumulated one"
        );
        assert!(derived.synthesize_threshold() < DEFAULT_SYNTHESIZE_THRESHOLD);
    }

    #[test]
    fn from_outcomes_keeps_only_the_last_window() {
        // Twenty failures then ten passes: only the passes are in the window.
        let verdicts = std::iter::repeat_n(false, 20).chain(std::iter::repeat_n(true, 10));
        let derived = StrategySelector::from_outcomes(verdicts);
        assert!(derived.synthesize_threshold() < DEFAULT_SYNTHESIZE_THRESHOLD);
    }

    #[test]
    fn personality_high_pass_rate_lowers_threshold() {
        let mut selector = StrategySelector::new();
        for _ in 0..10 {
            selector.record_cycle_outcome(true);
        }
        assert!(selector.synthesize_threshold() < DEFAULT_SYNTHESIZE_THRESHOLD);
    }

    #[test]
    fn personality_low_pass_rate_raises_threshold() {
        let mut selector = StrategySelector::new();
        for _ in 0..10 {
            selector.record_cycle_outcome(false);
        }
        assert!(selector.synthesize_threshold() > DEFAULT_SYNTHESIZE_THRESHOLD);
    }

    #[test]
    fn threshold_clamped_to_range() {
        let mut selector = StrategySelector::new();
        for _ in 0..20 {
            selector.record_cycle_outcome(true);
        }
        assert!(selector.synthesize_threshold() >= MIN_SYNTHESIZE_THRESHOLD);
        assert!(selector.synthesize_threshold() <= MAX_SYNTHESIZE_THRESHOLD);
    }

    #[test]
    fn serde_roundtrip_decision() {
        let decision = SelectionDecision {
            strategy: DreamStrategy::Synthesize,
            rationale: "high growth pressure".into(),
            personality_adjustment: -0.1,
        };
        let json = serde_json::to_string(&decision).unwrap();
        let back: SelectionDecision = serde_json::from_str(&json).unwrap();
        assert_eq!(back.strategy, DreamStrategy::Synthesize);
    }
}
