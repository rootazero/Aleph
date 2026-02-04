# Design: Prompt Analysis Routing and Semantic Cache

## Overview

This document details the architectural design for P2 Model Router improvements:
1. **Prompt Analyzer** - Extract features from prompt content for intelligent routing
2. **Semantic Cache** - Store and retrieve responses based on semantic similarity

## Architecture Diagrams

### System Context

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                              Aleph Core                                     │
├─────────────────────────────────────────────────────────────────────────────┤
│                                                                              │
│    User Request                                                              │
│         │                                                                    │
│         ▼                                                                    │
│    ┌─────────────────────────────────────────────────────────────────────┐  │
│    │                      IntelligentRouter                              │  │
│    │  ┌─────────────────────────────────────────────────────────────┐   │  │
│    │  │ 1. SemanticCache.lookup(prompt)                              │   │  │
│    │  │    └─ HIT → return cached response                          │   │  │
│    │  │    └─ MISS → continue                                        │   │  │
│    │  └─────────────────────────────────────────────────────────────┘   │  │
│    │                          │                                          │  │
│    │                          ▼                                          │  │
│    │  ┌─────────────────────────────────────────────────────────────┐   │  │
│    │  │ 2. PromptAnalyzer.analyze(prompt) → PromptFeatures          │   │  │
│    │  │    • token_count: 1,234                                     │   │  │
│    │  │    • complexity: 0.72                                        │   │  │
│    │  │    • language: Chinese(0.8), Code(0.2)                      │   │  │
│    │  │    • reasoning_required: true                                │   │  │
│    │  │    • domain: Technical                                       │   │  │
│    │  └─────────────────────────────────────────────────────────────┘   │  │
│    │                          │                                          │  │
│    │                          ▼                                          │  │
│    │  ┌─────────────────────────────────────────────────────────────┐   │  │
│    │  │ 3. ModelMatcher.route_with_features(intent, features)       │   │  │
│    │  │    • Adjust intent based on complexity/reasoning            │   │  │
│    │  │    • Check token_count vs model context window              │   │  │
│    │  │    • Consider language for provider selection               │   │  │
│    │  └─────────────────────────────────────────────────────────────┘   │  │
│    │                          │                                          │  │
│    │                          ▼                                          │  │
│    │  ┌─────────────────────────────────────────────────────────────┐   │  │
│    │  │ 4. RetryOrchestrator.execute(model, prompt)                 │   │  │
│    │  │    • Existing P1 retry/failover logic                       │   │  │
│    │  └─────────────────────────────────────────────────────────────┘   │  │
│    │                          │                                          │  │
│    │                          ▼                                          │  │
│    │  ┌─────────────────────────────────────────────────────────────┐   │  │
│    │  │ 5. SemanticCache.store(prompt, response, model, ttl)        │   │  │
│    │  └─────────────────────────────────────────────────────────────┘   │  │
│    └─────────────────────────────────────────────────────────────────────┘  │
│                                                                              │
└─────────────────────────────────────────────────────────────────────────────┘
```

### Component Architecture

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                         dispatcher/model_router/                            │
├─────────────────────────────────────────────────────────────────────────────┤
│                                                                              │
│  ┌─────────────────────────────────────────────────────────────────────────┐│
│  │                     prompt_analyzer.rs [NEW]                            ││
│  ├─────────────────────────────────────────────────────────────────────────┤│
│  │  pub struct PromptAnalyzer {                                            ││
│  │      tokenizer: Arc<Tokenizer>,                // tiktoken-rs           ││
│  │      lang_detector: LanguageDetector,          // whichlang             ││
│  │      config: PromptAnalyzerConfig,                                      ││
│  │  }                                                                      ││
│  │                                                                          ││
│  │  pub struct PromptFeatures {                                            ││
│  │      pub estimated_tokens: u32,                // Token count           ││
│  │      pub complexity_score: f64,                // 0.0-1.0               ││
│  │      pub primary_language: Language,           // En/Zh/Ja/Mixed        ││
│  │      pub code_ratio: f64,                      // 0.0-1.0               ││
│  │      pub reasoning_indicators: ReasoningLevel, // Low/Medium/High       ││
│  │      pub domain: Domain,                       // General/Technical/... ││
│  │      pub suggested_context_size: ContextSize,  // Small/Medium/Large    ││
│  │      pub analysis_time_us: u64,                // Performance tracking  ││
│  │  }                                                                      ││
│  │                                                                          ││
│  │  impl PromptAnalyzer {                                                  ││
│  │      pub fn analyze(&self, prompt: &str) -> PromptFeatures;             ││
│  │      pub fn analyze_batch(&self, prompts: &[&str]) -> Vec<PromptFeatures>;│
│  │      fn estimate_tokens(&self, text: &str) -> u32;                      ││
│  │      fn calculate_complexity(&self, text: &str) -> f64;                 ││
│  │      fn detect_language(&self, text: &str) -> (Language, f64);          ││
│  │      fn detect_code_ratio(&self, text: &str) -> f64;                    ││
│  │      fn detect_reasoning(&self, text: &str) -> ReasoningLevel;          ││
│  │      fn detect_domain(&self, text: &str) -> Domain;                     ││
│  │  }                                                                      ││
│  └─────────────────────────────────────────────────────────────────────────┘│
│                                                                              │
│  ┌─────────────────────────────────────────────────────────────────────────┐│
│  │                     semantic_cache.rs [NEW]                             ││
│  ├─────────────────────────────────────────────────────────────────────────┤│
│  │  pub struct SemanticCacheManager {                                      ││
│  │      embedder: Arc<dyn TextEmbedder>,          // fastembed/candle      ││
│  │      store: Arc<dyn VectorStore>,              // sqlite-vec            ││
│  │      config: SemanticCacheConfig,                                       ││
│  │  }                                                                      ││
│  │                                                                          ││
│  │  pub struct CacheEntry {                                                ││
│  │      pub id: String,                           // UUID                  ││
│  │      pub prompt_hash: String,                  // For exact match       ││
│  │      pub embedding: Vec<f32>,                  // 384-dim vector        ││
│  │      pub response: CachedResponse,             // Stored response       ││
│  │      pub model_used: String,                   // Model that generated  ││
│  │      pub created_at: SystemTime,                                        ││
│  │      pub expires_at: Option<SystemTime>,       // TTL                   ││
│  │      pub hit_count: u32,                       // For LFU               ││
│  │      pub last_accessed: SystemTime,            // For LRU               ││
│  │      pub metadata: CacheMetadata,              // Additional context    ││
│  │  }                                                                      ││
│  │                                                                          ││
│  │  pub struct CachedResponse {                                            ││
│  │      pub content: String,                      // Response text         ││
│  │      pub tokens_used: u32,                     // Actual tokens         ││
│  │      pub latency_ms: u64,                      // Original latency      ││
│  │      pub cost_usd: f64,                        // Original cost         ││
│  │  }                                                                      ││
│  │                                                                          ││
│  │  impl SemanticCacheManager {                                            ││
│  │      pub async fn lookup(&self, prompt: &str) -> Option<CacheHit>;      ││
│  │      pub async fn store(&self, prompt: &str, response: &CachedResponse, ││
│  │                         model: &str, ttl: Option<Duration>) -> Result<()>;│
│  │      pub async fn invalidate(&self, prompt: &str) -> Result<()>;        ││
│  │      pub async fn clear(&self) -> Result<()>;                           ││
│  │      pub async fn stats(&self) -> CacheStats;                           ││
│  │      fn compute_embedding(&self, text: &str) -> Vec<f32>;               ││
│  │      fn find_similar(&self, embedding: &[f32], threshold: f64)          ││
│  │          -> Option<CacheEntry>;                                         ││
│  │      fn evict_expired(&self) -> Result<usize>;                          ││
│  │      fn evict_by_policy(&self, target_count: usize) -> Result<usize>;   ││
│  │  }                                                                      ││
│  └─────────────────────────────────────────────────────────────────────────┘│
│                                                                              │
│  ┌─────────────────────────────────────────────────────────────────────────┐│
│  │              intelligent_router.rs [NEW - Coordination]                 ││
│  ├─────────────────────────────────────────────────────────────────────────┤│
│  │  pub struct IntelligentRouter {                                         ││
│  │      orchestrated_router: Arc<OrchestratedRouter>,  // P1               ││
│  │      prompt_analyzer: Arc<PromptAnalyzer>,          // P2               ││
│  │      semantic_cache: Arc<SemanticCacheManager>,     // P2               ││
│  │      config: IntelligentRouterConfig,                                   ││
│  │  }                                                                      ││
│  │                                                                          ││
│  │  impl IntelligentRouter {                                               ││
│  │      pub async fn route_and_execute(                                    ││
│  │          &self,                                                         ││
│  │          request: &RoutingRequest,                                      ││
│  │      ) -> Result<IntelligentRoutingResult, RoutingError>;               ││
│  │                                                                          ││
│  │      pub async fn route_with_cache_bypass(                              ││
│  │          &self,                                                         ││
│  │          request: &RoutingRequest,                                      ││
│  │      ) -> Result<IntelligentRoutingResult, RoutingError>;               ││
│  │                                                                          ││
│  │      pub fn analyze_only(&self, prompt: &str) -> PromptFeatures;        ││
│  │      pub async fn cache_stats(&self) -> CacheStats;                     ││
│  │  }                                                                      ││
│  └─────────────────────────────────────────────────────────────────────────┘│
│                                                                              │
└─────────────────────────────────────────────────────────────────────────────┘
```

## Data Flow

### Prompt Analysis Flow

```
Input: "请用 Rust 写一个快速排序算法，并解释时间复杂度"

┌─────────────────────────────────────────────────────────────────┐
│ Step 1: Token Estimation                                        │
├─────────────────────────────────────────────────────────────────┤
│ tokenizer.encode(prompt).len() → 42 tokens                      │
│ (Fast BPE encoding, no API call needed)                         │
└─────────────────────────────────────────────────────────────────┘
                              │
                              ▼
┌─────────────────────────────────────────────────────────────────┐
│ Step 2: Complexity Scoring                                      │
├─────────────────────────────────────────────────────────────────┤
│ Factors:                                                        │
│ • Sentence count: 2                                             │
│ • Average word length: 4.2                                      │
│ • Nested structure: false                                       │
│ • Technical terms: ["Rust", "快速排序", "时间复杂度"]            │
│ • Multi-step request: true ("写" + "解释")                      │
│                                                                  │
│ complexity_score = 0.65                                         │
└─────────────────────────────────────────────────────────────────┘
                              │
                              ▼
┌─────────────────────────────────────────────────────────────────┐
│ Step 3: Language Detection                                      │
├─────────────────────────────────────────────────────────────────┤
│ whichlang::detect_language(prompt):                             │
│ • Chinese: 0.75 confidence                                      │
│ • English terms: ["Rust"]                                       │
│ • Code markers: none yet (request, not code)                    │
│                                                                  │
│ primary_language = Chinese, code_ratio = 0.0                    │
└─────────────────────────────────────────────────────────────────┘
                              │
                              ▼
┌─────────────────────────────────────────────────────────────────┐
│ Step 4: Reasoning Detection                                     │
├─────────────────────────────────────────────────────────────────┤
│ Indicators found:                                               │
│ • "解释" → explanation required                                 │
│ • "时间复杂度" → computational complexity analysis              │
│ • Algorithm request → step-by-step reasoning                    │
│                                                                  │
│ reasoning_level = High                                          │
└─────────────────────────────────────────────────────────────────┘
                              │
                              ▼
┌─────────────────────────────────────────────────────────────────┐
│ Step 5: Domain Classification                                   │
├─────────────────────────────────────────────────────────────────┤
│ Terms matched:                                                  │
│ • Programming: ["Rust", "算法"]                                 │
│ • CS Theory: ["快速排序", "时间复杂度"]                          │
│                                                                  │
│ domain = Technical(Programming)                                 │
└─────────────────────────────────────────────────────────────────┘
                              │
                              ▼
┌─────────────────────────────────────────────────────────────────┐
│ Output: PromptFeatures                                          │
├─────────────────────────────────────────────────────────────────┤
│ {                                                               │
│   estimated_tokens: 42,                                         │
│   complexity_score: 0.65,                                       │
│   primary_language: Chinese,                                    │
│   code_ratio: 0.0,                                              │
│   reasoning_indicators: High,                                   │
│   domain: Technical(Programming),                               │
│   suggested_context_size: Medium,                               │
│   analysis_time_us: 1,234,                                      │
│ }                                                               │
└─────────────────────────────────────────────────────────────────┘
```

### Semantic Cache Flow

```
┌─────────────────────────────────────────────────────────────────┐
│                        Cache Lookup                              │
├─────────────────────────────────────────────────────────────────┤
│                                                                  │
│  Input: "What is quicksort in Rust?"                            │
│                                                                  │
│  Step 1: Exact Match (Fast Path)                                │
│  ┌──────────────────────────────────────────────────────────┐   │
│  │ hash = sha256(normalize(prompt))                         │   │
│  │ SELECT * FROM cache WHERE prompt_hash = ? AND            │   │
│  │        expires_at > NOW()                                │   │
│  │                                                          │   │
│  │ Result: MISS (different wording)                         │   │
│  └──────────────────────────────────────────────────────────┘   │
│                          │                                       │
│                          ▼                                       │
│  Step 2: Semantic Match (Embedding Search)                      │
│  ┌──────────────────────────────────────────────────────────┐   │
│  │ embedding = embedder.encode(prompt)  // [0.12, -0.34, ...]│   │
│  │                                                          │   │
│  │ SELECT *, vec_distance_cosine(embedding, ?) as distance  │   │
│  │ FROM cache                                               │   │
│  │ WHERE expires_at > NOW()                                 │   │
│  │ ORDER BY distance ASC                                    │   │
│  │ LIMIT 1                                                  │   │
│  │                                                          │   │
│  │ Best match: "Explain quicksort implementation in Rust"   │   │
│  │ Similarity: 0.91 (> threshold 0.85)                      │   │
│  └──────────────────────────────────────────────────────────┘   │
│                          │                                       │
│                          ▼                                       │
│  Step 3: Return Cached Response                                 │
│  ┌──────────────────────────────────────────────────────────┐   │
│  │ CacheHit {                                               │   │
│  │   response: CachedResponse { content: "...", ... },      │   │
│  │   similarity: 0.91,                                      │   │
│  │   original_prompt: "Explain quicksort in Rust",          │   │
│  │   model_used: "claude-sonnet",                           │   │
│  │   cached_at: "2024-01-15T10:30:00Z",                     │   │
│  │   hit_type: Semantic,  // vs Exact                       │   │
│  │ }                                                        │   │
│  └──────────────────────────────────────────────────────────┘   │
│                                                                  │
└─────────────────────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────────────────────┐
│                        Cache Storage                             │
├─────────────────────────────────────────────────────────────────┤
│                                                                  │
│  Input: prompt, response, model, ttl                            │
│                                                                  │
│  Step 1: Generate Entry                                         │
│  ┌──────────────────────────────────────────────────────────┐   │
│  │ CacheEntry {                                             │   │
│  │   id: uuid(),                                            │   │
│  │   prompt_hash: sha256(normalize(prompt)),                │   │
│  │   embedding: embedder.encode(prompt),                    │   │
│  │   response: CachedResponse { ... },                      │   │
│  │   model_used: "claude-sonnet",                           │   │
│  │   created_at: now(),                                     │   │
│  │   expires_at: now() + ttl,                               │   │
│  │   hit_count: 0,                                          │   │
│  │   last_accessed: now(),                                  │   │
│  │   metadata: { task_intent: "CodeGeneration", ... },      │   │
│  │ }                                                        │   │
│  └──────────────────────────────────────────────────────────┘   │
│                          │                                       │
│                          ▼                                       │
│  Step 2: Check Capacity & Evict if Needed                       │
│  ┌──────────────────────────────────────────────────────────┐   │
│  │ if cache.size() >= config.max_entries:                   │   │
│  │     // Evict expired entries first                       │   │
│  │     evict_expired()                                      │   │
│  │                                                          │   │
│  │     // Then apply eviction policy                        │   │
│  │     if still_full:                                       │   │
│  │         match config.eviction_policy:                    │   │
│  │             LRU => evict_least_recently_used(10%)        │   │
│  │             LFU => evict_least_frequently_used(10%)      │   │
│  │             Hybrid => evict_by_score(age, hits)          │   │
│  └──────────────────────────────────────────────────────────┘   │
│                          │                                       │
│                          ▼                                       │
│  Step 3: Insert Entry                                           │
│  ┌──────────────────────────────────────────────────────────┐   │
│  │ INSERT INTO cache (id, prompt_hash, embedding, ...)      │   │
│  │ VALUES (?, ?, vec_f32(?), ...)                           │   │
│  └──────────────────────────────────────────────────────────┘   │
│                                                                  │
└─────────────────────────────────────────────────────────────────┘
```

## Database Schema

```sql
-- Semantic Cache Table (sqlite-vec enabled)
CREATE TABLE semantic_cache (
    id TEXT PRIMARY KEY,
    prompt_hash TEXT NOT NULL,
    -- Vector stored via sqlite-vec extension
    embedding BLOB NOT NULL,  -- 384 x f32 = 1536 bytes

    -- Cached response
    response_content TEXT NOT NULL,
    response_tokens INTEGER NOT NULL,
    response_latency_ms INTEGER NOT NULL,
    response_cost_usd REAL NOT NULL,

    -- Metadata
    model_used TEXT NOT NULL,
    task_intent TEXT,
    prompt_features_json TEXT,  -- Serialized PromptFeatures

    -- Timestamps
    created_at INTEGER NOT NULL,  -- Unix timestamp
    expires_at INTEGER,           -- NULL = never expires
    last_accessed INTEGER NOT NULL,

    -- Stats
    hit_count INTEGER DEFAULT 0,

    -- Indexes
    UNIQUE(prompt_hash)
);

-- Create vector index for ANN search
CREATE INDEX idx_cache_embedding ON semantic_cache(embedding);
CREATE INDEX idx_cache_expires ON semantic_cache(expires_at) WHERE expires_at IS NOT NULL;
CREATE INDEX idx_cache_lru ON semantic_cache(last_accessed);

-- Prompt analysis cache (optional, for expensive analysis)
CREATE TABLE prompt_analysis_cache (
    prompt_hash TEXT PRIMARY KEY,
    features_json TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL  -- Short TTL, e.g., 1 hour
);
```

## Configuration

```toml
# config.toml additions

[model_routing.prompt_analysis]
enabled = true

# Token estimation
tokenizer = "cl100k_base"  # OpenAI's tiktoken encoding

# Complexity thresholds
complexity_weights = { length = 0.2, structure = 0.3, technical = 0.3, multi_step = 0.2 }
high_complexity_threshold = 0.7
low_complexity_threshold = 0.3

# Language detection
supported_languages = ["en", "zh", "ja"]
mixed_language_threshold = 0.3  # If secondary lang > 30%, mark as Mixed

# Reasoning detection keywords
reasoning_indicators = [
    "explain", "why", "how", "analyze", "compare", "evaluate",
    "解释", "为什么", "如何", "分析", "比较", "评估",
    "step by step", "chain of thought", "逐步", "推理"
]

# Domain detection
technical_domains = ["programming", "math", "science", "engineering"]
technical_keywords_file = "data/technical_keywords.json"  # Optional external file


[model_routing.semantic_cache]
enabled = true

# Embedding model
embedding_model = "bge-small-en-v1.5"  # 384 dimensions
embedding_device = "cpu"  # or "metal" for Apple Silicon

# Similarity matching
similarity_threshold = 0.85        # Minimum cosine similarity for cache hit
exact_match_priority = true        # Check exact hash match first

# Capacity limits
max_entries = 10000                # Maximum cached entries
max_memory_mb = 100                # Approximate memory limit

# TTL settings
default_ttl_secs = 86400           # 24 hours
max_ttl_secs = 604800              # 7 days
respect_cache_control = true       # Honor API response cache headers

# Eviction policy
eviction_policy = "hybrid"         # "lru", "lfu", or "hybrid"
eviction_batch_size = 100          # Entries to evict per batch
hybrid_age_weight = 0.4            # Weight for age in hybrid scoring
hybrid_hits_weight = 0.6           # Weight for hit count in hybrid scoring

# Performance
async_storage = true               # Non-blocking cache writes
prefetch_embeddings = false        # Pre-compute embeddings on startup

# Exclusions (never cache these)
exclude_intents = ["PrivacySensitive", "ImageGeneration"]
exclude_models = []                # Models whose responses shouldn't be cached
min_response_length = 50           # Don't cache very short responses
```

## Type Definitions

```rust
// prompt_analyzer.rs

/// Language detected in the prompt
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Language {
    English,
    Chinese,
    Japanese,
    Mixed,  // Multiple languages detected
    Unknown,
}

/// Level of reasoning required for the prompt
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReasoningLevel {
    Low,     // Simple factual query
    Medium,  // Some explanation needed
    High,    // Multi-step reasoning, analysis, or chain-of-thought
}

/// Domain classification for routing
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Domain {
    General,
    Technical(TechnicalDomain),
    Creative,
    Conversational,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TechnicalDomain {
    Programming,
    Mathematics,
    Science,
    Engineering,
    Other(String),
}

/// Suggested context window size based on analysis
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ContextSize {
    Small,   // < 4K tokens expected
    Medium,  // 4K - 32K tokens expected
    Large,   // > 32K tokens expected
}

/// Complete analysis result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptFeatures {
    pub estimated_tokens: u32,
    pub complexity_score: f64,  // 0.0 - 1.0
    pub primary_language: Language,
    pub language_confidence: f64,
    pub code_ratio: f64,  // 0.0 - 1.0
    pub reasoning_indicators: ReasoningLevel,
    pub domain: Domain,
    pub suggested_context_size: ContextSize,
    pub analysis_time_us: u64,

    // Additional metadata
    pub has_code_blocks: bool,
    pub question_count: u32,
    pub imperative_count: u32,  // Commands like "write", "explain"
}

/// Configuration for prompt analyzer
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptAnalyzerConfig {
    pub tokenizer_name: String,
    pub complexity_weights: ComplexityWeights,
    pub high_complexity_threshold: f64,
    pub low_complexity_threshold: f64,
    pub reasoning_keywords: Vec<String>,
    pub technical_keywords: HashMap<TechnicalDomain, Vec<String>>,
}


// semantic_cache.rs

/// Cache hit result
#[derive(Debug, Clone)]
pub struct CacheHit {
    pub entry: CacheEntry,
    pub similarity: f64,
    pub hit_type: CacheHitType,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CacheHitType {
    Exact,     // Hash match
    Semantic,  // Embedding similarity match
}

/// Cache statistics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheStats {
    pub total_entries: usize,
    pub total_size_bytes: usize,
    pub hit_count: u64,
    pub miss_count: u64,
    pub hit_rate: f64,
    pub exact_hits: u64,
    pub semantic_hits: u64,
    pub evictions: u64,
    pub avg_similarity: f64,
    pub oldest_entry_age_secs: u64,
}

/// Configuration for semantic cache
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SemanticCacheConfig {
    pub enabled: bool,
    pub embedding_model: String,
    pub embedding_dimensions: usize,  // 384 for bge-small
    pub similarity_threshold: f64,
    pub exact_match_priority: bool,
    pub max_entries: usize,
    pub max_memory_bytes: usize,
    pub default_ttl: Duration,
    pub max_ttl: Duration,
    pub eviction_policy: EvictionPolicy,
    pub exclude_intents: Vec<TaskIntent>,
    pub exclude_models: Vec<String>,
    pub min_response_length: usize,
    pub async_storage: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum EvictionPolicy {
    LRU,
    LFU,
    Hybrid { age_weight: f64, hits_weight: f64 },
}
```

## Integration with Routing

```rust
// matcher.rs - Modified route_with_features method

impl ModelMatcher {
    /// Route with prompt features for enhanced decision making
    pub fn route_with_features(
        &self,
        intent: &TaskIntent,
        features: &PromptFeatures,
    ) -> Result<ModelProfile, RoutingError> {
        // 1. Adjust intent based on features
        let effective_intent = self.adjust_intent(intent, features);

        // 2. Filter models by context window requirement
        let candidates = self.filter_by_context_size(
            &effective_intent,
            features.suggested_context_size,
        );

        // 3. Apply language preference
        let candidates = self.apply_language_preference(
            candidates,
            &features.primary_language,
        );

        // 4. Apply reasoning capability filter
        let candidates = if features.reasoning_indicators == ReasoningLevel::High {
            self.filter_by_capability(candidates, Capability::Reasoning)
        } else {
            candidates
        };

        // 5. Apply cost strategy to remaining candidates
        self.apply_cost_strategy(candidates)
    }

    fn adjust_intent(
        &self,
        intent: &TaskIntent,
        features: &PromptFeatures,
    ) -> TaskIntent {
        // If intent is GeneralChat but features suggest otherwise
        if *intent == TaskIntent::GeneralChat {
            if features.complexity_score > 0.7
                && features.reasoning_indicators == ReasoningLevel::High {
                return TaskIntent::Reasoning;
            }
            if features.code_ratio > 0.5 {
                return TaskIntent::CodeGeneration;
            }
            if features.domain == Domain::Technical(TechnicalDomain::Programming) {
                return TaskIntent::CodeGeneration;
            }
        }
        intent.clone()
    }

    fn filter_by_context_size(
        &self,
        intent: &TaskIntent,
        size: ContextSize,
    ) -> Vec<&ModelProfile> {
        let min_context = match size {
            ContextSize::Small => 4_000,
            ContextSize::Medium => 32_000,
            ContextSize::Large => 128_000,
        };

        self.profiles
            .values()
            .filter(|p| p.max_context.unwrap_or(4_000) >= min_context)
            .collect()
    }

    fn apply_language_preference(
        &self,
        candidates: Vec<&ModelProfile>,
        language: &Language,
    ) -> Vec<&ModelProfile> {
        // Prefer models known to be strong in the detected language
        // This is a soft preference, not a hard filter
        let mut scored: Vec<_> = candidates
            .into_iter()
            .map(|p| {
                let lang_score = self.language_affinity(p, language);
                (p, lang_score)
            })
            .collect();

        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
        scored.into_iter().map(|(p, _)| p).collect()
    }
}
```

## Error Handling

```rust
/// Errors specific to prompt analysis
#[derive(Debug, thiserror::Error)]
pub enum PromptAnalysisError {
    #[error("Tokenizer initialization failed: {0}")]
    TokenizerInit(String),

    #[error("Analysis timeout after {0}ms")]
    Timeout(u64),

    #[error("Empty prompt provided")]
    EmptyPrompt,
}

/// Errors specific to semantic cache
#[derive(Debug, thiserror::Error)]
pub enum SemanticCacheError {
    #[error("Embedding model failed: {0}")]
    EmbeddingFailed(String),

    #[error("Vector store error: {0}")]
    VectorStore(String),

    #[error("Cache capacity exceeded")]
    CapacityExceeded,

    #[error("Entry not found: {0}")]
    NotFound(String),

    #[error("Serialization error: {0}")]
    Serialization(String),
}
```

## Performance Considerations

### Prompt Analysis Performance

| Operation | Target Latency | Notes |
|-----------|---------------|-------|
| Token estimation | < 1ms | BPE encoding is O(n) |
| Complexity scoring | < 2ms | Regex-based, cached patterns |
| Language detection | < 1ms | whichlang is fast |
| Code ratio detection | < 1ms | Simple regex counting |
| Reasoning detection | < 1ms | Keyword matching |
| **Total analysis** | **< 5ms** | All operations combined |

### Semantic Cache Performance

| Operation | Target Latency | Notes |
|-----------|---------------|-------|
| Exact hash lookup | < 1ms | SQLite index |
| Embedding generation | < 50ms | Local model, batched |
| Vector similarity search | < 10ms | sqlite-vec ANN |
| Cache write | < 5ms | Async, non-blocking |
| **Cache hit (exact)** | **< 1ms** | Hash only |
| **Cache hit (semantic)** | **< 60ms** | Embedding + search |

### Memory Footprint

| Component | Memory Usage |
|-----------|-------------|
| Tokenizer (cl100k) | ~10 MB |
| Embedding model (bge-small) | ~50 MB |
| Cache entries (10K) | ~20 MB (embeddings only) |
| **Total** | **~80 MB** |

## Trade-offs and Decisions

### Decision 1: Local vs API Embeddings

**Options**:
- A) Use OpenAI's embedding API
- B) Use local embedding model (bge-small)

**Decision**: **Option B - Local embeddings**

**Rationale**:
- No network latency for cache lookups
- No additional API costs
- Works offline
- Privacy (prompts never leave device for caching)
- bge-small provides excellent quality at small size

### Decision 2: Similarity Threshold

**Options**:
- A) Fixed threshold (0.85)
- B) Adaptive threshold based on domain
- C) User-configurable per-intent

**Decision**: **Option A with future Option C**

**Rationale**:
- Start simple with proven threshold
- 0.85 is conservative enough to avoid false positives
- Make configurable in config.toml for power users
- Collect data to inform future adaptive thresholds

### Decision 3: Cache Invalidation Strategy

**Options**:
- A) TTL only
- B) Manual invalidation only
- C) Hybrid (TTL + semantic versioning)

**Decision**: **Option C - Hybrid**

**Rationale**:
- TTL handles staleness automatically
- Manual invalidation for user-triggered refresh
- Model change triggers cache clear for that model
- Intent-based exclusions for non-cacheable content

### Decision 4: Embedding Storage Format

**Options**:
- A) Raw f32 vectors in SQLite BLOB
- B) Quantized int8 vectors
- C) External vector database (Qdrant, etc.)

**Decision**: **Option A - Raw f32 with sqlite-vec**

**Rationale**:
- sqlite-vec handles ANN efficiently
- No quantization error
- Single database for all cache data
- Simpler deployment (no external services)
- 10K entries × 384 dims × 4 bytes = ~15 MB (acceptable)
