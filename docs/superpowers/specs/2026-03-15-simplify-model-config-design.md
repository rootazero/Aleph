# Simplify Model Configuration Design

## Summary

Remove all dynamic model discovery/probe/caching infrastructure. Replace with simple text-based model input supporting multiple models per provider. All four provider types (AI, Embedding, Generation, Reranking) are unified under the same change.

## Motivation

The dynamic model discovery system (ModelRegistry, probe endpoints, presets, ModelSelector UI) introduced excessive complexity for marginal benefit. Users already know which models they want to use. Returning to simple manual input eliminates this complexity while adding multi-model support per provider.

## Design

### 1. Configuration Type Changes

**`ProviderConfig`** — `model: String` becomes `models: Vec<String>`:

```toml
[providers.openai]
protocol = "openai"
models = ["gpt-4o", "gpt-4o-mini", "o1"]
enabled = true

[embedding_providers.siliconflow]
protocol = "openai"
models = ["BAAI/bge-m3", "BAAI/bge-large-zh-v1.5"]
base_url = "https://api.siliconflow.cn/v1"
```

- First entry is the default model
- Backward compatibility: deserialize `model = "xxx"` as `models = ["xxx"]` using custom serde deserializer that accepts both `String` and `Vec<String>`
- Serialize always uses `models`
- New convenience method: `ProviderConfig::default_model() -> &str` returns `models[0]`
- **Validation**: `models` must be non-empty at deserialization time (reject with error). Empty strings within the list are filtered out. `default_model()` includes `debug_assert!(!self.models.is_empty())` as safety net.

### 2. Default Model Resolution

Previously: `default_provider` implied default model (1:1 mapping).
Now: **default model = default provider's `models[0]`**.

Resolution chain:
1. `ExecutionEngine` → `provider_registry.default_provider()` → get provider
2. Provider → `config.default_model()` → `models[0]`
3. If agent has `model_config.primary`, use that instead (existing gap, not in this scope)

### 3. Backend Deletions

| Target | Path | Action |
|--------|------|--------|
| ModelRegistry | `src/providers/model_registry.rs` | Delete file |
| model-presets.toml | `shared/config/model-presets.toml` | Delete file |
| ProtocolAdapter::list_models() | `src/providers/adapter.rs` | Remove method + default impl |
| DiscoveredModel struct | `src/providers/adapter.rs` | Remove struct |
| OpenAI list_models | `src/providers/protocols/openai.rs` | Remove method |
| Anthropic list_models | `src/providers/protocols/anthropic.rs` | Remove method |
| ChatGPT list_models | `src/providers/protocols/chatgpt.rs` | Remove method |
| Gemini list_models | `src/providers/protocols/gemini.rs` | Remove method |
| OllamaProvider::list_models() | `src/providers/ollama.rs` | Remove method |
| OllamaDiscoveryAdapter | `src/gateway/handlers/models/` | Remove |
| models handlers module | `src/gateway/handlers/models/` | Delete entire directory |
| providers.probe handler | `src/gateway/handlers/providers/` | Remove probe handler + types |
| embedding_providers.probe | `src/gateway/handlers/embedding_providers.rs` | Remove probe handler + types |
| models.* RPC registrations | `src/bin/aleph/commands/start/builder/handlers.rs` | Remove 4 endpoints: `models.list`, `models.get`, `models.capabilities`, `models.refresh` |
| providers.probe RPC registration | `src/bin/aleph/commands/start/builder/handlers.rs` | Remove registration |
| embedding_providers.probe RPC | `src/bin/aleph/commands/start/builder/handlers.rs` | Remove registration |
| Extension provider list_models | `src/extension/provider_adapter.rs` | Remove `list_models()` and `static_models()` methods |

**NOT deleted** (intentionally preserved):
- `src/gateway/openai_api/routes.rs` — the OpenAI-compatible `/v1/models` endpoint for external tool integration is unrelated to internal model discovery and remains unchanged.

### 4. Frontend Deletions

| Target | Path | Action |
|--------|------|--------|
| ModelSelector component | `apps/panel/src/components/model_selector.rs` | Delete file |
| ProbeResultInfo, ProbeModelInfo | `apps/panel/src/api.rs` | Remove types |
| ProvidersApi::probe() | `apps/panel/src/api.rs` | Remove method |
| EmbeddingProvidersApi::probe() | `apps/panel/src/api.rs` | Remove method |
| Probe logic in providers view | `apps/panel/src/views/settings/providers.rs` | Remove probe flow |
| Probe logic in embedding view | `apps/panel/src/views/settings/embedding_providers.rs` | Remove probe flow |
| Probe logic in reranking view | `apps/panel/src/views/settings/reranking_providers.rs` | Remove probe flow |
| Probe logic in setup wizard | `apps/panel/src/views/wizard/setup_wizard.rs` | Remove probe flow |

### 5. Frontend UI Adaptation

All four provider settings pages get the same treatment:

- Replace `ModelSelector` with a simple `<input>` text field
- Placeholder: `e.g. gpt-4o, gpt-4o-mini, o1`
- Comma-separated input, saved as `Vec<String>` via split + trim
- Display as comma-joined string
- Remove all probe states (Loading/Success/Error), refresh buttons, model grouping

### 6. Code Reference Updates

All `config.model` references throughout the codebase change to `config.default_model()` (returns `&models[0]`). This is a large mechanical refactor (~114 occurrences across ~58 files). Key areas:

- Provider creation in `src/providers/mod.rs`
- Execution engine in `src/gateway/execution_engine/`
- Protocol implementations in `src/providers/protocols/` (including template.rs)
- Generation providers in `src/generation/providers/`
- Reranking providers in `src/memory/rerank/`
- Embedding provider in `src/memory/embedding_provider.rs`
- Dispatcher modules in `src/dispatcher/`
- Config presets in `src/config/presets_override.rs`
- Frontend API types in `apps/panel/src/api.rs`

**Template system compatibility**: `src/providers/protocols/template.rs` serializes config into a template context as `"model": config.model`. This must be updated to `"model": config.default_model()` so existing custom protocol templates using `{{config.model}}` continue to work.

### 7. Backward Compatibility

- Deserialization: `model = "xxx"` → `models = ["xxx"]` (custom serde deserializer accepting both String and Vec<String>)
- Serialization: always `models = [...]`
- `default_model()` method ensures callers don't need to know about the vec
- Template context: expose `config.model` key as `default_model()` value for backward-compatible templates

### 8. Out of Scope

- Agent-level model routing (`AgentModelConfig` usage at runtime) — separate follow-up
- Model capability inference — removed with ModelRegistry
- Model validation against provider — users are responsible for correct model names
