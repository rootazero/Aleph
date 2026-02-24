//! Configuration for the scoring pipeline.
//!
//! Each field maps to one or more scoring stages. Sensible defaults
//! are provided so callers can use `ScoringPipelineConfig::default()`.

/// Configuration controlling every stage of the scoring pipeline.
#[derive(Debug, Clone)]
pub struct ScoringPipelineConfig {
    // -- CosineRerank --
    /// Blend factor between original score and cosine similarity.
    /// `score = (1 - blend) * original + blend * cosine_sim`.
    /// Range: [0.0, 1.0]. Default: 0.3.
    pub rerank_blend: f32,

    // -- RecencyBoost --
    /// Half-life in days for the recency exponential boost.
    /// Default: 14 days.
    pub recency_half_life_days: f64,
    /// Additive weight for the recency boost. Default: 0.1.
    pub recency_weight: f64,

    // -- LengthNormalization --
    /// Anchor length (in bytes/chars) below which no penalty is applied.
    /// Default: 500.
    pub length_norm_anchor: usize,

    // -- TimeDecay --
    /// Half-life in days for the multiplicative time decay.
    /// Default: 60 days.
    pub time_decay_half_life_days: f64,

    // -- HardMinScore --
    /// Minimum score threshold. Facts below this are discarded.
    /// Default: 0.35.
    pub hard_min_score: f32,

    // -- MmrDiversity --
    /// Cosine similarity threshold for MMR deduplication.
    /// Candidates more similar than this to an already-selected fact
    /// are demoted (appended at the end). Default: 0.85.
    pub mmr_similarity_threshold: f32,
}

impl Default for ScoringPipelineConfig {
    fn default() -> Self {
        Self {
            rerank_blend: 0.3,
            recency_half_life_days: 14.0,
            recency_weight: 0.1,
            length_norm_anchor: 500,
            time_decay_half_life_days: 60.0,
            hard_min_score: 0.35,
            mmr_similarity_threshold: 0.85,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_has_sensible_values() {
        let cfg = ScoringPipelineConfig::default();
        assert!((cfg.rerank_blend - 0.3).abs() < f32::EPSILON);
        assert!((cfg.recency_half_life_days - 14.0).abs() < f64::EPSILON);
        assert!((cfg.recency_weight - 0.1).abs() < f64::EPSILON);
        assert_eq!(cfg.length_norm_anchor, 500);
        assert!((cfg.time_decay_half_life_days - 60.0).abs() < f64::EPSILON);
        assert!((cfg.hard_min_score - 0.35).abs() < f32::EPSILON);
        assert!((cfg.mmr_similarity_threshold - 0.85).abs() < f32::EPSILON);
    }
}
