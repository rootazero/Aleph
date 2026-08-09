use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct DreamingConfig {
    #[serde(default = "super::defaults::default_dreaming_enabled")]
    pub enabled: bool,
    #[serde(default = "super::defaults::default_dreaming_window_start")]
    pub window_start_local: String,
    #[serde(default = "super::defaults::default_dreaming_window_end")]
    pub window_end_local: String,
    #[serde(default = "super::defaults::default_dreaming_idle_threshold_seconds")]
    pub idle_threshold_seconds: u32,
    #[serde(default = "super::defaults::default_dreaming_max_duration_seconds")]
    pub max_duration_seconds: u32,
    #[serde(default = "super::defaults::default_drift_max_pairs_per_run")]
    pub drift_max_pairs_per_run: usize,
    /// Ceiling on `NoteConsolidate` pairs judged per corpus per night. See
    /// [`super::defaults::default_consolidate_max_pairs_per_run`].
    #[serde(default = "super::defaults::default_consolidate_max_pairs_per_run")]
    pub consolidate_max_pairs_per_run: usize,
    #[serde(default = "super::defaults::default_skill_distill_max_per_cycle")]
    pub skill_distill_max_per_cycle: usize,
    /// Days of inactivity before a skill is mechanically aged from
    /// `Active` to `Stale` by `SkillLifecycleStage`. Pinned skills are
    /// exempt. The `Stale → Archived` decision is reserved for a future
    /// LLM-driven curator stage; this knob only governs the deterministic
    /// side. Mirrors hermes-agent's `DEFAULT_STALE_AFTER_DAYS = 30`.
    #[serde(default = "super::defaults::default_skill_stale_after_days")]
    pub skill_stale_after_days: u32,
    #[serde(default = "super::defaults::default_feedback_distill_max_per_cycle")]
    pub feedback_distill_max_per_cycle: usize,
    #[serde(default = "super::defaults::default_feedback_distill_min_candidates")]
    pub feedback_distill_min_candidates: usize,
    #[serde(default = "super::defaults::default_feedback_lookback")]
    pub feedback_lookback: usize,
    /// How many non-base note corpora one night of maintenance may run
    /// (default: 8).
    ///
    /// The nightly fan-out visits every corpus on disk; each visit costs its
    /// own LLM stages, so this is the direct dial on unattended nightly spend.
    /// Corpora that do not fit tonight are not lost — they wait for the next
    /// window, and a corpus with nothing changed is skipped for free.
    #[serde(default = "super::defaults::default_dream_max_corpus_cycles_per_night")]
    pub max_corpus_cycles_per_night: usize,
    /// Optional dedicated model for the dream pipeline's LLM stages.
    ///
    /// Every LLM stage here is a small classification or summarization task
    /// (`CONSISTENT | CONTRADICTORY | STALE`, digest text, …) — cheap-tier work
    /// that has no business running on the operator's main reasoning model.
    /// Unset ⇒ fall back to the primary provider's declared cheap aux model;
    /// only a vendor with no cheap tier keeps using the main LLM.
    #[serde(default)]
    pub model: Option<String>,
}

impl Default for DreamingConfig {
    fn default() -> Self {
        Self {
            enabled: super::defaults::default_dreaming_enabled(),
            window_start_local: super::defaults::default_dreaming_window_start(),
            window_end_local: super::defaults::default_dreaming_window_end(),
            idle_threshold_seconds: super::defaults::default_dreaming_idle_threshold_seconds(),
            max_duration_seconds: super::defaults::default_dreaming_max_duration_seconds(),
            drift_max_pairs_per_run: super::defaults::default_drift_max_pairs_per_run(),
            consolidate_max_pairs_per_run: super::defaults::default_consolidate_max_pairs_per_run(),
            skill_distill_max_per_cycle: super::defaults::default_skill_distill_max_per_cycle(),
            skill_stale_after_days: super::defaults::default_skill_stale_after_days(),
            feedback_distill_max_per_cycle: super::defaults::default_feedback_distill_max_per_cycle(
            ),
            feedback_distill_min_candidates:
                super::defaults::default_feedback_distill_min_candidates(),
            feedback_lookback: super::defaults::default_feedback_lookback(),
            max_corpus_cycles_per_night: super::defaults::default_dream_max_corpus_cycles_per_night(
            ),
            model: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MemoryDecayPolicy {
    #[serde(default = "super::defaults::default_memory_decay_half_life_days")]
    pub half_life_days: f32,
    #[serde(default = "super::defaults::default_memory_decay_min_strength")]
    pub min_strength: f32,
    #[serde(default = "super::defaults::default_memory_decay_protected_types")]
    pub protected_types: Vec<String>,
}

impl Default for MemoryDecayPolicy {
    fn default() -> Self {
        Self {
            half_life_days: super::defaults::default_memory_decay_half_life_days(),
            min_strength: super::defaults::default_memory_decay_min_strength(),
            protected_types: super::defaults::default_memory_decay_protected_types(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The knob is operator-settable and unset siblings keep their defaults.
    ///
    /// (There is deliberately no guard tying this default to
    /// `memory::dreaming::MAX_CORPUS_CYCLES_PER_NIGHT`: the default function
    /// *is* that constant, so the compiler already enforces what a
    /// source-scanning guard used to approximate.)
    #[test]
    fn the_nightly_corpus_budget_is_operator_overridable() {
        let parsed: DreamingConfig =
            toml::from_str("max_corpus_cycles_per_night = 2").expect("parses");
        assert_eq!(parsed.max_corpus_cycles_per_night, 2);
        assert_eq!(
            parsed.feedback_lookback,
            crate::config::types::memory::defaults::default_feedback_lookback()
        );
    }
}
