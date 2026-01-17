//! Memory configuration types
//!
//! Contains Memory/RAG module configuration:
//! - MemoryConfig: Vector database, embedding, and compression settings

use serde::{Deserialize, Serialize};

// =============================================================================
// MemoryConfig
// =============================================================================

/// Memory module configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryConfig {
    /// Enable/disable memory module
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    /// Embedding model name
    #[serde(default = "default_embedding_model")]
    pub embedding_model: String,
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
}

// =============================================================================
// Default value functions for MemoryConfig
// =============================================================================

pub fn default_enabled() -> bool {
    true
}

pub fn default_embedding_model() -> String {
    "bge-small-zh-v1.5".to_string()
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

impl Default for MemoryConfig {
    fn default() -> Self {
        Self {
            enabled: default_enabled(),
            embedding_model: default_embedding_model(),
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
        }
    }
}
