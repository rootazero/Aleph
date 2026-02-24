//! Configuration for the scoring pipeline.
//!
//! Provides tuning knobs for each scoring stage. All fields have sensible
//! defaults and support partial deserialization (e.g. from TOML config files).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Configuration for the memory scoring pipeline.
///
/// Controls which stages are enabled and their parameters.
/// All fields carry `#[serde(default)]` so partial configs are accepted.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ScoringPipelineConfig {
    /// Whether the scoring pipeline is enabled at all.
    #[serde(default = "default_enabled")]
    pub enabled: bool,

    /// Blend weight for cosine-rerank interpolation (0.0 = pure vector, 1.0 = pure rerank).
    #[serde(default = "default_rerank_blend")]
    pub rerank_blend: f32,

    /// Half-life in days for the recency boost exponential decay.
    #[serde(default = "default_recency_half_life_days")]
    pub recency_half_life_days: f32,

    /// Weight of the recency boost component in the final score.
    #[serde(default = "default_recency_weight")]
    pub recency_weight: f32,

    /// Anchor length (in characters) for length normalization.
    /// Facts shorter or longer than this are penalised.
    #[serde(default = "default_length_norm_anchor")]
    pub length_norm_anchor: usize,

    /// Half-life in days for the general time-decay stage.
    #[serde(default = "default_time_decay_half_life_days")]
    pub time_decay_half_life_days: f32,

    /// Hard minimum score threshold. Facts below this are dropped.
    #[serde(default = "default_hard_min_score")]
    pub hard_min_score: f32,

    /// Cosine-similarity threshold for MMR diversity de-duplication.
    #[serde(default = "default_mmr_similarity_threshold")]
    pub mmr_similarity_threshold: f32,
}

// -- default value functions ------------------------------------------------

fn default_enabled() -> bool {
    true
}

fn default_rerank_blend() -> f32 {
    0.3
}

fn default_recency_half_life_days() -> f32 {
    14.0
}

fn default_recency_weight() -> f32 {
    0.1
}

fn default_length_norm_anchor() -> usize {
    500
}

fn default_time_decay_half_life_days() -> f32 {
    60.0
}

fn default_hard_min_score() -> f32 {
    0.35
}

fn default_mmr_similarity_threshold() -> f32 {
    0.85
}

impl Default for ScoringPipelineConfig {
    fn default() -> Self {
        Self {
            enabled: default_enabled(),
            rerank_blend: default_rerank_blend(),
            recency_half_life_days: default_recency_half_life_days(),
            recency_weight: default_recency_weight(),
            length_norm_anchor: default_length_norm_anchor(),
            time_decay_half_life_days: default_time_decay_half_life_days(),
            hard_min_score: default_hard_min_score(),
            mmr_similarity_threshold: default_mmr_similarity_threshold(),
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_values_are_correct() {
        let cfg = ScoringPipelineConfig::default();
        assert!(cfg.enabled);
        assert!((cfg.rerank_blend - 0.3).abs() < f32::EPSILON);
        assert!((cfg.recency_half_life_days - 14.0).abs() < f32::EPSILON);
        assert!((cfg.recency_weight - 0.1).abs() < f32::EPSILON);
        assert_eq!(cfg.length_norm_anchor, 500);
        assert!((cfg.time_decay_half_life_days - 60.0).abs() < f32::EPSILON);
        assert!((cfg.hard_min_score - 0.35).abs() < f32::EPSILON);
        assert!((cfg.mmr_similarity_threshold - 0.85).abs() < f32::EPSILON);
    }

    #[test]
    fn partial_toml_deserialization_uses_defaults() {
        let toml_str = r#"
            rerank_blend = 0.5
            hard_min_score = 0.2
        "#;
        let cfg: ScoringPipelineConfig = toml::from_str(toml_str).unwrap();
        // Overridden values
        assert!((cfg.rerank_blend - 0.5).abs() < f32::EPSILON);
        assert!((cfg.hard_min_score - 0.2).abs() < f32::EPSILON);
        // Defaults preserved
        assert!(cfg.enabled);
        assert!((cfg.recency_half_life_days - 14.0).abs() < f32::EPSILON);
        assert!((cfg.recency_weight - 0.1).abs() < f32::EPSILON);
        assert_eq!(cfg.length_norm_anchor, 500);
        assert!((cfg.time_decay_half_life_days - 60.0).abs() < f32::EPSILON);
        assert!((cfg.mmr_similarity_threshold - 0.85).abs() < f32::EPSILON);
    }

    #[test]
    fn empty_toml_deserialization_gives_all_defaults() {
        let cfg: ScoringPipelineConfig = toml::from_str("").unwrap();
        let def = ScoringPipelineConfig::default();
        assert_eq!(cfg.enabled, def.enabled);
        assert!((cfg.rerank_blend - def.rerank_blend).abs() < f32::EPSILON);
        assert!((cfg.recency_half_life_days - def.recency_half_life_days).abs() < f32::EPSILON);
        assert!((cfg.recency_weight - def.recency_weight).abs() < f32::EPSILON);
        assert_eq!(cfg.length_norm_anchor, def.length_norm_anchor);
        assert!((cfg.time_decay_half_life_days - def.time_decay_half_life_days).abs() < f32::EPSILON);
        assert!((cfg.hard_min_score - def.hard_min_score).abs() < f32::EPSILON);
        assert!((cfg.mmr_similarity_threshold - def.mmr_similarity_threshold).abs() < f32::EPSILON);
    }

    #[test]
    fn full_toml_round_trip() {
        let cfg = ScoringPipelineConfig {
            enabled: false,
            rerank_blend: 0.7,
            recency_half_life_days: 7.0,
            recency_weight: 0.2,
            length_norm_anchor: 300,
            time_decay_half_life_days: 30.0,
            hard_min_score: 0.5,
            mmr_similarity_threshold: 0.9,
        };
        let serialized = toml::to_string(&cfg).unwrap();
        let deserialized: ScoringPipelineConfig = toml::from_str(&serialized).unwrap();
        assert!(!deserialized.enabled);
        assert!((deserialized.rerank_blend - 0.7).abs() < f32::EPSILON);
        assert_eq!(deserialized.length_norm_anchor, 300);
    }
}
