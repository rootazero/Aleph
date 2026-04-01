# Model-Aware Provider Fallback & Agent Model Routing

**Date:** 2026-03-28
**Status:** Approved
**Approach:** Model-Aware Provider Registry (方案 A)

## Problem

Current provider/model system has three gaps:

1. **Agent model field is dead code** — `AgentInstanceConfig.model` is resolved but `ExecutionEngine` always calls `default_provider()`, ignoring it
2. **Fallback lacks health awareness** — `call_with_fallback()` in `thinker/fallback.rs` handles transient/permanent error classification, but has no persistent health tracking (each request starts fresh, no memory of previous failures) and no model-level routing (operates at provider-name level only)
3. **No user visibility** — when a fallback occurs, the user has no way to know which model actually served the response

## Design Decisions

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Architecture | Extend `MultiProviderRegistry` | Reuse existing infrastructure, no new abstraction layer (P6) |
| Error handling | Error-aware (transient vs permanent) | Simple classification, avoids wasting requests on permanently broken providers |
| Model selection | Agent config-driven | Let existing `AgentInstanceConfig.model` field work, not LLM self-selection (avoids bootstrap paradox) |
| Fallback scope | Agent-level first, global fallback | Two-layer: `agent.fallbacks → global.fallback_providers` — clear semantics |
| User notification | Silent + metadata annotation | Response metadata contains `ModelInfo`; Panel shows inline indicator on fallback |
| Retry strategy | No auto-retry, direct fallback | Fallback to another model is better UX than waiting for the same model to recover |

## Data Structures

### ProviderHealth (new — `src/providers/health.rs`)

```rust
pub enum ProviderHealth {
    Healthy,
    Degraded {
        since: Instant,
        cooldown_until: Instant,  // exponential backoff: 30s → 60s → 120s, cap 5min
        consecutive_failures: u32,
    },
    Unavailable {
        since: Instant,
        reason: String,           // "401 Unauthorized", "model not found"
    },
}

impl ProviderHealth {
    fn is_usable(&self) -> bool;       // Healthy=true, Degraded past cooldown=true, else false
    fn record_success(&mut self);       // → Healthy, reset counters
    fn record_failure(&mut self, err: &ProviderError); // classify → Degraded or Unavailable
}
```

### ProviderError (new — `src/providers/health.rs`)

```rust
pub enum ProviderError {
    Transient(TransientError),   // → Degraded
    Permanent(PermanentError),   // → Unavailable
}

pub enum TransientError {
    RateLimited { retry_after: Option<Duration> },  // 429
    ServerError { status: u16 },                     // 500, 502, 503
    Timeout,
    ConnectionFailed,
}

pub enum PermanentError {
    AuthFailed,          // 401, 403
    ModelNotFound,       // 404 + model-related error body
}
// Note: 400 InvalidRequest is NOT a provider health issue — it's request-specific.
// 400 errors are returned as AlephError but do NOT affect ProviderHealth state.
```

### RequestPayload change

```rust
pub struct RequestPayload<'a> {
    pub model: Option<String>,    // NEW — overrides provider's default model
    pub messages: ...,
    pub tools: ...,
    // ... existing fields
}
```

### MultiProviderRegistry changes

```rust
struct MultiProviderRegistry {
    providers: HashMap<String, Arc<dyn AiProvider>>,
    health: RwLock<HashMap<String, ProviderHealth>>,  // NEW
    default_name: String,
    fallbacks: Vec<String>,
}

impl MultiProviderRegistry {
    /// Resolve model → find healthy (provider, model) along fallback chain
    pub fn resolve_with_fallback(
        &self,
        model: &str,
        agent_fallbacks: &[String],
    ) -> Result<ResolvedModel, AllProvidersUnavailable>;

    /// Report request outcome to update health state
    pub fn report_outcome(&self, provider: &str, result: Result<(), ProviderError>);
}
```

### ResolvedModel (new)

```rust
pub struct ResolvedModel {
    pub provider: Arc<dyn AiProvider>,
    pub model: String,              // actual model used
    pub is_fallback: bool,
    pub original_model: String,     // originally requested model
}
```

### ModelInfo (new — response metadata)

```rust
pub struct ModelInfo {
    pub model: String,
    pub provider: String,
    pub is_fallback: bool,
    pub original_model: Option<String>,
}
```

## Execution Flow

### run_loop.rs — core change

**Before:**
```rust
let provider = self.provider_registry.default_provider();
let result = provider.process(payload).await;
```

**After:**
```rust
let model = &agent_config.model;  // String, always populated (defaults to "claude-sonnet-4-5")
let agent_fallbacks = &agent_config.fallback_models;  // Vec<String>

let resolved = self.provider_registry
    .resolve_with_fallback(model, agent_fallbacks)?;

payload.model = Some(resolved.model.clone());

let result = resolved.provider.process(payload).await;

self.provider_registry.report_outcome(
    &resolved.provider.name(),
    &result,
);
```

### resolve_with_fallback() logic

```
fn resolve_with_fallback(model, agent_fallbacks) -> Result<ResolvedModel> {
    // Build candidate chain:
    // [requested model] → [agent fallbacks] → [global fallbacks' default model]
    let candidates = [model]
        .chain(agent_fallbacks)
        .chain(global_fallbacks.map(|p| p.default_model));

    for candidate in candidates {
        // 1. Find provider for this model
        let provider_name = resolve_provider_from_model(candidate);
        let provider = self.providers.get(provider_name)?;

        // 2. Check health
        match health {
            Healthy => return Ok(ResolvedModel { provider, model: candidate, ... }),
            Degraded { cooldown_until } if now > cooldown_until => return Ok(...),
            Degraded { .. } => continue,       // still in cooldown, skip
            Unavailable { .. } => continue,    // permanent error, skip
        }
    }

    Err(AllProvidersUnavailable)
}
```

### Protocol Adapter changes

All adapters unified: `payload.model > config.models[0]`

```rust
// OpenAiProtocol::build_request()
"model": payload.model.unwrap_or(&self.config.models[0])

// AnthropicProtocol — same pattern
// GeminiProtocol — model in URL path, read from payload.model
// ResponsesProtocol — same as OpenAI
```

### HTTP error classification

In `HttpProvider`, after receiving HTTP response, classify into `ProviderError` for health tracking.
This complements the existing `AlephError::is_transient()` — conversion is via `impl From<&AlephError> for Option<ProviderError>`:

- 429 → `TransientError::RateLimited { retry_after }` (parse Retry-After header)
- 500/502/503 → `TransientError::ServerError`
- Request timeout → `TransientError::Timeout`
- Connection refused/DNS failure → `TransientError::ConnectionFailed`
- 401/403 → `PermanentError::AuthFailed`
- 404 (model-related) → `PermanentError::ModelNotFound`
- 400 → No health impact (request-specific error, returned as `AlephError` but does not affect `ProviderHealth`)
- 2xx → Success

### Relationship to existing `fallback.rs`

The existing `thinker/fallback.rs` (`call_with_fallback()`) will be **replaced** by the new `resolve_with_fallback()` + `report_outcome()` pattern:

- **Old:** `call_with_fallback()` attempts the primary provider, catches transient errors, retries with fallbacks — all in a single synchronous call chain. No persistent health memory.
- **New:** `resolve_with_fallback()` pre-selects a healthy provider based on accumulated health state, then `report_outcome()` feeds back the result. Health persists across requests within a session.
- **Migration:** `call_with_fallback()` callers in `run_loop.rs` switch to the new pattern. The `fallback.rs` file is removed after migration. Its test cases are migrated to `registry_test.rs`.

### Model name resolution

Models in fallback chains can use two formats:
- **Bare model name:** `"claude-sonnet-4"` — resolved via `resolve_provider_from_model()` prefix matching
- **Explicit `provider/model` syntax:** `"openai/gpt-4o"` — parsed by `MultiProviderRegistry.get()` directly (already supported)

The `provider/model` syntax is **recommended** for fallback configuration to avoid ambiguity with custom model names (e.g., OpenRouter models like `meta-llama/llama-3.1-405b`).

### Streaming and `report_outcome` timing

- **Success:** `report_outcome` called after stream completes (final delta received)
- **Pre-stream error:** called immediately when HTTP returns error status before streaming begins
- **Mid-stream error:** treated as transient failure (connection was initially healthy)

## User Notification

### Panel (Leptos WASM)

- **Normal:** message bubble shows model name in small text (top-right), same as current
- **Fallback (transient):** shows `opus → sonnet` with yellow arrow highlight, inline, non-disruptive
- **All unavailable:** toast notification at top asking user to check configuration

### Non-Panel channels (Telegram/CLI)

When `is_fallback = true`, append one line to reply:
```
⚡ 已从 opus-4 降级到 sonnet-4 (原因: 限流)
```

Normal responses show nothing extra.

### Stream event format

```json
{
  "type": "message_start",
  "model_info": {
    "model": "claude-sonnet-4-20250514",
    "provider": "claude",
    "is_fallback": true,
    "original_model": "claude-opus-4-20250115"
  }
}
```

## Edge Cases

| Scenario | Handling |
|----------|----------|
| Agent model always populated | `AgentInstanceConfig.model` is `String` (not `Option`), always has a value from agent resolver cascade. No "missing model" case |
| Model name can't resolve to a provider | Use `default_provider` and let API return natural error. Recommend `provider/model` syntax for custom models |
| All providers Unavailable | Return `AllProvidersUnavailable` error, Gateway converts to user-readable message, Panel shows toast |
| Degraded auto-recovery | After cooldown expires, next request attempts the provider. Success → `record_success()` → Healthy |
| Unavailable recovery path | No auto-recovery. User fixes config, then `test_connection` or `set_default` RPC resets to Healthy |
| Concurrency | `health` HashMap uses `RwLock`. `report_outcome` takes brief write lock. Read-heavy workload |

## Files Changed

### New files
- `src/providers/health.rs` — ProviderHealth + ProviderError enums + state machine logic
- `src/providers/health_test.rs` — health state transitions + cooldown tests
- `src/providers/registry_test.rs` — resolve_with_fallback chain tests

### Removed files
- `src/thinker/fallback.rs` — replaced by resolve_with_fallback() + report_outcome() in registry

### Modified files
- `src/providers/mod.rs` — re-export health module
- `src/providers/registry.rs` — MultiProviderRegistry + health HashMap + resolve_with_fallback() + report_outcome()
- `src/providers/types.rs` — RequestPayload adds model field + ResolvedModel + ModelInfo structs
- `src/providers/protocols/openai.rs` — payload.model > config.models[0]
- `src/providers/protocols/anthropic.rs` — same
- `src/providers/protocols/gemini.rs` — URL path model from payload
- `src/providers/protocols/responses.rs` — same as openai
- `src/providers/http_provider.rs` — HTTP error → ProviderError classification
- `src/thinker/run_loop.rs` — resolve_with_fallback + report_outcome
- `src/thinker/mod.rs` — ProviderRegistry trait new method signatures
- `src/thinker/stream_types.rs` — stream event includes ModelInfo
- `interfaces/webchat/src/views/chat/` — parse ModelInfo, fallback indicator

## Out of Scope

- Model list auto-discovery (Test button fetching model lists) — future iteration
- Health state persistence across restarts — YAGNI, restart frequency is low
- Per-model health granularity (current: per-provider) — same provider rarely has partial model failures
- Auto-retry on failure — direct fallback provides better UX
