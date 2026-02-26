# Embedding Provider LLM Migration — Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Remove fastembed entirely, migrate all embedding to remote OpenAI-compatible APIs with multi-provider configuration and Settings Panel UI.

**Architecture:** Refactor `EmbeddingProvider` trait to remove `LocalEmbeddingProvider`, enhance `RemoteEmbeddingProvider` with preset factories, add `EmbeddingManager` for provider lifecycle. Replace all `SmartEmbedder` usage with `Arc<dyn EmbeddingProvider>`. Add gateway RPC handlers and Leptos settings page.

**Tech Stack:** Rust (async-trait, reqwest, serde, schemars), Leptos/WASM (Tailwind CSS), JSON-RPC 2.0

---

## Task 1: New Config Types — EmbeddingProviderConfig & EmbeddingSettings

**Files:**
- Modify: `core/src/config/types/memory.rs`

**Step 1: Replace EmbeddingConfig with new types**

Replace the existing `EmbeddingConfig` (lines 153-193) and its default functions (lines 400-419) with the new multi-provider config types. Also remove `embedding_model` field from `MemoryConfig` (line 21), `embedding_cache_max_size` (line 131), `embedding_cache_ttl_seconds` (line 134), and their defaults.

In `core/src/config/types/memory.rs`, replace the `EmbeddingConfig` struct (lines 149-193) with:

```rust
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
```

**Step 2: Update MemoryConfig to use EmbeddingSettings**

In `MemoryConfig` (line 15), change:
- Remove field `embedding_model: String` (line 21) and its serde attribute
- Remove field `embedding_cache_max_size: usize` (line 131) and its serde attribute
- Remove field `embedding_cache_ttl_seconds: u64` (line 134) and its serde attribute
- Change field `embedding: EmbeddingConfig` (line 85) to `embedding: EmbeddingSettings`

In `Default for MemoryConfig` (line 496), update:
- Remove `embedding_model: default_embedding_model()` (line 500)
- Remove `embedding_cache_max_size: ...` (line 537)
- Remove `embedding_cache_ttl_seconds: ...` (line 538)
- Change `embedding: EmbeddingConfig::default()` (line 525) to `embedding: EmbeddingSettings::default()`

Remove these now-unused default functions:
- `default_embedding_model()` (line 331)
- `default_embedding_provider()` (line 401)
- `default_embedding_model_name()` (line 405)
- `default_embedding_dimension()` (line 409)
- `default_embedding_cache_max_size()` (line 492)
- `default_embedding_cache_ttl_seconds()` (line 493)

Keep `default_embedding_timeout_ms()` and `default_embedding_batch_size()` — still used by presets.

**Step 3: Verify it compiles (expect errors from downstream)**

Run: `cd /Users/zouguojun/Workspace/Aleph/core && cargo check 2>&1 | head -80`

Expected: Compile errors from files still referencing old `EmbeddingConfig`, `SmartEmbedder`, etc. This is expected — we fix them in subsequent tasks.

**Step 4: Commit**

```bash
git add core/src/config/types/memory.rs
git commit -m "config: replace EmbeddingConfig with multi-provider EmbeddingSettings"
```

---

## Task 2: Refactor EmbeddingProvider — Remove Local, Enhance Remote

**Files:**
- Modify: `core/src/memory/embedding_provider.rs`

**Step 1: Rewrite embedding_provider.rs**

Remove `LocalEmbeddingProvider`, remove `create_embedding_provider` factory, enhance `RemoteEmbeddingProvider` to work with `EmbeddingProviderConfig` presets. Add `test_connection` method.

Replace the entire file content with:

```rust
//! Embedding provider abstraction
//!
//! All embeddings go through remote OpenAI-compatible APIs.
//! Local fastembed has been removed.

use crate::config::types::memory::EmbeddingProviderConfig;
use crate::error::AlephError;
use std::sync::Arc;
use std::time::Duration;

/// Abstract embedding provider
#[async_trait::async_trait]
pub trait EmbeddingProvider: Send + Sync {
    /// Generate embedding for a single text
    async fn embed(&self, text: &str) -> Result<Vec<f32>, AlephError>;

    /// Generate embeddings for multiple texts (batch)
    async fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, AlephError>;

    /// Get the output dimension of this provider
    fn dimensions(&self) -> usize;

    /// Get the model name (e.g., "BAAI/bge-m3")
    fn model_name(&self) -> &str;

    /// Get the provider id (e.g., "siliconflow")
    fn provider_id(&self) -> &str;
}

/// Truncate embedding to target dimension and L2 normalize.
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

/// Remote embedding provider using OpenAI-compatible API
///
/// Works with SiliconFlow, OpenAI, Ollama, and any service
/// that implements the `/v1/embeddings` endpoint.
pub struct RemoteEmbeddingProvider {
    client: reqwest::Client,
    api_base: String,
    api_key: String,
    model: String,
    dimension: usize,
    batch_size: usize,
    provider_id: String,
}

impl RemoteEmbeddingProvider {
    /// Create from EmbeddingProviderConfig
    pub fn from_config(config: &EmbeddingProviderConfig) -> Result<Self, AlephError> {
        // Resolve API key: direct value > env var > empty
        let api_key = if let Some(ref key) = config.api_key {
            if !key.is_empty() {
                key.clone()
            } else {
                Self::resolve_env_key(&config.api_key_env)?
            }
        } else {
            Self::resolve_env_key(&config.api_key_env)?
        };

        let client = reqwest::Client::builder()
            .timeout(Duration::from_millis(config.timeout_ms))
            .build()
            .map_err(|e| AlephError::config(format!("Failed to create HTTP client: {}", e)))?;

        Ok(Self {
            client,
            api_base: config.api_base.clone(),
            api_key,
            model: config.model.clone(),
            dimension: config.dimensions as usize,
            batch_size: config.batch_size as usize,
            provider_id: config.id.clone(),
        })
    }

    fn resolve_env_key(env_var: &Option<String>) -> Result<String, AlephError> {
        if let Some(ref var) = env_var {
            match std::env::var(var) {
                Ok(val) if !val.is_empty() => Ok(val),
                _ => Ok(String::new()), // Not set — may be fine for Ollama
            }
        } else {
            Ok(String::new())
        }
    }

    /// Test connectivity by embedding a short text
    pub async fn test_connection(&self) -> Result<(), AlephError> {
        let result = self.embed("test").await?;
        if result.len() != self.dimension {
            return Err(AlephError::config(format!(
                "Dimension mismatch: expected {}, got {}",
                self.dimension,
                result.len()
            )));
        }
        Ok(())
    }

    /// Call the embeddings API for a batch of texts
    async fn call_api(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, AlephError> {
        let url = format!("{}/embeddings", self.api_base.trim_end_matches('/'));

        let mut body = serde_json::json!({
            "input": texts,
            "model": self.model,
        });

        if self.dimension > 0 {
            body["dimensions"] = serde_json::json!(self.dimension);
        }

        let mut request = self.client.post(&url).json(&body);

        if !self.api_key.is_empty() {
            request = request.header("Authorization", format!("Bearer {}", self.api_key));
        }

        let response = request
            .send()
            .await
            .map_err(|e| AlephError::config(format!("Embedding API request failed: {}", e)))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(AlephError::config(format!(
                "Embedding API returned {}: {}",
                status, body
            )));
        }

        let resp: serde_json::Value = response.json().await.map_err(|e| {
            AlephError::config(format!("Failed to parse embedding response: {}", e))
        })?;

        let data = resp["data"]
            .as_array()
            .ok_or_else(|| AlephError::config("Missing 'data' array in response".to_string()))?;

        let mut embeddings: Vec<(usize, Vec<f32>)> = Vec::with_capacity(data.len());

        for item in data {
            let index = item["index"].as_u64().unwrap_or(0) as usize;
            let embedding: Vec<f32> = item["embedding"]
                .as_array()
                .ok_or_else(|| AlephError::config("Missing 'embedding' array".to_string()))?
                .iter()
                .filter_map(|v| v.as_f64().map(|f| f as f32))
                .collect();

            embeddings.push((index, embedding));
        }

        embeddings.sort_by_key(|(idx, _)| *idx);

        let results: Vec<Vec<f32>> = embeddings
            .into_iter()
            .map(|(_, emb)| truncate_and_normalize(emb, self.dimension))
            .collect();

        Ok(results)
    }
}

#[async_trait::async_trait]
impl EmbeddingProvider for RemoteEmbeddingProvider {
    async fn embed(&self, text: &str) -> Result<Vec<f32>, AlephError> {
        let results = self.call_api(&[text]).await?;
        results
            .into_iter()
            .next()
            .ok_or_else(|| AlephError::config("No embedding returned from API".to_string()))
    }

    async fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, AlephError> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }

        let mut all_embeddings = Vec::with_capacity(texts.len());

        for chunk in texts.chunks(self.batch_size) {
            let batch_result = self.call_api(chunk).await?;
            all_embeddings.extend(batch_result);
        }

        Ok(all_embeddings)
    }

    fn dimensions(&self) -> usize {
        self.dimension
    }

    fn model_name(&self) -> &str {
        &self.model
    }

    fn provider_id(&self) -> &str {
        &self.provider_id
    }
}

/// Create an EmbeddingProvider from a provider config
pub fn create_provider(
    config: &EmbeddingProviderConfig,
) -> Result<Arc<dyn EmbeddingProvider>, AlephError> {
    let provider = RemoteEmbeddingProvider::from_config(config)?;
    Ok(Arc::new(provider))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_truncate_and_normalize_no_op_when_smaller() {
        let embedding = vec![0.6, 0.8];
        let result = truncate_and_normalize(embedding.clone(), 5);
        assert_eq!(result, embedding);
    }

    #[test]
    fn test_truncate_and_normalize_truncates_and_normalizes() {
        let embedding = vec![3.0, 4.0, 99.0, 99.0];
        let result = truncate_and_normalize(embedding, 2);
        assert_eq!(result.len(), 2);
        assert!((result[0] - 0.6).abs() < 1e-6);
        assert!((result[1] - 0.8).abs() < 1e-6);
    }

    /// Mock embedding provider for tests
    pub struct MockEmbeddingProvider {
        dim: usize,
        model: String,
    }

    impl MockEmbeddingProvider {
        pub fn new(dim: usize, model: &str) -> Self {
            Self {
                dim,
                model: model.to_string(),
            }
        }
    }

    #[async_trait::async_trait]
    impl EmbeddingProvider for MockEmbeddingProvider {
        async fn embed(&self, _text: &str) -> Result<Vec<f32>, AlephError> {
            Ok(vec![0.1; self.dim])
        }

        async fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, AlephError> {
            Ok(texts.iter().map(|_| vec![0.1; self.dim]).collect())
        }

        fn dimensions(&self) -> usize {
            self.dim
        }

        fn model_name(&self) -> &str {
            &self.model
        }

        fn provider_id(&self) -> &str {
            "mock"
        }
    }
}
```

**Step 2: Commit**

```bash
git add core/src/memory/embedding_provider.rs
git commit -m "memory: rewrite embedding_provider for remote-only with presets"
```

---

## Task 3: Add EmbeddingManager

**Files:**
- Create: `core/src/memory/embedding_manager.rs`
- Modify: `core/src/memory/mod.rs`

**Step 1: Create embedding_manager.rs**

```rust
//! Embedding manager — manages provider lifecycle and active provider switching.

use crate::config::types::memory::{EmbeddingProviderConfig, EmbeddingSettings};
use crate::error::AlephError;
use crate::memory::embedding_provider::{create_provider, EmbeddingProvider, RemoteEmbeddingProvider};
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{info, warn};

/// Manages embedding provider lifecycle
pub struct EmbeddingManager {
    settings: Arc<RwLock<EmbeddingSettings>>,
    active_provider: Arc<RwLock<Option<Arc<dyn EmbeddingProvider>>>>,
}

impl EmbeddingManager {
    /// Create a new EmbeddingManager from settings
    pub fn new(settings: EmbeddingSettings) -> Self {
        Self {
            settings: Arc::new(RwLock::new(settings)),
            active_provider: Arc::new(RwLock::new(None)),
        }
    }

    /// Initialize the active provider from current settings.
    /// Returns Ok(()) even if no provider is configured (degrades gracefully).
    pub async fn init(&self) -> Result<(), AlephError> {
        let settings = self.settings.read().await;
        let active_id = &settings.active_provider_id;

        if let Some(config) = settings.providers.iter().find(|p| p.id == *active_id) {
            match create_provider(config) {
                Ok(provider) => {
                    *self.active_provider.write().await = Some(provider);
                    info!(provider_id = %active_id, "Embedding provider initialized");
                }
                Err(e) => {
                    warn!(provider_id = %active_id, error = %e, "Failed to initialize embedding provider");
                }
            }
        } else {
            warn!("No active embedding provider configured (id={})", active_id);
        }

        Ok(())
    }

    /// Get the currently active provider. Returns None if not configured.
    pub async fn get_active_provider(&self) -> Option<Arc<dyn EmbeddingProvider>> {
        self.active_provider.read().await.clone()
    }

    /// Get the active provider or return an error.
    pub async fn require_active_provider(&self) -> Result<Arc<dyn EmbeddingProvider>, AlephError> {
        self.get_active_provider().await.ok_or_else(|| {
            AlephError::config("No active embedding provider configured. Please configure one in Settings > Embedding Providers.".to_string())
        })
    }

    /// Switch the active provider. Returns true if vector store should be cleared.
    pub async fn switch_provider(&self, new_id: &str) -> Result<bool, AlephError> {
        let mut settings = self.settings.write().await;
        let old_id = settings.active_provider_id.clone();

        let config = settings
            .providers
            .iter()
            .find(|p| p.id == new_id)
            .ok_or_else(|| AlephError::config(format!("Provider not found: {}", new_id)))?
            .clone();

        let provider = create_provider(&config)?;

        settings.active_provider_id = new_id.to_string();
        *self.active_provider.write().await = Some(provider);

        let should_clear = old_id != new_id;
        if should_clear {
            info!(old = %old_id, new = %new_id, "Embedding provider switched — vector store should be cleared");
        }

        Ok(should_clear)
    }

    /// Test a specific provider's connectivity
    pub async fn test_provider(&self, provider_id: &str) -> Result<(), AlephError> {
        let settings = self.settings.read().await;
        let config = settings
            .providers
            .iter()
            .find(|p| p.id == provider_id)
            .ok_or_else(|| AlephError::config(format!("Provider not found: {}", provider_id)))?;

        let provider = RemoteEmbeddingProvider::from_config(config)?;
        provider.test_connection().await
    }

    /// Test a provider config without it being saved (for "test connection" button)
    pub async fn test_config(config: &EmbeddingProviderConfig) -> Result<(), AlephError> {
        let provider = RemoteEmbeddingProvider::from_config(config)?;
        provider.test_connection().await
    }

    /// Update the internal settings (called after config save)
    pub async fn update_settings(&self, settings: EmbeddingSettings) {
        *self.settings.write().await = settings;
    }

    /// Get a snapshot of current settings
    pub async fn get_settings(&self) -> EmbeddingSettings {
        self.settings.read().await.clone()
    }
}
```

**Step 2: Update mod.rs exports**

In `core/src/memory/mod.rs`:
- Add `pub mod embedding_manager;` after `pub mod embedding_provider;` (line 43)
- Replace the re-exports (lines 112-118) with:

```rust
pub use embedding_provider::{
    EmbeddingProvider, RemoteEmbeddingProvider,
    create_provider as create_embedding_provider,
    truncate_and_normalize,
};
pub use embedding_manager::EmbeddingManager;
```

- Remove: `pub use smart_embedder::{SmartEmbedder, DEFAULT_MODEL_TTL_SECS, EMBEDDING_DIM};` (line 112)
- Remove: `pub use embedding_cache::EmbeddingCache;` (line 113)
- Remove: `pub use embedding_migration::{EmbeddingMigration, MigrationProgress};` (line 118)

**Step 3: Commit**

```bash
git add core/src/memory/embedding_manager.rs core/src/memory/mod.rs
git commit -m "memory: add EmbeddingManager for provider lifecycle"
```

---

## Task 4: Delete Removed Modules + fastembed Dependency

**Files:**
- Delete: `core/src/memory/smart_embedder.rs`
- Delete: `core/src/memory/embedding.rs`
- Delete: `core/src/memory/embedding_cache.rs`
- Delete: `core/src/memory/embedding_migration.rs`
- Modify: `core/src/memory/mod.rs` — remove module declarations
- Modify: `core/Cargo.toml` — remove fastembed

**Step 1: Delete the four files**

```bash
rm core/src/memory/smart_embedder.rs
rm core/src/memory/embedding.rs
rm core/src/memory/embedding_cache.rs
rm core/src/memory/embedding_migration.rs
```

**Step 2: Remove module declarations from mod.rs**

In `core/src/memory/mod.rs`, remove these lines:
- `pub mod embedding;` (line 31)
- `pub mod smart_embedder;` (line 41)
- `pub mod embedding_cache;` (line 42)
- `pub mod embedding_migration;` (line 44)

Also remove the deprecated re-export block (lines 99-103):
```rust
#[deprecated(...)]
pub use embedding::EmbeddingModel;
```

**Step 3: Remove fastembed from Cargo.toml**

In `core/Cargo.toml`, remove: `fastembed = "4"` (line 99)

**Step 4: Commit**

```bash
git add -A
git commit -m "memory: remove fastembed, smart_embedder, embedding_cache, embedding_migration"
```

---

## Task 5: Fix All SmartEmbedder → Arc<dyn EmbeddingProvider> Usages

This is the largest task. Every file that imports `SmartEmbedder` needs to be updated to use `Arc<dyn EmbeddingProvider>` instead.

**Files to modify (production code):**
- `core/src/memory/retrieval.rs` (lines 10, 20, 29)
- `core/src/memory/ingestion.rs` (lines 9, 21, 30)
- `core/src/memory/fact_retrieval.rs` (lines 9, 58, 66, 77)
- `core/src/memory/compression/service.rs` (lines 18, 73, 83)
- `core/src/memory/compression/extractor.rs` (lines 21, 48, 53)
- `core/src/memory/vfs/l1_generator.rs` (lines 11, 29, 37)
- `core/src/memory/transcript_indexer/indexer.rs` (lines 3, 17, 27)
- `core/src/memory/transcript_indexer/semantic_chunker.rs` (lines 5, 33, 39)
- `core/src/dispatcher/tool_index/pipeline.rs` (lines 10, 120, 128, 138)
- `core/src/dispatcher/experience_replay_layer.rs` (lines 9, 37, 45)
- `core/src/memory/cli/commands.rs` (lines 8, 327, 357, 468)
- `core/src/init_unified/coordinator.rs` (lines 260-314) — remove `download_embedding_model` phase entirely

**Step 1: For each file, apply the same pattern**

Replace:
```rust
use crate::memory::smart_embedder::SmartEmbedder;
// or
use crate::memory::SmartEmbedder;
```

With:
```rust
use crate::memory::EmbeddingProvider;
```

Then change struct fields and constructor params:
```rust
// Before:
embedder: Arc<SmartEmbedder>,
// or
embedder: SmartEmbedder,

// After:
embedder: Arc<dyn EmbeddingProvider>,
```

For method calls — `SmartEmbedder` and `EmbeddingProvider` share the same API surface (`embed`, `embed_batch`, `dimensions`, `model_name`), so call sites should compile without changes.

For constructors that take `SmartEmbedder` (not `Arc<SmartEmbedder>`), change to `Arc<dyn EmbeddingProvider>`.

**Step 2: Fix init_unified/coordinator.rs**

Remove the `download_embedding_model` method (lines 260-314) entirely. Remove the call to it from the init sequence. Remove the `use fastembed::...` imports.

**Step 3: Fix test code**

In test functions that create `SmartEmbedder::new(...)`, replace with `MockEmbeddingProvider`:

```rust
// Before:
let embedder = Arc::new(SmartEmbedder::new(cache_dir, 300));

// After (in tests):
use crate::memory::embedding_provider::tests::MockEmbeddingProvider;
let embedder: Arc<dyn EmbeddingProvider> = Arc::new(MockEmbeddingProvider::new(1024, "test-model"));
```

Files with test code to fix:
- `core/src/memory/retrieval.rs` (lines 276-278)
- `core/src/memory/ingestion.rs` (lines 166-168)
- `core/src/memory/fact_retrieval.rs` (line 283)
- `core/src/memory/compression/service.rs` (line 530)
- `core/src/memory/compression/extractor.rs` (line 334)
- `core/src/memory/transcript_indexer/semantic_chunker.rs` (lines 222, 240, 258)
- `core/src/memory/transcript_indexer/mod.rs` (lines 44, 63)
- `core/src/memory/transcript_indexer/semantic_tests.rs` (lines 12, 32, 50, 63)
- `core/src/dispatcher/experience_replay_layer.rs` (lines 257, 272)

**Note:** The `MockEmbeddingProvider` is in `embedding_provider.rs` under `#[cfg(test)] mod tests`. To use it from other test modules, either:
1. Make it `pub(crate)` and move it out of the `tests` module, or
2. Create a `test_utils` module in memory.

Recommended: Add to `core/src/memory/embedding_provider.rs` a public test utility:

```rust
/// Test utilities (available in test builds)
#[cfg(test)]
pub mod test_utils {
    use super::*;

    pub struct MockEmbeddingProvider { ... }
    // (move from tests module)
}
```

**Step 4: Run cargo check to verify**

Run: `cd /Users/zouguojun/Workspace/Aleph/core && cargo check 2>&1 | head -80`

Fix any remaining compile errors iteratively.

**Step 5: Commit**

```bash
git add -A
git commit -m "memory: migrate all SmartEmbedder usages to Arc<dyn EmbeddingProvider>"
```

---

## Task 6: Fix Semantic Cache — Remove FastEmbedEmbedder

**Files:**
- Modify: `core/src/dispatcher/model_router/intelligent/semantic_cache/embedder.rs`
- Modify: `core/src/dispatcher/model_router/intelligent/semantic_cache/manager.rs`
- Modify: `core/src/dispatcher/model_router/intelligent/semantic_cache/types.rs`

**Step 1: Replace FastEmbedEmbedder with a bridge to EmbeddingProvider**

Rewrite `embedder.rs` to delegate `TextEmbedder` to `Arc<dyn EmbeddingProvider>`:

```rust
//! Text embedding generation
//!
//! Bridges the semantic cache TextEmbedder trait to the memory EmbeddingProvider.

use crate::memory::EmbeddingProvider;
use std::sync::Arc;

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

/// Trait for text embedding generation
#[async_trait::async_trait]
pub trait TextEmbedder: Send + Sync {
    async fn embed(&self, text: &str) -> Result<Vec<f32>, EmbeddingError>;
    async fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, EmbeddingError>;
    fn dimensions(&self) -> usize;
    fn model_name(&self) -> &str;
}

/// Bridge from EmbeddingProvider to TextEmbedder
pub struct ProviderEmbedder {
    provider: Arc<dyn EmbeddingProvider>,
}

impl ProviderEmbedder {
    pub fn new(provider: Arc<dyn EmbeddingProvider>) -> Self {
        Self { provider }
    }
}

#[async_trait::async_trait]
impl TextEmbedder for ProviderEmbedder {
    async fn embed(&self, text: &str) -> Result<Vec<f32>, EmbeddingError> {
        self.provider
            .embed(text)
            .await
            .map_err(|e| EmbeddingError::GenerationFailed(e.to_string()))
    }

    async fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, EmbeddingError> {
        self.provider
            .embed_batch(texts)
            .await
            .map_err(|e| EmbeddingError::GenerationFailed(e.to_string()))
    }

    fn dimensions(&self) -> usize {
        self.provider.dimensions()
    }

    fn model_name(&self) -> &str {
        self.provider.model_name()
    }
}
```

**Step 2: Update SemanticCacheManager**

Where `SemanticCacheManager::new(config)` creates a `FastEmbedEmbedder`, change it to accept an `Arc<dyn TextEmbedder>` or `Arc<dyn EmbeddingProvider>` in its constructor. Use the `with_embedder` constructor path.

**Step 3: Update SemanticCacheConfig**

In `types.rs`, remove `embedding_model` and `embedding_dimensions` fields (these come from the provider now). Or keep them but make them derived from the active provider.

**Step 4: Commit**

```bash
git add -A
git commit -m "semantic_cache: replace FastEmbedEmbedder with ProviderEmbedder bridge"
```

---

## Task 7: Gateway RPC Handlers — embedding_providers.*

**Files:**
- Create: `core/src/gateway/handlers/embedding_providers.rs`
- Modify: `core/src/gateway/handlers/mod.rs`

**Step 1: Create the handler file**

Follow the exact pattern from `generation_providers.rs`. The handler needs access to the config (read/write) via the gateway context.

Implement these handlers:
- `handle_list` — return `settings.embedding.providers` from config
- `handle_get` — find provider by id
- `handle_add` — push new provider config
- `handle_update` — find and update by id
- `handle_remove` — remove by id (cannot remove active)
- `handle_set_active` — update `active_provider_id`, return `{ should_clear: true }`
- `handle_test` — create a `RemoteEmbeddingProvider` from params, call `test_connection()`
- `handle_presets` — return static list of preset configs

Each handler follows the pattern:
```rust
pub async fn handle_list(
    request: JsonRpcRequest,
    context: Arc<GatewayContext>,  // or whatever the context type is
) -> JsonRpcResponse { ... }
```

**Step 2: Register handlers in mod.rs**

In the `HandlerRegistry::new()` function, add registrations after generation_providers:

```rust
registry.register("embedding_providers.list", |req, ctx| embedding_providers::handle_list(req, ctx));
registry.register("embedding_providers.get", |req, ctx| embedding_providers::handle_get(req, ctx));
registry.register("embedding_providers.add", |req, ctx| embedding_providers::handle_add(req, ctx));
registry.register("embedding_providers.update", |req, ctx| embedding_providers::handle_update(req, ctx));
registry.register("embedding_providers.remove", |req, ctx| embedding_providers::handle_remove(req, ctx));
registry.register("embedding_providers.set_active", |req, ctx| embedding_providers::handle_set_active(req, ctx));
registry.register("embedding_providers.test", |req, ctx| embedding_providers::handle_test(req, ctx));
registry.register("embedding_providers.presets", |req, ctx| embedding_providers::handle_presets(req, ctx));
```

**Step 3: Commit**

```bash
git add core/src/gateway/handlers/embedding_providers.rs core/src/gateway/handlers/mod.rs
git commit -m "gateway: add embedding_providers RPC handlers"
```

---

## Task 8: Settings Panel API Client — EmbeddingProvidersApi

**Files:**
- Modify: `core/ui/control_plane/src/api.rs`

**Step 1: Add EmbeddingProvidersApi**

Follow the `GenerationProvidersApi` pattern. Add at the end of `api.rs`:

```rust
// =============================================================================
// Embedding Providers API
// =============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddingProviderEntry {
    pub id: String,
    pub name: String,
    pub preset: String,
    pub api_base: String,
    pub model: String,
    pub dimensions: u32,
    pub is_active: bool,
}

pub struct EmbeddingProvidersApi;

impl EmbeddingProvidersApi {
    pub async fn list(state: &DashboardState) -> Result<Vec<EmbeddingProviderEntry>, String> {
        let result = state.rpc_call("embedding_providers.list", Value::Null).await?;
        serde_json::from_value(result).map_err(|e| e.to_string())
    }

    pub async fn get(state: &DashboardState, id: &str) -> Result<EmbeddingProviderEntry, String> {
        let params = serde_json::json!({ "id": id });
        let result = state.rpc_call("embedding_providers.get", params).await?;
        serde_json::from_value(result).map_err(|e| e.to_string())
    }

    pub async fn add(state: &DashboardState, config: serde_json::Value) -> Result<(), String> {
        state.rpc_call("embedding_providers.add", config).await?;
        Ok(())
    }

    pub async fn update(state: &DashboardState, id: &str, config: serde_json::Value) -> Result<(), String> {
        let mut params = config;
        params["id"] = serde_json::json!(id);
        state.rpc_call("embedding_providers.update", params).await?;
        Ok(())
    }

    pub async fn remove(state: &DashboardState, id: &str) -> Result<(), String> {
        let params = serde_json::json!({ "id": id });
        state.rpc_call("embedding_providers.remove", params).await?;
        Ok(())
    }

    pub async fn set_active(state: &DashboardState, id: &str) -> Result<bool, String> {
        let params = serde_json::json!({ "id": id });
        let result = state.rpc_call("embedding_providers.set_active", params).await?;
        let should_clear = result["should_clear"].as_bool().unwrap_or(false);
        Ok(should_clear)
    }

    pub async fn test(state: &DashboardState, config: serde_json::Value) -> Result<(), String> {
        state.rpc_call("embedding_providers.test", config).await?;
        Ok(())
    }

    pub async fn presets(state: &DashboardState) -> Result<Vec<EmbeddingProviderEntry>, String> {
        let result = state.rpc_call("embedding_providers.presets", Value::Null).await?;
        serde_json::from_value(result).map_err(|e| e.to_string())
    }
}
```

**Step 2: Commit**

```bash
git add core/ui/control_plane/src/api.rs
git commit -m "ui: add EmbeddingProvidersApi RPC client"
```

---

## Task 9: Settings Panel UI — EmbeddingProvidersView

**Files:**
- Create: `core/ui/control_plane/src/views/settings/embedding_providers.rs`
- Modify: `core/ui/control_plane/src/views/settings/mod.rs`
- Modify: `core/ui/control_plane/src/components/settings_sidebar.rs`
- Modify: `core/ui/control_plane/src/app.rs`

**Step 1: Create EmbeddingProvidersView component**

Follow the pattern from `generation_providers.rs`. Create a Leptos component with:
- Provider list with cards
- Active provider status indicator
- Add/Edit/Delete/Test/SetActive actions
- Preset selector for adding new providers
- Confirmation dialog for switching active provider

The component should use the `EmbeddingProvidersApi` from the previous task.

**Step 2: Register in settings mod.rs**

Add to `core/ui/control_plane/src/views/settings/mod.rs`:
```rust
pub mod embedding_providers;
pub use embedding_providers::EmbeddingProvidersView;
```

**Step 3: Add sidebar entry**

In `core/ui/control_plane/src/components/settings_sidebar.rs`:

Add `EmbeddingProviders` variant to `SettingsTab` enum (after `Providers`, before `GenerationProviders`).

Implement:
- `path()` → `"/settings/embedding-providers"`
- `label()` → `"Embedding"`
- `icon_svg()` → an appropriate SVG path (e.g., vector/embedding icon)

Add to `SETTINGS_GROUPS` in the AI group, between Providers and GenerationProviders.

**Step 4: Add route**

In `core/ui/control_plane/src/app.rs`, add route after the providers route:
```rust
<Route path=path!("/settings/embedding-providers") view=EmbeddingProvidersView />
```

**Step 5: Commit**

```bash
git add -A
git commit -m "ui: add Embedding Providers settings page"
```

---

## Task 10: Integration — Wire EmbeddingManager into Server Startup

**Files:**
- Find and modify the server startup code that creates the memory system
- This is likely in `core/src/init_unified/coordinator.rs` or wherever the `MemoryRetrieval`/`MemoryIngestion` are constructed

**Step 1: Create EmbeddingManager during server init**

At server startup:
```rust
let embedding_manager = EmbeddingManager::new(config.memory.embedding.clone());
embedding_manager.init().await?;
let provider = embedding_manager.get_active_provider().await;
```

Pass `provider` (which is `Option<Arc<dyn EmbeddingProvider>>`) to `MemoryRetrieval`, `MemoryIngestion`, and `SemanticCacheManager`.

**Step 2: Handle graceful degradation**

Where `MemoryRetrieval`/`MemoryIngestion` are created, if no active provider:
- Skip vector operations (return empty results for retrieval, skip embedding for ingestion)
- Log a warning

**Step 3: Verify it compiles**

Run: `cd /Users/zouguojun/Workspace/Aleph/core && cargo check`

Expected: Clean compile with no errors.

**Step 4: Run tests**

Run: `cd /Users/zouguojun/Workspace/Aleph/core && cargo test 2>&1 | tail -30`

Fix any test failures. Most tests should still pass since they use MockEmbeddingProvider.

**Step 5: Commit**

```bash
git add -A
git commit -m "core: wire EmbeddingManager into server startup"
```

---

## Task 11: Final Cleanup and Verification

**Step 1: Search for any remaining references**

```bash
cd /Users/zouguojun/Workspace/Aleph && grep -r "fastembed\|SmartEmbedder\|EmbeddingModel\|EmbeddingCache\|EmbeddingMigration\|LocalEmbeddingProvider" core/src/ --include="*.rs" | grep -v "test\|target"
```

Fix any remaining references.

**Step 2: Run full test suite**

```bash
cd /Users/zouguojun/Workspace/Aleph/core && cargo test
```

**Step 3: Build check**

```bash
cd /Users/zouguojun/Workspace/Aleph/core && cargo build
```

**Step 4: Commit any final fixes**

```bash
git add -A
git commit -m "cleanup: remove all remaining fastembed references"
```
