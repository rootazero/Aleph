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
}

impl RetrievalScoringConfig {
    /// True when at least one refinement is active — lets the retrieval engine
    /// skip the over-fetch + reordering work entirely in the default config.
    pub fn is_active(&self) -> bool {
        self.recency_enabled || self.mmr_enabled
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
        let mut cfg = RetrievalScoringConfig::default();
        cfg.recency_enabled = true;
        assert!(cfg.is_active());
        let mut cfg = RetrievalScoringConfig::default();
        cfg.mmr_enabled = true;
        assert!(cfg.is_active());
    }
}
