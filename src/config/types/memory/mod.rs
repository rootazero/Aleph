use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub mod assembler;
pub mod defaults;
pub mod dreaming;
pub mod embed;
pub mod ingest;
pub mod orientation;
pub mod profile;
pub mod reflection;
pub mod retrieval;
#[cfg(test)]
mod tests;

pub use assembler::{AssemblerConfig, FallbackSkeleton};
pub use defaults::*;
pub use dreaming::{DreamingConfig, MemoryDecayPolicy};
pub use embed::{EmbeddingPreset, EmbeddingProviderConfig, EmbeddingSettings};
pub use ingest::{CompoundIngestConfig, CuratedSection, QueryFilerConfig};
pub use orientation::OrientationConfig;
pub use profile::UserProfileConfig;
pub use reflection::ReflectionConfig;
pub use retrieval::{ExpansionConfig, RetrievalScoringConfig};

/// Controls how memory is surfaced to the LLM.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
#[derive(Default)]
pub enum MemoryInjectionMode {
    Context,
    Tools,
    #[default]
    Hybrid,
}

/// Background reconciler daemon config (event-log \u2194 notes filesystem
/// divergence scan).
///
/// The daemon walks every fact_id in `memory_events` and compares the
/// expected file path against the notes filesystem, surfacing
/// divergence as `tracing::warn!` lines + an introspection endpoint
/// (`/v1/admin/reconciler/latest`). Off by default — the scan is
/// non-trivial on installations with millions of rows so the operator
/// must explicitly opt in via `[memory.reconciler] enabled = true`.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema)]
pub struct ReconcilerConfig {
    /// Start the daemon on boot. First scan runs immediately on
    /// spawn (not after a full interval) so the operator can confirm
    /// wiring without waiting.
    #[serde(default)]
    pub enabled: bool,
    /// Cadence between scans, in seconds. Recommended: minutes, not
    /// seconds — see `default_reconciler_interval_secs`.
    #[serde(default = "defaults::default_reconciler_interval_secs")]
    pub interval_secs: u64,
}

impl Default for ReconcilerConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            interval_secs: defaults::default_reconciler_interval_secs(),
        }
    }
}

/// Memory module configuration
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MemoryConfig {
    #[serde(default = "defaults::default_enabled")]
    pub enabled: bool,
    #[serde(default = "defaults::default_vector_db")]
    pub vector_db: String,
    // A top-level `similarity_threshold` used to live here. It was cut rather
    // than wired: retrieval ranks on RRF-fused rank scores (and rank-derived
    // FTS scores), so no quantity with honest "similarity in [0,1]" semantics
    // exists for such a gate to act on. Stray keys in existing config.toml
    // files are silently ignored (no `deny_unknown_fields`).
    #[serde(default)]
    pub embedding: EmbeddingSettings,

    #[serde(default)]
    pub dreaming: DreamingConfig,
    #[serde(default)]
    pub memory_decay: MemoryDecayPolicy,

    #[serde(default = "defaults::default_rrf_k")]
    pub rrf_k: u32,
    #[serde(default = "defaults::default_bm25_bonus")]
    pub bm25_bonus_weight: f32,
    #[serde(default)]
    pub rerank: crate::memory::rerank::RerankConfig,

    /// Retrieval-time recency decay + MMR diversity (default: both off →
    /// byte-for-byte legacy ranking).
    #[serde(default)]
    pub retrieval_scoring: RetrievalScoringConfig,

    /// Associative 5-signal graph expansion of the retrieval candidate pool
    /// (default-on; cold cache = no-op). Surfaces notes tied to a hit even
    /// without lexical/semantic overlap.
    #[serde(default)]
    pub expansion: ExpansionConfig,

    // Write-time semantic dedup is owned by
    // `[memory.compound_ingest] dedup_similarity_threshold` (`ingest.rs`).
    // A top-level twin of that key used to live here with zero readers; do
    // not reintroduce it — two spellings of one knob means one is a lie.
    #[serde(default)]
    pub reflection: ReflectionConfig,

    #[serde(default)]
    pub assembler: AssemblerConfig,

    #[serde(default)]
    pub injection_mode: MemoryInjectionMode,

    #[serde(default)]
    pub orientation: OrientationConfig,

    #[serde(default)]
    pub compound_ingest: CompoundIngestConfig,

    #[serde(default)]
    pub profile: UserProfileConfig,

    #[serde(default)]
    pub query_filer: QueryFilerConfig,

    #[serde(default)]
    pub curated: CuratedSection,

    /// Background reconciler daemon (event-log \u2194 notes filesystem).
    /// Disabled by default; opt-in via `[memory.reconciler] enabled = true`.
    #[serde(default)]
    pub reconciler: ReconcilerConfig,

    /// Isolate memory per project directory (Claude-Code-style workspaces).
    ///
    /// When `true` and a project root is active for the run, general notes and
    /// raw captures are partitioned by the project (via
    /// [`crate::memory::project_scope`]) so work in one project does not bleed
    /// into another; reads union the project's memories with the agent's global
    /// knowledge, and the always-on profile/feedback floors stay global.
    ///
    /// Default `false` — byte-for-byte the pre-feature single-namespace
    /// behaviour, so existing memory stores are unaffected.
    #[serde(default)]
    pub project_scoped: bool,
}

impl MemoryConfig {
    /// The assembler config with the top-level `project_scoped` toggle folded
    /// in, so callers building a `HybridAssembler` get a single source of truth
    /// for project namespacing (the user sets `memory.project_scoped`, not the
    /// nested `memory.assembler.project_scoped`).
    #[must_use]
    pub fn assembler_config(&self) -> AssemblerConfig {
        let mut cfg = self.assembler.clone();
        cfg.project_scoped = self.project_scoped;
        // Fold the top-level retrieval refinements in so the proactive
        // memory-context path applies the same recency / reinforcement / MMR
        // and cross-encoder rerank as the on-demand `memory_search` tool. The
        // user sets `memory.retrieval_scoring` / `memory.rerank`, not the
        // nested assembler copies.
        cfg.retrieval_scoring = self.retrieval_scoring.clone();
        cfg.rerank = self.rerank.clone();
        cfg.expansion = self.expansion.clone();
        cfg
    }
}

impl Default for MemoryConfig {
    fn default() -> Self {
        Self {
            enabled: defaults::default_enabled(),
            vector_db: defaults::default_vector_db(),
            embedding: EmbeddingSettings::default(),
            dreaming: DreamingConfig::default(),
            memory_decay: MemoryDecayPolicy::default(),
            rrf_k: defaults::default_rrf_k(),
            bm25_bonus_weight: defaults::default_bm25_bonus(),
            rerank: crate::memory::rerank::RerankConfig::default(),
            retrieval_scoring: RetrievalScoringConfig::default(),
            expansion: ExpansionConfig::default(),
            reflection: ReflectionConfig::default(),
            assembler: AssemblerConfig::default(),
            injection_mode: MemoryInjectionMode::Hybrid,
            orientation: OrientationConfig::default(),
            compound_ingest: CompoundIngestConfig::default(),
            profile: UserProfileConfig::default(),
            query_filer: QueryFilerConfig::default(),
            curated: CuratedSection::default(),
            reconciler: ReconcilerConfig::default(),
            project_scoped: false,
        }
    }
}
