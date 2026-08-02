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
    /// mem0-style write-time semantic dedup. When enabled, a planned `Create`
    /// whose embedding is within `dedup_similarity_threshold` cosine of an
    /// existing related note is redirected into an `Append` so near-duplicate
    /// pages never enter the store (instead of waiting for the offline dream
    /// consolidator to merge them).
    #[serde(default = "super::defaults::default_dedup_enabled")]
    pub dedup_enabled: bool,
    #[serde(default = "super::defaults::default_dedup_similarity_threshold")]
    pub dedup_similarity_threshold: f32,
    #[serde(default = "super::defaults::default_dedup_noop_threshold")]
    pub dedup_noop_threshold: f32,
    /// Admission governance gate (Phase C2). When `true`, ingest routes each
    /// planned `PageOp::Create` through `DefaultNoteWriteGate`: low-confidence,
    /// high-severity-under-confidence, and contradicting creates are deferred
    /// into `notes_review_queue` for LLM re-adjudication (`NoteReviewStage`)
    /// next dream cycle instead of being admitted immediately. **Default
    /// `false`** — the mount point ships wired but dormant, so existing
    /// deployments keep byte-identical immediate-admit behaviour until opted in.
    #[serde(default = "super::defaults::default_governance_enabled")]
    pub governance_enabled: bool,
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
            dedup_enabled: true,
            dedup_similarity_threshold: 0.92,
            dedup_noop_threshold: 0.985,
            governance_enabled: false,
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

/// Consumed by the `MemoryContextProvider` factory
/// (`init_memory_context_provider_with_extensions` →
/// `MemoryContextProvider::with_curated_config`). Until that call existed every
/// construction path hardcoded `CuratedConfig::default()`, so both char limits
/// were unreachable from config no matter what the user wrote.
impl From<CuratedSection> for CuratedConfig {
    fn from(s: CuratedSection) -> Self {
        Self {
            memory_char_limit: s.memory_char_limit,
            user_char_limit: s.user_char_limit,
            legacy_warn_threshold: s.legacy_warn_threshold,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::CompoundIngestConfig;

    #[test]
    fn dedup_is_enabled_by_default() {
        assert!(super::super::defaults::default_dedup_enabled());
        assert_eq!(
            super::super::defaults::default_dedup_noop_threshold(),
            0.985
        );
        let cfg = CompoundIngestConfig::default();
        assert!(cfg.dedup_enabled);
        assert_eq!(cfg.dedup_noop_threshold, 0.985);
    }

    #[test]
    fn compound_ingest_config_parses_without_dedup_noop_key() {
        // Backward-compat: TOML lacking the new key falls back to the serde default.
        let cfg: CompoundIngestConfig = serde_json::from_str("{}").unwrap();
        assert_eq!(cfg.dedup_noop_threshold, 0.985);
        assert!(cfg.dedup_enabled);
    }

    #[test]
    fn governance_is_disabled_by_default() {
        // The gate ships wired but dormant: default off, and TOML lacking the
        // key falls back to off, so existing deployments keep immediate-admit.
        assert!(!super::super::defaults::default_governance_enabled());
        assert!(!CompoundIngestConfig::default().governance_enabled);
        let cfg: CompoundIngestConfig = serde_json::from_str("{}").unwrap();
        assert!(!cfg.governance_enabled);
    }
}
