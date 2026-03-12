# Model Discovery: API-Driven Model List with Preset Fallback

## Summary

Replace manual model string entry with automatic model discovery from provider APIs. Agents inherit models from the available model pool of configured providers, defaulting to the default provider's default model.

## Problem

Currently, users must manually type model ID strings (e.g., `model = "gpt-4o"`) when configuring providers and agents. This is error-prone, requires knowing exact model IDs, and doesn't reflect what models are actually available to the user's API key.

## Solution Overview

A three-layer model discovery system:

1. **API probe** — query provider APIs for available models at runtime
2. **Preset fallback** — external TOML file with known models per protocol (for providers without list API or when API fails)
3. **Manual fallback** — user can still type a model ID as last resort

Results are cached with a 24-hour TTL and exposed through existing RPC tools for LLM-driven configuration.

## Design

### 1. ProtocolAdapter Trait Extension

Add an optional `list_models()` method to the existing `ProtocolAdapter` trait in `core/src/providers/adapter.rs`:

```rust
#[async_trait]
pub trait ProtocolAdapter: Send + Sync {
    // ... existing methods unchanged ...

    /// Fetch available models from the provider API.
    /// Returns None if the protocol does not support model listing.
    async fn list_models(&self, config: &ProviderConfig) -> Result<Option<Vec<DiscoveredModel>>> {
        Ok(None)
    }
}
```

New data structure:

```rust
/// A model discovered via API probe or preset list
pub struct DiscoveredModel {
    pub id: String,                       // Model ID (e.g., "gpt-4o")
    pub name: Option<String>,             // Display name
    pub owned_by: Option<String>,         // Owner/organization
    pub capabilities: Vec<String>,        // ["chat", "vision", "tools", "thinking"]
}
```

Protocol implementation matrix:

| Protocol | Discovery Method | Endpoint |
|----------|-----------------|----------|
| OpenAI | `GET /v1/models` | Native support |
| Gemini | `GET /v1beta/models` | Supported with API key |
| Anthropic | Returns `None` | No public list API |
| ChatGPT | Returns `None` | OAuth mode, not applicable |

Note: Ollama uses a standalone `OllamaProvider` (not the `ProtocolAdapter` pattern). Its model discovery (`GET /api/tags`) is implemented directly on `OllamaProvider` as a separate `list_models()` method, not through this trait.

The default implementation `Ok(None)` means no existing protocol code needs modification to compile.

For API-probed models, `capabilities` defaults to `["chat"]`. Protocols that can extract richer capability info from the API response (e.g., OpenAI's model metadata) should populate this field. For preset-sourced models, capabilities are declared in the preset TOML file.

### 2. Preset Model List (External Config)

File: `shared/config/model-presets.toml`

```toml
[anthropic]
models = [
    { id = "claude-opus-4-20250514", name = "Claude Opus 4", capabilities = ["chat", "vision", "tools", "thinking"] },
    { id = "claude-sonnet-4-20250514", name = "Claude Sonnet 4", capabilities = ["chat", "vision", "tools", "thinking"] },
    { id = "claude-haiku-4-20250506", name = "Claude Haiku 4", capabilities = ["chat", "vision", "tools"] },
]

[openai]
# API probe takes priority; this is fallback for network failures
models = [
    { id = "gpt-4o", name = "GPT-4o", capabilities = ["chat", "vision", "tools"] },
    { id = "gpt-4o-mini", name = "GPT-4o Mini", capabilities = ["chat", "vision", "tools"] },
    { id = "o3", name = "O3", capabilities = ["chat", "tools", "thinking"] },
    { id = "o3-mini", name = "O3 Mini", capabilities = ["chat", "tools", "thinking"] },
]

[gemini]
models = [
    { id = "gemini-2.5-pro", name = "Gemini 2.5 Pro", capabilities = ["chat", "vision", "tools", "thinking"] },
    { id = "gemini-2.5-flash", name = "Gemini 2.5 Flash", capabilities = ["chat", "vision", "tools", "thinking"] },
]

# Ollama has no presets — models depend on local installation
```

Resolution priority:

```
API probe success → use API result
       ↓ fail
Preset file has protocol → use preset list
       ↓ missing
Empty list → UI/Tool layer prompts user for manual input
```

### 3. ModelRegistry Cache Service

New file: `core/src/providers/model_registry.rs`

```rust
pub struct ModelRegistry {
    /// Per-provider cache: provider_name → CachedModelList
    cache: RwLock<HashMap<String, CachedModelList>>,
    /// Preset lists (loaded from model-presets.toml at startup)
    presets: HashMap<String, Vec<DiscoveredModel>>,
    /// Cache TTL (default 24 hours)
    ttl: Duration,
}

struct CachedModelList {
    models: Vec<DiscoveredModel>,
    source: ModelSource,
    fetched_at: Instant,
}

enum ModelSource {
    Api,     // From provider API probe
    Preset,  // From preset file
    Manual,  // User-specified
}
```

Core methods:

```rust
impl ModelRegistry {
    /// Get available models for a provider (with caching)
    pub async fn list_models(
        &self,
        provider_name: &str,
        adapter: &dyn ProtocolAdapter,
        config: &ProviderConfig,
    ) -> Vec<DiscoveredModel>;

    /// Force refresh a provider's model list
    pub async fn refresh(
        &self,
        provider_name: &str,
        adapter: &dyn ProtocolAdapter,
        config: &ProviderConfig,
    ) -> Result<Vec<DiscoveredModel>>;

    /// Get all models across all configured providers (for Agent model selection)
    pub async fn all_available_models(&self) -> Vec<AvailableModel>;
}

/// Aggregated view: model + owning provider
pub struct AvailableModel {
    pub model: DiscoveredModel,
    pub provider_name: String,
    pub protocol: String,
}
```

`list_models` internal logic:

1. Check cache → hit and not expired → return cached
2. Call `adapter.list_models(config)` → success → write cache with source = Api
3. API returns `None` or fails → check presets → found → write cache with source = Preset
4. No presets either → return empty list

Lifecycle: `ModelRegistry` is a global singleton via `static Lazy<ModelRegistry>`, following the same pattern as `PROTOCOL_REGISTRY` in `core/src/providers/protocols/registry.rs`. Initialized lazily on first access.

Concurrency: The `cache` field uses `tokio::sync::RwLock` (not `std::sync::RwLock`) since `list_models()` is async. The lock is acquired for read to check cache, released, then if a refresh is needed, the async API call happens without holding the lock. After the call completes, a write lock is acquired to update the cache. This means two concurrent refreshes for the same provider may both execute, but the last write wins — acceptable since model lists are idempotent.

Probe timeout: API probe requests use a 5-second timeout (`reqwest::Client::builder().timeout(Duration::from_secs(5))`).

### 4. RPC Interface Changes

#### `models.list` — Modified

Current behavior: iterates config.providers, returns one ModelInfo per provider (only the configured model).
New behavior: iterates config.providers, returns all available models per provider.

New parameter:

```json
{
    "provider": "openai",    // optional: filter by provider
    "enabled_only": true,    // optional: only enabled providers
    "refresh": false         // NEW: force cache refresh
}
```

Response changes:

```json
{
    "models": [
        {
            "id": "gpt-4o",
            "provider": "openai",
            "provider_type": "openai",
            "enabled": true,
            "is_default": true,
            "is_current": true,
            "capabilities": ["chat", "vision", "tools"],
            "source": "api"
        },
        {
            "id": "gpt-4o-mini",
            "provider": "openai",
            "provider_type": "openai",
            "enabled": true,
            "is_default": false,
            "is_current": false,
            "capabilities": ["chat", "vision", "tools"],
            "source": "api"
        }
    ]
}
```

New fields: `is_current` (whether this is the provider's currently configured model), `source` (api/preset/manual).

#### `models.refresh` — New

Force refresh a provider's model list. Returns the refreshed models directly to avoid a second `models.list` round-trip:

```json
// Request
{ "provider": "openai" }  // optional, omit to refresh all

// Response
{
    "provider": "openai",
    "count": 15,
    "source": "api",
    "last_refreshed": "2026-03-12T10:30:00Z",
    "models": [ ... ]
}
```

#### `models.set_default` — New (replaces ambiguous `models.set`)

Set the default provider (what the old `models.set` did):

```json
// Request
{ "provider": "openai" }

// Response
{ "message": "Default provider set to openai" }
```

#### `models.set_model` — New

Change a specific provider's configured model, with validation:

```json
// Request
{ "provider": "openai", "model": "gpt-4o-mini" }

// Success
{ "message": "Provider openai model set to gpt-4o-mini" }

// Failure
{ "error": "Model 'gpt-99' not found in openai's available models" }
```

### 5. Agent Model Inheritance

Agent `model` field behavior change:

- **Empty/omitted** → uses `general.default_provider`'s currently configured model
- **Model ID specified** → matched against all providers' available model pool, validated for existence

`ProviderConfig.model` remains `String` (required). Every provider must have a configured model — this is the "current model" shown via `is_current` in the API. The `models.set_model` RPC changes this value. This avoids a high-blast-radius breaking change across all protocol adapters and RPC handlers that read `config.model`.

## File Change Summary

| Action | File | Description |
|--------|------|-------------|
| Modify | `core/src/providers/adapter.rs` | Add `list_models()` default method + `DiscoveredModel` |
| Modify | `core/src/providers/protocols/openai.rs` | Implement `list_models` via GET /v1/models |
| Modify | `core/src/providers/protocols/gemini.rs` | Implement `list_models` via GET /v1beta/models |
| Modify | `core/src/providers/ollama.rs` | Add `list_models()` method on `OllamaProvider` directly (not via ProtocolAdapter) |
| No change | `core/src/providers/protocols/anthropic.rs` | Uses default `Ok(None)` |
| No change | `core/src/providers/protocols/chatgpt.rs` | Uses default `Ok(None)` |
| New | `core/src/providers/model_registry.rs` | ModelRegistry cache service |
| New | `shared/config/model-presets.toml` | Preset model lists per protocol |
| Modify | `core/src/gateway/handlers/models.rs` | RPC handlers use ModelRegistry |
| No change | `core/src/config/types/provider.rs` | `model` stays `String` (required) |

## Out of Scope

- UI development (Leptos model selection interface)
- Agent ModelRoutingConfig auto-routing changes
- model_router/dispatcher layer changes

## Error Handling

| Scenario | Behavior |
|----------|----------|
| API probe timeout | 5-second timeout, silent fallback to presets |
| API key unauthorized | Warn log, fallback to presets |
| Preset file missing/parse error | Warn log at startup, return empty list for that protocol |
| Network completely unavailable | All presets + user manual input |
