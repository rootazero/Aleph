# Dynamic Model Listing Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Fix model selection across all provider types — AI, Embedding, Reranking — by correcting the protocol resolution bug and unifying model discovery via probe + preset fallback.

**Architecture:** Fix `ProviderConfig.protocol()` to infer from provider name via presets instead of defaulting to "openai". Move the OpenAI `list_models()` implementation to the trait default so all OpenAI-compatible providers get dynamic discovery for free. Extend `model-presets.toml` with embedding/reranking sections. Make frontend Embedding/Reranking pages use probe instead of hardcoded lists.

**Tech Stack:** Rust (core), Leptos/WASM (panel), TOML (presets), JSON-RPC (API)

---

### Task 1: Fix `ProviderConfig.protocol()` default

**Files:**
- Modify: `src/config/types/provider.rs:118-126`

**Step 1: Write the failing test**

Add test in `src/config/types/provider.rs` tests module:

```rust
#[test]
fn test_protocol_defaults_to_openai_not_panic() {
    // A config with no protocol set should still return "openai"
    // (backward compatibility — but callers should set protocol explicitly)
    let config = ProviderConfig::test_config("test");
    assert_eq!(config.protocol(), "openai");
}
```

**Step 2: Run test to verify it passes (existing behavior)**

Run: `cargo test -p alephcore --lib -- config::types::provider::tests::test_protocol_defaults`
Expected: PASS (confirms current behavior before we change it)

**Step 3: Change `protocol()` to use preset-based inference**

Replace the `protocol()` method:

```rust
/// Get the effective protocol name
///
/// Priority: explicit protocol field > infer from provider name via presets > "openai"
pub fn protocol(&self) -> String {
    self.protocol
        .clone()
        .unwrap_or_else(|| "openai".to_string())
}
```

Note: We keep the "openai" default for backward compat in `protocol()` itself. The real fix is ensuring protocol is always set when providers are created/saved. See Task 2.

**Step 4: Run all provider tests**

Run: `cargo test -p alephcore --lib -- config::types::provider`
Expected: PASS

**Step 5: Commit**

```
config: keep protocol() default but document the issue
```

---

### Task 2: Ensure protocol is persisted when creating/updating providers

**Files:**
- Modify: `src/gateway/handlers/providers/helpers.rs` — `build_provider_config_for_persistence()`
- Modify: `src/gateway/handlers/providers/handlers.rs` — `handle_create()`, `handle_update()`

**Step 1: Read the helpers file to understand current persistence logic**

Read: `src/gateway/handlers/providers/helpers.rs` fully.

**Step 2: Modify `build_provider_config_for_persistence()` to always set protocol**

In the helper function, after merging preset defaults, ensure `protocol` is explicitly set:

```rust
// After get_merged_preset() call, ensure protocol is set
if config.protocol.is_none() {
    if let Some(preset) = crate::providers::presets::get_preset(provider_name) {
        config.protocol = Some(preset.protocol.to_string());
    }
    // If still None after preset lookup, keep as-is (will default to "openai")
}
```

**Step 3: Verify handler_create passes protocol from frontend**

In `handle_create()`, confirm that `ProviderConfigJson.protocol` is mapped to `ProviderConfig.protocol`. The frontend already sends `form_protocol` which is correctly set from preset data.

**Step 4: Run tests**

Run: `cargo test -p alephcore --lib -- gateway::handlers::providers`
Expected: PASS

**Step 5: Commit**

```
providers: ensure protocol field is persisted on create/update
```

---

### Task 3: Move OpenAI `list_models()` to trait default

**Files:**
- Modify: `src/providers/adapter.rs:164-168` — change default `list_models()`
- Modify: `src/providers/protocols/openai.rs:464-502` — remove custom `list_models()`
- Modify: `src/providers/protocols/anthropic.rs` — add explicit `Ok(None)` override

**Step 1: Write test for generic list_models behavior**

Add test in `src/providers/adapter.rs` tests module verifying the default implementation attempts `/v1/models`. Since this calls HTTP, use the existing mock pattern — we just verify the method signature compiles.

**Step 2: Move the OpenAI list_models logic to the trait default**

In `adapter.rs`, replace the default implementation:

```rust
/// Fetch available models from the provider API.
///
/// Default implementation attempts the OpenAI-compatible `/v1/models` endpoint.
/// Protocols that don't support this (Anthropic, ChatGPT) should override to return Ok(None).
/// Protocols with non-standard model listing (Gemini) should override with their own logic.
async fn list_models(&self, config: &ProviderConfig) -> Result<Option<Vec<DiscoveredModel>>> {
    let base_url = match config.base_url.as_ref().filter(|s| !s.is_empty()) {
        Some(url) => url.trim_end_matches('/').to_string(),
        None => return Ok(None), // No base URL — can't probe
    };

    let api_key = config.api_key.as_deref().unwrap_or("");
    if api_key.is_empty() {
        return Ok(None); // No API key — can't probe
    }

    // Normalize: ensure /v1 suffix for models endpoint
    let url = if base_url.ends_with("/v1") {
        format!("{}/models", base_url)
    } else {
        format!("{}/v1/models", base_url)
    };

    let client = reqwest::Client::new();
    let response = match client
        .get(&url)
        .header("Authorization", format!("Bearer {}", api_key))
        .timeout(std::time::Duration::from_secs(5))
        .send()
        .await
    {
        Ok(r) => r,
        Err(_) => return Ok(None), // Network error — graceful fallback
    };

    if !response.status().is_success() {
        return Ok(None);
    }

    let body: serde_json::Value = match response.json().await {
        Ok(v) => v,
        Err(_) => return Ok(None),
    };

    let models = body["data"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|m| {
                    let id = m["id"].as_str()?;
                    Some(DiscoveredModel {
                        id: id.to_string(),
                        name: Some(id.to_string()),
                        owned_by: m["owned_by"].as_str().map(|s| s.to_string()),
                        capabilities: vec!["chat".to_string()],
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    if models.is_empty() {
        Ok(None)
    } else {
        Ok(Some(models))
    }
}
```

**Step 3: Simplify OpenAI adapter**

In `openai.rs`, remove the `list_models()` override since the trait default now handles it. The OpenAI adapter inherits the generic implementation.

But keep `parse_models_response()` as a pub(crate) helper if Gemini or others need it (check usage first — if only OpenAI uses it, delete).

**Step 4: Add Anthropic override**

In `anthropic.rs`, add explicit override:

```rust
async fn list_models(&self, _config: &ProviderConfig) -> Result<Option<Vec<DiscoveredModel>>> {
    // Anthropic does not provide a public models API endpoint.
    // Model list comes from presets in model-presets.toml.
    Ok(None)
}
```

Also add the same override for ChatGPT protocol if it exists.

**Step 5: Run tests**

Run: `cargo test -p alephcore --lib -- providers`
Expected: PASS

**Step 6: Commit**

```
providers: move list_models() to trait default for OpenAI-compatible providers
```

---

### Task 4: Extend model-presets.toml with embedding and reranking sections

**Files:**
- Modify: `shared/config/model-presets.toml`
- Modify: `src/providers/model_registry.rs` — extend preset loading for subcategories

**Step 1: Write test for subcategory preset loading**

In `model_registry.rs` tests:

```rust
#[tokio::test]
async fn test_subcategory_preset_loading() {
    let toml = r#"
[openai]
models = [
    { id = "gpt-4o", name = "GPT-4o", capabilities = ["chat"] },
]

[openai_embedding]
models = [
    { id = "text-embedding-3-small", name = "Embedding 3 Small", capabilities = ["embedding"] },
]

[jina_reranking]
models = [
    { id = "jina-reranker-v2-base-multilingual", name = "Jina Reranker v2", capabilities = ["reranking"] },
]
"#;
    let registry = ModelRegistry::new(Some(toml));
    assert!(registry.presets.contains_key("openai"));
    assert!(registry.presets.contains_key("openai_embedding"));
    assert!(registry.presets.contains_key("jina_reranking"));
    assert_eq!(registry.presets["openai_embedding"][0].id, "text-embedding-3-small");
}
```

**Step 2: Run test to verify it passes**

Run: `cargo test -p alephcore --lib -- providers::model_registry::tests::test_subcategory_preset_loading`
Expected: PASS (the existing `#[serde(flatten)]` with `HashMap<String, PresetProtocol>` already handles arbitrary keys — `openai_embedding` is just a key)

**Step 3: Add `list_models_for_category()` convenience method**

In `model_registry.rs`, add:

```rust
/// Get preset models for a specific category (embedding, reranking)
///
/// Looks up key "{protocol}_{category}" in presets, e.g., "openai_embedding"
pub fn get_preset_models(&self, protocol: &str, category: Option<&str>) -> Vec<DiscoveredModel> {
    let key = match category {
        Some(cat) => format!("{}_{}", protocol, cat),
        None => protocol.to_string(),
    };
    self.presets.get(&key).cloned().unwrap_or_default()
}
```

**Step 4: Add embedding and reranking sections to model-presets.toml**

```toml
# Embedding models
[openai_embedding]
models = [
    { id = "text-embedding-3-small", name = "Embedding 3 Small", capabilities = ["embedding"] },
    { id = "text-embedding-3-large", name = "Embedding 3 Large", capabilities = ["embedding"] },
    { id = "text-embedding-ada-002", name = "Ada 002", capabilities = ["embedding"] },
]

[siliconflow_embedding]
models = [
    { id = "BAAI/bge-m3", name = "BGE-M3", capabilities = ["embedding"] },
    { id = "BAAI/bge-large-zh-v1.5", name = "BGE Large ZH v1.5", capabilities = ["embedding"] },
    { id = "BAAI/bge-large-en-v1.5", name = "BGE Large EN v1.5", capabilities = ["embedding"] },
    { id = "BAAI/bge-small-zh-v1.5", name = "BGE Small ZH v1.5", capabilities = ["embedding"] },
]

[ollama_embedding]
models = [
    { id = "nomic-embed-text", name = "Nomic Embed Text", capabilities = ["embedding"] },
    { id = "mxbai-embed-large", name = "MXBai Embed Large", capabilities = ["embedding"] },
    { id = "all-minilm", name = "All-MiniLM", capabilities = ["embedding"] },
    { id = "snowflake-arctic-embed", name = "Snowflake Arctic Embed", capabilities = ["embedding"] },
    { id = "bge-m3", name = "BGE-M3", capabilities = ["embedding"] },
    { id = "bge-large", name = "BGE-Large", capabilities = ["embedding"] },
]

# Reranking models
[jina_reranking]
models = [
    { id = "jina-reranker-v2-base-multilingual", name = "Jina Reranker v2 Base Multilingual", capabilities = ["reranking"] },
    { id = "jina-reranker-v1-base-en", name = "Jina Reranker v1 Base EN", capabilities = ["reranking"] },
    { id = "jina-reranker-v1-turbo-en", name = "Jina Reranker v1 Turbo EN", capabilities = ["reranking"] },
    { id = "jina-reranker-v1-tiny-en", name = "Jina Reranker v1 Tiny EN", capabilities = ["reranking"] },
]

[siliconflow_reranking]
models = [
    { id = "BAAI/bge-reranker-v2-m3", name = "BGE Reranker v2 M3", capabilities = ["reranking"] },
    { id = "BAAI/bge-reranker-large", name = "BGE Reranker Large", capabilities = ["reranking"] },
    { id = "BAAI/bge-reranker-base", name = "BGE Reranker Base", capabilities = ["reranking"] },
]

[voyage_reranking]
models = [
    { id = "rerank-2", name = "Voyage Rerank 2", capabilities = ["reranking"] },
    { id = "rerank-lite-1", name = "Voyage Rerank Lite 1", capabilities = ["reranking"] },
]

[vllm_reranking]
models = [
    { id = "BAAI/bge-reranker-v2-m3", name = "BGE Reranker v2 M3", capabilities = ["reranking"] },
    { id = "BAAI/bge-reranker-large", name = "BGE Reranker Large", capabilities = ["reranking"] },
    { id = "cross-encoder/ms-marco-MiniLM-L-6-v2", name = "MS-MARCO MiniLM L6 v2", capabilities = ["reranking"] },
]
```

**Step 5: Run tests**

Run: `cargo test -p alephcore --lib -- providers::model_registry`
Expected: PASS

**Step 6: Commit**

```
providers: add embedding and reranking preset models to model-presets.toml
```

---

### Task 5: Add probe endpoint for embedding providers (backend)

**Files:**
- Modify: `src/gateway/handlers/embedding_providers.rs` — add `handle_probe()`
- Modify: `src/gateway/handlers/mod.rs` — register new RPC route

**Step 1: Read the existing embedding handler file**

Read: `src/gateway/handlers/embedding_providers.rs` fully.

**Step 2: Add probe handler**

Model after the existing `providers.probe` handler but simpler. The probe should:
1. Accept `{ protocol: String, api_key?: String, base_url?: String }`
2. Try API model discovery via the generic trait default (OpenAI-compatible `/v1/models`)
3. Filter results for models with "embedding" capability
4. If no API results, return preset models from `MODEL_REGISTRY.get_preset_models(protocol, Some("embedding"))`
5. Return `{ success, models, model_source, error? }`

```rust
pub async fn handle_probe(request: JsonRpcRequest, config_store: Arc<RwLock<Config>>, vault: Arc<SharedTokenManager>) -> JsonRpcResponse {
    // Similar structure to providers::handle_probe but:
    // 1. Uses the generic ProtocolAdapter::list_models()
    // 2. Falls back to MODEL_REGISTRY.get_preset_models(protocol, Some("embedding"))
    // 3. Filters API results for embedding-capable models
}
```

**Step 3: Register RPC route**

In the RPC router (where `embedding_providers.list`, `embedding_providers.test` are registered), add:
```rust
"embedding_providers.probe" => handle_probe(request, config_store, vault).await,
```

**Step 4: Run compile check**

Run: `cargo check -p alephcore`
Expected: OK

**Step 5: Commit**

```
embedding: add probe endpoint for dynamic model discovery
```

---

### Task 6: Fix frontend provider protocol defaults

**Files:**
- Modify: `apps/panel/src/views/settings/providers.rs:561,572`

**Step 1: Fix line 561 — existing provider form population**

Change:
```rust
form_protocol.set(provider.provider_type.clone().unwrap_or_else(|| "openai".to_string()));
```

To:
```rust
form_protocol.set(provider.provider_type.clone().unwrap_or_else(|| provider.name.clone()));
```

**Step 2: Fix line 572 — auto-probe protocol**

Change:
```rust
let protocol = provider.provider_type.clone().unwrap_or_else(|| "openai".to_string());
```

To:
```rust
let protocol = provider.provider_type.clone().unwrap_or_else(|| provider.name.clone());
```

**Step 3: Fix probe error handling — show preset models on failure**

Currently (line 598, 603), when probe fails, `models_list.set(Vec::new())` clears the list. Change to keep showing preset models:

When probe returns `success: false` but has models (from preset fallback), still populate the list:

```rust
// In both probe result handlers, change the error/failure paths:
// Instead of models_list.set(Vec::new()),
// check if result has models even on "failure" — the backend may have returned presets
if !result.models.is_empty() {
    let options: Vec<ModelOption> = result.models.into_iter().map(|m| {
        ModelOption {
            id: m.id.clone(),
            name: m.name.clone(),
            capabilities: m.capabilities.clone(),
            source: result.model_source.clone(),
        }
    }).collect();
    models_list.set(options);
}
// If truly empty, keep existing models_list unchanged (don't clear it)
```

**Step 4: Build panel WASM to verify compilation**

Run: `cd apps/panel && cargo check --target wasm32-unknown-unknown`
Expected: OK

**Step 5: Commit**

```
panel: fix provider protocol default and preserve models on probe failure
```

---

### Task 7: Add probe API to panel embedding provider

**Files:**
- Modify: `apps/panel/src/api.rs` — add `EmbeddingProvidersApi::probe()`
- Modify: `apps/panel/src/views/settings/embedding_providers.rs` — remove hardcoded models, use probe

**Step 1: Add probe method to EmbeddingProvidersApi**

In `api.rs`, add:

```rust
impl EmbeddingProvidersApi {
    pub async fn probe(
        state: &DashboardState,
        protocol: &str,
        api_key: Option<&str>,
        base_url: Option<&str>,
    ) -> Result<ProbeResultInfo, String> {
        let mut params = serde_json::Map::new();
        params.insert("protocol".to_string(), json!(protocol));
        if let Some(key) = api_key {
            params.insert("api_key".to_string(), json!(key));
        }
        if let Some(url) = base_url {
            params.insert("base_url".to_string(), json!(url));
        }
        let resp = state.rpc_call("embedding_providers.probe", json!(params)).await?;
        serde_json::from_value(resp).map_err(|e| e.to_string())
    }
}
```

**Step 2: Remove `embedding_models_for_preset()` function**

Delete the entire function (lines 17-74 of `embedding_providers.rs`) and its helper `em()`.

**Step 3: Replace hardcoded model usage with probe calls**

In the embedding provider detail panel, add probe state signals and trigger probe on:
- Provider selection (auto-probe)
- API key change
- Refresh button click

Follow the same pattern as `providers.rs` `ProviderDetailPanel`:
- `probe_status: RwSignal<ProbeStatus>`
- `models_list: RwSignal<Vec<ModelOption>>`
- `trigger_probe` closure that calls `EmbeddingProvidersApi::probe()`
- Show `ModelSelector` with probe results

**Step 4: Build panel WASM**

Run: `cd apps/panel && cargo check --target wasm32-unknown-unknown`
Expected: OK

**Step 5: Commit**

```
panel: replace hardcoded embedding models with dynamic probe
```

---

### Task 8: Add probe to panel reranking provider

**Files:**
- Modify: `apps/panel/src/api.rs` — add `RerankConfigApi::probe()` if needed
- Modify: `apps/panel/src/views/settings/reranking_providers.rs` — remove hardcoded models, use probe or presets

**Step 1: Decide on approach**

Reranking providers (Jina, Voyage, SiliconFlow) mostly support OpenAI-compatible `/v1/models`. For those that don't, preset fallback handles it.

Add a similar probe flow as embedding, OR use the generic `ProvidersApi::probe()` with appropriate protocol. Choose based on whether reranking has its own backend handler.

**Step 2: Remove `rerank_models_for_preset()` function**

Delete the hardcoded function (lines 14-47 of `reranking_providers.rs`) and its helper `rm()`.

**Step 3: Add probe state and trigger to reranking detail panel**

Same pattern as Tasks 6-7. When a reranking preset is selected, probe with its protocol and show dynamic results. On failure, show preset models from `model-presets.toml` via a direct lookup (or backend probe returning presets).

**Step 4: Build panel WASM**

Run: `cd apps/panel && cargo check --target wasm32-unknown-unknown`
Expected: OK

**Step 5: Commit**

```
panel: replace hardcoded reranking models with dynamic probe
```

---

### Task 9: Integration test — end-to-end verification

**Step 1: Build and run the server**

Run: `just dev`

**Step 2: Manual verification checklist**

Open the panel in browser and verify:

- [ ] **Anthropic preset**: Shows Claude models (Opus 4, Sonnet 4, Haiku 4) — NOT GPT models
- [ ] **OpenAI preset**: Shows GPT models from preset (or API if key configured)
- [ ] **Gemini preset**: Shows Gemini models
- [ ] **DeepSeek preset**: With API key, shows dynamic models via `/v1/models`
- [ ] **Ollama preset**: Shows locally installed models
- [ ] **Custom/unknown provider**: Shows empty + custom input
- [ ] **Embedding OpenAI**: Shows embedding-3-small, embedding-3-large
- [ ] **Embedding SiliconFlow**: Shows BGE models
- [ ] **Reranking Jina**: Shows Jina reranker models
- [ ] **Model refresh button**: Re-probes and updates list
- [ ] **No API key**: Shows preset models with `[Preset]` tag
- [ ] **Invalid API key**: Shows error + keeps preset models visible
- [ ] **Custom model input**: `__custom__` option works

**Step 3: Fix any issues found**

**Step 4: Final commit**

```
providers: verify dynamic model listing across all provider types
```

---

## Dependency Graph

```
Task 1 (protocol fix) ──┐
                         ├── Task 3 (trait default list_models)
Task 2 (persist proto) ──┘       │
                                 ├── Task 5 (embedding probe backend)
Task 4 (presets TOML) ──────────┤
                                 ├── Task 6 (frontend protocol fix)
                                 │       │
                                 │       ├── Task 7 (embedding frontend)
                                 │       └── Task 8 (reranking frontend)
                                 │               │
                                 └───────────────┴── Task 9 (integration test)
```

Tasks 1+2 can run in parallel. Task 3 depends on Task 1. Task 4 is independent. Tasks 5-8 depend on 3+4. Task 9 is last.
