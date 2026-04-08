//! 8-dimensional scoring engine for ShortTerm → LongTerm fact promotion.
//!
//! Each dimension returns a f32 clamped to [0.0, 1.0]. The final score
//! is a weighted sum of all dimensions, compared against configurable
//! thresholds to decide whether a fact should be promoted.

/// Thresholds that must all be met for promotion to occur.
pub struct PromotionThresholds {
    pub min_score: f32,
    pub min_signal_count: u32,
    pub min_unique_queries: u32,
    pub min_age_hours: u64,
}

impl Default for PromotionThresholds {
    fn default() -> Self {
        Self {
            min_score: 0.65,
            min_signal_count: 3,
            min_unique_queries: 2,
            min_age_hours: 24,
        }
    }
}

/// Weighted scorer across 8 dimensions of fact quality.
///
/// Dimensions (in order):
/// 0. Frequency
/// 1. Relevance
/// 2. Diversity
/// 3. Recency
/// 4. Consolidation
/// 5. Conceptual
/// 6. Graph centrality
/// 7. Cluster cohesion
pub struct PromotionScorer {
    pub weights: [f32; 8],
    pub thresholds: PromotionThresholds,
}

impl Default for PromotionScorer {
    fn default() -> Self {
        Self {
            weights: [0.20, 0.22, 0.13, 0.12, 0.10, 0.05, 0.10, 0.08],
            thresholds: PromotionThresholds::default(),
        }
    }
}

// ── Dimension functions ─────────────────────────────────────────────

fn clamp01(v: f32) -> f32 {
    v.clamp(0.0, 1.0)
}

/// How often the fact has been referenced.
pub fn compute_frequency(signal_count: u32) -> f32 {
    clamp01((signal_count as f32).ln_1p() / 10_f32.ln_1p())
}

/// Average relevance score per signal.
pub fn compute_relevance(total_score: f32, signal_count: u32) -> f32 {
    if signal_count == 0 {
        return 0.0;
    }
    clamp01(total_score / signal_count as f32)
}

/// Breadth of contexts that surfaced the fact.
pub fn compute_diversity(unique_queries: u32, unique_channels: u32) -> f32 {
    clamp01(unique_queries.max(unique_channels) as f32 / 5.0)
}

/// Exponential decay with a 14-day half-life.
pub fn compute_recency(days_since_access: f32) -> f32 {
    let ln2 = 2_f32.ln();
    clamp01((-ln2 / 14.0 * days_since_access).exp())
}

/// Spaced-repetition signal: how spread out the recalls are.
pub fn compute_consolidation(recall_days: u32, span_days: f32) -> f32 {
    if recall_days <= 1 {
        return 0.2;
    }
    let spacing = ((recall_days - 1) as f32).ln_1p() / 4_f32.ln_1p();
    let span_score = (span_days / 30.0).min(1.0);
    clamp01(0.55 * spacing + 0.45 * span_score)
}

/// Richness of semantic tags attached to the fact.
pub fn compute_conceptual(tag_count: u32) -> f32 {
    clamp01(tag_count as f32 / 6.0)
}

/// Importance within the knowledge graph.
pub fn compute_graph_centrality(edge_count: u32) -> f32 {
    clamp01((edge_count as f32).ln_1p() / 8_f32.ln_1p())
}

/// Proximity to the cluster centroid (tighter = more cohesive).
pub fn compute_cluster_cohesion(dist_to_centroid: f32, cluster_radius: f32) -> f32 {
    if cluster_radius <= 0.0 {
        return 0.0;
    }
    clamp01(1.0 - dist_to_centroid / cluster_radius)
}

// ── Scorer implementation ───────────────────────────────────────────

impl PromotionScorer {
    /// Weighted sum of the 8 dimension scores.
    pub fn score(&self, dims: &[f32; 8]) -> f32 {
        self.weights
            .iter()
            .zip(dims.iter())
            .map(|(w, d)| w * d)
            .sum()
    }

    /// Whether all promotion thresholds are satisfied.
    pub fn should_promote(
        &self,
        score: f32,
        signal_count: u32,
        unique_queries: u32,
        age_hours: u64,
    ) -> bool {
        score >= self.thresholds.min_score
            && signal_count >= self.thresholds.min_signal_count
            && unique_queries >= self.thresholds.min_unique_queries
            && age_hours >= self.thresholds.min_age_hours
    }
}

// ── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f32, b: f32, eps: f32) {
        assert!(
            (a - b).abs() < eps,
            "expected {b} ± {eps}, got {a}"
        );
    }

    #[test]
    fn default_weights_sum_to_one() {
        let scorer = PromotionScorer::default();
        let sum: f32 = scorer.weights.iter().sum();
        approx(sum, 1.0, 0.001);
    }

    #[test]
    fn score_all_zeros_returns_zero() {
        let scorer = PromotionScorer::default();
        assert_eq!(scorer.score(&[0.0; 8]), 0.0);
    }

    #[test]
    fn score_all_ones_returns_one() {
        let scorer = PromotionScorer::default();
        approx(scorer.score(&[1.0; 8]), 1.0, 0.001);
    }

    #[test]
    fn frequency_dimension() {
        approx(compute_frequency(5), 0.748, 0.01);
    }

    #[test]
    fn relevance_dimension() {
        approx(compute_relevance(4.5, 5), 0.9, 0.01);
    }

    #[test]
    fn diversity_dimension() {
        approx(compute_diversity(3, 2), 0.6, 0.01);
    }

    #[test]
    fn recency_dimension_fresh() {
        approx(compute_recency(0.0), 1.0, 0.01);
    }

    #[test]
    fn recency_dimension_14_days() {
        approx(compute_recency(14.0), 0.5, 0.01);
    }

    #[test]
    fn graph_centrality_dimension() {
        approx(compute_graph_centrality(4), 0.733, 0.01);
    }

    #[test]
    fn cluster_cohesion_at_centroid() {
        approx(compute_cluster_cohesion(0.0, 1.0), 1.0, 0.01);
    }

    #[test]
    fn cluster_cohesion_at_edge() {
        approx(compute_cluster_cohesion(1.0, 1.0), 0.0, 0.01);
    }

    #[test]
    fn cluster_cohesion_no_cluster() {
        approx(compute_cluster_cohesion(0.0, 0.0), 0.0, 0.01);
    }
}
