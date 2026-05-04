//! DreamStrategy — signal-driven strategy selection for the dream pipeline.
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
    /// Defensive mode: deterministic-only ops, skip all LLM stages.
    Conserve,
}

impl DreamStrategy {
    /// Ordered list of stage names this strategy will execute.
    pub fn stage_names(&self) -> Vec<&'static str> {
        match self {
            Self::Consolidate => vec![
                "note_lint",
                "note_review",
                "note_consolidate",
                "note_drift",
                "index_refresher",
                "note_decay",
            ],
            Self::Synthesize => vec![
                "note_lint",
                "note_review",
                "note_consolidate",
                "note_synthesis",
                "skill_distill",
                "daily_digest",
            ],
            Self::Conserve => vec!["note_lint", "note_review", "index_refresher"],
        }
    }
}

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
    fn consolidate_stages() {
        let names = DreamStrategy::Consolidate.stage_names();
        assert_eq!(
            names,
            vec![
                "note_lint",
                "note_review",
                "note_consolidate",
                "note_drift",
                "index_refresher",
                "note_decay"
            ]
        );
    }

    #[test]
    fn synthesize_stages() {
        let names = DreamStrategy::Synthesize.stage_names();
        assert_eq!(
            names,
            vec![
                "note_lint",
                "note_review",
                "note_consolidate",
                "note_synthesis",
                "skill_distill",
                "daily_digest"
            ]
        );
    }

    #[test]
    fn conserve_stages() {
        let names = DreamStrategy::Conserve.stage_names();
        assert_eq!(names, vec!["note_lint", "note_review", "index_refresher"]);
    }

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
