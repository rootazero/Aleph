//! Retrieval-time scoring configuration (recency decay + MMR diversity).
//!
//! These knobs refine ranking *after* RRF fusion and any cross-encoder rerank,
//! mirroring what reference memory systems (hermes-agent, openclaw) apply at
//! recall time.
//!
//! `recency` and `reinforcement` default **on**: they realise the advertised
//! "热门记忆浮顶 / 冷门自然沉底 / 时间衰减" behaviour, which must work out of
//! the box (the feature is "自动冒泡"). Both read data already present
//! (`updated_at`, `recall_signals` hit counts) — no extra LLM/embedding calls —
//! and use sub-linear, conservatively-weighted blends so they nudge ordering
//! without ever dominating raw relevance. `mmr` (diversity de-dup) stays
//! opt-out because dropping near-duplicates can surprise callers that expect
//! every match returned. Setting a field to `false` restores the legacy path.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

fn default_recency_half_life_days() -> f32 {
    90.0
}

/// Default-on switch for `recency_enabled` / `reinforcement_enabled` so the
/// hot-surfacing + time-decay ranking is active without explicit config.
fn default_scoring_enabled() -> bool {
    true
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
    /// notes outrank equally-relevant stale ones. Default `true` ("时间衰减" —
    /// stale memories fade in ranking). Set `false` for the legacy path.
    #[serde(default = "default_scoring_enabled")]
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
    /// calls. Default `true` ("热门记忆浮顶" — frequently-recalled notes bubble
    /// up, cold ones sink). Set `false` for the legacy path. Inspired by memU's
    /// `sim × log(reinforcement+1)` salience.
    #[serde(default = "default_scoring_enabled")]
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
            recency_enabled: default_scoring_enabled(),
            recency_half_life_days: default_recency_half_life_days(),
            recency_weight: default_recency_weight(),
            mmr_enabled: false,
            mmr_lambda: default_mmr_lambda(),
            reinforcement_enabled: default_scoring_enabled(),
            reinforcement_weight: default_reinforcement_weight(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_enables_recency_and_reinforcement() {
        // Hot-surfacing + time-decay must be active out of the box; MMR stays
        // opt-out.
        let cfg = RetrievalScoringConfig::default();
        assert!(cfg.is_active());
        assert!(cfg.recency_enabled);
        assert!(cfg.reinforcement_enabled);
        assert!(!cfg.mmr_enabled);
    }

    #[test]
    fn fully_disabled_is_inactive() {
        let cfg = RetrievalScoringConfig {
            recency_enabled: false,
            reinforcement_enabled: false,
            mmr_enabled: false,
            ..Default::default()
        };
        assert!(!cfg.is_active());
    }

    #[test]
    fn active_when_any_enabled() {
        let cfg = RetrievalScoringConfig {
            mmr_enabled: true,
            ..Default::default()
        };
        assert!(cfg.is_active());
    }
}
