# Smart Embedder Design: Multilingual E5 with TTL Lazy Loading

**Date**: 2026-01-29
**Status**: Approved
**Author**: Claude + User

---

## Overview

Replace the current `bge-small-zh-v1.5` embedding model with `multilingual-e5-small` and implement a TTL-based lazy loading strategy ("Keep-Alive" pattern) to balance memory efficiency with response latency.

### Key Decisions

| Decision | Choice |
|----------|--------|
| Vector dimension migration | Drop and rebuild (512 → 384) |
| Model scope | Single shared instance via `Arc` |
| TTL strategy | Fixed 300s, no task awareness |
| Cleanup task lifecycle | `CancellationToken` for graceful shutdown |
| Configuration location | `semantic_cache` config block |
| Future extensibility | Reranker trait pre-reserved |

---

## Architecture

### Component Diagram

```
┌─────────────────────────────────────────────────────────┐
│                    SmartEmbedder                        │
│  ┌─────────────────────────────────────────────────┐   │
│  │  Arc<Mutex<InnerState>>                         │   │
│  │  ├─ model: Option<TextEmbedding>                │   │
│  │  └─ last_used: Instant                          │   │
│  └─────────────────────────────────────────────────┘   │
│  ┌─────────────────────────────────────────────────┐   │
│  │  CancellationToken (shutdown signal)            │   │
│  └─────────────────────────────────────────────────┘   │
│  ┌─────────────────────────────────────────────────┐   │
│  │  Background Cleaner Task (5s interval)          │   │
│  └─────────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────────┘
          │
          │ Arc<SmartEmbedder>
          ├──────────────────────┐
          ▼                      ▼
   MemoryIngestion        SemanticCache
   MemoryRetrieval        (TextEmbedder trait)
```

### TTL Strategy Flow

```
Cold Start (first request)
    │
    ▼
Load Model (~150ms) ─────────────────┐
    │                                │
    ▼                                │
Update last_used = now()             │
    │                                │
    ▼                                │
Execute Embedding                    │
    │                                │
    ▼                                │
Return Result                        │
    │                                │
    │  ┌─────────────────────────────┘
    │  │
    │  │  Within 300s
    │  │
    ▼  ▼
Hot Call (0ms latency)
    │
    ▼
Update last_used = now()
    │
    ▼
Execute Embedding
    │
    │
    │  No activity for 300s
    │
    ▼
Background Cleaner unloads model (Drop)
```

---

## Implementation

### Data Structures

```rust
// core/src/memory/smart_embedder.rs

use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use fastembed::{TextEmbedding, InitOptions, EmbeddingModel};

/// Multilingual E5-Small vector dimension
pub const EMBEDDING_DIM: usize = 384;

/// Default TTL in seconds
pub const DEFAULT_MODEL_TTL_SECS: u64 = 300;

#[derive(Clone)]
pub struct SmartEmbedder {
    state: Arc<Mutex<InnerState>>,
    cancel_token: CancellationToken,
    ttl: Duration,
    cache_dir: PathBuf,
}

struct InnerState {
    model: Option<TextEmbedding>,
    last_used: Instant,
}
```

### Lifecycle Management

```rust
impl SmartEmbedder {
    pub fn new(cache_dir: PathBuf, ttl_secs: u64) -> Self {
        let cancel_token = CancellationToken::new();
        let ttl = Duration::from_secs(ttl_secs);

        let embedder = Self {
            state: Arc::new(Mutex::new(InnerState {
                model: None,
                last_used: Instant::now(),
            })),
            cancel_token: cancel_token.clone(),
            ttl,
            cache_dir,
        };

        // Start background cleaner task
        let cleaner = embedder.clone();
        tokio::spawn(async move {
            cleaner.cleanup_loop().await;
        });

        embedder
    }
}

impl Drop for SmartEmbedder {
    fn drop(&mut self) {
        // Signal background task to exit
        self.cancel_token.cancel();
    }
}
```

### Cleanup Loop

```rust
impl SmartEmbedder {
    async fn cleanup_loop(&self) {
        loop {
            tokio::select! {
                _ = self.cancel_token.cancelled() => {
                    tracing::debug!("SmartEmbedder cleaner shutting down");
                    break;
                }
                _ = tokio::time::sleep(Duration::from_secs(5)) => {
                    self.maybe_unload().await;
                }
            }
        }
    }

    async fn maybe_unload(&self) {
        let mut state = self.state.lock().await;
        if state.model.is_some() && state.last_used.elapsed() > self.ttl {
            tracing::info!("Embedding model idle for {:?}, unloading", self.ttl);
            state.model = None;
        }
    }
}
```

### Embed API

```rust
impl SmartEmbedder {
    /// Single text embedding
    pub async fn embed(&self, text: &str) -> Result<Vec<f32>, AlephError> {
        let embeddings = self.embed_batch(&[text]).await?;
        Ok(embeddings.into_iter().next().unwrap())
    }

    /// Batch embedding (reduces lock contention)
    pub async fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, AlephError> {
        if texts.is_empty() {
            return Ok(vec![]);
        }

        let mut state = self.state.lock().await;

        // Update last used time
        state.last_used = Instant::now();

        // Lazy load model
        if state.model.is_none() {
            tracing::info!("Loading multilingual-e5-small model (cold start)");
            let start = Instant::now();

            let model = TextEmbedding::try_new(InitOptions {
                model_name: EmbeddingModel::MultilingualE5Small,
                cache_dir: self.cache_dir.clone(),
                show_download_progress: true,
            })?;

            tracing::info!("Model loaded in {:?}", start.elapsed());
            state.model = Some(model);
        }

        // Execute inference
        let model = state.model.as_ref().unwrap();
        let embeddings = model.embed(texts.to_vec(), None)?;

        Ok(embeddings)
    }

    pub fn dimensions(&self) -> usize {
        EMBEDDING_DIM
    }

    pub fn model_name(&self) -> &'static str {
        "multilingual-e5-small"
    }
}
```

### TextEmbedder Trait Implementation

```rust
#[async_trait]
impl TextEmbedder for SmartEmbedder {
    async fn embed(&self, text: &str) -> Result<Vec<f32>, EmbeddingError> {
        SmartEmbedder::embed(self, text)
            .await
            .map_err(|e| EmbeddingError::ModelError(e.to_string()))
    }

    async fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, EmbeddingError> {
        SmartEmbedder::embed_batch(self, texts)
            .await
            .map_err(|e| EmbeddingError::ModelError(e.to_string()))
    }

    fn dimensions(&self) -> usize {
        EMBEDDING_DIM
    }

    fn model_name(&self) -> &str {
        "multilingual-e5-small"
    }
}
```

---

## Configuration

### Config Schema

```rust
// core/src/config/types/agent/semantic_cache.rs

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SemanticCacheConfigToml {
    pub enabled: bool,
    pub embedding_model: String,        // "multilingual-e5-small"
    pub model_ttl_secs: u64,            // New: default 300
    pub similarity_threshold: f64,
    // ... other fields unchanged

    // Reranker config (reserved, disabled by default)
    #[serde(default)]
    pub reranker: RerankerConfig,
}

impl Default for SemanticCacheConfigToml {
    fn default() -> Self {
        Self {
            enabled: true,
            embedding_model: "multilingual-e5-small".to_string(),
            model_ttl_secs: 300,
            similarity_threshold: 0.85,
            reranker: RerankerConfig::default(),
            // ...
        }
    }
}
```

### Example TOML

```toml
[cowork.model_routing.semantic_cache]
enabled = true
embedding_model = "multilingual-e5-small"
model_ttl_secs = 300
similarity_threshold = 0.85

# Future: Reranker (disabled by default)
[cowork.model_routing.semantic_cache.reranker]
enabled = false
model = "mxbai-rerank-xsmall-v1"
ttl_secs = 300
candidate_pool_size = 100
```

---

## Database Migration

### Schema Changes

```rust
// core/src/memory/database/core.rs

pub const CURRENT_EMBEDDING_DIM: u32 = 384;  // Changed from 512

impl VectorDatabase {
    pub async fn initialize(&self) -> Result<(), AlephError> {
        let conn = self.conn.lock().await;

        // Check if migration needed (dimension change)
        if self.needs_dimension_migration(&conn)? {
            tracing::warn!("Embedding dimension changed, dropping old vector tables");
            conn.execute("DROP TABLE IF EXISTS memories_vec", [])?;
            conn.execute("DROP TABLE IF EXISTS facts_vec", [])?;
        }

        // Rebuild 384-dim vector tables
        conn.execute(
            "CREATE VIRTUAL TABLE IF NOT EXISTS memories_vec USING vec0(
                embedding float[384]
            )",
            [],
        )?;

        conn.execute(
            "CREATE VIRTUAL TABLE IF NOT EXISTS facts_vec USING vec0(
                embedding float[384]
            )",
            [],
        )?;

        Ok(())
    }
}
```

---

## Reranker Extension Point (Reserved)

### Two-Stage Retrieval Architecture

```
Current:
  Query → Embed → KNN Top-K → Return

Future (with Reranker):
  Query → Embed → KNN Top-100 → Rerank → Top-K → Return
                                  ↑
                    mxbai-rerank-xsmall-v1
```

### Reranker Trait

```rust
// core/src/memory/reranker.rs (reserved file, not implemented yet)

#[async_trait]
pub trait Reranker: Send + Sync {
    async fn rerank(
        &self,
        query: &str,
        candidates: &[(String, String)],  // (doc_id, doc_text)
        top_k: usize,
    ) -> Result<Vec<RerankResult>, AlephError>;
}

pub struct RerankResult {
    pub doc_id: String,
    pub score: f32,
}

/// No-op implementation: returns original order
pub struct NoOpReranker;

#[async_trait]
impl Reranker for NoOpReranker {
    async fn rerank(
        &self,
        _query: &str,
        candidates: &[(String, String)],
        top_k: usize,
    ) -> Result<Vec<RerankResult>, AlephError> {
        Ok(candidates
            .iter()
            .take(top_k)
            .enumerate()
            .map(|(i, (id, _))| RerankResult {
                doc_id: id.clone(),
                score: 1.0 - (i as f32 * 0.01),
            })
            .collect())
    }
}
```

### Reranker Config

```rust
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RerankerConfig {
    pub enabled: bool,                    // Default: false
    pub model: String,                    // "mxbai-rerank-xsmall-v1"
    pub ttl_secs: u64,                    // Reuse TTL strategy
    pub candidate_pool_size: usize,       // KNN pool size, default 100
}
```

---

## Global Singleton

```rust
// core/src/memory/mod.rs

use once_cell::sync::OnceCell;

static SMART_EMBEDDER: OnceCell<Arc<SmartEmbedder>> = OnceCell::new();

/// Initialize global SmartEmbedder (call once at startup)
pub fn init_embedder(config: &SemanticCacheConfigToml) -> Result<(), AlephError> {
    let cache_dir = SmartEmbedder::default_cache_dir()?;
    let embedder = SmartEmbedder::new(cache_dir, config.model_ttl_secs);

    SMART_EMBEDDER
        .set(Arc::new(embedder))
        .map_err(|_| AlephError::AlreadyInitialized("SmartEmbedder"))?;

    Ok(())
}

/// Get global SmartEmbedder instance
pub fn embedder() -> &'static Arc<SmartEmbedder> {
    SMART_EMBEDDER.get().expect("SmartEmbedder not initialized")
}
```

---

## Testing Strategy

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_cold_start_and_hot_call() {
        let tmp = TempDir::new().unwrap();
        let embedder = SmartEmbedder::new(tmp.path().to_path_buf(), 300);

        // Cold start
        let v1 = embedder.embed("hello world").await.unwrap();
        assert_eq!(v1.len(), 384);

        // Hot call
        let v2 = embedder.embed("hello again").await.unwrap();
        assert_eq!(v2.len(), 384);
    }

    #[tokio::test]
    async fn test_ttl_unload() {
        let tmp = TempDir::new().unwrap();
        let embedder = SmartEmbedder::new(tmp.path().to_path_buf(), 1); // 1s TTL

        embedder.embed("test").await.unwrap();
        assert!(embedder.is_loaded().await);

        tokio::time::sleep(Duration::from_secs(7)).await;
        assert!(!embedder.is_loaded().await);
    }

    #[tokio::test]
    async fn test_multilingual() {
        let tmp = TempDir::new().unwrap();
        let embedder = SmartEmbedder::new(tmp.path().to_path_buf(), 300);

        let zh = embedder.embed("你好世界").await.unwrap();
        let en = embedder.embed("hello world").await.unwrap();
        let ja = embedder.embed("こんにちは").await.unwrap();

        assert_eq!(zh.len(), 384);
        assert_eq!(en.len(), 384);
        assert_eq!(ja.len(), 384);
    }
}
```

---

## Implementation Checklist

| # | File | Change |
|---|------|--------|
| 1 | `core/src/memory/smart_embedder.rs` | **New** - SmartEmbedder full implementation |
| 2 | `core/src/memory/reranker.rs` | **New** - Reranker trait + NoOpReranker |
| 3 | `core/src/memory/mod.rs` | Add global singleton `init_embedder()` / `embedder()` |
| 4 | `core/src/memory/embedding.rs` | Delete or mark deprecated |
| 5 | `core/src/config/types/agent/semantic_cache.rs` | Add `model_ttl_secs` + `RerankerConfig` |
| 6 | `core/src/memory/database/core.rs` | Dimension 512→384, migration logic |
| 7 | `core/src/memory/ingestion.rs` | Use global `embedder()` |
| 8 | `core/src/memory/retrieval.rs` | Use global `embedder()` + Reranker interface |
| 9 | `core/src/dispatcher/.../embedder.rs` | Remove `FastEmbedEmbedder`, use global singleton |
| 10 | `core/src/init_unified/coordinator.rs` | Download `MultilingualE5Small` |
| 11 | `core/Cargo.toml` | Verify fastembed version supports new model |

---

## Model Comparison

| Model | Dimensions | Languages | Size | Use Case |
|-------|------------|-----------|------|----------|
| `bge-small-zh-v1.5` | 512 | Chinese-focused | ~33MB | Previous |
| `multilingual-e5-small` | 384 | 100+ languages | ~118MB | **New** |
| `mxbai-rerank-xsmall-v1` | N/A | Multilingual | ~30MB | Future reranker |

---

## References

- [intfloat/multilingual-e5-small](https://huggingface.co/intfloat/multilingual-e5-small)
- [mixedbread-ai/mxbai-rerank-xsmall-v1](https://huggingface.co/mixedbread-ai/mxbai-rerank-xsmall-v1)
- [fastembed-rs](https://github.com/Anush008/fastembed-rs)
