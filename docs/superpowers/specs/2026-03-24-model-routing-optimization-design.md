# Model Routing Optimization: Dead Code Cleanup + Lightweight Routing + Tool Use Adaptation

**Date**: 2026-03-24
**Status**: Approved
**Scope**: core/src/dispatcher/, core/src/thinker/, core/src/providers/

## Problem Statement

Aleph's model routing system has three critical issues:

1. **35,764 lines of dead code** in `dispatcher/` — `model_router/`, `engine/`, `scheduler/`, `planner/`, `executor/`, `agent_types/`, `monitor/`, `tool_index/`, `analyzer.rs`, `callback.rs`, `context/` are never called by the actual execution path (`gateway/` → `agent_loop/`)
2. **ProviderRegistry ignores model parameter** — `SingleProviderRegistry` and `SwappableProviderRegistry` both return the same provider regardless of model key, making multi-provider routing impossible
3. **No tool_choice control across protocols** — All three protocol adapters (OpenAI, Anthropic, Gemini) hardcode tool calling to "auto" mode, with no way to force tool use, specify a tool, or disable tools per-request

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
core/src/dispatcher/
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
├── context/               (task context, unused)
└── loom_concurrency.rs    (loom tests for dead code)

core/src/config/types/
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
core/src/dispatcher/
├── registry/              (ToolRegistry — used by gateway)
├── types/                 (UnifiedTool, ToolDefinition — used by agent_loop)
├── confirmation.rs        (tool confirmation system)
├── async_confirmation.rs
├── integration/           (DispatcherIntegration)
├── risk/                  (RiskEvaluator)
└── mod.rs                 (cleaned re-exports)
```

#### Cleanup `dispatcher/mod.rs`

Remove all re-exports for deleted modules: `model_router::*`, `engine::*`, `scheduler::*`, `planner::*`, `executor::*`, `agent_types::*`, `monitor::*`, `tool_index::*`, `callback::*`, `analyzer::*`, `context::*`.

Remove `config/types/agent/mod.rs` references to deleted config types.

### Module 2: Lightweight Model Routing (~300 lines)

#### 2.1 MultiProviderRegistry

Replace the trivial `SingleProviderRegistry` / `SwappableProviderRegistry` with a real multi-provider registry.

**File**: `core/src/thinker/mod.rs`

```rust
/// Multi-provider registry: routes by provider name, supports hot-swap and fallback
pub struct MultiProviderRegistry {
    /// provider_name → provider instance
    providers: RwLock<HashMap<String, Arc<dyn AiProvider>>>,
    /// Current default provider name
    default_name: RwLock<String>,
    /// Fallback chain: ordered provider names to try on transient failure
    fallbacks: RwLock<Vec<String>>,
}

impl ProviderRegistry for MultiProviderRegistry {
    fn get(&self, model_key: &str) -> Option<Arc<dyn AiProvider>> {
        let providers = self.providers.read().unwrap_or_else(|e| e.into_inner());
        // 1. Try "provider/model" format → extract provider name
        if let Some(provider_name) = model_key.split('/').next() {
            if let Some(p) = providers.get(provider_name) {
                return Some(p.clone());
            }
        }
        // 2. Try model name → resolve via preset mapping
        if let Some(provider_name) = resolve_provider_from_model(model_key) {
            if let Some(p) = providers.get(&provider_name) {
                return Some(p.clone());
            }
        }
        // 3. Fallback to default
        None
    }

    fn default_provider(&self) -> Arc<dyn AiProvider> {
        let name = self.default_name.read().unwrap_or_else(|e| e.into_inner());
        let providers = self.providers.read().unwrap_or_else(|e| e.into_inner());
        providers.get(name.as_str())
            .cloned()
            .expect("default provider must exist")
    }
}
```

**Mutation methods** (for runtime management):

```rust
impl MultiProviderRegistry {
    pub fn register(&self, name: String, provider: Arc<dyn AiProvider>);
    pub fn remove(&self, name: &str) -> Option<Arc<dyn AiProvider>>;
    pub fn set_default(&self, name: &str) -> Result<()>;
    pub fn set_fallbacks(&self, chain: Vec<String>);
    pub fn list_providers(&self) -> Vec<String>;
}
```

#### 2.2 Fallback on Transient Errors

**File**: `core/src/thinker/fallback.rs` (new, ~80 lines)

```rust
/// Try primary provider, fall back on transient errors
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
    Err(AlephError::AllProvidersFailed)
}
```

**Transient vs permanent classification**:
- Transient: HTTP 429 (rate limit), 503 (service unavailable), timeout, connection refused
- Permanent: HTTP 400 (bad request), 401 (auth), 403 (forbidden), 404

#### 2.3 Model Key Resolution

**File**: `core/src/providers/presets.rs` (extend existing)

Add `resolve_provider_from_model(model: &str) -> Option<String>` that maps known model prefixes to provider names:
- `gpt-*`, `o1-*`, `o3-*`, `o4-*` → `"openai"`
- `claude-*` → `"anthropic"`
- `gemini-*` → `"google"`
- `deepseek-*` → `"deepseek"`
- etc. (from existing preset data)

#### 2.4 Session-Level Model Override

The execution path already passes model info through `AgentInstance`. Add model override to session state:

```rust
// In ExecutionEngine run_loop: check session model override before calling provider
let provider = if let Some(override_model) = session.model_override() {
    registry.get(override_model).unwrap_or_else(|| registry.default_provider())
} else {
    registry.default_provider()
};
```

The `/model` slash command (already exists) writes the override to session state.

### Module 3: Tool Use Protocol Adaptation (~200 lines)

#### 3.1 ToolChoice in RequestPayload

**File**: `core/src/providers/adapter.rs`

```rust
/// Tool selection control
#[derive(Debug, Clone, PartialEq)]
pub enum ToolChoice {
    Auto,
    Required,
    Specific(String),
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
match tool_choice {
    ToolChoice::Auto => json!("auto"),
    ToolChoice::Required => json!("required"),
    ToolChoice::Specific(name) => json!({"type": "function", "function": {"name": name}}),
    ToolChoice::None => json!("none"),
}
```

**Anthropic** (`protocols/anthropic.rs`):
```rust
match tool_choice {
    ToolChoice::Auto => json!({"type": "auto"}),
    ToolChoice::Required => json!({"type": "any"}),
    ToolChoice::Specific(name) => json!({"type": "tool", "name": name}),
    ToolChoice::None => /* don't send tools array */,
}
```

**Gemini** (`protocols/gemini.rs`):
```rust
match tool_choice {
    ToolChoice::Auto => json!({"function_calling_config": {"mode": "AUTO"}}),
    ToolChoice::Required => json!({"function_calling_config": {"mode": "ANY"}}),
    ToolChoice::Specific(name) => json!({"function_calling_config": {
        "mode": "ANY", "allowed_function_names": [name]
    }}),
    ToolChoice::None => json!({"function_calling_config": {"mode": "NONE"}}),
}
```

#### 3.3 Gemini Tool Call ID Improvement

**File**: `core/src/providers/protocols/gemini.rs`

Replace index-based synthetic IDs with content-hash-based deterministic IDs:

```rust
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

**File**: `core/src/providers/protocols/openai.rs`

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

**File**: `core/src/providers/adapter.rs` (extend ProtocolAdapter)

```rust
pub trait ProtocolAdapter: Send + Sync {
    // ... existing methods ...

    /// Protocol capability declarations
    fn supports_native_tools(&self) -> bool;
    fn supports_parallel_tools(&self) -> bool { true }
    fn returns_tool_call_ids(&self) -> bool { true }
    fn supports_tool_choice(&self) -> bool { true }
    fn supports_strict_schema(&self) -> bool { false }
}
```

Implementation per protocol:

| Method | OpenAI | Anthropic | Gemini |
|--------|--------|-----------|--------|
| `supports_native_tools` | true | true | true |
| `supports_parallel_tools` | true | true | true |
| `returns_tool_call_ids` | true | true | **false** |
| `supports_tool_choice` | true | true | true |
| `supports_strict_schema` | **true** | false | false |

## Migration Notes

- `SingleProviderRegistry` and `SwappableProviderRegistry` remain as convenience wrappers, but `MultiProviderRegistry` becomes the default in server initialization
- Existing tests for tool confirmation, tool registry remain unchanged
- No breaking changes to the gateway/agent_loop execution path — this is purely additive + cleanup
- Config files: `model_routing`, `model_profiles` TOML sections become no-ops (silently ignored if present in user configs)

## Files Changed Summary

| Action | Files | Lines |
|--------|-------|-------|
| Delete | ~70 files in dispatcher/ + config/ | ~35,764 |
| New | `thinker/fallback.rs` | ~80 |
| Modify | `thinker/mod.rs` | ~150 (MultiProviderRegistry) |
| Modify | `providers/adapter.rs` | ~30 (ToolChoice + capabilities) |
| Modify | `providers/protocols/openai.rs` | ~25 (tool_choice + parse fix) |
| Modify | `providers/protocols/anthropic.rs` | ~20 (tool_choice) |
| Modify | `providers/protocols/gemini.rs` | ~30 (tool_choice + ID fix) |
| Modify | `dispatcher/mod.rs` | ~50 (cleanup re-exports) |
| Modify | `config/types/agent/mod.rs` | ~20 (remove dead config refs) |
| Modify | `config/types/dispatcher/mod.rs` | ~10 (remove dead config refs) |
| **Net** | | **~-35,250 lines** |
