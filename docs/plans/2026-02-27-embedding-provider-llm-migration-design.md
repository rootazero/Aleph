# Embedding Provider LLM Migration Design

> Date: 2026-02-27
> Status: Approved
> Supersedes: 2026-02-22-embedding-evolution-design.md

## Problem

Aleph's memory system currently relies on local fastembed models (multilingual-e5-small 384d, bge-small-zh-v1.5 512d) for embeddings. This has several issues:

1. **Two incompatible embedding systems**: Memory uses e5-small (384d), Semantic Cache uses bge-small-zh (512d)
2. **Config/implementation mismatch**: Config says bge-small-zh, but code uses e5-small
3. **Limited model quality**: Local models are smaller and less capable than cloud-hosted alternatives
4. **Binary size overhead**: fastembed adds significant binary size

## Decision

**Completely remove fastembed. All embeddings go through remote OpenAI-compatible APIs.** Support multiple configurable embedding providers with presets. No backward compatibility — switching models clears and rebuilds the vector store.

## Design

### 1. Configuration Data Model

```rust
pub struct EmbeddingProviderConfig {
    pub id: String,                  // "siliconflow", "openai", "ollama", "custom-xxx"
    pub name: String,                // Display name
    pub preset: EmbeddingPreset,     // Preset type
    pub api_base: String,            // API endpoint
    pub api_key_env: Option<String>, // Environment variable name for API key
    pub api_key: Option<String>,     // Direct API key (encrypted storage)
    pub model: String,               // Model name e.g. "BAAI/bge-m3"
    pub dimensions: u32,             // Output dimensions
    pub batch_size: u32,             // Batch size, default 32
    pub timeout_ms: u64,             // Timeout, default 10000
}

pub enum EmbeddingPreset {
    SiliconFlow,  // api_base: https://api.siliconflow.cn/v1/embeddings, model: BAAI/bge-m3, dim: 1024
    OpenAI,       // api_base: https://api.openai.com/v1/embeddings, model: text-embedding-3-small, dim: 1536
    Ollama,       // api_base: http://localhost:11434/v1/embeddings, model: nomic-embed-text, dim: 768
    Custom,       // User-defined OpenAI-compatible endpoint
}

pub struct EmbeddingSettings {
    pub providers: Vec<EmbeddingProviderConfig>,
    pub active_provider_id: String,
}
```

### 2. Preset Defaults

| Preset | API Base | Model | Dimensions | Notes |
|--------|----------|-------|------------|-------|
| **SiliconFlow** | `https://api.siliconflow.cn/v1/embeddings` | `BAAI/bge-m3` | 1024 | Domestic first choice, excellent CJK support |
| **OpenAI** | `https://api.openai.com/v1/embeddings` | `text-embedding-3-small` | 1536 | Global standard |
| **Ollama** | `http://localhost:11434/v1/embeddings` | `nomic-embed-text` | 768 | Local deployment, no API key required |
| **Custom** | User-defined | User-defined | User-defined | Any OpenAI-compatible endpoint |

### 3. Core Layer Changes

#### Deleted Modules
- `core/src/memory/smart_embedder.rs` — Local fastembed wrapper
- `core/src/memory/embedding.rs` — Legacy local embedding
- `core/src/memory/embedding_cache.rs` — Local model LRU cache (remote API doesn't need local cache)
- `core/src/memory/embedding_migration.rs` — Migration engine (replaced by clear-and-rebuild)
- `FastEmbedEmbedder` implementation in Semantic Cache
- `fastembed = "4"` dependency from `Cargo.toml`

#### Retained and Refactored
- **`embedding_provider.rs`**: Keep `EmbeddingProvider` trait, remove `LocalEmbeddingProvider`, enhance `RemoteEmbeddingProvider`:
  - Add preset factory: `RemoteEmbeddingProvider::from_preset(preset, config)`
  - Add connection test: `async fn test_connection(&self) -> Result<()>`
- **`retrieval.rs` / `ingestion.rs`**: Change `Arc<SmartEmbedder>` to `Arc<dyn EmbeddingProvider>`
- **Semantic Cache**: `TextEmbedder` trait implementation delegates to `Arc<dyn EmbeddingProvider>`

#### New Module
- **`embedding_manager.rs`**: Manages provider lifecycle
  - `get_active_provider() -> Arc<dyn EmbeddingProvider>`
  - `switch_provider(id)` — Switch active provider, trigger vector store clear
  - `test_provider(id) -> Result<()>` — Test provider connectivity

#### Vector Store Clear Logic
When `active_provider_id` changes:
1. Clear LanceDB facts_vec table
2. Clear Semantic Cache
3. Log and notify user

### 4. Gateway RPC Methods

New handler: `core/src/gateway/handlers/embedding_providers.rs`

| Method | Description |
|--------|-------------|
| `embedding_providers.list` | List all configured embedding providers |
| `embedding_providers.get` | Get single provider details |
| `embedding_providers.add` | Add new provider |
| `embedding_providers.update` | Update provider config |
| `embedding_providers.remove` | Remove provider |
| `embedding_providers.set_active` | Set active provider (triggers vector store clear) |
| `embedding_providers.test` | Test provider connectivity |
| `embedding_providers.presets` | Get available presets |

### 5. Settings Panel UI

New page **"Embedding Providers"** in settings sidebar, positioned between "Providers" and "Generation Providers" (grouped under "AI Models"):

**Layout**:
- **Top**: Active provider status card (name, model, dimensions, connection status)
- **Middle**: Provider list — each card shows name/model/status, editable/deletable
- **Bottom**: "+ Add Provider" button, opens preset selector (SiliconFlow/OpenAI/Ollama/Custom)

**Per-Provider Edit Form**:
- API Base URL (auto-filled from preset, editable)
- API Key (password input)
- Model Name (auto-filled from preset, editable)
- Dimensions (auto-filled from preset, editable)
- "Test Connection" button
- "Set as Active" button (shown for non-active providers)

**Switch Confirmation**: Setting a provider as active shows confirmation: "Switching embedding model will clear existing vector data. New interactions will automatically rebuild the index. Confirm?"

### 6. Error Handling

- **No active provider**: Memory system degrades to text-only storage (no vector retrieval), UI prompts user to configure
- **API call failure**: Retry 2x with exponential backoff, on failure queue raw text for later embedding
- **Switch/clear failure**: Rollback active_provider_id, notify user

### 7. YAGNI — Explicitly Not Doing

- No provider failover (embedding doesn't need HA like LLM)
- No local embedding cache (remote API is fast enough, cache invalidated on model switch anyway)
- No automatic model recommendation
- No automatic dimension detection (rely on presets and user config)
- No backward compatibility or migration (clean break)
