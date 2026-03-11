//! KnowledgeConsolidator — semantic deduplication and skill merging.
//!
//! Prevents skill explosion by detecting semantically similar skills
//! and merging them based on vitality comparison.

use serde::{Deserialize, Serialize};
use tracing::info;

/// Decision on how to handle a duplicate skill pair.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum MergeType {
    /// Winner absorbs loser's parameter mappings.
    Absorb,
    /// Both retired, new synthesized skill replaces them.
    Synthesize,
}

/// Result of a consolidation check.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ConsolidationVerdict {
    /// No similar skill found — proceed with deployment.
    Unique,
    /// Similar skill found — reject candidate as duplicate.
    Duplicate { existing_skill_id: String },
    /// Similar skill found — merge candidate into existing or vice versa.
    Merge {
        winner_id: String,
        loser_id: String,
        merge_type: MergeType,
    },
}

/// Configuration for the consolidator.
#[derive(Debug, Clone)]
pub struct ConsolidatorConfig {
    /// Cosine similarity threshold for considering skills as duplicates.
    pub similarity_threshold: f64,
    /// Vitality threshold: both above this → synthesize; else absorb.
    pub synthesize_vitality_threshold: f32,
}

impl Default for ConsolidatorConfig {
    fn default() -> Self {
        Self {
            similarity_threshold: 0.85,
            synthesize_vitality_threshold: 0.5,
        }
    }
}

/// A skill candidate for consolidation checking.
pub struct SkillCandidate {
    pub skill_id: String,
    pub vitality: f32,
}

/// Determine consolidation verdict for a candidate against existing skills.
///
/// `existing_matches` is a list of (skill_id, similarity, vitality) found via
/// vector search on skill description embeddings.
pub fn check_consolidation(
    candidate: &SkillCandidate,
    existing_matches: &[(String, f64, f32)], // (skill_id, similarity, vitality)
    config: &ConsolidatorConfig,
) -> ConsolidationVerdict {
    // Find the most similar existing skill above threshold
    let best_match = existing_matches
        .iter()
        .filter(|(_, sim, _)| *sim >= config.similarity_threshold)
        .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));

    let Some((existing_id, _similarity, existing_vitality)) = best_match else {
        info!(
            target: "aleph::evolution::probe",
            probe = "consolidation_verdict",
            candidate_id = %candidate.skill_id,
            verdict = "unique",
            matches_above_threshold = 0,
            "Consolidation: no similar skill found — unique"
        );
        return ConsolidationVerdict::Unique;
    };

    // If existing has higher vitality → reject candidate
    if *existing_vitality >= candidate.vitality {
        info!(
            target: "aleph::evolution::probe",
            probe = "consolidation_verdict",
            candidate_id = %candidate.skill_id,
            existing_id = %existing_id,
            verdict = "duplicate",
            candidate_vitality = candidate.vitality,
            existing_vitality = *existing_vitality,
            similarity = _similarity,
            "Consolidation: duplicate rejected — existing has higher vitality"
        );
        return ConsolidationVerdict::Duplicate {
            existing_skill_id: existing_id.clone(),
        };
    }

    // Candidate is better — decide merge type
    let merge_type = if candidate.vitality > config.synthesize_vitality_threshold
        && *existing_vitality > config.synthesize_vitality_threshold
    {
        MergeType::Synthesize
    } else {
        MergeType::Absorb
    };

    info!(
        target: "aleph::evolution::probe",
        probe = "consolidation_verdict",
        candidate_id = %candidate.skill_id,
        existing_id = %existing_id,
        verdict = "merge",
        merge_type = ?merge_type,
        candidate_vitality = candidate.vitality,
        existing_vitality = *existing_vitality,
        similarity = _similarity,
        "Consolidation: merge triggered"
    );

    ConsolidationVerdict::Merge {
        winner_id: candidate.skill_id.clone(),
        loser_id: existing_id.clone(),
        merge_type,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unique_when_no_matches() {
        let candidate = SkillCandidate {
            skill_id: "new-skill".to_string(),
            vitality: 0.8,
        };
        let verdict = check_consolidation(&candidate, &[], &ConsolidatorConfig::default());
        assert_eq!(verdict, ConsolidationVerdict::Unique);
    }

    #[test]
    fn unique_when_below_threshold() {
        let candidate = SkillCandidate {
            skill_id: "new".to_string(),
            vitality: 0.8,
        };
        let matches = vec![("existing".to_string(), 0.7, 0.9)]; // sim < 0.85
        let verdict = check_consolidation(&candidate, &matches, &ConsolidatorConfig::default());
        assert_eq!(verdict, ConsolidationVerdict::Unique);
    }

    #[test]
    fn duplicate_when_existing_has_higher_vitality() {
        let candidate = SkillCandidate {
            skill_id: "new".to_string(),
            vitality: 0.3,
        };
        let matches = vec![("existing".to_string(), 0.9, 0.7)];
        let verdict = check_consolidation(&candidate, &matches, &ConsolidatorConfig::default());
        assert_eq!(
            verdict,
            ConsolidationVerdict::Duplicate {
                existing_skill_id: "existing".to_string()
            }
        );
    }

    #[test]
    fn absorb_when_candidate_better_but_existing_weak() {
        let candidate = SkillCandidate {
            skill_id: "new".to_string(),
            vitality: 0.8,
        };
        let matches = vec![("old".to_string(), 0.9, 0.3)]; // existing vitality < 0.5
        let verdict = check_consolidation(&candidate, &matches, &ConsolidatorConfig::default());
        assert_eq!(
            verdict,
            ConsolidationVerdict::Merge {
                winner_id: "new".to_string(),
                loser_id: "old".to_string(),
                merge_type: MergeType::Absorb,
            }
        );
    }

    #[test]
    fn synthesize_when_both_strong() {
        let candidate = SkillCandidate {
            skill_id: "new".to_string(),
            vitality: 0.8,
        };
        let matches = vec![("old".to_string(), 0.9, 0.6)]; // both above 0.5
        let verdict = check_consolidation(&candidate, &matches, &ConsolidatorConfig::default());
        assert_eq!(
            verdict,
            ConsolidationVerdict::Merge {
                winner_id: "new".to_string(),
                loser_id: "old".to_string(),
                merge_type: MergeType::Synthesize,
            }
        );
    }
}
