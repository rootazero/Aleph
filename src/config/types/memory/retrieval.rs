//! Retrieval-time scoring configuration (recency decay + MMR diversity).
//!
//! These knobs refine ranking *after* RRF fusion and any cross-encoder rerank,
//! mirroring what reference memory systems (hermes-agent, openclaw) apply at
//! recall time.
//!
//! `recency` and `reinforcement` default **on**: they realise the advertised
//! "hot memories bubble up / cold ones sink naturally / time-decay" behaviour, which must work out of
//! the box (the feature is "automatic bubbling"). Both read data already present
//! (`updated_at`, `recall_signals` hit counts) — no extra LLM/embedding calls —
//! and use sub-linear, conservatively-weighted blends so they nudge ordering
//! without ever dominating raw relevance. `mmr` (diversity de-dup) stays
//! opt-out because dropping near-duplicates can surprise callers that expect
//! every match returned. Setting a field to `false` restores the legacy path.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

const fn default_recency_half_life_days() -> f32 {
    90.0
}

/// Default-on switch for `recency_enabled` / `reinforcement_enabled` so the
/// hot-surfacing + time-decay ranking is active without explicit config.
const fn default_scoring_enabled() -> bool {
    true
}

const fn default_recency_weight() -> f32 {
    0.3
}

const fn default_mmr_lambda() -> f32 {
    0.7
}

const fn default_reinforcement_weight() -> f32 {
    0.3
}

/// Configuration for retrieval-time score adjustments applied by
/// [`crate::memory::note_retrieval::NoteFactRetrieval`].
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct RetrievalScoringConfig {
    /// Re-weight relevance scores by an exponential recency multiplier so fresh
    /// notes outrank equally-relevant stale ones. Default `true` ("time-decay" —
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
    /// calls. Default `true` ("hot memories bubble up" — frequently-recalled notes bubble
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
    pub const fn is_active(&self) -> bool {
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

const fn default_expansion_enabled() -> bool {
    true
}
const fn default_max_seeds() -> usize {
    5
}
const fn default_peers_per_seed() -> usize {
    3
}
const fn default_max_expanded() -> usize {
    8
}
const fn default_expansion_weight() -> f32 {
    0.5
}

/// Associative graph expansion of the retrieval candidate pool. Pulls the
/// strongest 5-signal related peers of the top direct hits into the pool before
/// rerank, so notes tied to a match surface even without lexical/semantic
/// overlap. Default-on and conservative: a peer's propagated score is scaled
/// strictly below its seed, and a cold graph cache makes the stage a no-op.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ExpansionConfig {
    /// Master switch. Default `true`. `false` restores the legacy path (zero
    /// expansion, byte-for-byte).
    #[serde(default = "default_expansion_enabled")]
    pub enabled: bool,
    /// How many top hits seed expansion. Default 5.
    #[serde(default = "default_max_seeds")]
    pub max_seeds: usize,
    /// Related peers pulled per seed (passed to `related_peers`). Default 3.
    #[serde(default = "default_peers_per_seed")]
    pub peers_per_seed: usize,
    /// Hard cap on total expansion candidates added to the pool. Default 8.
    #[serde(default = "default_max_expanded")]
    pub max_expanded: usize,
    /// Propagation strength in `[0,1]`: a peer's score is
    /// `seed.score * weight * (edge / seed_top_edge)`. Default 0.5 (a peer maxes
    /// at half its seed's score). Clamped to `[0,1]` by `with_expansion_config`.
    #[serde(default = "default_expansion_weight")]
    pub weight: f32,
}

impl ExpansionConfig {
    /// True when expansion will do any work.
    #[must_use]
    pub const fn is_active(&self) -> bool {
        self.enabled && self.max_seeds > 0 && self.max_expanded > 0
    }
}

impl Default for ExpansionConfig {
    fn default() -> Self {
        Self {
            enabled: default_expansion_enabled(),
            max_seeds: default_max_seeds(),
            peers_per_seed: default_peers_per_seed(),
            max_expanded: default_max_expanded(),
            weight: default_expansion_weight(),
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

    #[test]
    fn expansion_default_is_on_and_active() {
        let c = ExpansionConfig::default();
        assert!(c.enabled);
        assert!(c.is_active());
        assert_eq!(c.max_seeds, 5);
        assert_eq!(c.peers_per_seed, 3);
        assert_eq!(c.max_expanded, 8);
        assert!((c.weight - 0.5).abs() < f32::EPSILON);
    }

    #[test]
    fn expansion_inactive_when_disabled_or_zero_caps() {
        assert!(!ExpansionConfig {
            enabled: false,
            ..Default::default()
        }
        .is_active());
        assert!(!ExpansionConfig {
            max_seeds: 0,
            ..Default::default()
        }
        .is_active());
        assert!(!ExpansionConfig {
            max_expanded: 0,
            ..Default::default()
        }
        .is_active());
    }
}
