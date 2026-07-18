# Model Routing Optimization: Dead Code Cleanup + Lightweight Routing + Tool Use Adaptation

**Date**: 2026-03-24
**Status**: Approved
**Scope**: src/dispatcher/, src/thinker/, src/providers/

## Problem Statement

Aleph's model routing system has three critical issues:

1. **35,764 lines of dead code** in `dispatcher/` — `model_router/`, `engine/`, `scheduler/`, `planner/`, `executor/`, `agent_types/`, `monitor/`, `tool_index/`, `analyzer.rs`, `callback.rs`, `context/` are never called by the actual execution path (`gateway/` → `agent_loop/`)
2. **ProviderRegistry ignores model parameter** — `SingleProviderRegistry` and `SwappableProviderRegistry` both return the same provider regardless of model key, making multi-provider routing impossible
3. **No tool_choice control across protocols** — All four protocol adapters (OpenAI, Anthropic, Gemini, Codex) hardcode tool calling to "auto" mode, with no way to force tool use, specify a tool, or disable tools per-request

## Reference

Inspired by OpenClaw's model routing patterns:
- Multi-level model resolution (Session > Channel > Agent > Default)
- Model fallback chains with observation logging
- `ProviderCapabilities` per-provider capability declaration
- `tool_choice` normalization across protocols (`normalizeOpenAiStringModeAnthropicToolChoice`)
- Occurrence-aware tool call ID rewriting

Adapted to Aleph's architecture principles (R8 LLM Sovereignty, R9 Everything is a Tool, P6 Simplicity).

## Design

### Module 1: Dead Code Cleanup (~35,764 lines)

#### Delete entirely

```
src/dispatcher/
├── model_router/          (22,712 lines — rules-based routing, violates R8)
├── engine/                (1,385 lines — AgentEngine, unused)
├── scheduler/             (DAG scheduler, unused)
├── planner/               (LLM task planner, unused)
├── executor/              (2,822 lines — task executors, unused)
├── agent_types/           (TaskGraph etc., unused)
├── monitor/               (progress monitoring, unused)
├── tool_index/            (semantic tool retrieval, unused)
├── callback.rs            (execution callbacks, unused)
├── analyzer.rs            (task analysis, unused)
└── context/               (task context, unused)

src/config/types/
├── agent/model_routing.rs
├── agent/model_profile.rs
├── agent/metrics.rs
├── agent/health.rs
├── agent/prompt_analysis.rs
├── dispatcher/retry.rs
├── dispatcher/budget.rs
├── dispatcher/backoff.rs
└── routing.rs
```

#### Retain (actively used by gateway/agent_loop)

```
src/dispatcher/
├── registry/              (ToolRegistry — used by gateway)
├── types/                 (UnifiedTool, ToolDefinition — used by agent_loop)
├── confirmation.rs        (tool confirmation system)
├── async_confirmation.rs
├── integration/           (DispatcherIntegration)
├── risk/                  (RiskEvaluator)
├── loom_concurrency.rs    (RETAIN — tests registry concurrent patterns, still valid for ToolRegistry)
└── mod.rs                 (cleaned re-exports)
```

**Note**: `loom_concurrency.rs` tests abstract `RwLock<HashMap>` concurrent patterns modeled after `dispatcher/registry/`. These patterns remain valid for the retained ToolRegistry. Keep this file.

#### Cleanup `dispatcher/mod.rs`

Remove all re-exports for deleted modules: `model_router::*`, `engine::*`, `scheduler::*`, `planner::*`, `executor::*`, `agent_types::*`, `monitor::*`, `tool_index::*`, `callback::*`, `analyzer::*`, `context::*`.

Remove `config/types/agent/mod.rs` references to deleted config types.

### Module 2: Lightweight Model Routing (~300 lines)

#### 2.1 MultiProviderRegistry

Replace the trivial `SingleProviderRegistry` / `SwappableProviderRegistry` with a real multi-provider registry.

**File**: `src/thinker/mod.rs`

```rust
/// Internal state protected by a single RwLock for snapshot consistency.
struct RegistryState {
    /// provider_name → provider instance
    providers: HashMap<String, Arc<dyn AiProvider>>,
    /// Current default provider name
    default_name: String,
    /// Fallback chain: ordered provider names to try on transient failure
    fallbacks: Vec<String>,
}

/// Multi-provider registry: routes by provider name, supports hot-swap and fallback.
/// Uses a single RwLock to guarantee consistent snapshots across fields.
pub struct MultiProviderRegistry {
    state: RwLock<RegistryState>,
}

impl ProviderRegistry for MultiProviderRegistry {
    fn get(&self, model_key: &str) -> Option<Arc<dyn AiProvider>> {
        let state = self.state.read().unwrap_or_else(|e| e.into_inner());
        // 1. Try "provider/model" format → extract provider name
        if let Some(provider_name) = model_key.split('/').next() {
            if let Some(p) = state.providers.get(provider_name) {
                return Some(p.clone());
            }
        }
        // 2. Try model name → resolve via preset mapping
        if let Some(provider_name) = resolve_provider_from_model(model_key) {
            if let Some(p) = state.providers.get(&provider_name) {
                return Some(p.clone());
            }
        }
        // 3. Not found — caller should fall back to default_provider()
        None
    }

    fn default_provider(&self) -> Arc<dyn AiProvider> {
        let state = self.state.read().unwrap_or_else(|e| e.into_inner());
        // Graceful fallback: if default was removed, return first available provider
        state.providers.get(&state.default_name)
            .or_else(|| state.providers.values().next())
            .cloned()
            .expect("registry must have at least one provider")
    }

    fn list_providers(&self) -> Vec<String> {
        let state = self.state.read().unwrap_or_else(|e| e.into_inner());
        state.providers.keys().cloned().collect()
    }
}
```

**Design decision**: Single `RwLock<RegistryState>` instead of three separate locks. This prevents the TOCTOU race where `set_default("X")` + `remove("old")` between two reads could panic. Trade-off: slightly coarser locking, but provider registry operations are infrequent (config changes, not per-request).

**Mutation methods** (for runtime management):

```rust
impl MultiProviderRegistry {
    pub fn new(default_name: String, default_provider: Arc<dyn AiProvider>) -> Self;
    pub fn register(&self, name: String, provider: Arc<dyn AiProvider>);
    pub fn remove(&self, name: &str) -> Result<Option<Arc<dyn AiProvider>>>;
    // remove() returns Err if trying to remove the last provider
    pub fn set_default(&self, name: &str) -> Result<()>;
    pub fn set_fallbacks(&self, chain: Vec<String>);
}
```

**Migration from SwappableProviderRegistry**: `SwappableProviderRegistry.swap(new_provider)` is equivalent to `MultiProviderRegistry.register(name, new_provider) + set_default(name)`. Both `SingleProviderRegistry` and `SwappableProviderRegistry` remain available for simple cases (e.g., tests, single-provider setups). `MultiProviderRegistry` becomes the default in server initialization (`server_init.rs`).

#### 2.2 Fallback on Transient Errors

**File**: `src/thinker/fallback.rs` (new, ~100 lines)

```rust
/// Try primary provider, fall back on transient errors.
/// Called from ExecutionEngine's run_loop when provider_registry has fallbacks configured.
pub async fn call_with_fallback(
    registry: &dyn ProviderRegistry,
    primary: &str,
    fallbacks: &[String],
    payload: RequestPayload<'_>,
) -> Result<ProviderResponse> {
    // Try primary
    match try_provider(registry, primary, &payload).await {
        Ok(resp) => return Ok(resp),
        Err(e) if e.is_transient() => {
            warn!(provider = primary, error = %e, "Primary provider failed, trying fallbacks");
        }
        Err(e) => return Err(e), // Permanent error, don't retry
    }
    // Try fallbacks in order
    for fallback_name in fallbacks {
        match try_provider(registry, fallback_name, &payload).await {
            Ok(resp) => {
                info!(provider = fallback_name, "Fallback provider succeeded");
                return Ok(resp);
            }
            Err(e) if e.is_transient() => {
                warn!(provider = fallback_name, error = %e, "Fallback also failed");
                continue;
            }
            Err(e) => return Err(e),
        }
    }
    Err(AlephError::provider("All providers failed (primary + fallbacks exhausted)"))
}

fn try_provider(
    registry: &dyn ProviderRegistry,
    name: &str,
    payload: &RequestPayload<'_>,
) -> Result<ProviderResponse> {
    let provider = registry.get(name)
        .ok_or_else(|| AlephError::provider(format!("Provider '{}' not found in registry", name)))?;
    provider.process(payload.clone()).await
}
```

**Integration point**: In `gateway/execution_engine/run_loop.rs`, the current call:
```rust
let provider = self.provider_registry.default_provider();
```
Becomes:
```rust
let provider = if let Some(override_model) = session.model_override() {
    self.provider_registry.get(override_model)
        .unwrap_or_else(|| self.provider_registry.default_provider())
} else {
    self.provider_registry.default_provider()
};
```
The `call_with_fallback` is optionally used when the registry has fallbacks configured.

**Transient error classification**: Add `is_transient()` to `AlephError`:

```rust
// src/error.rs
impl AlephError {
    /// Whether this error is transient (worth retrying with another provider)
    pub fn is_transient(&self) -> bool {
        match self {
            // HTTP status-based
            AlephError::Provider { status: Some(429), .. } => true,  // Rate limit
            AlephError::Provider { status: Some(503), .. } => true,  // Service unavailable
            AlephError::Provider { status: Some(502), .. } => true,  // Bad gateway
            // Network errors
            AlephError::Network(_) => true,
            AlephError::Timeout(_) => true,
            // Everything else is permanent
            _ => false,
        }
    }
}
```

Note: `AlephError::provider()` already exists as a constructor. No new `AllProvidersFailed` variant needed — we use the existing `AlephError::provider(msg)` with a descriptive message.

#### 2.3 Model Key Resolution

**File**: `src/providers/presets.rs` (extend existing)

Add `resolve_provider_from_model(model: &str) -> Option<String>` that maps known model prefixes to provider names. This is mechanical name resolution (not semantic reasoning), staying within R8 bounds:

```rust
/// Resolve provider name from model name using known prefix patterns.
/// Returns None for unknown models — caller falls back to default_provider().
pub fn resolve_provider_from_model(model: &str) -> Option<String> {
    let model_lower = model.to_lowercase();
    // Derived from existing PRESETS data
    if model_lower.starts_with("gpt-") || model_lower.starts_with("o1-")
        || model_lower.starts_with("o3-") || model_lower.starts_with("o4-") {
        Some("openai".into())
    } else if model_lower.starts_with("claude-") {
        Some("anthropic".into())
    } else if model_lower.starts_with("gemini-") {
        Some("google".into())
    } else if model_lower.starts_with("deepseek-") {
        Some("deepseek".into())
    } else {
        None // Unknown prefix — use default provider
    }
}
```

This mapping is derived from the existing `PRESETS` HashMap in `presets.rs`. It can be extended by simply adding more prefix patterns. Unknown models gracefully fall through to `None`.

#### 2.4 Session-Level Model Override

Add `model_override` field to session state and wire it through:

1. **Session state**: Add `model_override: Option<String>` to the session struct
2. **`/model` slash command**: Already exists in `slash_command.rs`, needs to write the override to session state
3. **`run_loop.rs`**: Read session override before selecting provider (see integration point in 2.2)

### Module 3: Tool Use Protocol Adaptation (~200 lines)

#### 3.1 ToolChoice in RequestPayload

**File**: `src/providers/adapter.rs`

```rust
/// Tool selection control
#[derive(Debug, Clone, PartialEq)]
pub enum ToolChoice {
    /// LLM decides whether to use tools (default)
    Auto,
    /// LLM MUST call at least one tool
    Required,
    /// LLM must call this specific tool
    Specific(String),
    /// Disable all tool use for this request
    None,
}

pub struct RequestPayload<'a> {
    pub messages: &'a [UnifiedMessage],
    pub system_prompt: Option<&'a str>,
    pub tools: Option<&'a [ToolDefinition]>,
    pub think_level: Option<ThinkLevel>,
    pub temperature: Option<f32>,
    pub max_tokens: Option<u32>,
    pub tool_choice: Option<ToolChoice>,  // NEW
}
```

#### 3.2 Protocol-Specific ToolChoice Mapping

**OpenAI** (`protocols/openai.rs`):
```rust
// In build_request(), after tools serialization:
if let Some(choice) = &payload.tool_choice {
    let value = match choice {
        ToolChoice::Auto => json!("auto"),
        ToolChoice::Required => json!("required"),
        ToolChoice::Specific(name) => json!({"type": "function", "function": {"name": name}}),
        ToolChoice::None => json!("none"),
    };
    body["tool_choice"] = value;
}
```

**Anthropic** (`protocols/anthropic.rs`):
```rust
// In build_request():
if let Some(choice) = &payload.tool_choice {
    match choice {
        ToolChoice::Auto => { body["tool_choice"] = json!({"type": "auto"}); }
        ToolChoice::Required => { body["tool_choice"] = json!({"type": "any"}); }
        ToolChoice::Specific(name) => { body["tool_choice"] = json!({"type": "tool", "name": name}); }
        ToolChoice::None => {
            // Anthropic: don't send tools array at all to disable tool use
            body.as_object_mut().map(|o| o.remove("tools"));
        }
    }
}
```

Note: `ToolChoice::None` on Anthropic requires removing the `tools` key entirely (not just setting a field). This is handled in `build_request()`, not in the serialization helper.

**Gemini** (`protocols/gemini.rs`):
```rust
// In build_request(), as `tool_config` field:
if let Some(choice) = &payload.tool_choice {
    let config = match choice {
        ToolChoice::Auto => json!({"function_calling_config": {"mode": "AUTO"}}),
        ToolChoice::Required => json!({"function_calling_config": {"mode": "ANY"}}),
        ToolChoice::Specific(name) => json!({"function_calling_config": {
            "mode": "ANY", "allowed_function_names": [name]
        }}),
        ToolChoice::None => json!({"function_calling_config": {"mode": "NONE"}}),
    };
    body["tool_config"] = config;
}
```

**Codex** (`protocols/codex.rs`):
```rust
// Codex already has tool_choice: Some("auto".to_string()) at line 223.
// Update to use the unified ToolChoice enum:
if let Some(choice) = &payload.tool_choice {
    let value = match choice {
        ToolChoice::Auto => "auto",
        ToolChoice::Required => "required",
        ToolChoice::None => "none",
        ToolChoice::Specific(_) => "auto", // Codex doesn't support specific tool forcing
    };
    body["tool_choice"] = json!(value);
}
```

#### 3.3 Gemini Tool Call ID Improvement

**File**: `src/providers/protocols/gemini.rs`

Replace index-based synthetic IDs with content-hash-based deterministic IDs:

```rust
/// Generate deterministic tool call IDs for Gemini (which doesn't return IDs).
/// Uses content hashing to produce unique, reproducible IDs.
/// Note: DefaultHasher output may differ across Rust versions, but IDs are
/// ephemeral (only need uniqueness within a single conversation turn).
fn generate_tool_call_id(name: &str, args: &Value, turn_idx: usize, call_idx: usize) -> String {
    use std::hash::{Hash, Hasher};
    use std::collections::hash_map::DefaultHasher;
    let mut hasher = DefaultHasher::new();
    name.hash(&mut hasher);
    args.to_string().hash(&mut hasher);
    turn_idx.hash(&mut hasher);
    call_idx.hash(&mut hasher);
    format!("gemini-{:016x}", hasher.finish())
}
```

#### 3.4 OpenAI Argument Parse Error Handling

**File**: `src/providers/protocols/openai.rs`

Replace silent empty-object fallback with error-preserving fallback:

```rust
let arguments: Value = serde_json::from_str(&tc.function.arguments)
    .unwrap_or_else(|e| {
        warn!(tool = %tc.function.name, error = %e,
              "Failed to parse tool call arguments");
        json!({
            "_raw_arguments": tc.function.arguments,
            "_parse_error": e.to_string()
        })
    });
```

#### 3.5 Protocol Capabilities Declaration

**File**: `src/providers/adapter.rs` (extend ProtocolAdapter)

```rust
pub trait ProtocolAdapter: Send + Sync {
    // ... existing methods ...

    /// Protocol capability declarations (with sensible defaults)
    fn supports_native_tools(&self) -> bool;
    fn supports_parallel_tools(&self) -> bool { true }
    fn returns_tool_call_ids(&self) -> bool { true }
    fn supports_tool_choice(&self) -> bool { true }
    fn supports_strict_schema(&self) -> bool { false }
}
```

Implementation per protocol:

| Method | OpenAI | Anthropic | Gemini | Codex |
|--------|--------|-----------|--------|-------|
| `supports_native_tools` | true | true | true | true |
| `supports_parallel_tools` | true | true | true | true |
| `returns_tool_call_ids` | true | true | **false** | true |
| `supports_tool_choice` | true | true | true | true |
| `supports_strict_schema` | **true** | false | false | false |

## Migration Notes

- `SingleProviderRegistry` and `SwappableProviderRegistry` remain for simple cases (tests, single-provider). `MultiProviderRegistry` becomes the default in `server_init.rs`
- `SwappableProviderRegistry.swap(p)` → equivalent to `MultiProviderRegistry.register(name, p) + set_default(name)`
- Existing tests for tool confirmation, tool registry remain unchanged
- No breaking changes to the gateway/agent_loop execution path — this is purely additive + cleanup
- Config files: `model_routing`, `model_profiles` TOML sections become no-ops (silently ignored if present in user configs)
- `AlephError` gets a new `is_transient()` method classifying HTTP 429/502/503, timeouts, and network errors as retryable

## Files Changed Summary

| Action | Files | Lines |
|--------|-------|-------|
| Delete | ~70 files in dispatcher/ + config/ | ~35,764 |
| New | `thinker/fallback.rs` | ~100 |
| Modify | `thinker/mod.rs` | ~150 (MultiProviderRegistry) |
| Modify | `providers/adapter.rs` | ~30 (ToolChoice + capabilities) |
| Modify | `providers/protocols/openai.rs` | ~25 (tool_choice + parse fix) |
| Modify | `providers/protocols/anthropic.rs` | ~25 (tool_choice, None removes tools) |
| Modify | `providers/protocols/gemini.rs` | ~30 (tool_choice + ID fix) |
| Modify | `providers/protocols/codex.rs` | ~15 (unified tool_choice) |
| Modify | `dispatcher/mod.rs` | ~50 (cleanup re-exports) |
| Modify | `config/types/agent/mod.rs` | ~20 (remove dead config refs) |
| Modify | `config/types/dispatcher/mod.rs` | ~10 (remove dead config refs) |
| Modify | `error.rs` | ~15 (is_transient method) |
| Modify | `gateway/execution_engine/run_loop.rs` | ~10 (session model override) |
| **Net** | | **~-35,250 lines** |
