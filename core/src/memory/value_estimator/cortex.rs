//! Cortex-specific value estimation extensions
//!
//! Extends ValueEstimator with multi-dimensional scoring for experience replay.

use crate::error::Result;
use crate::memory::cortex::Experience;

/// Multi-dimensional score for experience evaluation
#[derive(Debug, Clone)]
pub struct ExperienceScore {
    /// Success rate (0.0-1.0)
    pub success_rate: f64,

    /// Token efficiency (output_value / token_cost)
    pub token_efficiency: f64,

    /// User feedback (0.0-1.0, if available)
    pub user_feedback: Option<f64>,

    /// Novelty score (0.0-1.0)
    pub novelty_score: f64,

    /// Final weighted score (0.0-1.0)
    pub final_score: f64,
}

/// Cortex value estimator for experience scoring
pub struct CortexValueEstimator {
    /// Weight for success rate (default: 0.4)
    pub weight_success: f64,

    /// Weight for token efficiency (default: 0.3)
    pub weight_efficiency: f64,

    /// Weight for user feedback (default: 0.2)
    pub weight_feedback: f64,

    /// Weight for novelty (default: 0.1)
    pub weight_novelty: f64,
}

impl Default for CortexValueEstimator {
    fn default() -> Self {
        Self {
            weight_success: 0.4,
            weight_efficiency: 0.3,
            weight_feedback: 0.2,
            weight_novelty: 0.1,
        }
    }
}

impl CortexValueEstimator {
    /// Create a new Cortex value estimator with default weights
    pub fn new() -> Self {
        Self::default()
    }

    /// Create with custom weights
    pub fn with_weights(
        weight_success: f64,
        weight_efficiency: f64,
        weight_feedback: f64,
        weight_novelty: f64,
    ) -> Self {
        Self {
            weight_success,
            weight_efficiency,
            weight_feedback,
            weight_novelty,
        }
    }

    /// Estimate the value of an experience
    pub async fn estimate(&self, experience: &Experience) -> Result<ExperienceScore> {
        // Calculate success rate
        // If experience has been used (usage_count > 1), use actual success rate
        // Otherwise, use the initial success_score
        let success_rate = if experience.usage_count > 1 {
            experience.success_count as f64 / experience.usage_count as f64
        } else {
            experience.success_score
        };

        // Get token efficiency (default to 0.5 if not available)
        let token_efficiency = experience.token_efficiency.unwrap_or(0.5);

        // Get user feedback (default to 0.5 if not available)
        let user_feedback = Some(0.5); // TODO: Implement user feedback collection

        // Get novelty score (default to 0.5 if not available)
        let novelty_score = experience.novelty_score.unwrap_or(0.5);

        // Calculate weighted final score
        let final_score = self.weight_success * success_rate
            + self.weight_efficiency * token_efficiency
            + self.weight_feedback * user_feedback.unwrap_or(0.5)
            + self.weight_novelty * novelty_score;

        Ok(ExperienceScore {
            success_rate,
            token_efficiency,
            user_feedback,
            novelty_score,
            final_score: final_score.clamp(0.0, 1.0),
        })
    }

    /// Calculate novelty score based on pattern similarity
    ///
    /// Uses Levenshtein distance to measure how different this pattern is
    /// from existing patterns in the experience database.
    pub fn calculate_novelty(
        &self,
        current_pattern: &str,
        existing_patterns: &[String],
    ) -> f64 {
        if existing_patterns.is_empty() {
            return 1.0; // Completely novel if no existing patterns
        }

        // Calculate minimum edit distance to any existing pattern
        let min_distance = existing_patterns
            .iter()
            .map(|pattern| levenshtein_distance(current_pattern, pattern))
            .min()
            .unwrap_or(current_pattern.len());

        // Normalize by max possible distance
        let max_distance = current_pattern.len().max(
            existing_patterns
                .iter()
                .map(|p| p.len())
                .max()
                .unwrap_or(0),
        );

        if max_distance == 0 {
            return 0.0;
        }

        // Convert to novelty score (0.0 = identical, 1.0 = completely different)
        (min_distance as f64 / max_distance as f64).clamp(0.0, 1.0)
    }
}

/// Calculate Levenshtein distance between two strings
fn levenshtein_distance(s1: &str, s2: &str) -> usize {
    let len1 = s1.len();
    let len2 = s2.len();

    if len1 == 0 {
        return len2;
    }
    if len2 == 0 {
        return len1;
    }

    let mut matrix = vec![vec![0; len2 + 1]; len1 + 1];

    // Initialize first row and column
    for i in 0..=len1 {
        matrix[i][0] = i;
    }
    for j in 0..=len2 {
        matrix[0][j] = j;
    }

    // Fill matrix
    for (i, c1) in s1.chars().enumerate() {
        for (j, c2) in s2.chars().enumerate() {
            let cost = if c1 == c2 { 0 } else { 1 };
            matrix[i + 1][j + 1] = (matrix[i][j + 1] + 1) // deletion
                .min(matrix[i + 1][j] + 1) // insertion
                .min(matrix[i][j] + cost); // substitution
        }
    }

    matrix[len1][len2]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::cortex::{EvolutionStatus, ExperienceBuilder};

    #[tokio::test]
    async fn test_estimate_experience() {
        let estimator = CortexValueEstimator::new();

        let exp = ExperienceBuilder::new(
            "test-1".to_string(),
            "test intent".to_string(),
            "{}".to_string(),
        )
        .success_score(0.95)
        .token_efficiency(0.8)
        .novelty_score(0.6)
        .build();

        let score = estimator.estimate(&exp).await.unwrap();

        assert!(score.final_score > 0.0);
        assert!(score.final_score <= 1.0);
        assert_eq!(score.success_rate, 0.95);
        assert_eq!(score.token_efficiency, 0.8);
        assert_eq!(score.novelty_score, 0.6);
    }

    #[test]
    fn test_levenshtein_distance() {
        assert_eq!(levenshtein_distance("", ""), 0);
        assert_eq!(levenshtein_distance("abc", "abc"), 0);
        assert_eq!(levenshtein_distance("abc", "abd"), 1);
        assert_eq!(levenshtein_distance("abc", "def"), 3);
        assert_eq!(levenshtein_distance("kitten", "sitting"), 3);
    }

    #[test]
    fn test_calculate_novelty() {
        let estimator = CortexValueEstimator::new();

        // Completely novel (no existing patterns)
        let novelty = estimator.calculate_novelty("new_pattern", &[]);
        assert_eq!(novelty, 1.0);

        // Identical pattern
        let novelty = estimator.calculate_novelty("pattern", &["pattern".to_string()]);
        assert_eq!(novelty, 0.0);

        // Similar pattern
        let novelty = estimator.calculate_novelty(
            "pattern_a",
            &["pattern_b".to_string(), "pattern_c".to_string()],
        );
        assert!(novelty > 0.0 && novelty < 1.0);

        // Completely different pattern
        let novelty = estimator.calculate_novelty("xyz", &["abc".to_string()]);
        assert_eq!(novelty, 1.0);
    }

    #[tokio::test]
    async fn test_custom_weights() {
        let estimator = CortexValueEstimator::with_weights(0.5, 0.3, 0.1, 0.1);

        let exp = ExperienceBuilder::new(
            "test-2".to_string(),
            "test intent".to_string(),
            "{}".to_string(),
        )
        .success_score(1.0)
        .token_efficiency(0.5)
        .novelty_score(0.5)
        .build();

        let score = estimator.estimate(&exp).await.unwrap();

        // With higher weight on success (0.5), score should be higher
        assert!(score.final_score > 0.6);
    }
}
