//! Semantic cache configuration (P2)
//!
//! Contains SemanticCacheConfigToml for configuring the semantic cache
//! for storing and retrieving responses based on prompt similarity.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

// =============================================================================
// SemanticCacheConfigToml
// =============================================================================

/// Semantic cache configuration from TOML
///
/// Configures the semantic cache for storing and retrieving responses
/// based on prompt similarity.
///
/// # Example TOML
/// ```toml
/// [cowork.model_routing.semantic_cache]
/// enabled = true
/// similarity_threshold = 0.85
/// max_entries = 10000
/// default_ttl_secs = 86400
/// eviction_policy = "hybrid"
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SemanticCacheConfigToml {
    /// Enable semantic caching
    #[serde(default = "default_semantic_cache_enabled")]
    pub enabled: bool,

    /// Embedding model name (legacy — actual model comes from active EmbeddingProvider)
    #[serde(default = "default_embedding_model")]
    pub embedding_model: String,

    /// Minimum cosine similarity for cache hit
    #[serde(default = "default_similarity_threshold")]
    pub similarity_threshold: f64,

    /// Check exact hash match before semantic search
    #[serde(default = "default_exact_match_priority")]
    pub exact_match_priority: bool,

    /// Maximum number of cached entries
    #[serde(default = "default_max_cache_entries")]
    pub max_entries: usize,

    /// Default TTL in seconds (86400 = 24 hours)
    #[serde(default = "default_cache_ttl_secs")]
    pub default_ttl_secs: u64,

    /// Maximum TTL in seconds (604800 = 7 days)
    #[serde(default = "default_max_ttl_secs")]
    pub max_ttl_secs: u64,

    /// Eviction policy: "lru", "lfu", or "hybrid"
    #[serde(default = "default_eviction_policy")]
    pub eviction_policy: String,

    /// Weight for age in hybrid eviction (0.0 - 1.0)
    #[serde(default = "default_hybrid_age_weight")]
    pub hybrid_age_weight: f64,

    /// Weight for hit count in hybrid eviction (0.0 - 1.0)
    #[serde(default = "default_hybrid_hits_weight")]
    pub hybrid_hits_weight: f64,

    /// Minimum response length to cache
    #[serde(default = "default_min_response_length")]
    pub min_response_length: usize,

    /// Task intents to exclude from caching
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exclude_intents: Vec<String>,

    /// Use async (non-blocking) storage writes
    #[serde(default = "default_async_storage")]
    pub async_storage: bool,
}

impl Default for SemanticCacheConfigToml {
    fn default() -> Self {
        Self {
            enabled: default_semantic_cache_enabled(),
            embedding_model: default_embedding_model(),
            similarity_threshold: default_similarity_threshold(),
            exact_match_priority: default_exact_match_priority(),
            max_entries: default_max_cache_entries(),
            default_ttl_secs: default_cache_ttl_secs(),
            max_ttl_secs: default_max_ttl_secs(),
            eviction_policy: default_eviction_policy(),
            hybrid_age_weight: default_hybrid_age_weight(),
            hybrid_hits_weight: default_hybrid_hits_weight(),
            min_response_length: default_min_response_length(),
            exclude_intents: Vec::new(),
            async_storage: default_async_storage(),
        }
    }
}

impl SemanticCacheConfigToml {
    /// Validate semantic cache configuration
    pub fn validate(&self) -> Result<(), String> {
        if self.similarity_threshold < 0.0 || self.similarity_threshold > 1.0 {
            return Err(format!(
                "similarity_threshold must be between 0.0 and 1.0, got {}",
                self.similarity_threshold
            ));
        }

        if self.max_entries == 0 {
            return Err("max_entries must be greater than 0".to_string());
        }

        if self.default_ttl_secs > self.max_ttl_secs {
            return Err(format!(
                "default_ttl_secs ({}) cannot exceed max_ttl_secs ({})",
                self.default_ttl_secs, self.max_ttl_secs
            ));
        }

        let valid_policies = ["lru", "lfu", "hybrid"];
        if !valid_policies.contains(&self.eviction_policy.as_str()) {
            return Err(format!(
                "eviction_policy must be one of {:?}, got '{}'",
                valid_policies, self.eviction_policy
            ));
        }

        if self.eviction_policy == "hybrid" {
            let total = self.hybrid_age_weight + self.hybrid_hits_weight;
            if (total - 1.0).abs() > 0.01 {
                tracing::warn!(
                    "Hybrid eviction weights do not sum to 1.0 ({}), they will be normalized",
                    total
                );
            }
        }

        Ok(())
    }
}

// =============================================================================
// Default Functions
// =============================================================================

fn default_semantic_cache_enabled() -> bool {
    true
}

fn default_embedding_model() -> String {
    "bge-small-zh-v1.5".to_string()
}

fn default_similarity_threshold() -> f64 {
    0.85
}

fn default_exact_match_priority() -> bool {
    true
}

fn default_max_cache_entries() -> usize {
    10000
}

fn default_cache_ttl_secs() -> u64 {
    86400 // 24 hours
}

fn default_max_ttl_secs() -> u64 {
    604800 // 7 days
}

fn default_eviction_policy() -> String {
    "hybrid".to_string()
}

fn default_hybrid_age_weight() -> f64 {
    0.4
}

fn default_hybrid_hits_weight() -> f64 {
    0.6
}

fn default_min_response_length() -> usize {
    50
}

fn default_async_storage() -> bool {
    true
}
