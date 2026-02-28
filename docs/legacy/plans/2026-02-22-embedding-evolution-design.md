# Embedding Evolution Design: Abstract Provider + Lazy Migration

> **Goal**: Decouple Aleph's embedding system from the hardcoded 384-dim multilingual-e5-small model, enabling configurable providers (local/remote), dynamic dimensions, and zero-downtime model switching.

## 1. Problem Statement

### Current State

Aleph's embedding system is tightly coupled to a single local model:

- `SmartEmbedder` hardcodes `multilingual-e5-small` (384-dim) via fastembed
- `CURRENT_EMBEDDING_DIM = 384` in `database/core.rs`
- vec0 virtual tables use `float[384]` in static schema SQL
- No abstraction for remote embedding providers (OpenAI, Volcengine, etc.)
- `TextEmbedder` trait exists in `dispatcher/semantic_cache/` but `SmartEmbedder` does not implement it

### Impact

1. **Memory migration blocked**: Switching models requires full re-indexing with no path forward
2. **No hybrid capability**: Cannot use higher-quality LLM embeddings for L0/L1 overviews
3. **Inflexible**: Cannot leverage Matryoshka-capable models or adaptive dimensions
4. **Config mismatch**: `MemoryConfig.embedding_model` defaults to `bge-small-zh-v1.5` but actual model is `multilingual-e5-small`

### OpenViking Reference

OpenViking solves this with:
- Config-driven `dimension` field in `ov.conf`
- Dynamic schema creation: `"Dim": vector_dim` parameter
- `truncate_and_normalize()`: truncate to target dimension + L2 normalize
- Provider abstraction: OpenAI / Volcengine / VikingDB embedders
- Auto-detect dimension via test API call

## 2. Design Goals

| Goal | Priority |
|------|----------|
| Config-driven dimension (no hardcoding) | P0 |
| Abstract `EmbeddingProvider` trait (local + remote) | P0 |
| Backward-compatible with existing 384-dim data | P0 |
| Lazy background re-embedding migration | P1 |
| `truncate_and_normalize` compatibility bridge | P1 |
| Per-fact embedding model metadata | P1 |
| Remote provider support (OpenAI, custom) | P2 |

## 3. Architecture

### 3.1 EmbeddingProvider Trait

Unify local and remote embedding behind a single async trait:

```rust
// core/src/memory/embedding_provider.rs

#[async_trait::async_trait]
pub trait EmbeddingProvider: Send + Sync {
    /// Generate embedding for a single text
    async fn embed(&self, text: &str) -> Result<Vec<f32>, AlephError>;

    /// Generate embeddings for multiple texts (batch)
    async fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, AlephError>;

    /// Get the output dimension of this provider
    fn dimensions(&self) -> usize;

    /// Get the model name (e.g., "multilingual-e5-small", "text-embedding-3-small")
    fn model_name(&self) -> &str;

    /// Get the provider type (e.g., "local", "openai", "volcengine")
    fn provider_type(&self) -> &str;
}
```

### 3.2 Provider Implementations

```
EmbeddingProvider (trait)
├── LocalEmbeddingProvider      # Wraps SmartEmbedder (fastembed)
├── OpenAiEmbeddingProvider     # OpenAI text-embedding-3-* via HTTP
└── CustomEmbeddingProvider     # User-defined via config (any OpenAI-compatible API)
```

**LocalEmbeddingProvider**: Thin wrapper around existing `SmartEmbedder`, implementing the new trait.

**OpenAiEmbeddingProvider**: HTTP client calling `/v1/embeddings` endpoint. Supports native `dimensions` parameter for Matryoshka-capable models (text-embedding-3-*).

**CustomEmbeddingProvider**: Configurable endpoint URL for any OpenAI-compatible embedding API (e.g., Ollama, vLLM, DeepSeek).

### 3.3 Configuration

Add `[memory.embedding]` section to `config.toml`:

```toml
[memory.embedding]
# Provider type: "local" | "openai" | "custom"
provider = "local"

# Model name (provider-specific)
model = "multilingual-e5-small"

# Output dimension (must match vec0 table)
# For Matryoshka models, can be smaller than native dimension
dimension = 384

# --- Remote provider settings (only for openai/custom) ---
# Environment variable name for API key
api_key_env = "OPENAI_API_KEY"
# API base URL (for custom providers or proxy)
api_base = ""
# Request timeout in milliseconds
timeout_ms = 10000
# Batch size for remote requests
batch_size = 32
```

**Rust config struct**:

```rust
// core/src/config/types/memory.rs

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct EmbeddingConfig {
    #[serde(default = "default_embedding_provider")]
    pub provider: String,  // "local" | "openai" | "custom"

    #[serde(default = "default_embedding_model_name")]
    pub model: String,

    #[serde(default = "default_embedding_dimension")]
    pub dimension: u32,

    #[serde(default)]
    pub api_key_env: Option<String>,

    #[serde(default)]
    pub api_base: Option<String>,

    #[serde(default = "default_embedding_timeout_ms")]
    pub timeout_ms: u64,

    #[serde(default = "default_embedding_batch_size")]
    pub batch_size: u32,
}
```

### 3.4 Dynamic vec0 Table Creation

Replace hardcoded schema SQL with dynamic dimension:

```rust
// database/core.rs

fn vec_schema_sql(dim: u32) -> String {
    format!(r#"
        CREATE VIRTUAL TABLE IF NOT EXISTS memories_vec USING vec0(
            embedding float[{dim}]
        );
        CREATE VIRTUAL TABLE IF NOT EXISTS facts_vec USING vec0(
            embedding float[{dim}]
        );
    "#, dim = dim)
}
```

The `CURRENT_EMBEDDING_DIM` constant becomes a configuration-read value at database initialization.

### 3.5 Per-Fact Embedding Metadata

Add `embedding_model` column to `memory_facts` table:

```sql
ALTER TABLE memory_facts ADD COLUMN embedding_model TEXT NOT NULL DEFAULT '';
```

This records which model generated each fact's embedding, enabling:
- Detection of stale embeddings after model switch
- Mixed-model queries (if needed in future)
- Migration progress tracking

### 3.6 Lazy Migration Engine

When the configured model differs from stored `embedding_model` on facts:

```rust
// core/src/memory/embedding_migration.rs

pub struct EmbeddingMigration {
    database: Arc<VectorDatabase>,
    provider: Arc<dyn EmbeddingProvider>,
    batch_size: usize,
}

impl EmbeddingMigration {
    /// Migrate facts with outdated embeddings in background
    pub async fn run_batch(&self) -> Result<MigrationProgress> {
        // 1. SELECT facts WHERE embedding_model != current_model LIMIT batch_size
        // 2. Re-embed in batch
        // 3. Update embedding BLOB + vec0 row + embedding_model column
        // 4. Return progress (migrated / remaining)
    }
}
```

Migration runs during:
- DreamDaemon idle periods
- Compression daemon idle cycles
- Explicit `aleph memory migrate` CLI command

### 3.7 truncate_and_normalize

Compatibility bridge when a remote model returns vectors larger than configured dimension:

```rust
/// Truncate embedding to target dimension and L2 normalize
pub fn truncate_and_normalize(embedding: Vec<f32>, target_dim: usize) -> Vec<f32> {
    if embedding.len() <= target_dim {
        return embedding;
    }
    let truncated = &embedding[..target_dim];
    let norm: f32 = truncated.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 {
        truncated.iter().map(|x| x / norm).collect()
    } else {
        truncated.to_vec()
    }
}
```

This is useful when:
- Using `text-embedding-3-large` (3072-dim) but wanting 1024-dim storage
- Switching to a higher-dim model without rebuilding vec0 tables immediately

### 3.8 Dimension Change Strategy

When `config.memory.embedding.dimension` differs from stored `schema_info.embedding_dimension`:

| Scenario | Action |
|----------|--------|
| Same dimension, different model | Lazy re-embed (keep vec0 table) |
| Smaller dimension → larger | Rebuild vec0 tables, lazy re-embed |
| Larger dimension → smaller | Rebuild vec0 tables, lazy re-embed (truncate old) |
| First-time setup | Create vec0 with configured dimension |

**Rebuild** = drop and recreate vec0 virtual tables (not the base `memory_facts` table). The BLOB embeddings in `memory_facts.embedding` are model-specific and will be re-generated during migration.

## 4. Data Flow

```
User Query
    │
    ▼
EmbeddingProvider.embed(query)   ← Config determines provider
    │
    ▼
Vec<f32> (N-dim)
    │
    ├─ if len > configured_dim ──→ truncate_and_normalize()
    │
    ▼
facts_vec KNN search (configured_dim)
    │
    ▼
Results (with embedding_model metadata)
```

## 5. Migration Path

### Phase 1: Foundation (This Design)
1. Create `EmbeddingProvider` trait
2. Wrap `SmartEmbedder` as `LocalEmbeddingProvider`
3. Add `EmbeddingConfig` to config system
4. Make vec0 tables use dynamic dimension
5. Add `embedding_model` column to `memory_facts`
6. Replace `CURRENT_EMBEDDING_DIM` constant with config value
7. Add `truncate_and_normalize` utility

### Phase 2: Remote Providers
8. Implement `OpenAiEmbeddingProvider`
9. Implement `CustomEmbeddingProvider`
10. Factory function to create provider from config

### Phase 3: Migration Engine
11. Implement `EmbeddingMigration` background task
12. Integrate with DreamDaemon / CompressionDaemon
13. Add `aleph memory migrate` CLI command

## 6. Backward Compatibility

- Default config matches current behavior: `provider = "local"`, `model = "multilingual-e5-small"`, `dimension = 384`
- Existing databases with no `embedding_model` column are auto-migrated (column added with default value)
- Existing 384-dim vec0 tables remain functional until dimension change is configured
- `SmartEmbedder` continues to work as-is, wrapped by `LocalEmbeddingProvider`

## 7. Testing Strategy

| Test | Type |
|------|------|
| EmbeddingProvider trait compliance | Unit (mock provider) |
| LocalEmbeddingProvider wrapping | Integration |
| truncate_and_normalize correctness | Unit |
| Dynamic vec0 table creation | Integration |
| Dimension change detection | Unit |
| Config parsing | Unit |
| Migration batch processing | Integration (mock) |

## 8. Files Changed

| File | Change |
|------|--------|
| `core/src/memory/embedding_provider.rs` | **NEW** - EmbeddingProvider trait + LocalEmbeddingProvider |
| `core/src/memory/embedding_migration.rs` | **NEW** - Lazy migration engine |
| `core/src/config/types/memory.rs` | Add `EmbeddingConfig` struct |
| `core/src/memory/smart_embedder.rs` | Implement EmbeddingProvider for SmartEmbedder |
| `core/src/memory/database/core.rs` | Dynamic dimension, remove CURRENT_EMBEDDING_DIM constant |
| `core/src/memory/database/migration.rs` | Add `embedding_model` column migration |
| `core/src/memory/mod.rs` | Re-export new types |
| `core/src/memory/database/facts.rs` | Store/read `embedding_model` per fact |
