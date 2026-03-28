//! Review scoring types and validation logic.
//!
//! Provides structured review scores with per-dimension ratings,
//! challenges, and configurable validation thresholds.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::types::{Severity, TeamRoleConfig};

// ---------------------------------------------------------------------------
// DimensionScore
// ---------------------------------------------------------------------------

/// A score for a single review dimension (e.g., correctness, completeness).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct DimensionScore {
    /// The dimension being scored (e.g., "correctness", "completeness").
    pub dimension: String,
    /// Score from 1 to 10.
    pub score: u8,
    /// Rationale for the score.
    pub rationale: String,
}

// ---------------------------------------------------------------------------
// Challenge
// ---------------------------------------------------------------------------

/// A specific challenge or concern raised during a review.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Challenge {
    /// The point being challenged.
    pub point: String,
    /// How severe the challenge is.
    pub severity: Severity,
    /// Evidence supporting the challenge.
    pub evidence: String,
}

// ---------------------------------------------------------------------------
// ReviewScore
// ---------------------------------------------------------------------------

/// A structured review score for a task artifact.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ReviewScore {
    /// The task being reviewed.
    pub task_id: String,
    /// The artifact being reviewed.
    pub artifact_id: String,
    /// Per-dimension scores.
    pub scores: Vec<DimensionScore>,
    /// Whether the artifact passes overall.
    pub overall_pass: bool,
    /// Challenges raised during review.
    pub challenges: Vec<Challenge>,
    /// Suggestions for improvement.
    #[serde(default)]
    pub improvement_suggestions: Vec<String>,
    /// Risks if the artifact is accepted as-is.
    #[serde(default)]
    pub risks_if_accepted: Vec<String>,
}

impl ReviewScore {
    /// Validate the review against role config thresholds.
    ///
    /// Returns `Ok(())` if valid, or `Err(errors)` with a list of validation
    /// failures.
    pub fn validate(&self, config: &TeamRoleConfig) -> Result<(), Vec<String>> {
        let mut errors = Vec::new();

        if self.challenges.len() < config.min_challenges as usize {
            errors.push(format!(
                "Minimum {} challenges required, got {}",
                config.min_challenges,
                self.challenges.len()
            ));
        }

        if self.overall_pass {
            for score in &self.scores {
                if score.score < config.min_score_threshold {
                    errors.push(format!(
                        "Cannot pass: dimension '{}' scored {}/10, minimum is {}",
                        score.dimension, score.score, config.min_score_threshold
                    ));
                }
            }
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::teams::roles::types::AgentRole;

    fn make_config(min_challenges: u32, min_score_threshold: u8) -> TeamRoleConfig {
        TeamRoleConfig {
            role: AgentRole::Critic,
            prompt_template: String::new(),
            review_dimensions: vec!["correctness".into(), "completeness".into()],
            min_score_threshold,
            min_challenges,
        }
    }

    fn make_challenge(point: &str) -> Challenge {
        Challenge {
            point: point.to_string(),
            severity: Severity::Major,
            evidence: "evidence".to_string(),
        }
    }

    fn make_dimension(name: &str, score: u8) -> DimensionScore {
        DimensionScore {
            dimension: name.to_string(),
            score,
            rationale: "rationale".to_string(),
        }
    }

    #[test]
    fn test_review_score_rejects_insufficient_challenges() {
        let config = make_config(3, 7);
        let review = ReviewScore {
            task_id: "task-1".into(),
            artifact_id: "art-1".into(),
            scores: vec![make_dimension("correctness", 8)],
            overall_pass: false,
            challenges: vec![make_challenge("only one challenge")],
            improvement_suggestions: vec![],
            risks_if_accepted: vec![],
        };

        let result = review.validate(&config);
        assert!(result.is_err());
        let errors = result.unwrap_err();
        assert_eq!(errors.len(), 1);
        assert!(errors[0].contains("Minimum 3 challenges required, got 1"));
    }

    #[test]
    fn test_review_score_rejects_pass_with_low_scores() {
        let config = make_config(1, 7);
        let review = ReviewScore {
            task_id: "task-1".into(),
            artifact_id: "art-1".into(),
            scores: vec![
                make_dimension("correctness", 5),
                make_dimension("completeness", 8),
            ],
            overall_pass: true,
            challenges: vec![make_challenge("challenge 1")],
            improvement_suggestions: vec![],
            risks_if_accepted: vec![],
        };

        let result = review.validate(&config);
        assert!(result.is_err());
        let errors = result.unwrap_err();
        assert_eq!(errors.len(), 1);
        assert!(errors[0].contains("correctness"));
        assert!(errors[0].contains("5/10"));
        assert!(errors[0].contains("minimum is 7"));
    }

    #[test]
    fn test_review_score_accepts_valid_review() {
        let config = make_config(3, 7);
        let review = ReviewScore {
            task_id: "task-1".into(),
            artifact_id: "art-1".into(),
            scores: vec![
                make_dimension("correctness", 8),
                make_dimension("completeness", 9),
            ],
            overall_pass: true,
            challenges: vec![
                make_challenge("challenge 1"),
                make_challenge("challenge 2"),
                make_challenge("challenge 3"),
            ],
            improvement_suggestions: vec![],
            risks_if_accepted: vec![],
        };

        let result = review.validate(&config);
        assert!(result.is_ok());
    }

    #[test]
    fn test_review_score_allows_fail_with_low_scores() {
        // When overall_pass is false, low scores are acceptable (reviewer is rejecting)
        let config = make_config(3, 7);
        let review = ReviewScore {
            task_id: "task-1".into(),
            artifact_id: "art-1".into(),
            scores: vec![
                make_dimension("correctness", 3),
                make_dimension("completeness", 2),
            ],
            overall_pass: false,
            challenges: vec![
                make_challenge("major flaw 1"),
                make_challenge("major flaw 2"),
                make_challenge("major flaw 3"),
            ],
            improvement_suggestions: vec!["Rewrite everything".into()],
            risks_if_accepted: vec!["System failure".into()],
        };

        let result = review.validate(&config);
        assert!(result.is_ok());
    }
}
