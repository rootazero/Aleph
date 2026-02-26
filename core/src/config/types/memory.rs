//! Memory configuration types
//!
//! Contains Memory/RAG module configuration:
//! - MemoryConfig: Vector database, embedding, and compression settings

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

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
    /// Vector database backend: "sqlite-vec" or "lancedb"
    #[serde(default = "default_vector_db")]
    pub vector_db: String,
    /// Minimum similarity score to include memory (0.0-1.0)
    #[serde(default = "default_similarity_threshold")]
    pub similarity_threshold: f32,
    /// List of app bundle IDs to exclude from memory storage
    #[serde(default)]
    pub excluded_apps: Vec<String>,

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
    // LanceDB Settings
    // ========================================
    /// LanceDB-specific configuration
    #[serde(default)]
    pub lancedb: LanceDbConfig,

    // ========================================
    // Dreaming + Memory Graph
    // ========================================
    /// DreamDaemon scheduling configuration
    #[serde(default)]
    pub dreaming: DreamingConfig,
    /// Graph decay policy for entity/relationship pruning
    #[serde(default)]
    pub graph_decay: GraphDecayPolicy,
    /// Memory fact decay policy
    #[serde(default)]
    pub memory_decay: MemoryDecayPolicy,

    // ========================================
    // Scoring, Retrieval Gate & Noise Filter
    // ========================================
    /// Scoring pipeline configuration.
    #[serde(default)]
    pub scoring_pipeline: crate::memory::scoring_pipeline::config::ScoringPipelineConfig,

    /// Adaptive retrieval gate configuration.
    #[serde(default)]
    pub adaptive_retrieval: crate::memory::adaptive_retrieval::AdaptiveRetrievalConfig,

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
    /// Environment variable name for API key
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key_env: Option<String>,
    /// Direct API key (for settings UI; prefer api_key_env in production)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    /// Model name (e.g., "BAAI/bge-m3")
    pub model: String,
    /// Output vector dimensions
    pub dimensions: u32,
    /// Batch size for embedding requests
    #[serde(default = "default_embedding_batch_size")]
    pub batch_size: u32,
    /// Request timeout in milliseconds
    #[serde(default = "default_embedding_timeout_ms")]
    pub timeout_ms: u64,
}

impl EmbeddingProviderConfig {
    /// Create a SiliconFlow preset
    pub fn siliconflow() -> Self {
        Self {
            id: "siliconflow".to_string(),
            name: "SiliconFlow".to_string(),
            preset: EmbeddingPreset::SiliconFlow,
            api_base: "https://api.siliconflow.cn/v1".to_string(),
            api_key_env: Some("SILICONFLOW_API_KEY".to_string()),
            api_key: None,
            model: "BAAI/bge-m3".to_string(),
            dimensions: 1024,
            batch_size: default_embedding_batch_size(),
            timeout_ms: default_embedding_timeout_ms(),
        }
    }

    /// Create an OpenAI preset
    pub fn openai() -> Self {
        Self {
            id: "openai".to_string(),
            name: "OpenAI".to_string(),
            preset: EmbeddingPreset::OpenAi,
            api_base: "https://api.openai.com/v1".to_string(),
            api_key_env: Some("OPENAI_API_KEY".to_string()),
            api_key: None,
            model: "text-embedding-3-small".to_string(),
            dimensions: 1536,
            batch_size: default_embedding_batch_size(),
            timeout_ms: default_embedding_timeout_ms(),
        }
    }

    /// Create an Ollama preset
    pub fn ollama() -> Self {
        Self {
            id: "ollama".to_string(),
            name: "Ollama".to_string(),
            preset: EmbeddingPreset::Ollama,
            api_base: "http://localhost:11434/v1".to_string(),
            api_key_env: None,
            api_key: None,
            model: "nomic-embed-text".to_string(),
            dimensions: 768,
            batch_size: default_embedding_batch_size(),
            timeout_ms: default_embedding_timeout_ms(),
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
    vec![
        EmbeddingProviderConfig::siliconflow(),
        EmbeddingProviderConfig::openai(),
        EmbeddingProviderConfig::ollama(),
    ]
}

fn default_active_provider_id() -> String {
    "siliconflow".to_string()
}

// =============================================================================
// LanceDbConfig
// =============================================================================

/// LanceDB-specific configuration
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct LanceDbConfig {
    /// Data directory for LanceDB files
    #[serde(default = "default_lance_data_dir")]
    pub data_dir: String,
    /// ANN index type: "IVF_PQ", "IVF_HNSW_SQ", or "none"
    #[serde(default = "default_ann_index_type")]
    pub ann_index_type: String,
    /// Row count threshold to auto-build ANN index
    #[serde(default = "default_ann_index_threshold")]
    pub ann_index_threshold: usize,
    /// FTS tokenizer: "default", "jieba", "simple"
    #[serde(default = "default_fts_tokenizer")]
    pub fts_tokenizer: String,
}

impl Default for LanceDbConfig {
    fn default() -> Self {
        Self {
            data_dir: default_lance_data_dir(),
            ann_index_type: default_ann_index_type(),
            ann_index_threshold: default_ann_index_threshold(),
            fts_tokenizer: default_fts_tokenizer(),
        }
    }
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
}

impl Default for DreamingConfig {
    fn default() -> Self {
        Self {
            enabled: default_dreaming_enabled(),
            idle_threshold_seconds: default_dreaming_idle_threshold_seconds(),
            window_start_local: default_dreaming_window_start(),
            window_end_local: default_dreaming_window_end(),
            max_duration_seconds: default_dreaming_max_duration_seconds(),
        }
    }
}

// =============================================================================
// GraphDecayPolicy
// =============================================================================

/// Decay policy for graph nodes/edges
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct GraphDecayPolicy {
    /// Per-day decay multiplier for nodes (0.0-1.0)
    #[serde(default = "default_graph_node_decay_per_day")]
    pub node_decay_per_day: f32,
    /// Per-day decay multiplier for edges (0.0-1.0)
    #[serde(default = "default_graph_edge_decay_per_day")]
    pub edge_decay_per_day: f32,
    /// Minimum score before pruning
    #[serde(default = "default_graph_min_score")]
    pub min_score: f32,
}

impl Default for GraphDecayPolicy {
    fn default() -> Self {
        Self {
            node_decay_per_day: default_graph_node_decay_per_day(),
            edge_decay_per_day: default_graph_edge_decay_per_day(),
            min_score: default_graph_min_score(),
        }
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
    "lancedb".to_string()
}

pub fn default_similarity_threshold() -> f32 {
    0.7 // Minimum similarity score for real embedding models
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

// Graph decay defaults
pub fn default_graph_node_decay_per_day() -> f32 {
    0.02
}

pub fn default_graph_edge_decay_per_day() -> f32 {
    0.03
}

pub fn default_graph_min_score() -> f32 {
    0.1
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

// LanceDB configuration defaults
pub fn default_lance_data_dir() -> String {
    "~/.aleph".to_string()
}

pub fn default_ann_index_type() -> String {
    "none".to_string()
}

pub fn default_ann_index_threshold() -> usize {
    10000
}

pub fn default_fts_tokenizer() -> String {
    "default".to_string()
}


// Scoring, retrieval gate, noise filter & backup defaults
fn default_dedup_similarity_threshold() -> f32 { 0.95 }
fn default_backup_enabled() -> bool { true }
fn default_backup_max_files() -> usize { 7 }
impl Default for MemoryConfig {
    fn default() -> Self {
        Self {
            enabled: default_enabled(),
            max_context_items: default_max_context_items(),
            retention_days: default_retention_days(),
            vector_db: default_vector_db(),
            similarity_threshold: default_similarity_threshold(),
            excluded_apps: vec![
                "com.apple.keychainaccess".to_string(),
                "com.agilebits.onepassword7".to_string(),
                "com.lastpass.LastPass".to_string(),
                "com.bitwarden.desktop".to_string(),
            ],
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
            // LanceDB settings
            lancedb: LanceDbConfig::default(),
            dreaming: DreamingConfig::default(),
            graph_decay: GraphDecayPolicy::default(),
            memory_decay: MemoryDecayPolicy::default(),
            // Scoring, retrieval gate & noise filter
            scoring_pipeline: crate::memory::scoring_pipeline::config::ScoringPipelineConfig::default(),
            adaptive_retrieval: crate::memory::adaptive_retrieval::AdaptiveRetrievalConfig::default(),
            noise_filter: crate::memory::noise_filter::NoiseFilterConfig::default(),
            // Storage
            dedup_similarity_threshold: default_dedup_similarity_threshold(),
            // Backup
            backup_enabled: default_backup_enabled(),
            backup_max_files: default_backup_max_files(),
        }
    }
}
