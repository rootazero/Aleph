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
#[cfg(test)]
mod tests;

pub use assembler::{AssemblerConfig, AssemblyLogConfig, FallbackSkeleton};
pub use dreaming::{DreamingConfig, MemoryDecayPolicy};
pub use embed::{EmbeddingPreset, EmbeddingProviderConfig, EmbeddingSettings};
pub use ingest::{CompoundIngestConfig, CuratedSection, QueryFilerConfig};
pub use orientation::OrientationConfig;
pub use profile::UserProfileConfig;
pub use reflection::ReflectionConfig;
pub use defaults::*;

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

/// Memory module configuration
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MemoryConfig {
    #[serde(default = "defaults::default_enabled")]
    pub enabled: bool,
    #[serde(default = "defaults::default_max_context_items")]
    pub max_context_items: u32,
    #[serde(default = "defaults::default_retention_days")]
    pub retention_days: u32,
    #[serde(default = "defaults::default_vector_db")]
    pub vector_db: String,
    #[serde(default = "defaults::default_similarity_threshold")]
    pub similarity_threshold: f32,

    #[serde(default = "defaults::default_compression_enabled")]
    pub compression_enabled: bool,
    #[serde(default = "defaults::default_compression_idle_timeout")]
    pub compression_idle_timeout_seconds: u32,
    #[serde(default = "defaults::default_compression_turn_threshold")]
    pub compression_turn_threshold: u32,
    #[serde(default = "defaults::default_compression_interval")]
    pub compression_interval_seconds: u32,
    #[serde(default = "defaults::default_compression_batch_size")]
    pub compression_batch_size: u32,
    #[serde(default = "defaults::default_max_facts_in_context")]
    pub max_facts_in_context: u32,
    #[serde(default = "defaults::default_raw_memory_fallback_count")]
    pub raw_memory_fallback_count: u32,

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

    #[serde(default = "defaults::default_dedup_similarity_threshold")]
    pub dedup_similarity_threshold: f32,

    #[serde(default = "defaults::default_backup_enabled")]
    pub backup_enabled: bool,
    #[serde(default = "defaults::default_backup_max_files")]
    pub backup_max_files: usize,

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
}

impl Default for MemoryConfig {
    fn default() -> Self {
        Self {
            enabled: defaults::default_enabled(),
            max_context_items: defaults::default_max_context_items(),
            retention_days: defaults::default_retention_days(),
            vector_db: defaults::default_vector_db(),
            similarity_threshold: defaults::default_similarity_threshold(),
            compression_enabled: defaults::default_compression_enabled(),
            compression_idle_timeout_seconds: defaults::default_compression_idle_timeout(),
            compression_turn_threshold: defaults::default_compression_turn_threshold(),
            compression_interval_seconds: defaults::default_compression_interval(),
            compression_batch_size: defaults::default_compression_batch_size(),
            max_facts_in_context: defaults::default_max_facts_in_context(),
            raw_memory_fallback_count: defaults::default_raw_memory_fallback_count(),
            embedding: EmbeddingSettings::default(),
            dreaming: DreamingConfig::default(),
            memory_decay: MemoryDecayPolicy::default(),
            rrf_k: defaults::default_rrf_k(),
            bm25_bonus_weight: defaults::default_bm25_bonus(),
            rerank: crate::memory::rerank::RerankConfig::default(),
            dedup_similarity_threshold: defaults::default_dedup_similarity_threshold(),
            backup_enabled: defaults::default_backup_enabled(),
            backup_max_files: defaults::default_backup_max_files(),
            reflection: ReflectionConfig::default(),
            assembler: AssemblerConfig::default(),
            injection_mode: MemoryInjectionMode::Hybrid,
            orientation: OrientationConfig::default(),
            compound_ingest: CompoundIngestConfig::default(),
            profile: UserProfileConfig::default(),
            query_filer: QueryFilerConfig::default(),
            curated: CuratedSection::default(),
        }
    }
}
