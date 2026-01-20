//! Semantic Cache Module
//!
//! Provides semantic similarity-based caching for AI responses.
//! Uses text embeddings to match similar prompts and return cached responses.
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────┐
//! │                     SemanticCacheManager                        │
//! ├─────────────────────────────────────────────────────────────────┤
//! │  ┌──────────────┐  ┌──────────────┐  ┌────────────────────────┐ │
//! │  │ Embedder     │  │ VectorStore  │  │   SimilarityMatcher    │ │
//! │  │ (fastembed)  │  │ (in-memory)  │  │   (cosine sim)         │ │
//! │  └──────────────┘  └──────────────┘  └────────────────────────┘ │
//! └─────────────────────────────────────────────────────────────────┘
//! ```
//!
//! # Example
//!
//! ```rust,ignore
//! use aethecore::dispatcher::model_router::{SemanticCacheManager, SemanticCacheConfig};
//!
//! let config = SemanticCacheConfig::default();
//! let cache = SemanticCacheManager::new(config)?;
//!
//! // Lookup
//! if let Some(hit) = cache.lookup("What is Rust?").await? {
//!     println!("Cache hit! Similarity: {:.2}", hit.similarity);
//!     return Ok(hit.response);
//! }
//!
//! // Store after API call
//! cache.store("What is Rust?", &response, "claude-sonnet", None).await?;
//! ```

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use tokio::sync::RwLock;

use super::TaskIntent;

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

    /// Embedding model name (for fastembed)
    pub embedding_model: String,

    /// Embedding dimensions (384 for bge-small)
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
// Text Embedder Trait
// =============================================================================

/// Trait for text embedding generation
#[async_trait::async_trait]
pub trait TextEmbedder: Send + Sync {
    /// Generate embedding for a single text
    async fn embed(&self, text: &str) -> Result<Vec<f32>, EmbeddingError>;

    /// Generate embeddings for multiple texts (batch)
    async fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, EmbeddingError>;

    /// Get the dimension of embeddings
    fn dimensions(&self) -> usize;

    /// Get the model name
    fn model_name(&self) -> &str;
}

/// Errors from embedding generation
#[derive(Debug, thiserror::Error)]
pub enum EmbeddingError {
    #[error("Model not initialized: {0}")]
    NotInitialized(String),

    #[error("Embedding generation failed: {0}")]
    GenerationFailed(String),

    #[error("Invalid input: {0}")]
    InvalidInput(String),

    #[error("Model loading failed: {0}")]
    ModelLoadFailed(String),
}

// =============================================================================
// FastEmbed Embedder
// =============================================================================

/// Text embedder using fastembed library
pub struct FastEmbedEmbedder {
    model: fastembed::TextEmbedding,
    model_name: String,
    dimensions: usize,
}

impl FastEmbedEmbedder {
    /// Create a new FastEmbed embedder with the default model
    pub fn new() -> Result<Self, EmbeddingError> {
        Self::with_model("bge-small-zh-v1.5")
    }

    /// Create a new FastEmbed embedder with a specific model
    pub fn with_model(model_name: &str) -> Result<Self, EmbeddingError> {
        // Map model name to fastembed model type
        let model_type = match model_name {
            "bge-small-zh-v1.5" => fastembed::EmbeddingModel::BGESmallZHV15,
            "bge-small-en-v1.5" => fastembed::EmbeddingModel::BGESmallENV15,
            "bge-base-en-v1.5" => fastembed::EmbeddingModel::BGEBaseENV15,
            _ => {
                return Err(EmbeddingError::ModelLoadFailed(format!(
                    "Unknown model: {}. Supported: bge-small-zh-v1.5, bge-small-en-v1.5, bge-base-en-v1.5",
                    model_name
                )));
            }
        };

        let init_options =
            fastembed::InitOptions::new(model_type).with_show_download_progress(false);

        let model = fastembed::TextEmbedding::try_new(init_options).map_err(|e| {
            EmbeddingError::ModelLoadFailed(format!("Failed to load fastembed model: {}", e))
        })?;

        // Get dimensions based on model
        let dimensions = match model_name {
            "bge-small-zh-v1.5" => 512,
            "bge-small-en-v1.5" => 384,
            "bge-base-en-v1.5" => 768,
            _ => 512,
        };

        Ok(Self {
            model,
            model_name: model_name.to_string(),
            dimensions,
        })
    }
}

#[async_trait::async_trait]
impl TextEmbedder for FastEmbedEmbedder {
    async fn embed(&self, text: &str) -> Result<Vec<f32>, EmbeddingError> {
        if text.is_empty() {
            return Err(EmbeddingError::InvalidInput(
                "Empty text provided".to_string(),
            ));
        }

        let texts = vec![text.to_string()];
        let embeddings = self
            .model
            .embed(texts, None)
            .map_err(|e| EmbeddingError::GenerationFailed(e.to_string()))?;

        embeddings
            .into_iter()
            .next()
            .ok_or_else(|| EmbeddingError::GenerationFailed("No embedding generated".to_string()))
    }

    async fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, EmbeddingError> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }

        let texts: Vec<String> = texts.iter().map(|s| s.to_string()).collect();
        self.model
            .embed(texts, None)
            .map_err(|e| EmbeddingError::GenerationFailed(e.to_string()))
    }

    fn dimensions(&self) -> usize {
        self.dimensions
    }

    fn model_name(&self) -> &str {
        &self.model_name
    }
}

// =============================================================================
// Utility Functions
// =============================================================================

/// Calculate cosine similarity between two vectors
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f64 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }

    let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();

    if norm_a == 0.0 || norm_b == 0.0 {
        return 0.0;
    }

    (dot / (norm_a * norm_b)) as f64
}

/// Normalize a prompt for hashing (lowercase, trim, collapse whitespace)
pub fn normalize_prompt(prompt: &str) -> String {
    prompt
        .trim()
        .to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Generate SHA-256 hash of a prompt
pub fn hash_prompt(prompt: &str) -> String {
    let normalized = normalize_prompt(prompt);
    let mut hasher = Sha256::new();
    hasher.update(normalized.as_bytes());
    format!("{:x}", hasher.finalize())
}

/// Generate a short preview of a prompt (first N characters)
pub fn prompt_preview(prompt: &str, max_len: usize) -> String {
    if prompt.len() <= max_len {
        prompt.to_string()
    } else {
        format!("{}...", &prompt[..max_len.min(prompt.len())])
    }
}

// =============================================================================
// In-Memory Vector Store
// =============================================================================

/// In-memory vector store for cache entries
pub struct InMemoryVectorStore {
    /// Entries indexed by ID
    entries: RwLock<HashMap<String, CacheEntry>>,

    /// Hash index for exact matching
    hash_index: RwLock<HashMap<String, String>>, // prompt_hash -> entry_id

    /// Statistics
    stats: RwLock<CacheStats>,

    /// Configuration
    config: SemanticCacheConfig,
}

impl InMemoryVectorStore {
    /// Create a new in-memory vector store
    pub fn new(config: SemanticCacheConfig) -> Self {
        Self {
            entries: RwLock::new(HashMap::new()),
            hash_index: RwLock::new(HashMap::new()),
            stats: RwLock::new(CacheStats::default()),
            config,
        }
    }

    /// Insert a cache entry
    pub async fn insert(&self, entry: CacheEntry) -> Result<(), SemanticCacheError> {
        let mut entries = self.entries.write().await;
        let mut hash_index = self.hash_index.write().await;

        // Check capacity
        if entries.len() >= self.config.max_entries {
            // Need to evict
            drop(entries);
            drop(hash_index);
            self.evict_by_policy(self.config.max_entries / 10).await?;
            entries = self.entries.write().await;
            hash_index = self.hash_index.write().await;
        }

        // Add to hash index
        hash_index.insert(entry.prompt_hash.clone(), entry.id.clone());

        // Add entry
        entries.insert(entry.id.clone(), entry);

        Ok(())
    }

    /// Lookup by exact hash match
    pub async fn lookup_exact(&self, prompt_hash: &str) -> Option<CacheEntry> {
        let hash_index = self.hash_index.read().await;
        let entry_id = hash_index.get(prompt_hash)?;

        let entries = self.entries.read().await;
        let entry = entries.get(entry_id)?;

        if entry.is_expired() {
            return None;
        }

        Some(entry.clone())
    }

    /// Search by semantic similarity
    pub async fn search_similar(
        &self,
        embedding: &[f32],
        threshold: f64,
    ) -> Option<(CacheEntry, f64)> {
        let entries = self.entries.read().await;

        let mut best_match: Option<(CacheEntry, f64)> = None;
        let mut best_similarity = threshold;

        for entry in entries.values() {
            if entry.is_expired() {
                continue;
            }

            let similarity = cosine_similarity(embedding, &entry.embedding);
            if similarity > best_similarity {
                best_similarity = similarity;
                best_match = Some((entry.clone(), similarity));
            }
        }

        best_match
    }

    /// Update hit count and last accessed time
    pub async fn record_hit(&self, entry_id: &str) {
        let mut entries = self.entries.write().await;
        if let Some(entry) = entries.get_mut(entry_id) {
            entry.hit_count += 1;
            entry.last_accessed = SystemTime::now();
        }
    }

    /// Delete an entry by ID
    pub async fn delete(&self, entry_id: &str) -> Result<(), SemanticCacheError> {
        let mut entries = self.entries.write().await;
        let mut hash_index = self.hash_index.write().await;

        if let Some(entry) = entries.remove(entry_id) {
            hash_index.remove(&entry.prompt_hash);
        }

        Ok(())
    }

    /// Clear all entries
    pub async fn clear(&self) -> Result<(), SemanticCacheError> {
        let mut entries = self.entries.write().await;
        let mut hash_index = self.hash_index.write().await;

        entries.clear();
        hash_index.clear();

        Ok(())
    }

    /// Get entry count
    pub async fn count(&self) -> usize {
        self.entries.read().await.len()
    }

    /// Evict expired entries
    pub async fn evict_expired(&self) -> Result<usize, SemanticCacheError> {
        let mut entries = self.entries.write().await;
        let mut hash_index = self.hash_index.write().await;
        let mut stats = self.stats.write().await;

        let expired_ids: Vec<String> = entries
            .iter()
            .filter(|(_, e)| e.is_expired())
            .map(|(id, _)| id.clone())
            .collect();

        let count = expired_ids.len();

        for id in &expired_ids {
            if let Some(entry) = entries.remove(id) {
                hash_index.remove(&entry.prompt_hash);
            }
        }

        stats.evictions += count as u64;

        Ok(count)
    }

    /// Evict entries by policy
    pub async fn evict_by_policy(&self, target_count: usize) -> Result<usize, SemanticCacheError> {
        // First evict expired
        let expired_count = self.evict_expired().await?;
        if expired_count >= target_count {
            return Ok(expired_count);
        }

        let remaining = target_count - expired_count;

        let mut entries = self.entries.write().await;
        let mut hash_index = self.hash_index.write().await;
        let mut stats = self.stats.write().await;

        // Score all entries based on policy
        let mut scored: Vec<(String, f64)> = match &self.config.eviction_policy {
            EvictionPolicy::Lru => entries
                .iter()
                .map(|(id, e)| {
                    let time_since_access = SystemTime::now()
                        .duration_since(e.last_accessed)
                        .map(|d| d.as_secs())
                        .unwrap_or(0);
                    (id.clone(), time_since_access as f64)
                })
                .collect(),
            EvictionPolicy::Lfu => entries
                .iter()
                .map(|(id, e)| (id.clone(), -(e.hit_count as f64)))
                .collect(),
            EvictionPolicy::Hybrid {
                age_weight,
                hits_weight,
            } => entries
                .iter()
                .map(|(id, e)| (id.clone(), -e.eviction_score(*age_weight, *hits_weight)))
                .collect(),
        };

        // Sort by score (lower = evict first for LRU, higher for LFU)
        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        // Evict bottom entries
        let to_evict: Vec<String> = scored
            .into_iter()
            .take(remaining)
            .map(|(id, _)| id)
            .collect();

        for id in &to_evict {
            if let Some(entry) = entries.remove(id) {
                hash_index.remove(&entry.prompt_hash);
            }
        }

        stats.evictions += to_evict.len() as u64;

        Ok(expired_count + to_evict.len())
    }

    /// Get cache statistics
    pub async fn get_stats(&self) -> CacheStats {
        let entries = self.entries.read().await;
        let mut stats = self.stats.write().await;

        stats.total_entries = entries.len();
        stats.expired_entries = entries.values().filter(|e| e.is_expired()).count();

        // Calculate oldest entry age
        stats.oldest_entry_age_secs = entries.values().map(|e| e.age_secs()).max().unwrap_or(0);

        // Approximate size (rough estimate)
        stats.total_size_bytes = entries
            .values()
            .map(|e| {
                e.embedding.len() * 4 // f32 = 4 bytes
                    + e.response.content.len()
                    + e.prompt_preview.len()
                    + e.prompt_hash.len()
                    + e.model_used.len()
                    + 200 // overhead estimate
            })
            .sum();

        stats.update_hit_rate();

        stats.clone()
    }
}

// =============================================================================
// Semantic Cache Manager
// =============================================================================

/// Main semantic cache manager
pub struct SemanticCacheManager {
    /// Text embedder for generating embeddings
    embedder: Arc<dyn TextEmbedder>,

    /// Vector store for cache entries
    store: Arc<InMemoryVectorStore>,

    /// Configuration
    config: SemanticCacheConfig,
}

impl SemanticCacheManager {
    /// Create a new semantic cache manager
    pub fn new(config: SemanticCacheConfig) -> Result<Self, SemanticCacheError> {
        // Create embedder
        let embedder = FastEmbedEmbedder::with_model(&config.embedding_model)
            .map_err(|e| SemanticCacheError::InitializationFailed(e.to_string()))?;

        let store = InMemoryVectorStore::new(config.clone());

        Ok(Self {
            embedder: Arc::new(embedder),
            store: Arc::new(store),
            config,
        })
    }

    /// Create with a custom embedder (for testing)
    pub fn with_embedder(embedder: Arc<dyn TextEmbedder>, config: SemanticCacheConfig) -> Self {
        let store = InMemoryVectorStore::new(config.clone());

        Self {
            embedder,
            store: Arc::new(store),
            config,
        }
    }

    /// Check if caching is enabled
    pub fn is_enabled(&self) -> bool {
        self.config.enabled
    }

    /// Lookup a prompt in the cache
    pub async fn lookup(&self, prompt: &str) -> Result<Option<CacheHit>, SemanticCacheError> {
        if !self.config.enabled {
            return Ok(None);
        }

        // Try exact match first if enabled
        if self.config.exact_match_priority {
            let prompt_hash = hash_prompt(prompt);
            if let Some(mut entry) = self.store.lookup_exact(&prompt_hash).await {
                // Record hit
                self.store.record_hit(&entry.id).await;
                entry.hit_count += 1;
                entry.last_accessed = SystemTime::now();

                // Update stats
                {
                    let mut stats = self.store.stats.write().await;
                    stats.hit_count += 1;
                    stats.exact_hits += 1;
                }

                return Ok(Some(CacheHit {
                    entry,
                    similarity: 1.0,
                    hit_type: CacheHitType::Exact,
                }));
            }
        }

        // Generate embedding for semantic search
        let embedding = self
            .embedder
            .embed(prompt)
            .await
            .map_err(|e| SemanticCacheError::EmbeddingFailed(e.to_string()))?;

        // Search for similar entries
        if let Some((mut entry, similarity)) = self
            .store
            .search_similar(&embedding, self.config.similarity_threshold)
            .await
        {
            // Record hit
            self.store.record_hit(&entry.id).await;
            entry.hit_count += 1;
            entry.last_accessed = SystemTime::now();

            // Update stats
            {
                let mut stats = self.store.stats.write().await;
                stats.hit_count += 1;
                stats.semantic_hits += 1;
                // Update average similarity (running average)
                let total_semantic = stats.semantic_hits as f64;
                stats.avg_similarity =
                    (stats.avg_similarity * (total_semantic - 1.0) + similarity) / total_semantic;
            }

            return Ok(Some(CacheHit {
                entry,
                similarity,
                hit_type: CacheHitType::Semantic,
            }));
        }

        // Cache miss
        {
            let mut stats = self.store.stats.write().await;
            stats.miss_count += 1;
        }

        Ok(None)
    }

    /// Store a response in the cache
    pub async fn store(
        &self,
        prompt: &str,
        response: &CachedResponse,
        model: &str,
        ttl: Option<Duration>,
        metadata: Option<CacheMetadata>,
    ) -> Result<(), SemanticCacheError> {
        if !self.config.enabled {
            return Ok(());
        }

        // Check exclusions
        if self.config.exclude_models.contains(&model.to_string()) {
            return Ok(());
        }

        if let Some(ref meta) = metadata {
            if let Some(ref intent) = meta.task_intent {
                if self.config.exclude_intents.contains(intent) {
                    return Ok(());
                }
            }
        }

        // Check minimum response length
        if response.content.len() < self.config.min_response_length {
            return Ok(());
        }

        // Generate embedding
        let embedding = self
            .embedder
            .embed(prompt)
            .await
            .map_err(|e| SemanticCacheError::EmbeddingFailed(e.to_string()))?;

        // Calculate TTL
        let effective_ttl = ttl
            .map(|t| t.min(self.config.max_ttl))
            .unwrap_or(self.config.default_ttl);

        let now = SystemTime::now();

        // Create entry
        let entry = CacheEntry {
            id: uuid::Uuid::new_v4().to_string(),
            prompt_hash: hash_prompt(prompt),
            prompt_preview: prompt_preview(prompt, 100),
            embedding,
            response: response.clone(),
            model_used: model.to_string(),
            created_at: now,
            expires_at: Some(now + effective_ttl),
            hit_count: 0,
            last_accessed: now,
            metadata: metadata.unwrap_or_default(),
        };

        // Store
        self.store.insert(entry).await
    }

    /// Invalidate a specific prompt from cache
    pub async fn invalidate(&self, prompt: &str) -> Result<(), SemanticCacheError> {
        let prompt_hash = hash_prompt(prompt);

        if let Some(entry) = self.store.lookup_exact(&prompt_hash).await {
            self.store.delete(&entry.id).await?;
        }

        Ok(())
    }

    /// Clear all cache entries
    pub async fn clear(&self) -> Result<(), SemanticCacheError> {
        self.store.clear().await
    }

    /// Get cache statistics
    pub async fn stats(&self) -> CacheStats {
        self.store.get_stats().await
    }

    /// Get the number of cached entries
    pub async fn entry_count(&self) -> usize {
        self.store.count().await
    }

    /// Manually trigger eviction
    pub async fn evict(&self, count: usize) -> Result<usize, SemanticCacheError> {
        self.store.evict_by_policy(count).await
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

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // Mock embedder for testing
    struct MockEmbedder {
        dimensions: usize,
    }

    impl MockEmbedder {
        fn new(dimensions: usize) -> Self {
            Self { dimensions }
        }
    }

    #[async_trait::async_trait]
    impl TextEmbedder for MockEmbedder {
        async fn embed(&self, text: &str) -> Result<Vec<f32>, EmbeddingError> {
            // Generate a deterministic embedding based on text hash
            let hash = hash_prompt(text);
            let seed: u64 = hash
                .bytes()
                .take(8)
                .fold(0u64, |acc, b| acc * 256 + b as u64);

            let mut embedding = Vec::with_capacity(self.dimensions);
            let mut x = seed;
            for _ in 0..self.dimensions {
                x = x
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                embedding.push((x as f32 / u64::MAX as f32) * 2.0 - 1.0);
            }

            // Normalize
            let norm: f32 = embedding.iter().map(|x| x * x).sum::<f32>().sqrt();
            for v in &mut embedding {
                *v /= norm;
            }

            Ok(embedding)
        }

        async fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, EmbeddingError> {
            let mut results = Vec::with_capacity(texts.len());
            for text in texts {
                results.push(self.embed(text).await?);
            }
            Ok(results)
        }

        fn dimensions(&self) -> usize {
            self.dimensions
        }

        fn model_name(&self) -> &str {
            "mock-embedder"
        }
    }

    fn create_test_cache() -> SemanticCacheManager {
        let config = SemanticCacheConfig {
            enabled: true,
            embedding_dimensions: 128,
            similarity_threshold: 0.8,
            max_entries: 100,
            default_ttl: Duration::from_secs(3600),
            min_response_length: 10, // Lower for testing
            ..Default::default()
        };

        let embedder = Arc::new(MockEmbedder::new(128));
        SemanticCacheManager::with_embedder(embedder, config)
    }

    #[test]
    fn test_cosine_similarity() {
        let a = vec![1.0, 0.0, 0.0];
        let b = vec![1.0, 0.0, 0.0];
        assert!((cosine_similarity(&a, &b) - 1.0).abs() < 0.001);

        let c = vec![0.0, 1.0, 0.0];
        assert!(cosine_similarity(&a, &c).abs() < 0.001);

        let d = vec![-1.0, 0.0, 0.0];
        assert!((cosine_similarity(&a, &d) + 1.0).abs() < 0.001);
    }

    #[test]
    fn test_hash_prompt() {
        let hash1 = hash_prompt("Hello World");
        let hash2 = hash_prompt("  hello   world  ");
        assert_eq!(hash1, hash2); // Normalized hashes should match

        let hash3 = hash_prompt("Different text");
        assert_ne!(hash1, hash3);
    }

    #[test]
    fn test_normalize_prompt() {
        assert_eq!(normalize_prompt("  Hello   World  "), "hello world");
        assert_eq!(normalize_prompt("TEST"), "test");
    }

    #[test]
    fn test_prompt_preview() {
        let short = "Short text";
        assert_eq!(prompt_preview(short, 20), "Short text");

        let long = "This is a very long text that should be truncated";
        assert_eq!(prompt_preview(long, 20), "This is a very long ...");
    }

    #[tokio::test]
    async fn test_cache_store_and_lookup() {
        let cache = create_test_cache();

        let prompt = "What is Rust?";
        let response =
            CachedResponse::new("Rust is a programming language".to_string(), 50, 100, 0.001);

        // Store
        cache
            .store(prompt, &response, "test-model", None, None)
            .await
            .unwrap();

        // Lookup (exact match)
        let hit = cache.lookup(prompt).await.unwrap();
        assert!(hit.is_some());

        let hit = hit.unwrap();
        assert_eq!(hit.hit_type, CacheHitType::Exact);
        assert_eq!(hit.similarity, 1.0);
        assert_eq!(hit.response().content, "Rust is a programming language");
    }

    #[tokio::test]
    async fn test_cache_miss() {
        let cache = create_test_cache();

        let hit = cache.lookup("Non-existent prompt").await.unwrap();
        assert!(hit.is_none());
    }

    #[tokio::test]
    async fn test_cache_invalidate() {
        let cache = create_test_cache();

        let prompt = "Test prompt";
        let response = CachedResponse::new("Test response".to_string(), 10, 50, 0.001);

        cache
            .store(prompt, &response, "test-model", None, None)
            .await
            .unwrap();

        // Verify it exists
        let hit = cache.lookup(prompt).await.unwrap();
        assert!(hit.is_some());

        // Invalidate
        cache.invalidate(prompt).await.unwrap();

        // Verify it's gone
        let hit = cache.lookup(prompt).await.unwrap();
        assert!(hit.is_none());
    }

    #[tokio::test]
    async fn test_cache_clear() {
        let cache = create_test_cache();

        // Store multiple entries
        for i in 0..5 {
            let prompt = format!("Prompt {}", i);
            let response = CachedResponse::new(format!("Response {}", i), 10, 50, 0.001);
            cache
                .store(&prompt, &response, "test-model", None, None)
                .await
                .unwrap();
        }

        assert_eq!(cache.entry_count().await, 5);

        // Clear
        cache.clear().await.unwrap();

        assert_eq!(cache.entry_count().await, 0);
    }

    #[tokio::test]
    async fn test_cache_stats() {
        let cache = create_test_cache();

        let response = CachedResponse::new(
            "Response text that is long enough".to_string(),
            10,
            50,
            0.001,
        );
        cache
            .store("prompt1", &response, "model", None, None)
            .await
            .unwrap();
        cache
            .store("prompt2", &response, "model", None, None)
            .await
            .unwrap();

        // Lookup to generate hits/misses
        cache.lookup("prompt1").await.unwrap();
        cache.lookup("prompt1").await.unwrap();
        cache.lookup("nonexistent").await.unwrap();

        let stats = cache.stats().await;
        assert_eq!(stats.total_entries, 2);
        assert_eq!(stats.hit_count, 2);
        assert_eq!(stats.miss_count, 1);
    }

    #[tokio::test]
    async fn test_cache_disabled() {
        let config = SemanticCacheConfig {
            enabled: false,
            ..Default::default()
        };

        let embedder = Arc::new(MockEmbedder::new(128));
        let cache = SemanticCacheManager::with_embedder(embedder, config);

        let response = CachedResponse::new("Response".to_string(), 10, 50, 0.001);

        // Store should be a no-op when disabled
        cache
            .store("prompt", &response, "model", None, None)
            .await
            .unwrap();

        // Lookup should return None when disabled
        let hit = cache.lookup("prompt").await.unwrap();
        assert!(hit.is_none());
    }

    #[tokio::test]
    async fn test_cache_min_response_length() {
        let cache = create_test_cache();

        // Short response should not be cached
        let short_response = CachedResponse::new("Hi".to_string(), 1, 10, 0.001);
        cache
            .store("prompt", &short_response, "model", None, None)
            .await
            .unwrap();

        let hit = cache.lookup("prompt").await.unwrap();
        assert!(hit.is_none()); // Not cached due to min length

        // Longer response should be cached
        let long_response = CachedResponse::new(
            "This is a sufficiently long response that exceeds the minimum length requirement"
                .to_string(),
            20,
            50,
            0.001,
        );
        cache
            .store("prompt2", &long_response, "model", None, None)
            .await
            .unwrap();

        let hit = cache.lookup("prompt2").await.unwrap();
        assert!(hit.is_some());
    }

    #[test]
    fn test_cache_entry_expired() {
        let entry = CacheEntry {
            id: "test".to_string(),
            prompt_hash: "hash".to_string(),
            prompt_preview: "preview".to_string(),
            embedding: vec![0.0; 10],
            response: CachedResponse::new("response".to_string(), 10, 50, 0.001),
            model_used: "model".to_string(),
            created_at: SystemTime::now() - Duration::from_secs(3600),
            expires_at: Some(SystemTime::now() - Duration::from_secs(1)), // Expired
            hit_count: 0,
            last_accessed: SystemTime::now(),
            metadata: CacheMetadata::default(),
        };

        assert!(entry.is_expired());

        let entry_not_expired = CacheEntry {
            expires_at: Some(SystemTime::now() + Duration::from_secs(3600)),
            ..entry.clone()
        };

        assert!(!entry_not_expired.is_expired());

        let entry_no_expiry = CacheEntry {
            expires_at: None,
            ..entry
        };

        assert!(!entry_no_expiry.is_expired());
    }

    #[test]
    fn test_eviction_score() {
        let entry = CacheEntry {
            id: "test".to_string(),
            prompt_hash: "hash".to_string(),
            prompt_preview: "preview".to_string(),
            embedding: vec![0.0; 10],
            response: CachedResponse::new("response".to_string(), 10, 50, 0.001),
            model_used: "model".to_string(),
            created_at: SystemTime::now(),
            expires_at: None,
            hit_count: 10,
            last_accessed: SystemTime::now(),
            metadata: CacheMetadata::default(),
        };

        let score = entry.eviction_score(0.4, 0.6);
        assert!(score > 0.0);

        // Higher hit count should give higher score
        let entry_high_hits = CacheEntry {
            hit_count: 100,
            ..entry
        };
        assert!(entry_high_hits.eviction_score(0.4, 0.6) > score);
    }
}
