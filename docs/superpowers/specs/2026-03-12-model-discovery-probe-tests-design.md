# Model Discovery Probe Tests Design

## Summary

Comprehensive probe tests to validate the model discovery feature at production-level quality before merging. Uses a mixed strategy: mock-based tests (CI-runnable) as the primary line, plus `#[ignore]`-tagged real API tests for optional production compatibility verification.

Scope: focused on model discovery (list_models, ModelRegistry, preset fallback, RPC handlers). Full-stack provider verification (streaming, tool calls, multimodal) is a separate future effort.

## Problem

The model discovery implementation (8 commits on `worktree-model-discovery`) adds:
- `list_models()` on ProtocolAdapter trait
- OpenAI, Gemini, Ollama protocol implementations
- ModelRegistry cache with TTL and preset fallback
- New RPC handlers (models.refresh, models.set_default, models.set_model)

Current unit tests (81 passing) cover individual components. What's missing is **cross-layer integration validation**: API probe → parse → cache → fallback → RPC response, including error scenarios, concurrency, and backward compatibility.

## Design

### Architecture

```
L4: Real API (#[ignore])     ← optional, developer-triggered
L3: BDD (Cucumber)           ← RPC handler end-to-end scenarios
L2: Integration (wiremock)   ← mock HTTP server, full ModelRegistry flow
L1: Unit (JSON parsing)      ← protocol response parsing in isolation
```

Each layer catches different classes of bugs:
- L1: JSON schema changes, field mapping errors
- L2: Fallback logic, cache behavior, timeout handling, concurrency
- L3: RPC request/response format, parameter validation, backward compat
- L4: Real-world API compatibility drift

### Technology

| Tool | Purpose |
|------|---------|
| `wiremock` crate | Async mock HTTP server for L2 integration tests |
| Cucumber (existing) | BDD framework for L3 handler tests |
| `#[ignore]` + env vars | L4 real API tests, gated by API key availability |

### Prerequisites — Implementation Changes Required

The following minimal implementation changes are needed to make the tests feasible:

1. **`ModelRegistry::with_ttl()`** — Add a builder method to override TTL for cache expiry tests. Without this, cache expiry tests cannot run in reasonable time (default is 24h).

```rust
impl ModelRegistry {
    /// Create with custom TTL (primarily for testing)
    pub fn with_ttl(mut self, ttl: Duration) -> Self {
        self.ttl = ttl;
        self
    }
}
```

2. **Extract parse helpers** — For L1 unit tests, extract the JSON→DiscoveredModel mapping logic from `list_models()` into standalone `pub(crate)` functions in each protocol:

```rust
// openai.rs
pub(crate) fn parse_models_response(body: &serde_json::Value) -> Result<Vec<DiscoveredModel>>

// gemini.rs
pub(crate) fn parse_gemini_models_response(body: &serde_json::Value) -> Result<Vec<DiscoveredModel>>
```

Ollama already has `TagsResponse` struct with serde derive — L1 tests can use `serde_json::from_str::<TagsResponse>()` directly.

3. **BDD mutable config** — The `models.set_model` and `models.set_default` handlers require `Arc<RwLock<Config>>`. The existing `ModelsContext` provides `Arc<Config>` (read-only). New BDD steps must construct `Arc<tokio::sync::RwLock<Config>>` for these mutable handlers.

### File Layout

| Action | File | Layer | Tests |
|--------|------|-------|-------|
| Modify | `core/src/providers/protocols/openai.rs` | L1 | ~3 parse tests + extract `parse_models_response()` |
| Modify | `core/src/providers/protocols/gemini.rs` | L1 | ~3 parse tests + extract `parse_gemini_models_response()` |
| Modify | `core/src/providers/ollama.rs` | L1 | ~3 parse tests (uses existing `TagsResponse` serde) |
| Modify | `core/src/providers/model_registry.rs` | — | Add `with_ttl()` builder method |
| New | `core/tests/model_discovery_integration.rs` | L2 | ~12 wiremock tests |
| Modify | `core/tests/features/models/chat_handlers.feature` | L3 | Extend with ~10 model discovery scenarios |
| Modify | `core/tests/steps/models_steps.rs` | L3 | Add model discovery step implementations |
| New | `core/tests/real_api_probe.rs` | L4 | 4 `#[ignore]` real API tests |

Note: L2 and L4 tests go in `core/tests/` (integration test directory), not `core/src/`, to avoid module declaration complexity and follow Rust convention.

Note: L3 BDD scenarios are added to the existing `chat_handlers.feature` rather than a new file, since the existing file already covers `models.*` RPC handlers.

### L1: Unit Tests — Protocol Response Parsing

Pure JSON parsing tests, no network. Each protocol extracts a standalone parse function; tests exercise it directly.

#### OpenAI (openai.rs)

```rust
// Extract from list_models():
pub(crate) fn parse_models_response(body: &serde_json::Value) -> Result<Vec<DiscoveredModel>>

#[test] fn parse_models_response_success()
// Input: {"data": [{"id":"gpt-4o","owned_by":"openai"}, {"id":"gpt-4o-mini","owned_by":"openai"}]}
// Assert: 2 DiscoveredModels, ids correct, capabilities default to ["chat"]

#[test] fn parse_models_response_empty()
// Input: {"data": []}
// Assert: empty Vec

#[test] fn parse_models_response_malformed()
// Input: {"invalid": true}
// Assert: returns Error
```

#### Gemini (gemini.rs)

```rust
pub(crate) fn parse_gemini_models_response(body: &serde_json::Value) -> Result<Vec<DiscoveredModel>>

#[test] fn parse_gemini_models_success()
// Input: {"models": [{"name":"models/gemini-2.5-pro","displayName":"Gemini 2.5 Pro",...}]}
// Assert: id = "gemini-2.5-pro" (prefix stripped), name = "Gemini 2.5 Pro"

#[test] fn parse_gemini_models_empty()
#[test] fn parse_gemini_models_malformed()
```

#### Ollama (ollama.rs)

```rust
// No extraction needed — TagsResponse already has #[derive(Deserialize)]

#[test] fn parse_tags_response_success()
// Input: serde_json::from_str::<TagsResponse>(r#"{"models":[{"name":"llama3:latest",...}]}"#)
// Assert: 1 model, name = "llama3:latest"

#[test] fn parse_tags_response_empty()
// Input: {"models": []}
// Assert: empty models vec

#[test] fn parse_tags_response_vision_model()
// Input: model name = "llava:latest"
// Assert: vision model detection logic works (name contains "llava")
```

### L2: Integration Tests — wiremock + ModelRegistry

Full model discovery flow with mock HTTP servers. Located in `core/tests/model_discovery_integration.rs`.

Each test spins up a wiremock `MockServer`, creates a `ProviderConfig` with `base_url` pointing to the mock, and exercises `ModelRegistry` methods.

#### Scenario Group 1: API Probe Success

```rust
#[tokio::test] async fn probe_openai_models_via_api()
// wiremock serves GET /v1/models → standard OpenAI response
// Create OpenAiProtocol adapter, ProviderConfig with base_url = wiremock.uri()
// ModelRegistry.list_models() returns models with source = Api

#[tokio::test] async fn probe_gemini_models_via_api()
// wiremock serves GET /v1beta/models?key=... → standard Gemini response
// Verify id prefix stripping, capabilities mapping

#[tokio::test] async fn probe_ollama_tags_via_api()
// wiremock serves GET /api/tags → standard Ollama response
// Create OllamaProvider with base_url = wiremock.uri()
// Wrap in OllamaDiscoveryAdapter, pass to ModelRegistry
```

#### Scenario Group 2: Fallback Degradation

```rust
#[tokio::test] async fn fallback_to_preset_on_api_failure()
// wiremock returns 500 → ModelRegistry falls back to preset
// Assert: source = Preset, models match preset TOML content

#[tokio::test] async fn fallback_to_preset_on_timeout()
// wiremock delays 10s (exceeds 5s probe timeout)
// Assert: doesn't hang, degrades to preset

#[tokio::test] async fn fallback_to_preset_on_401_unauthorized()
// wiremock returns 401 → degrades to preset

#[tokio::test] async fn fallback_to_empty_when_no_preset()
// Ollama (no preset entry) + API failure → returns empty Vec

#[tokio::test] async fn fallback_to_preset_on_empty_api_response()
// wiremock returns 200 with {"data": []} (valid but empty)
// Assert: falls back to preset (not empty list)
// Tests the Ok(Some(vec![])) → preset fallback path
```

#### Scenario Group 3: Cache Behavior

```rust
#[tokio::test] async fn cache_hit_avoids_api_call()
// First call triggers wiremock, second call hits cache
// Assert: wiremock received exactly 1 request (use wiremock received_requests())

#[tokio::test] async fn cache_expires_after_ttl()
// Construct ModelRegistry::new(...).with_ttl(Duration::from_millis(100))
// Call list_models → wait 150ms → call again
// Assert: wiremock received 2 requests

#[tokio::test] async fn force_refresh_bypasses_cache()
// Cache not expired, call refresh() → wiremock receives new request
// Assert: returns updated model list
```

#### Scenario Group 4: Concurrency

```rust
#[tokio::test] async fn concurrent_list_models_no_panic()
// 10 concurrent list_models() + 2 concurrent refresh() via tokio::spawn
// Assert: all succeed, no panic/deadlock
// Tests RwLock behavior under contention
```

### L3: BDD Scenarios — RPC Handler End-to-End

Added to existing `core/tests/features/models/chat_handlers.feature`. Reuses AlephWorld + ModelsContext with extensions for mutable config.

```gherkin
  # === Model Discovery Scenarios ===

  # models.list integration
  Scenario: List models returns discovered models with source field
    Given a provider "openai" with model "gpt-4o"
    When I call "models.list"
    Then the response contains models with "source" field

  Scenario: List models with refresh=true forces re-probe
    Given a provider "openai" with model "gpt-4o"
    When I call "models.list" with refresh=true
    Then the model list is freshly probed

  Scenario: List models shows is_current for configured model
    Given a provider "openai" with model "gpt-4o"
    When I call "models.list"
    Then model "gpt-4o" has is_current=true
    And model "gpt-4o-mini" has is_current=false

  # models.refresh
  Scenario: Refresh single provider returns updated model list
    Given a provider "openai" with model "gpt-4o"
    When I call "models.refresh" with provider="openai"
    Then the response contains refreshed models and source

  Scenario: Refresh all providers returns aggregated results
    Given providers "openai" and "anthropic"
    When I call "models.refresh" without provider filter
    Then both providers' models are returned

  Scenario: Refresh unknown provider returns error
    When I call "models.refresh" with provider="nonexistent"
    Then the response is an error

  # models.set_model validation (requires Arc<RwLock<Config>>)
  Scenario: Set model to discovered model succeeds
    Given a mutable config with provider "openai" model "gpt-4o"
    When I call "models.set_model" with provider="openai" model="gpt-4o-mini"
    Then the response confirms model change

  Scenario: Set model to unknown model returns error
    Given a mutable config with provider "openai" model "gpt-4o"
    When I call "models.set_model" with provider="openai" model="gpt-99"
    Then the response is an error with model not found

  # Backward compatibility (models.set uses "model" param, not "provider")
  Scenario: models.set delegates to set_default
    Given a mutable config with providers "openai" and "anthropic" default "openai"
    When I call "models.set" with model="anthropic"
    Then the default provider is now "anthropic"

  # Preset-only path
  Scenario: Anthropic returns preset models without API probe
    Given a provider "anthropic" with model "claude-opus-4-20250514"
    When I call "models.list" for provider "anthropic"
    Then models are from source "preset"
    And models include Claude Opus, Sonnet, and Haiku
```

Implementation notes for new BDD steps:
- `Given a mutable config with ...` steps create `Arc<tokio::sync::RwLock<Config>>` instead of `Arc<Config>`
- `models.set` scenario uses `model="anthropic"` (matching legacy `handle_set` which reads `params["model"]`)
- Existing `chat_handlers.feature` scenarios that assert "models array should have N models" may need updating since `handle_list` now returns discovered models per provider, not just 1 per provider

### L4: Real API Tests

Located in `core/tests/real_api_probe.rs`. Gated by `#[ignore]`. Run via `cargo test -p alephcore --test real_api_probe -- --ignored`.

```rust
#[tokio::test]
#[ignore]
async fn real_openai_list_models()
// Requires: OPENAI_API_KEY env var
// Creates OpenAiProtocol + ProviderConfig with real API key
// Calls list_models() → asserts non-empty, contains "gpt-" model
// Prints model count and first 5 IDs for human inspection

#[tokio::test]
#[ignore]
async fn real_gemini_list_models()
// Requires: GEMINI_API_KEY env var
// Assert: contains model with "gemini-" prefix

#[tokio::test]
#[ignore]
async fn real_ollama_list_models()
// Requires: local Ollama running on :11434
// Creates OllamaProvider, calls list_models()
// Assert: returns Ok (may be empty if no models pulled)

#[tokio::test]
#[ignore]
async fn real_full_discovery_flow()
// Checks which env vars are set, tests all available providers
// Runs full ModelRegistry flow: probe → verify cache → refresh
// Validates: source = Api, capabilities non-empty, cache hit on 2nd call
```

## Test Summary

| Layer | Count | Dependencies | CI | Purpose |
|-------|-------|--------------|----|---------|
| L1 Unit | ~9 | None | ✅ | JSON parsing correctness |
| L2 Integration | ~12 | wiremock | ✅ | Full discovery flow with mock HTTP |
| L3 BDD | ~10 | Cucumber | ✅ | RPC handler end-to-end |
| L4 Real API | 4 | API keys / Ollama | ❌ Manual | Production compatibility |
| **Total** | **~35** | | | |

## New Dependency

```toml
# core/Cargo.toml [dev-dependencies]
wiremock = "0.6"
```

## Error Handling Coverage

| Scenario | Layer | Expected Behavior |
|----------|-------|-------------------|
| API returns 500 | L2 | Silent fallback to preset |
| API timeout (>5s) | L2 | No hang, fallback to preset |
| API returns 401 | L2 | Warn log, fallback to preset |
| API returns empty list | L2 | Fallback to preset |
| No preset for protocol | L2 | Return empty list |
| Malformed JSON response | L1 | Return Error |
| Concurrent read + write | L2 | No panic/deadlock |
| Unknown provider in RPC | L3 | Error response |
| Invalid model in set_model | L3 | Error with suggestion |

## Out of Scope (Future Work)

- Full-stack provider verification (streaming, tool calls, multimodal)
- Load/performance testing
- Provider health monitoring
- Cross-provider model routing tests
