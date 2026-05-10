use crate::memory::curated::CuratedConfig;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CompoundIngestConfig {
    #[serde(default = "super::defaults::default_compound_enabled")]
    pub enabled: bool,
    #[serde(default = "super::defaults::default_max_related_pages")]
    pub max_related_pages: usize,
    #[serde(default = "super::defaults::default_related_preview_char_cap")]
    pub related_preview_char_cap: usize,
    #[serde(default = "super::defaults::default_related_total_byte_cap")]
    pub related_total_byte_cap: usize,
    #[serde(default = "super::defaults::default_replan_on_hash_conflict")]
    pub replan_on_hash_conflict: u32,
    #[serde(default = "super::defaults::default_failure_cooldown_seconds")]
    pub failure_cooldown_seconds: u64,
    #[serde(default = "super::defaults::default_tx_residue_gc_seconds")]
    pub tx_residue_gc_seconds: u64,
}

impl Default for CompoundIngestConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_related_pages: 15,
            related_preview_char_cap: 800,
            related_total_byte_cap: 12 * 1024,
            replan_on_hash_conflict: 1,
            failure_cooldown_seconds: 300,
            tx_residue_gc_seconds: 3600,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct QueryFilerConfig {
    #[serde(default = "super::defaults::default_qf_enabled")]
    pub enabled: bool,
    #[serde(default = "super::defaults::default_qf_min_sources")]
    pub min_sources: usize,
    #[serde(default = "super::defaults::default_qf_min_answer_chars")]
    pub min_answer_chars: usize,
    #[serde(default = "super::defaults::default_qf_llm_gate_enabled")]
    pub llm_gate_enabled: bool,
}

impl Default for QueryFilerConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            min_sources: 3,
            min_answer_chars: 200,
            llm_gate_enabled: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct CuratedSection {
    pub memory_char_limit: usize,
    pub user_char_limit: usize,
    pub legacy_warn_threshold: f32,
}

impl Default for CuratedSection {
    fn default() -> Self {
        let c = CuratedConfig::default();
        Self {
            memory_char_limit: c.memory_char_limit,
            user_char_limit: c.user_char_limit,
            legacy_warn_threshold: c.legacy_warn_threshold,
        }
    }
}

impl From<CuratedSection> for CuratedConfig {
    fn from(s: CuratedSection) -> Self {
        Self {
            memory_char_limit: s.memory_char_limit,
            user_char_limit: s.user_char_limit,
            legacy_warn_threshold: s.legacy_warn_threshold,
        }
    }
}
