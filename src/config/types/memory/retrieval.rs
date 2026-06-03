//! Retrieval-time scoring configuration (recency decay + MMR diversity).
//!
//! These knobs refine ranking *after* RRF fusion and any cross-encoder rerank,
//! mirroring what reference memory systems (hermes-agent, openclaw) apply at
//! recall time. Every field defaults to "off" / identity so an unconfigured
//! deployment behaves byte-for-byte like the pre-feature retrieval path.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

fn default_recency_half_life_days() -> f32 {
    90.0
}

fn default_recency_weight() -> f32 {
    0.3
}

fn default_mmr_lambda() -> f32 {
    0.7
}

fn default_reinforcement_weight() -> f32 {
    0.3
}

/// Configuration for retrieval-time score adjustments applied by
/// [`crate::memory::note_retrieval::NoteFactRetrieval`].
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct RetrievalScoringConfig {
    /// Re-weight relevance scores by an exponential recency multiplier so fresh
    /// notes outrank equally-relevant stale ones. Default `false` (no change).
    #[serde(default)]
    pub recency_enabled: bool,

    /// Half-life (in days) for the recency multiplier `0.5^(age / half_life)`.
    #[serde(default = "default_recency_half_life_days")]
    pub recency_half_life_days: f32,

    /// Blend strength of recency in `[0,1]`: `score * (1 - w + w * mult)`.
    /// `0.0` leaves scores untouched; `1.0` fully multiplies by the multiplier.
    #[serde(default = "default_recency_weight")]
    pub recency_weight: f32,

    /// Reorder results with Maximal Marginal Relevance (token-Jaccard proxy) to
    /// demote near-duplicate notes. Default `false` (pure relevance ordering).
    #[serde(default)]
    pub mmr_enabled: bool,

    /// MMR trade-off in `[0,1]`: `1.0` = pure relevance, `0.0` = pure diversity.
    #[serde(default = "default_mmr_lambda")]
    pub mmr_lambda: f32,

    /// Boost notes by how often they have been recalled (reinforcement salience),
    /// so a frequently-retrieved note outranks an equally-relevant never-touched
    /// one. Reads the already-recorded `recall_signals` counts — no extra LLM
    /// calls. Default `false` (no change). Inspired by memU's
    /// `sim × log(reinforcement+1)` salience.
    #[serde(default)]
    pub reinforcement_enabled: bool,

    /// Blend strength of reinforcement in `[0,1]`: `score * (1 + w * ln(1+hits))`.
    /// `0.0` leaves scores untouched; higher values let recall frequency nudge
    /// ordering. The `ln(1+hits)` shape grows sub-linearly so a note recalled 50
    /// times never dominates raw relevance.
    #[serde(default = "default_reinforcement_weight")]
    pub reinforcement_weight: f32,
}

impl RetrievalScoringConfig {
    /// True when at least one refinement is active — lets the retrieval engine
    /// skip the over-fetch + reordering work entirely in the default config.
    pub fn is_active(&self) -> bool {
        self.recency_enabled || self.mmr_enabled || self.reinforcement_enabled
    }
}

impl Default for RetrievalScoringConfig {
    fn default() -> Self {
        Self {
            recency_enabled: false,
            recency_half_life_days: default_recency_half_life_days(),
            recency_weight: default_recency_weight(),
            mmr_enabled: false,
            mmr_lambda: default_mmr_lambda(),
            reinforcement_enabled: false,
            reinforcement_weight: default_reinforcement_weight(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_inactive() {
        assert!(!RetrievalScoringConfig::default().is_active());
    }

    #[test]
    fn active_when_any_enabled() {
        let cfg = RetrievalScoringConfig {
            recency_enabled: true,
            ..Default::default()
        };
        assert!(cfg.is_active());
        let cfg = RetrievalScoringConfig {
            mmr_enabled: true,
            ..Default::default()
        };
        assert!(cfg.is_active());
        let cfg = RetrievalScoringConfig {
            reinforcement_enabled: true,
            ..Default::default()
        };
        assert!(cfg.is_active());
    }
}
