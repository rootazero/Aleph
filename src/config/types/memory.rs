//! Memory configuration types
//!
//! Contains Memory/RAG module configuration:
//! - MemoryConfig: Vector database, embedding, and compression settings

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

// =============================================================================
// MemoryInjectionMode
// =============================================================================

/// Controls how memory is surfaced to the LLM.
///
/// - `Context`: auto-inject retrieved memory as a fenced user-message; no memory tools exposed.
/// - `Tools`: no auto-inject; LLM must call `memory_*` tools to retrieve.
/// - `Hybrid` (default): both — memory is auto-injected AND tools are available.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum MemoryInjectionMode {
    Context,
    Tools,
    Hybrid,
}

impl Default for MemoryInjectionMode {
    fn default() -> Self {
        Self::Hybrid
    }
}

// =============================================================================
// MemoryConfig
// =============================================================================

/// Memory module configuration
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MemoryConfig {
    /// Enable/disable memory module
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    /// Maximum number of past interactions to retrieve
    #[serde(default = "default_max_context_items")]
    pub max_context_items: u32,
    /// Auto-delete memories older than N days (0 = never delete)
    #[serde(default = "default_retention_days")]
    pub retention_days: u32,
    /// Vector database backend: "sqlite-vec"
    #[serde(default = "default_vector_db")]
    pub vector_db: String,
    /// Minimum similarity score to include memory (0.0-1.0)
    #[serde(default = "default_similarity_threshold")]
    pub similarity_threshold: f32,

    // AI-based memory retrieval settings
    /// Enable AI-based memory retrieval (replaces embedding similarity)
    #[serde(default = "default_ai_retrieval_enabled")]
    pub ai_retrieval_enabled: bool,
    /// Timeout for AI memory retrieval in milliseconds
    #[serde(default = "default_ai_retrieval_timeout_ms")]
    pub ai_retrieval_timeout_ms: u64,
    /// Maximum candidates to send to AI for selection
    #[serde(default = "default_ai_retrieval_max_candidates")]
    pub ai_retrieval_max_candidates: u32,
    /// Fallback count if AI selection fails
    #[serde(default = "default_ai_retrieval_fallback_count")]
    pub ai_retrieval_fallback_count: u32,

    // ========================================
    // Memory Compression Settings
    // ========================================
    /// Enable memory compression (LLM-based fact extraction)
    #[serde(default = "default_compression_enabled")]
    pub compression_enabled: bool,
    /// Idle timeout in seconds to trigger compression (default: 300 = 5 minutes)
    #[serde(default = "default_compression_idle_timeout")]
    pub compression_idle_timeout_seconds: u32,
    /// Number of conversation turns to trigger compression (default: 20)
    #[serde(default = "default_compression_turn_threshold")]
    pub compression_turn_threshold: u32,
    /// Background compression check interval in seconds (default: 3600 = 1 hour)
    #[serde(default = "default_compression_interval")]
    pub compression_interval_seconds: u32,
    /// Maximum memories to process per compression batch (default: 50)
    #[serde(default = "default_compression_batch_size")]
    pub compression_batch_size: u32,
    /// Similarity threshold for conflict detection (default: 0.85)
    #[serde(default = "default_conflict_similarity_threshold")]
    pub conflict_similarity_threshold: f32,
    /// Maximum facts to include in RAG context (default: 5)
    #[serde(default = "default_max_facts_in_context")]
    pub max_facts_in_context: u32,
    /// Maximum raw memories to fallback when facts insufficient (default: 3)
    #[serde(default = "default_raw_memory_fallback_count")]
    pub raw_memory_fallback_count: u32,

    // ========================================
    // Embedding Settings
    // ========================================
    /// Embedding provider configuration
    #[serde(default)]
    pub embedding: EmbeddingSettings,

    // ========================================
    // Dreaming + Memory Graph
    // ========================================
    /// DreamDaemon scheduling configuration
    #[serde(default)]
    pub dreaming: DreamingConfig,
    /// Memory fact decay policy
    #[serde(default)]
    pub memory_decay: MemoryDecayPolicy,

    // ========================================
    // Hybrid Retrieval & Reranking
    // ========================================
    /// RRF constant k.
    #[serde(default = "default_rrf_k")]
    pub rrf_k: u32,

    /// Extra BM25 bonus weight for RRF fusion.
    #[serde(default = "default_bm25_bonus")]
    pub bm25_bonus_weight: f32,

    /// Whether query expansion is enabled.
    #[serde(default)]
    pub query_expansion_enabled: bool,

    /// Cross-encoder reranking configuration.
    #[serde(default)]
    pub rerank: crate::memory::rerank::RerankConfig,

    // ========================================
    // Scoring, Retrieval Gate & Noise Filter
    // ========================================
    /// Scoring pipeline configuration.
    #[serde(default)]
    pub scoring_pipeline: crate::memory::scoring_pipeline::config::ScoringPipelineConfig,

    /// Noise filter configuration.
    #[serde(default)]
    pub noise_filter: crate::memory::noise_filter::NoiseFilterConfig,

    // ========================================
    // Storage & Cache
    // ========================================
    /// Storage deduplication similarity threshold (0.0-1.0).
    #[serde(default = "default_dedup_similarity_threshold")]
    pub dedup_similarity_threshold: f32,

    // ========================================
    // Backup
    // ========================================
    /// Enable automatic JSONL backup.
    #[serde(default = "default_backup_enabled")]
    pub backup_enabled: bool,

    /// Maximum number of backup files to retain.
    #[serde(default = "default_backup_max_files")]
    pub backup_max_files: usize,

    // ========================================
    // Reflection
    // ========================================
    /// Session-end reflection configuration.
    #[serde(default)]
    pub reflection: ReflectionConfig,

    // ========================================
    // Working Memory Assembler (Spec 1)
    // ========================================
    /// Working memory assembler configuration.
    #[serde(default)]
    pub assembler: AssemblerConfig,

    // ========================================
    // Memory Injection Mode (Spec 3)
    // ========================================
    /// How memory is surfaced to the LLM: context / tools / hybrid.
    #[serde(default)]
    pub injection_mode: MemoryInjectionMode,
}

// =============================================================================
// EmbeddingProviderConfig & EmbeddingSettings
// =============================================================================

/// Preset type for embedding providers
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum EmbeddingPreset {
    SiliconFlow,
    OpenAi,
    Ollama,
    Custom,
}

impl std::fmt::Display for EmbeddingPreset {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SiliconFlow => write!(f, "SiliconFlow"),
            Self::OpenAi => write!(f, "OpenAI"),
            Self::Ollama => write!(f, "Ollama"),
            Self::Custom => write!(f, "Custom"),
        }
    }
}

/// Configuration for a single embedding provider
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct EmbeddingProviderConfig {
    /// Unique identifier: "siliconflow", "openai", "ollama", "custom-xxx"
    pub id: String,
    /// Display name
    pub name: String,
    /// Preset type
    pub preset: EmbeddingPreset,
    /// API endpoint (e.g., "https://api.siliconflow.cn/v1")
    pub api_base: String,
    /// Runtime-only API key (populated from encrypted vault, never persisted to config.toml)
    #[serde(default, skip_serializing)]
    #[schemars(skip)]
    pub api_key: Option<String>,
    /// Model names (first entry is the active model)
    #[serde(
        deserialize_with = "crate::config::types::serde_helpers::deserialize_models",
        alias = "model"
    )]
    pub models: Vec<String>,
    /// Output vector dimensions
    pub dimensions: u32,
    /// Batch size for embedding requests
    #[serde(default = "default_embedding_batch_size")]
    pub batch_size: u32,
    /// Request timeout in milliseconds
    #[serde(default = "default_embedding_timeout_ms")]
    pub timeout_ms: u64,
    /// Whether this provider has been verified via a successful test connection
    #[serde(default)]
    pub verified: bool,
    /// Whether this provider is enabled
    #[serde(default = "default_provider_enabled")]
    pub enabled: bool,
}

fn default_provider_enabled() -> bool {
    true
}

impl EmbeddingProviderConfig {
    /// Returns the active model name (first entry in the models list).
    pub fn default_model(&self) -> &str {
        debug_assert!(!self.models.is_empty());
        &self.models[0]
    }

    /// Create a SiliconFlow preset
    pub fn siliconflow() -> Self {
        Self {
            id: "siliconflow".to_string(),
            name: "SiliconFlow".to_string(),
            preset: EmbeddingPreset::SiliconFlow,
            api_base: "https://api.siliconflow.cn/v1".to_string(),
            api_key: None,
            models: vec!["BAAI/bge-m3".to_string()],
            dimensions: 1024,
            batch_size: default_embedding_batch_size(),
            timeout_ms: default_embedding_timeout_ms(),
            verified: false,
            enabled: true,
        }
    }

    /// Create an OpenAI preset
    pub fn openai() -> Self {
        Self {
            id: "openai".to_string(),
            name: "OpenAI".to_string(),
            preset: EmbeddingPreset::OpenAi,
            api_base: "https://api.openai.com/v1".to_string(),
            api_key: None,
            models: vec!["text-embedding-3-small".to_string()],
            dimensions: 1536,
            batch_size: default_embedding_batch_size(),
            timeout_ms: default_embedding_timeout_ms(),
            verified: false,
            enabled: true,
        }
    }

    /// Create an Ollama preset
    pub fn ollama() -> Self {
        Self {
            id: "ollama".to_string(),
            name: "Ollama".to_string(),
            preset: EmbeddingPreset::Ollama,
            api_base: "http://localhost:11434/v1".to_string(),
            api_key: None,
            models: vec!["nomic-embed-text".to_string()],
            dimensions: 768,
            batch_size: default_embedding_batch_size(),
            timeout_ms: default_embedding_timeout_ms(),
            verified: false,
            enabled: true,
        }
    }
}

/// Top-level embedding settings with multi-provider support
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct EmbeddingSettings {
    /// Configured embedding providers
    #[serde(default = "default_embedding_providers")]
    pub providers: Vec<EmbeddingProviderConfig>,
    /// ID of the active provider
    #[serde(default = "default_active_provider_id")]
    pub active_provider_id: String,
}

impl Default for EmbeddingSettings {
    fn default() -> Self {
        Self {
            providers: default_embedding_providers(),
            active_provider_id: default_active_provider_id(),
        }
    }
}

fn default_embedding_providers() -> Vec<EmbeddingProviderConfig> {
    Vec::new()
}

fn default_active_provider_id() -> String {
    String::new()
}

// =============================================================================
// DreamingConfig
// =============================================================================

/// DreamDaemon scheduling configuration
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct DreamingConfig {
    /// Enable/disable DreamDaemon
    #[serde(default = "default_dreaming_enabled")]
    pub enabled: bool,
    /// Idle threshold before running (seconds)
    #[serde(default = "default_dreaming_idle_threshold_seconds")]
    pub idle_threshold_seconds: u32,
    /// Window start time (local, HH:MM)
    #[serde(default = "default_dreaming_window_start")]
    pub window_start_local: String,
    /// Window end time (local, HH:MM)
    #[serde(default = "default_dreaming_window_end")]
    pub window_end_local: String,
    /// Maximum duration per run (seconds)
    #[serde(default = "default_dreaming_max_duration_seconds")]
    pub max_duration_seconds: u32,
    /// Enable weekly deep synthesis
    #[serde(default = "default_weekly_enabled")]
    pub weekly_enabled: bool,
    /// Days between weekly synthesis runs
    #[serde(default = "default_weekly_interval_days")]
    pub weekly_interval_days: u32,
    /// Max drift pairs per dream run
    #[serde(default = "default_drift_max_pairs_per_run")]
    pub drift_max_pairs_per_run: usize,
    /// Minimum cluster size for synthesis
    #[serde(default = "default_synthesis_min_cluster_size")]
    pub synthesis_min_cluster_size: usize,
    /// Max insights per weekly synthesis
    #[serde(default = "default_synthesis_max_insights")]
    pub synthesis_max_insights: usize,
}

impl Default for DreamingConfig {
    fn default() -> Self {
        Self {
            enabled: default_dreaming_enabled(),
            idle_threshold_seconds: default_dreaming_idle_threshold_seconds(),
            window_start_local: default_dreaming_window_start(),
            window_end_local: default_dreaming_window_end(),
            max_duration_seconds: default_dreaming_max_duration_seconds(),
            weekly_enabled: default_weekly_enabled(),
            weekly_interval_days: default_weekly_interval_days(),
            drift_max_pairs_per_run: default_drift_max_pairs_per_run(),
            synthesis_min_cluster_size: default_synthesis_min_cluster_size(),
            synthesis_max_insights: default_synthesis_max_insights(),
        }
    }
}

impl DreamingConfig {
    pub fn weekly_enabled(&self) -> bool {
        self.weekly_enabled
    }
    pub fn weekly_interval_days(&self) -> u32 {
        self.weekly_interval_days
    }
    pub fn drift_max_pairs_per_run(&self) -> usize {
        self.drift_max_pairs_per_run
    }
    pub fn synthesis_min_cluster_size(&self) -> usize {
        self.synthesis_min_cluster_size
    }
    pub fn synthesis_max_insights(&self) -> usize {
        self.synthesis_max_insights
    }
}

// =============================================================================
// MemoryDecayPolicy
// =============================================================================

/// Decay policy for memory facts
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MemoryDecayPolicy {
    /// Half-life in days
    #[serde(default = "default_memory_decay_half_life_days")]
    pub half_life_days: f32,
    /// Access boost factor
    #[serde(default = "default_memory_decay_access_boost")]
    pub access_boost: f32,
    /// Minimum strength before pruning
    #[serde(default = "default_memory_decay_min_strength")]
    pub min_strength: f32,
    /// Protected fact types (strings like "personal")
    #[serde(default = "default_memory_decay_protected_types")]
    pub protected_types: Vec<String>,
}

impl Default for MemoryDecayPolicy {
    fn default() -> Self {
        Self {
            half_life_days: default_memory_decay_half_life_days(),
            access_boost: default_memory_decay_access_boost(),
            min_strength: default_memory_decay_min_strength(),
            protected_types: default_memory_decay_protected_types(),
        }
    }
}

// =============================================================================
// Default value functions for MemoryConfig
// =============================================================================

pub fn default_enabled() -> bool {
    true
}

pub fn default_max_context_items() -> u32 {
    5
}

pub fn default_retention_days() -> u32 {
    90
}

pub fn default_vector_db() -> String {
    "sqlite-vec".to_string()
}

pub fn default_similarity_threshold() -> f32 {
    0.3 // L2 distance in high-dim (1536) produces lower similarity via 1/(1+d)
}

pub fn default_ai_retrieval_enabled() -> bool {
    true // Use AI-based memory retrieval by default
}

pub fn default_ai_retrieval_timeout_ms() -> u64 {
    3000 // 3 seconds timeout for AI memory selection
}

pub fn default_ai_retrieval_max_candidates() -> u32 {
    20 // Send up to 20 recent memories to AI for selection
}

pub fn default_ai_retrieval_fallback_count() -> u32 {
    3 // Return 3 most recent memories if AI fails
}

// Compression configuration defaults
pub fn default_compression_enabled() -> bool {
    true
}

pub fn default_compression_idle_timeout() -> u32 {
    300 // 5 minutes
}

pub fn default_compression_turn_threshold() -> u32 {
    20
}

pub fn default_compression_interval() -> u32 {
    3600 // 1 hour
}

pub fn default_compression_batch_size() -> u32 {
    50
}

pub fn default_conflict_similarity_threshold() -> f32 {
    0.85
}

pub fn default_max_facts_in_context() -> u32 {
    5
}

pub fn default_raw_memory_fallback_count() -> u32 {
    3
}

// Embedding configuration defaults
pub fn default_embedding_timeout_ms() -> u64 {
    10000 // 10 seconds
}

pub fn default_embedding_batch_size() -> u32 {
    32
}

// Dreaming defaults
pub fn default_dreaming_enabled() -> bool {
    true
}

pub fn default_dreaming_idle_threshold_seconds() -> u32 {
    900 // 15 minutes
}

pub fn default_dreaming_window_start() -> String {
    "02:00".to_string()
}

pub fn default_dreaming_window_end() -> String {
    "05:00".to_string()
}

pub fn default_dreaming_max_duration_seconds() -> u32 {
    600 // 10 minutes
}

pub fn default_weekly_enabled() -> bool {
    true
}

pub fn default_weekly_interval_days() -> u32 {
    7
}

pub fn default_drift_max_pairs_per_run() -> usize {
    20
}

pub fn default_synthesis_min_cluster_size() -> usize {
    3
}

pub fn default_synthesis_max_insights() -> usize {
    10
}

// Memory decay defaults
pub fn default_memory_decay_half_life_days() -> f32 {
    30.0
}

pub fn default_memory_decay_access_boost() -> f32 {
    0.2
}

pub fn default_memory_decay_min_strength() -> f32 {
    0.1
}

pub fn default_memory_decay_protected_types() -> Vec<String> {
    vec!["personal".to_string()]
}

// =============================================================================
// ReflectionConfig
// =============================================================================

/// Session-end reflection configuration
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ReflectionConfig {
    /// Enable/disable session-end reflection
    #[serde(default)]
    pub enabled: bool,
    /// Minimum conversation turns before triggering reflection
    #[serde(default = "default_reflection_min_turns")]
    pub min_turns: u32,
    /// Minimum total user characters before triggering reflection
    #[serde(default = "default_reflection_min_chars")]
    pub min_user_chars: u32,
    /// Cooldown in minutes between reflections
    #[serde(default = "default_reflection_cooldown")]
    pub cooldown_minutes: u32,
    /// Enable open loop tracking
    #[serde(default)]
    pub open_loop_tracking: bool,
    /// Inject open loop prompt into next session
    #[serde(default)]
    pub open_loop_inject_prompt: bool,
}

impl Default for ReflectionConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            min_turns: default_reflection_min_turns(),
            min_user_chars: default_reflection_min_chars(),
            cooldown_minutes: default_reflection_cooldown(),
            open_loop_tracking: false,
            open_loop_inject_prompt: false,
        }
    }
}

fn default_reflection_min_turns() -> u32 {
    5
}

fn default_reflection_min_chars() -> u32 {
    200
}

fn default_reflection_cooldown() -> u32 {
    30
}

// =============================================================================
// AssemblerConfig (Memory Evolution Spec 1)
// =============================================================================

/// Working Memory Assembler configuration.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AssemblerConfig {
    #[serde(default = "default_assembler_enabled")]
    pub enabled: bool,
    #[serde(default = "default_total_budget")]
    pub total_budget_tokens: u32,
    #[serde(default = "default_pool_limit")]
    pub candidate_pool_limit: usize,
    #[serde(default = "default_rerank_timeout")]
    pub rerank_timeout_ms: u64,
    #[serde(default)]
    pub rerank_model: Option<String>,
    #[serde(default)]
    pub render_style: crate::memory::assembler::render::RenderStyle,
    #[serde(default)]
    pub force_fallback: bool,
    #[serde(default)]
    pub fallback_skeleton: FallbackSkeleton,
    #[serde(default)]
    pub assembly_log: AssemblyLogConfig,
}

impl Default for AssemblerConfig {
    fn default() -> Self {
        Self {
            enabled: default_assembler_enabled(),
            total_budget_tokens: default_total_budget(),
            candidate_pool_limit: default_pool_limit(),
            rerank_timeout_ms: default_rerank_timeout(),
            rerank_model: None,
            render_style: crate::memory::assembler::render::RenderStyle::default(),
            force_fallback: false,
            fallback_skeleton: FallbackSkeleton::default(),
            assembly_log: AssemblyLogConfig::default(),
        }
    }
}

fn default_assembler_enabled() -> bool {
    true
}
fn default_total_budget() -> u32 {
    8000
}
fn default_pool_limit() -> usize {
    20
}
fn default_rerank_timeout() -> u64 {
    800
}

/// Deterministic slot budgets used when the LLM rerank path falls back.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct FallbackSkeleton {
    #[serde(default = "default_user_profile_tokens")]
    pub user_profile_tokens: u32,
    #[serde(default = "default_session_recent_tokens")]
    pub session_recent_tokens: u32,
    #[serde(default = "default_relevant_notes_tokens")]
    pub relevant_notes_tokens: u32,
    #[serde(default = "default_raw_fragments_tokens")]
    pub raw_fragments_tokens: u32,
    #[serde(default)]
    pub nudges_tokens: u32,
}

impl Default for FallbackSkeleton {
    fn default() -> Self {
        Self {
            user_profile_tokens: default_user_profile_tokens(),
            session_recent_tokens: default_session_recent_tokens(),
            relevant_notes_tokens: default_relevant_notes_tokens(),
            raw_fragments_tokens: default_raw_fragments_tokens(),
            nudges_tokens: 0,
        }
    }
}

fn default_user_profile_tokens() -> u32 {
    200
}
fn default_session_recent_tokens() -> u32 {
    1500
}
fn default_relevant_notes_tokens() -> u32 {
    5000
}
fn default_raw_fragments_tokens() -> u32 {
    1000
}

/// Optional persistence for assembly decisions (consumed by Spec 2).
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct AssemblyLogConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_assembly_retention_days")]
    pub retention_days: u32,
}

fn default_assembly_retention_days() -> u32 {
    14
}

// Hybrid retrieval defaults
fn default_rrf_k() -> u32 {
    60
}
fn default_bm25_bonus() -> f32 {
    0.15
}

// Scoring, retrieval gate, noise filter & backup defaults
fn default_dedup_similarity_threshold() -> f32 {
    0.95
}
fn default_backup_enabled() -> bool {
    true
}
fn default_backup_max_files() -> usize {
    7
}
impl Default for MemoryConfig {
    fn default() -> Self {
        Self {
            enabled: default_enabled(),
            max_context_items: default_max_context_items(),
            retention_days: default_retention_days(),
            vector_db: default_vector_db(),
            similarity_threshold: default_similarity_threshold(),
            ai_retrieval_enabled: default_ai_retrieval_enabled(),
            ai_retrieval_timeout_ms: default_ai_retrieval_timeout_ms(),
            ai_retrieval_max_candidates: default_ai_retrieval_max_candidates(),
            ai_retrieval_fallback_count: default_ai_retrieval_fallback_count(),
            // Compression settings
            compression_enabled: default_compression_enabled(),
            compression_idle_timeout_seconds: default_compression_idle_timeout(),
            compression_turn_threshold: default_compression_turn_threshold(),
            compression_interval_seconds: default_compression_interval(),
            compression_batch_size: default_compression_batch_size(),
            conflict_similarity_threshold: default_conflict_similarity_threshold(),
            max_facts_in_context: default_max_facts_in_context(),
            raw_memory_fallback_count: default_raw_memory_fallback_count(),
            // Embedding settings
            embedding: EmbeddingSettings::default(),
            dreaming: DreamingConfig::default(),
            memory_decay: MemoryDecayPolicy::default(),
            // Hybrid retrieval & reranking
            rrf_k: default_rrf_k(),
            bm25_bonus_weight: default_bm25_bonus(),
            query_expansion_enabled: false,
            rerank: crate::memory::rerank::RerankConfig::default(),
            // Scoring, retrieval gate & noise filter
            scoring_pipeline:
                crate::memory::scoring_pipeline::config::ScoringPipelineConfig::default(),
            noise_filter: crate::memory::noise_filter::NoiseFilterConfig::default(),
            // Storage
            dedup_similarity_threshold: default_dedup_similarity_threshold(),
            // Backup
            backup_enabled: default_backup_enabled(),
            backup_max_files: default_backup_max_files(),
            // Reflection
            reflection: ReflectionConfig::default(),
            // Working memory assembler
            assembler: AssemblerConfig::default(),
            // Memory injection mode (Spec 3)
            injection_mode: MemoryInjectionMode::Hybrid,
        }
    }
}

#[cfg(test)]
mod spec3_tests {
    use super::*;

    #[test]
    fn injection_mode_default_is_hybrid() {
        assert_eq!(MemoryInjectionMode::default(), MemoryInjectionMode::Hybrid);
    }

    #[test]
    fn injection_mode_round_trips_json() {
        for mode in [
            MemoryInjectionMode::Context,
            MemoryInjectionMode::Tools,
            MemoryInjectionMode::Hybrid,
        ] {
            let s = serde_json::to_string(&mode).unwrap();
            let back: MemoryInjectionMode = serde_json::from_str(&s).unwrap();
            assert_eq!(back, mode);
        }
    }

    #[test]
    fn injection_mode_serialises_lowercase() {
        assert_eq!(serde_json::to_string(&MemoryInjectionMode::Context).unwrap(), "\"context\"");
        assert_eq!(serde_json::to_string(&MemoryInjectionMode::Tools).unwrap(), "\"tools\"");
        assert_eq!(serde_json::to_string(&MemoryInjectionMode::Hybrid).unwrap(), "\"hybrid\"");
    }

    #[test]
    fn memory_config_default_injection_mode_is_hybrid() {
        let cfg = MemoryConfig::default();
        assert_eq!(cfg.injection_mode, MemoryInjectionMode::Hybrid);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dreaming_config_defaults_include_new_fields() {
        let config = DreamingConfig::default();
        assert!(config.weekly_enabled);
        assert_eq!(config.weekly_interval_days, 7);
        assert_eq!(config.drift_max_pairs_per_run, 20);
        assert_eq!(config.synthesis_min_cluster_size, 3);
        assert_eq!(config.synthesis_max_insights, 10);
    }

    #[test]
    fn assembler_config_defaults_sane() {
        let c = AssemblerConfig::default();
        assert!(c.enabled);
        assert_eq!(c.total_budget_tokens, 8000);
        assert_eq!(c.rerank_timeout_ms, 800);
        assert!(!c.force_fallback);
        assert_eq!(c.fallback_skeleton.relevant_notes_tokens, 5000);
        assert!(!c.assembly_log.enabled);
    }

    #[test]
    fn assembler_partial_toml_falls_back_to_defaults() {
        let toml_src = r#"
            enabled = false
            total_budget_tokens = 4000
        "#;
        let c: AssemblerConfig = toml::from_str(toml_src).expect("parse");
        assert!(!c.enabled);
        assert_eq!(c.total_budget_tokens, 4000);
        assert_eq!(c.rerank_timeout_ms, 800);
        assert_eq!(c.fallback_skeleton.relevant_notes_tokens, 5000);
    }

    #[test]
    fn dreaming_config_accessors_match_fields() {
        let config = DreamingConfig::default();
        assert_eq!(config.weekly_enabled(), config.weekly_enabled);
        assert_eq!(config.weekly_interval_days(), config.weekly_interval_days);
        assert_eq!(
            config.drift_max_pairs_per_run(),
            config.drift_max_pairs_per_run
        );
        assert_eq!(
            config.synthesis_min_cluster_size(),
            config.synthesis_min_cluster_size
        );
        assert_eq!(
            config.synthesis_max_insights(),
            config.synthesis_max_insights
        );
    }
}
