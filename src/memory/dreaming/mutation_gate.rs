//! `MutationGate` — detects evolution pathologies and enforces cooldown.
//!
//! Three detection mechanisms:
//! 1. Merge cycle: same note pair merged 3+ consecutive cycles
//! 2. Synthesis oscillation: negation patterns between recent syntheses
//! 3. Wasted distillation: mature skill-notes that never get recalled

use std::collections::{HashSet, VecDeque};

use regex::Regex;

// Re-use GateDecision from selector
pub use super::selector::GateDecision;

const MERGE_CYCLE_WINDOW: usize = 5;
const MERGE_CYCLE_THRESHOLD: usize = 3;
const DISTILL_WINDOW: usize = 5;
const DISTILL_MIN_RECALL_RATE: f64 = 0.1;

/// Tracks evolution pathology state across Dream cycles.
pub struct MutationGate {
    merge_history: VecDeque<HashSet<(String, String)>>,
    current_merges: HashSet<(String, String)>,
    synthesis_assertions: VecDeque<Vec<String>>,
    current_assertions: Vec<String>,
    distill_history: VecDeque<(u32, u32)>,
    cooldown: u32,
}

impl MutationGate {
    #[must_use]
    pub fn new() -> Self {
        Self {
            merge_history: VecDeque::with_capacity(MERGE_CYCLE_WINDOW),
            current_merges: HashSet::new(),
            synthesis_assertions: VecDeque::with_capacity(2),
            current_assertions: Vec::new(),
            distill_history: VecDeque::with_capacity(DISTILL_WINDOW),
            cooldown: 0,
        }
    }

    pub fn record_merge_pair(&mut self, note_a: &str, note_b: &str) {
        let pair = if note_a < note_b {
            (note_a.to_string(), note_b.to_string())
        } else {
            (note_b.to_string(), note_a.to_string())
        };
        self.current_merges.insert(pair);
    }

    pub fn record_synthesis_assertion(&mut self, assertion: &str) {
        self.current_assertions.push(assertion.to_string());
    }

    /// Record one cycle's mature skill-note cohort: `cohort_size` skill notes
    /// old enough to have had a recall opportunity, `cohort_recalled` of which
    /// were actually recalled. The detector fires when the recall rate across
    /// the window stays below `DISTILL_MIN_RECALL_RATE`.
    pub fn record_skill_distill_output(&mut self, cohort_size: u32, cohort_recalled: u32) {
        if self.distill_history.len() >= DISTILL_WINDOW {
            self.distill_history.pop_front();
        }
        self.distill_history
            .push_back((cohort_size, cohort_recalled));
    }

    pub fn advance_cycle(&mut self) {
        if self.merge_history.len() >= MERGE_CYCLE_WINDOW {
            self.merge_history.pop_front();
        }
        self.merge_history
            .push_back(std::mem::take(&mut self.current_merges));

        if self.synthesis_assertions.len() >= 2 {
            self.synthesis_assertions.pop_front();
        }
        self.synthesis_assertions
            .push_back(std::mem::take(&mut self.current_assertions));
    }

    pub const fn activate_cooldown(&mut self, cycles: u32) {
        self.cooldown = cycles;
    }

    pub const fn tick_cooldown(&mut self) {
        self.cooldown = self.cooldown.saturating_sub(1);
    }

    #[must_use]
    pub fn evaluate(&self) -> GateDecision {
        if self.cooldown > 0 {
            return GateDecision::Conserve {
                reason: "cooldown active".into(),
                cooldown_remaining: self.cooldown,
            };
        }

        if let Some(reason) = self.detect_merge_cycle() {
            return GateDecision::Conserve {
                reason,
                cooldown_remaining: 0,
            };
        }

        if let Some(reason) = self.detect_oscillation() {
            return GateDecision::Conserve {
                reason,
                cooldown_remaining: 0,
            };
        }

        if let Some(reason) = self.detect_wasted_distillation() {
            return GateDecision::Conserve {
                reason,
                cooldown_remaining: 0,
            };
        }

        GateDecision::Allow
    }

    fn detect_merge_cycle(&self) -> Option<String> {
        let all_sets: Vec<&HashSet<(String, String)>> = self
            .merge_history
            .iter()
            .chain(std::iter::once(&self.current_merges))
            .collect();

        if all_sets.len() < MERGE_CYCLE_THRESHOLD {
            return None;
        }

        for window in all_sets.windows(MERGE_CYCLE_THRESHOLD) {
            let intersection: HashSet<_> = window[0]
                .iter()
                .filter(|pair| window[1..].iter().all(|set| set.contains(*pair)))
                .cloned()
                .collect();

            if let Some(pair) = intersection.into_iter().next() {
                return Some(format!(
                    "merge cycle: ({}, {}) repeated {} consecutive cycles",
                    pair.0, pair.1, MERGE_CYCLE_THRESHOLD
                ));
            }
        }

        None
    }

    fn detect_oscillation(&self) -> Option<String> {
        let prev = self.synthesis_assertions.back()?;
        let curr = &self.current_assertions;

        if prev.is_empty() || curr.is_empty() {
            return None;
        }

        let negation_pairs = [
            (r"should\s+", r"should\s+not\s+"),
            (r"prefer\s+", r"avoid\s+"),
            (r"use\s+", r"do\s+not\s+use\s+"),
        ];

        for prev_assertion in prev {
            for curr_assertion in curr {
                for (positive, negative) in &negation_pairs {
                    let pos_re = Regex::new(positive).ok()?;
                    let neg_re = Regex::new(negative).ok()?;

                    let is_oscillation = (pos_re.is_match(prev_assertion)
                        && neg_re.is_match(curr_assertion))
                        || (neg_re.is_match(prev_assertion) && pos_re.is_match(curr_assertion));

                    if is_oscillation {
                        return Some(format!(
                            "synthesis oscillation: '{}' vs '{}'",
                            crate::utils::text_format::truncate_text(prev_assertion, 60),
                            crate::utils::text_format::truncate_text(curr_assertion, 60),
                        ));
                    }
                }
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

impl Default for MutationGate {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_history_allows() {
        let gate = MutationGate::new();
        assert!(matches!(gate.evaluate(), GateDecision::Allow));
    }

    #[test]
    fn merge_cycle_detected_after_three_repeats() {
        let mut gate = MutationGate::new();
        gate.record_merge_pair("note_a", "note_b");
        assert!(matches!(gate.evaluate(), GateDecision::Allow));
        gate.advance_cycle();
        gate.record_merge_pair("note_a", "note_b");
        assert!(matches!(gate.evaluate(), GateDecision::Allow));
        gate.advance_cycle();
        gate.record_merge_pair("note_a", "note_b");
        assert!(matches!(gate.evaluate(), GateDecision::Conserve { .. }));
    }

    #[test]
    fn different_pairs_do_not_trigger() {
        let mut gate = MutationGate::new();
        gate.record_merge_pair("a", "b");
        gate.advance_cycle();
        gate.record_merge_pair("c", "d");
        gate.advance_cycle();
        gate.record_merge_pair("e", "f");
        assert!(matches!(gate.evaluate(), GateDecision::Allow));
    }

    #[test]
    fn oscillation_detected_with_negation() {
        let mut gate = MutationGate::new();
        gate.record_synthesis_assertion("should use async");
        gate.advance_cycle();
        gate.record_synthesis_assertion("should not use async");
        assert!(matches!(gate.evaluate(), GateDecision::Conserve { .. }));
    }

    #[test]
    fn no_oscillation_without_negation() {
        let mut gate = MutationGate::new();
        gate.record_synthesis_assertion("should use async");
        gate.advance_cycle();
        gate.record_synthesis_assertion("should use traits");
        assert!(matches!(gate.evaluate(), GateDecision::Allow));
    }

    #[test]
    fn wasted_distillation_detected() {
        let mut gate = MutationGate::new();
        for _ in 0..5 {
            gate.record_skill_distill_output(2, 0);
            gate.advance_cycle();
        }
        assert!(matches!(gate.evaluate(), GateDecision::Conserve { .. }));
    }

    #[test]
    fn distillation_with_recalls_ok() {
        let mut gate = MutationGate::new();
        for _ in 0..5 {
            gate.record_skill_distill_output(2, 1);
            gate.advance_cycle();
        }
        assert!(matches!(gate.evaluate(), GateDecision::Allow));
    }

    #[test]
    fn cooldown_prevents_reevaluation() {
        let mut gate = MutationGate::new();
        gate.activate_cooldown(3);
        let d = gate.evaluate();
        assert!(matches!(
            d,
            GateDecision::Conserve {
                cooldown_remaining: 3,
                ..
            }
        ));
        gate.tick_cooldown();
        let d = gate.evaluate();
        assert!(matches!(
            d,
            GateDecision::Conserve {
                cooldown_remaining: 2,
                ..
            }
        ));
        gate.tick_cooldown();
        gate.tick_cooldown();
        assert!(matches!(gate.evaluate(), GateDecision::Allow));
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
