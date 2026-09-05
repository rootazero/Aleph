//! `DreamStrategy` — signal-driven strategy selection for the dream pipeline.
//!
//! Replaces the hardcoded daily/weekly pipeline with three adaptive strategies,
//! each defining which stages to execute.

use serde::{Deserialize, Serialize};

/// Evolution strategy for a Dream cycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DreamStrategy {
    /// Default mode: merge duplicates, fix formats, detect drift, maintain index.
    Consolidate,
    /// Growth mode: cross-category synthesis, skill-note distillation.
    Synthesize,
    /// Defensive mode: no corpus-mutating LLM stages (consolidate / synthesis /
    /// distill). The pending review queue is still adjudicated — `note_review`
    /// calls the LLM once per queued candidate — because its rows have no other
    /// consumer and rubber-stamping or dropping them would void the ingest
    /// gate's Defer.
    Conserve,
}

// NOTE: there is deliberately no `stage_names()` here. One used to exist as a
// hand-maintained second source of truth, with zero production consumers and
// three tests that locked its drift in place. It had drifted badly — its
// Synthesize list omitted `feedback_distill` even though `DreamPipeline::from_strategy`
// builds that stage on BOTH the Consolidate and Synthesize paths, and its
// Consolidate list omitted co_recall_edges / graph_recompute / note_weave /
// mention_weave / goal_lessons_promote entirely. Anyone asking "when does
// feedback_distill actually run?" read it and got the wrong answer.
//
// The pipeline is the only source of truth. If a name list is ever needed again,
// derive it: `DreamPipeline::from_strategy(..).stages.iter().map(|s| s.name())`.

impl std::fmt::Display for DreamStrategy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Consolidate => write!(f, "consolidate"),
            Self::Synthesize => write!(f, "synthesize"),
            Self::Conserve => write!(f, "conserve"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_roundtrip() {
        for strategy in [
            DreamStrategy::Consolidate,
            DreamStrategy::Synthesize,
            DreamStrategy::Conserve,
        ] {
            let s = serde_json::to_string(&strategy).unwrap();
            let back: DreamStrategy = serde_json::from_str(&s).unwrap();
            assert_eq!(back, strategy);
        }
    }
}
