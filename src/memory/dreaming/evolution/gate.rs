//! SkillOpt validation gate — pure, deterministic accept/reject decision.
//!
//! Ported from SkillOpt's `evaluate_gate` (arXiv 2605.23904). A candidate
//! memory state is accepted only when its score **strictly improves** on the
//! current state; the best-ever score is tracked so the daemon can detect
//! degradation and conserve instead of compounding bad edits.
//!
//! This is arithmetic comparison only — it does NOT replace any LLM reasoning
//! (R7): the *content* of edits is still proposed by the model; the gate just
//! decides whether an already-formed candidate is worth keeping.

use serde::{Deserialize, Serialize};

/// Outcome of comparing a candidate score against the current and best scores.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GateOutcome {
    /// Candidate improves on current AND beats the best-ever score.
    AcceptNewBest,
    /// Candidate improves on current but does not beat the best.
    Accept,
    /// Candidate does not improve on current — keep the current state.
    Reject,
}

impl GateOutcome {
    /// Whether the candidate should be applied / kept.
    #[must_use]
    pub const fn is_accepted(self) -> bool {
        matches!(self, Self::AcceptNewBest | Self::Accept)
    }
}

/// Pure gate decision: is `candidate` strictly better than `current`, and does
/// it beat `best`? `epsilon` is the minimum improvement that counts as "better"
/// (guards against float noise and rewards only meaningful gains).
#[must_use]
pub fn evaluate_gate(candidate: f64, current: f64, best: f64, epsilon: f64) -> GateOutcome {
    let eps = epsilon.max(0.0);
    if candidate > current + eps {
        if candidate > best + eps {
            GateOutcome::AcceptNewBest
        } else {
            GateOutcome::Accept
        }
    } else {
        GateOutcome::Reject
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_new_best_when_candidate_beats_everything() {
        assert_eq!(
            evaluate_gate(0.9, 0.5, 0.8, 0.0),
            GateOutcome::AcceptNewBest
        );
    }

    #[test]
    fn accepts_without_new_best_when_below_best() {
        assert_eq!(evaluate_gate(0.7, 0.5, 0.9, 0.0), GateOutcome::Accept);
    }

    #[test]
    fn rejects_when_not_improving_on_current() {
        assert_eq!(evaluate_gate(0.5, 0.5, 0.9, 0.0), GateOutcome::Reject);
        assert_eq!(evaluate_gate(0.4, 0.5, 0.9, 0.0), GateOutcome::Reject);
    }

    #[test]
    fn epsilon_band_rejects_marginal_gains() {
        // +0.005 improvement, but epsilon demands +0.01.
        assert_eq!(evaluate_gate(0.505, 0.5, 0.5, 0.01), GateOutcome::Reject);
        // +0.02 clears the band.
        assert_eq!(
            evaluate_gate(0.52, 0.5, 0.5, 0.01),
            GateOutcome::AcceptNewBest
        );
    }

    #[test]
    fn is_accepted_helper() {
        assert!(GateOutcome::AcceptNewBest.is_accepted());
        assert!(GateOutcome::Accept.is_accepted());
        assert!(!GateOutcome::Reject.is_accepted());
    }
}
