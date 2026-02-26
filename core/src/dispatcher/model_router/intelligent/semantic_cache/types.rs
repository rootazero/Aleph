//! Core types for semantic cache
//!
//! Contains cache entry structures, hit results, configuration, and statistics.

use serde::{Deserialize, Serialize};
use std::time::{Duration, SystemTime};

use crate::dispatcher::model_router::TaskIntent;

// =============================================================================
// Core Types
// =============================================================================

/// Type of cache hit
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CacheHitType {
    /// Exact hash match (fast path)
    Exact,
    /// Semantic similarity match
    Semantic,
}

impl std::fmt::Display for CacheHitType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CacheHitType::Exact => write!(f, "Exact"),
            CacheHitType::Semantic => write!(f, "Semantic"),
        }
    }
}

/// Cached response data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedResponse {
    /// Response content text
    pub content: String,

    /// Actual tokens used in the response
    pub tokens_used: u32,

    /// Original latency in milliseconds
    pub latency_ms: u64,

    /// Original cost in USD
    pub cost_usd: f64,
}

impl CachedResponse {
    /// Create a new cached response
    pub fn new(content: String, tokens_used: u32, latency_ms: u64, cost_usd: f64) -> Self {
        Self {
            content,
            tokens_used,
            latency_ms,
            cost_usd,
        }
    }
}

/// Metadata for a cache entry
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CacheMetadata {
    /// Task intent that generated this response
    pub task_intent: Option<TaskIntent>,

    /// Prompt features hash for grouping similar prompts
    pub features_hash: Option<String>,

    /// Custom tags for filtering
    pub tags: Vec<String>,
}

/// A cache entry storing prompt-response pair
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheEntry {
    /// Unique entry ID
    pub id: String,

    /// SHA-256 hash of normalized prompt for exact matching
    pub prompt_hash: String,

    /// Original prompt text (for debugging/display)
    pub prompt_preview: String,

    /// Embedding vector for semantic matching
    pub embedding: Vec<f32>,

    /// Cached response
    pub response: CachedResponse,

    /// Model that generated the response
    pub model_used: String,

    /// Creation timestamp
    pub created_at: SystemTime,

    /// Expiration timestamp (None = never expires)
    pub expires_at: Option<SystemTime>,

    /// Number of cache hits
    pub hit_count: u32,

    /// Last access timestamp (for LRU)
    pub last_accessed: SystemTime,

    /// Additional metadata
    pub metadata: CacheMetadata,
}

impl CacheEntry {
    /// Check if the entry has expired
    pub fn is_expired(&self) -> bool {
        if let Some(expires_at) = self.expires_at {
            SystemTime::now() > expires_at
        } else {
            false
        }
    }

    /// Get the age of the entry in seconds
    pub fn age_secs(&self) -> u64 {
        SystemTime::now()
            .duration_since(self.created_at)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    }

    /// Calculate eviction score for hybrid policy
    /// Lower score = more likely to be evicted
    pub fn eviction_score(&self, age_weight: f64, hits_weight: f64) -> f64 {
        let age_score = 1.0 / (self.age_secs() as f64 + 1.0); // Newer = higher
        let hits_score = self.hit_count as f64; // More hits = higher

        age_weight * age_score + hits_weight * hits_score
    }
}

/// Result of a cache lookup
#[derive(Debug, Clone)]
pub struct CacheHit {
    /// The matched cache entry
    pub entry: CacheEntry,

    /// Similarity score (1.0 for exact match)
    pub similarity: f64,

    /// Type of cache hit
    pub hit_type: CacheHitType,
}

impl CacheHit {
    /// Get the cached response content
    pub fn response(&self) -> &CachedResponse {
        &self.entry.response
    }

    /// Check if this was an exact match
    pub fn is_exact(&self) -> bool {
        self.hit_type == CacheHitType::Exact
    }
}

/// Cache statistics
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CacheStats {
    /// Total number of entries
    pub total_entries: usize,

    /// Approximate total size in bytes
    pub total_size_bytes: usize,

    /// Total number of cache hits
    pub hit_count: u64,

    /// Total number of cache misses
    pub miss_count: u64,

    /// Cache hit rate (0.0 - 1.0)
    pub hit_rate: f64,

    /// Number of exact hash hits
    pub exact_hits: u64,

    /// Number of semantic similarity hits
    pub semantic_hits: u64,

    /// Total number of evictions
    pub evictions: u64,

    /// Average similarity score for semantic hits
    pub avg_similarity: f64,

    /// Age of the oldest entry in seconds
    pub oldest_entry_age_secs: u64,

    /// Number of expired entries (not yet evicted)
    pub expired_entries: usize,
}

impl CacheStats {
    /// Update hit rate based on hit/miss counts
    pub fn update_hit_rate(&mut self) {
        let total = self.hit_count + self.miss_count;
        self.hit_rate = if total > 0 {
            self.hit_count as f64 / total as f64
        } else {
            0.0
        };
    }
}

// =============================================================================
// Configuration
// =============================================================================

/// Eviction policy for the cache
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvictionPolicy {
    /// Least Recently Used
    Lru,
    /// Least Frequently Used
    Lfu,
    /// Hybrid combining age and hit count
    Hybrid { age_weight: f64, hits_weight: f64 },
}

impl Default for EvictionPolicy {
    fn default() -> Self {
        EvictionPolicy::Hybrid {
            age_weight: 0.4,
            hits_weight: 0.6,
        }
    }
}

/// Configuration for the semantic cache
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SemanticCacheConfig {
    /// Enable semantic caching
    pub enabled: bool,

    /// Embedding model name (legacy — actual model comes from active EmbeddingProvider)
    pub embedding_model: String,

    /// Embedding dimensions (legacy — actual dimensions come from active EmbeddingProvider)
    pub embedding_dimensions: usize,

    /// Minimum cosine similarity for cache hit
    pub similarity_threshold: f64,

    /// Check exact hash match before semantic search
    pub exact_match_priority: bool,

    /// Maximum number of cached entries
    pub max_entries: usize,

    /// Default TTL for cache entries
    pub default_ttl: Duration,

    /// Maximum TTL allowed
    pub max_ttl: Duration,

    /// Eviction policy
    pub eviction_policy: EvictionPolicy,

    /// Task intents to exclude from caching
    pub exclude_intents: Vec<TaskIntent>,

    /// Models whose responses should not be cached
    pub exclude_models: Vec<String>,

    /// Minimum response length to cache
    pub min_response_length: usize,

    /// Use async (non-blocking) storage writes
    pub async_storage: bool,
}

impl Default for SemanticCacheConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            embedding_model: "bge-small-zh-v1.5".to_string(),
            embedding_dimensions: 512, // bge-small-zh uses 512 dimensions
            similarity_threshold: 0.85,
            exact_match_priority: true,
            max_entries: 10_000,
            default_ttl: Duration::from_secs(86400), // 24 hours
            max_ttl: Duration::from_secs(604800),    // 7 days
            eviction_policy: EvictionPolicy::default(),
            exclude_intents: vec![TaskIntent::PrivacySensitive],
            exclude_models: Vec::new(),
            min_response_length: 50,
            async_storage: true,
        }
    }
}

// =============================================================================
// Error Types
// =============================================================================

/// Errors from semantic cache operations
#[derive(Debug, thiserror::Error)]
pub enum SemanticCacheError {
    #[error("Cache initialization failed: {0}")]
    InitializationFailed(String),

    #[error("Embedding generation failed: {0}")]
    EmbeddingFailed(String),

    #[error("Storage error: {0}")]
    StorageError(String),

    #[error("Entry not found: {0}")]
    NotFound(String),

    #[error("Serialization error: {0}")]
    SerializationError(String),

    #[error("Cache capacity exceeded")]
    CapacityExceeded,
}
