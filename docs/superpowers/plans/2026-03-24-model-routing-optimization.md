# Model Routing Optimization Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Delete ~30K lines of dead code, build lightweight multi-provider routing with fallback, and add tool_choice support across all four protocol adapters.

**Architecture:** Three independent modules executed sequentially: (1) dead code cleanup, (2) MultiProviderRegistry with fallback, (3) ToolChoice + protocol capabilities. Each module is independently compilable and testable.

**Tech Stack:** Rust, thiserror, serde_json, tracing, std::sync::RwLock

**Spec:** `docs/superpowers/specs/2026-03-24-model-routing-optimization-design.md`

**Deferred:** Session-level model override (`model_override` on session state + `/model` slash command wiring) — planned as follow-up.

---

## Task 1: Delete Dead Dispatcher Modules

**Files:**
- Delete: `src/dispatcher/model_router/` (entire directory, ~22,712 lines)
- Delete: `src/dispatcher/engine/` (entire directory, ~1,385 lines)
- Delete: `src/dispatcher/scheduler/` (entire directory)
- Delete: `src/dispatcher/planner/` (entire directory)
- Delete: `src/dispatcher/executor/` (entire directory, ~2,822 lines)
- Delete: `src/dispatcher/agent_types/` (entire directory)
- Delete: `src/dispatcher/monitor/` (entire directory)
- Delete: `src/dispatcher/callback.rs`
- Delete: `src/dispatcher/analyzer.rs`
- Delete: `src/dispatcher/context.rs` (file, not directory)
- Retain: `src/dispatcher/tool_index/` (actively used by thinker/prompt_builder)
- Modify: `src/dispatcher/mod.rs`

**Note:** `tool_index/` is RETAINED — it's actively used by `thinker/prompt_layer.rs`, `thinker/prompt_builder/`, `thinker/layers/tools.rs` for `HydrationResult` and `HydratedToolsLayer`.

- [ ] **Step 1: Delete all dead directories and files**

```bash
cd /Users/zouguojun/Workspace/Aleph
rm -rf src/dispatcher/model_router
rm -rf src/dispatcher/engine
rm -rf src/dispatcher/scheduler
rm -rf src/dispatcher/planner
rm -rf src/dispatcher/executor
rm -rf src/dispatcher/agent_types
rm -rf src/dispatcher/monitor
rm -f src/dispatcher/callback.rs
rm -f src/dispatcher/analyzer.rs
rm -f src/dispatcher/context.rs
```

- [ ] **Step 2: Rewrite `src/dispatcher/mod.rs`**

Replace the entire file. Keep tool_index and risk, remove everything else:

```rust
//! Dispatcher Layer - Tool Management
//!
//! This module manages tool registration, discovery, and confirmation:
//!
//! - **Tool Registry**: Aggregates all tool sources (Native, MCP, Skills, Custom)
//! - **Confirmation System**: User confirmation for tool execution
//! - **Risk Evaluation**: Tool risk assessment
//! - **Tool Index**: Semantic tool retrieval and hydration

// === Tool Management ===
mod async_confirmation;
mod confirmation;
mod integration;
mod registry;
mod types;

// === Risk Evaluation ===
pub mod risk;

// === Tool Index: Semantic tool retrieval ===
pub mod tool_index;

// === Re-exports: Tool Management ===
pub use async_confirmation::{
    AsyncConfirmationConfig, AsyncConfirmationHandler, ConfirmationState, PendingConfirmation,
    PendingConfirmationInfo, PendingConfirmationStore, UserConfirmationDecision,
};
pub use confirmation::{
    ConfirmationAction, ConfirmationConfig, ConfirmationDecision, ToolConfirmation, OPTION_CANCEL,
    OPTION_EDIT, OPTION_EXECUTE,
};
pub use integration::{
    ConfidenceAction, ConfidenceThresholds, DispatcherAction, DispatcherConfig,
    DispatcherIntegration, DispatcherResult,
};
pub use registry::ToolRegistry;
pub use registry::ResolvedCommand;
pub use types::{
    ChannelType, ConflictInfo, ConflictResolution, DispatchMode, RoutingLayer, StructuredToolMeta,
    ToolCategory, ToolDefinition, ToolDiff, ToolIndex, ToolIndexCategory, ToolIndexEntry,
    ToolPriority, ToolResult, ToolSafetyLevel, ToolSource, ToolSourceType, UnifiedTool,
    UnifiedToolInfo,
};

// === Re-exports: Risk Evaluation ===
pub use risk::{RiskEvaluator, RiskLevel};

// === Re-exports: Tool Index (Semantic Retrieval) ===
pub use tool_index::{
    HydrationLevel, HydrationPipeline, HydrationPipelineConfig, HydrationResult,
    HydratedTool, InferredPurpose, SemanticPurposeInferrer, ToolIndexCoordinator,
    ToolMeta, ToolRetrieval, ToolRetrievalConfig,
};

#[cfg(all(test, feature = "loom"))]
mod loom_concurrency;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tool_source_display() {
        assert_eq!(format!("{:?}", ToolSource::Native), "Native");
        assert_eq!(
            format!(
                "{:?}",
                ToolSource::Mcp {
                    server: "github".into()
                }
            ),
            "Mcp { server: \"github\" }"
        );
    }
}
```

- [ ] **Step 3: Compile check — find all broken references**

Run: `cargo check -p alephcore 2>&1 | head -80`

This will show broken imports. Expected failures in: `config/types/agent/mod.rs`, `config/types/dispatcher/mod.rs`, `config/types/routing.rs`, `lib.rs`.

- [ ] **Step 4: Fix `config/types/agent/mod.rs`**

Remove dead sub-module declarations and imports:
- Delete: `mod ab_testing; mod ensemble; mod health; mod metrics; mod model_profile; mod model_routing; mod prompt_analysis; mod semantic_cache;`
- Delete: `pub use model_profile::ModelProfileConfigToml; pub use model_routing::ModelRoutingConfigToml;`
- Delete: `use crate::dispatcher::model_router::{ModelProfile, ModelRoutingRules};`
- Delete: `use crate::dispatcher::{DEFAULT_SANDBOX_ENABLED, MAX_PARALLELISM, MAX_TASK_RETRIES, REQUIRE_CONFIRMATION};`
- Delete files: `src/config/types/agent/ab_testing.rs`, `ensemble.rs`, `health.rs`, `metrics.rs`, `model_profile.rs`, `model_routing.rs`, `prompt_analysis.rs`, `semantic_cache.rs`

- [ ] **Step 5: Fix `config/types/dispatcher/mod.rs`**

Remove dead sub-modules:
- Remove: `mod backoff; mod budget; mod retry;`
- Remove: `pub use budget::*; pub use retry::*;`
- Keep: `mod core; pub use core::*;`
- Delete files: `src/config/types/dispatcher/backoff.rs`, `budget.rs`, `retry.rs`
- Remove tests that reference deleted types

- [ ] **Step 6: Fix `config/types/routing.rs`**

Remove `TaskIntent` dependency. This file is used by `gateway/handlers/routing_rules.rs` for `RoutingRuleConfig`, but `get_task_intent()` is never called by gateway.
- Remove: `use crate::dispatcher::model_router::TaskIntent;` (line 15)
- Remove: the `get_task_intent()` method and its tests (lines 265-421)
- Keep: everything else (`RoutingRuleConfig`, `get_intent_type()`, `get_model()`, etc.)

- [ ] **Step 7: Fix `lib.rs` re-exports**

Remove re-exports of deleted types from `src/lib.rs`. Search for lines re-exporting types from `dispatcher::engine`, `dispatcher::model_router`, `dispatcher::agent_types`, `dispatcher::callback`, `dispatcher::executor`, `dispatcher::planner`, `dispatcher::scheduler`, `dispatcher::monitor`, `dispatcher::analyzer`, `dispatcher::context`. Remove those lines.

Keep the `tool_index` re-exports (they're still valid).

- [ ] **Step 8: Fix any remaining compilation errors**

Run: `cargo check -p alephcore 2>&1 | head -80`

Iteratively fix any remaining broken imports until clean. Common pattern: remove `use crate::dispatcher::{...deleted_type...}`.

- [ ] **Step 9: Run tests**

Run: `cargo test -p alephcore --lib 2>&1 | tail -20`
Expected: All tests pass (minus pre-existing `markdown_skill::loader` failures)

- [ ] **Step 10: Commit**

```bash
git add -A src/dispatcher/ src/config/types/ src/lib.rs
git commit -m "refactor: delete ~30K lines of dead dispatcher code

Remove model_router/, engine/, scheduler/, planner/, executor/,
agent_types/, monitor/, callback, analyzer, context and their
config types. None were referenced by gateway or agent_loop.

Retain: registry/, types/, confirmation, integration, risk,
tool_index (used by prompt builder), loom tests."
```

---

## Task 2: Add `is_transient()` to AlephError

**Files:**
- Modify: `src/error.rs`

- [ ] **Step 1: Add `is_transient` method**

In the `impl AlephError` block (after line 196), add:

```rust
    /// Whether this error is transient (worth retrying with another provider).
    /// Transient: rate limits, server errors, timeouts, network failures.
    /// Permanent: auth errors, bad requests, not found, config errors.
    pub fn is_transient(&self) -> bool {
        matches!(
            self,
            AlephError::RateLimitError { .. }
                | AlephError::Timeout { .. }
                | AlephError::NetworkError { .. }
                | AlephError::ExecutionTimeout { .. }
        )
    }
```

- [ ] **Step 2: Add tests**

```rust
#[cfg(test)]
mod transient_tests {
    use super::*;

    #[test]
    fn test_transient_errors() {
        assert!(AlephError::RateLimitError {
            message: "429".into(), suggestion: None,
        }.is_transient());
        assert!(AlephError::Timeout { suggestion: None }.is_transient());
        assert!(AlephError::NetworkError {
            message: "connection refused".into(), suggestion: None,
        }.is_transient());
    }

    #[test]
    fn test_permanent_errors() {
        assert!(!AlephError::AuthenticationError {
            message: "invalid key".into(), provider: "openai".into(), suggestion: None,
        }.is_transient());
        assert!(!AlephError::ProviderError {
            message: "bad request".into(), suggestion: None,
        }.is_transient());
    }
}
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p alephcore --lib transient_tests`
Expected: 2 tests pass

- [ ] **Step 4: Commit**

```bash
git add src/error.rs
git commit -m "feat: add is_transient() to AlephError for provider fallback"
```

---

## Task 3: Add ToolChoice to RequestPayload

**Files:**
- Modify: `src/providers/adapter.rs`

- [ ] **Step 1: Add ToolChoice enum**

After line 14 (`use super::message::UnifiedMessage;`), add:

```rust
/// Tool selection control for protocol adapters.
#[derive(Debug, Clone, PartialEq)]
pub enum ToolChoice {
    /// LLM decides whether to use tools (default)
    Auto,
    /// LLM MUST call at least one tool
    Required,
    /// LLM must call this specific tool by name
    Specific(String),
    /// Disable all tool use for this request
    None,
}
```

- [ ] **Step 2: Add field to RequestPayload**

After `pub max_tokens: Option<u32>,` (line 33), add:

```rust
    /// Tool selection control (auto/required/specific/none)
    pub tool_choice: Option<ToolChoice>,
```

Update Default impl — after `max_tokens: None,` (line 45), add:

```rust
            tool_choice: None,
```

Add builder method — after `with_max_tokens` (line 87), add:

```rust
    /// Set tool choice
    pub fn with_tool_choice(mut self, choice: Option<ToolChoice>) -> Self {
        self.tool_choice = choice;
        self
    }
```

- [ ] **Step 3: Add protocol capability methods**

After `supports_native_tools` (line 126-128), add:

```rust
    /// Whether this protocol supports parallel tool calls in one response
    fn supports_parallel_tools(&self) -> bool { true }
    /// Whether this protocol returns tool call IDs (false for Gemini)
    fn returns_tool_call_ids(&self) -> bool { true }
    /// Whether this protocol supports tool_choice control
    fn supports_tool_choice(&self) -> bool { true }
    /// Whether this protocol supports strict JSON schema mode
    fn supports_strict_schema(&self) -> bool { false }
```

- [ ] **Step 4: Add tests**

Add to existing test module:

```rust
    #[test]
    fn test_tool_choice_enum() {
        assert_eq!(ToolChoice::Auto, ToolChoice::Auto);
        assert_ne!(ToolChoice::Auto, ToolChoice::Required);
        assert_eq!(ToolChoice::Specific("s".into()), ToolChoice::Specific("s".into()));
    }

    #[test]
    fn test_payload_with_tool_choice() {
        let msgs = [UnifiedMessage::user("test")];
        let payload = RequestPayload::new(&msgs)
            .with_tool_choice(Some(ToolChoice::Required));
        assert_eq!(payload.tool_choice, Some(ToolChoice::Required));
    }
```

- [ ] **Step 5: Compile and test**

Run: `cargo check -p alephcore && cargo test -p alephcore --lib test_tool_choice`
Expected: Clean compile, 2 tests pass

- [ ] **Step 6: Commit**

```bash
git add src/providers/adapter.rs
git commit -m "feat: add ToolChoice enum and protocol capabilities to adapter"
```

---

## Task 4: Wire ToolChoice into OpenAI Protocol

**Files:**
- Modify: `src/providers/protocols/openai.rs`

- [ ] **Step 1: Add tool_choice serialization**

After tools are serialized into request body (around line 334), add:

```rust
if let Some(ref choice) = payload.tool_choice {
    use crate::providers::adapter::ToolChoice;
    body["tool_choice"] = match choice {
        ToolChoice::Auto => json!("auto"),
        ToolChoice::Required => json!("required"),
        ToolChoice::Specific(name) => json!({"type": "function", "function": {"name": name}}),
        ToolChoice::None => json!("none"),
    };
}
```

- [ ] **Step 2: Fix argument parse error handling**

At line ~393 (the `serde_json::from_str` for tool arguments), replace the silent fallback:

```rust
let arguments: serde_json::Value =
    serde_json::from_str(&tc.function.arguments).unwrap_or_else(|e| {
        tracing::warn!(
            tool = %tc.function.name, error = %e,
            "Failed to parse tool call arguments, preserving raw"
        );
        serde_json::json!({
            "_raw_arguments": tc.function.arguments,
            "_parse_error": e.to_string()
        })
    });
```

- [ ] **Step 3: Add `supports_strict_schema` override**

In `impl ProtocolAdapter for OpenAiProtocol`, add:

```rust
    fn supports_strict_schema(&self) -> bool { true }
```

- [ ] **Step 4: Compile check**

Run: `cargo check -p alephcore`

- [ ] **Step 5: Commit**

```bash
git add src/providers/protocols/openai.rs
git commit -m "feat: add tool_choice support and fix argument parsing in OpenAI protocol"
```

---

## Task 5: Wire ToolChoice into Anthropic Protocol

**Files:**
- Modify: `src/providers/protocols/anthropic.rs`

- [ ] **Step 1: Add tool_choice serialization**

After tools are added to request body (around line 306-307), add:

```rust
if let Some(ref choice) = payload.tool_choice {
    use crate::providers::adapter::ToolChoice;
    match choice {
        ToolChoice::Auto => { body["tool_choice"] = json!({"type": "auto"}); }
        ToolChoice::Required => { body["tool_choice"] = json!({"type": "any"}); }
        ToolChoice::Specific(name) => {
            body["tool_choice"] = json!({"type": "tool", "name": name});
        }
        ToolChoice::None => {
            // Anthropic: remove tools array entirely to disable tool use
            if let Some(obj) = body.as_object_mut() {
                obj.remove("tools");
            }
        }
    }
}
```

- [ ] **Step 2: Compile check**

Run: `cargo check -p alephcore`

- [ ] **Step 3: Commit**

```bash
git add src/providers/protocols/anthropic.rs
git commit -m "feat: add tool_choice support in Anthropic protocol"
```

---

## Task 6: Wire ToolChoice into Gemini Protocol + Fix Tool Call IDs

**Files:**
- Modify: `src/providers/protocols/gemini.rs`

- [ ] **Step 1: Add tool_choice serialization**

After tools are added to request body (around line 254), add:

```rust
if let Some(ref choice) = payload.tool_choice {
    use crate::providers::adapter::ToolChoice;
    body["tool_config"] = match choice {
        ToolChoice::Auto => json!({"function_calling_config": {"mode": "AUTO"}}),
        ToolChoice::Required => json!({"function_calling_config": {"mode": "ANY"}}),
        ToolChoice::Specific(name) => json!({"function_calling_config": {
            "mode": "ANY", "allowed_function_names": [name]
        }}),
        ToolChoice::None => json!({"function_calling_config": {"mode": "NONE"}}),
    };
}
```

- [ ] **Step 2: Fix BOTH synthetic ID locations**

There are TWO locations generating synthetic IDs (lines 343 AND 702). Replace BOTH occurrences of:
```rust
id: format!("gemini-fc-{}", index),
```
with:
```rust
id: {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    fc.name.hash(&mut hasher);
    fc.args.to_string().hash(&mut hasher);
    index.hash(&mut hasher);
    format!("gemini-{:016x}", hasher.finish())
},
```

- [ ] **Step 3: Fix the test assertion**

At line 766, update the test to match the new ID prefix:
```rust
// OLD: assert!(result.tool_calls[0].id.starts_with("gemini-fc-"));
// NEW:
assert!(result.tool_calls[0].id.starts_with("gemini-"));
```

- [ ] **Step 4: Add `returns_tool_call_ids` override**

In `impl ProtocolAdapter for GeminiProtocol`, add:

```rust
    fn returns_tool_call_ids(&self) -> bool { false }
```

- [ ] **Step 5: Compile and run tests**

Run: `cargo check -p alephcore && cargo test -p alephcore --lib gemini`

- [ ] **Step 6: Commit**

```bash
git add src/providers/protocols/gemini.rs
git commit -m "feat: add tool_choice support and hash-based IDs in Gemini protocol"
```

---

## Task 7: Wire ToolChoice into Codex Protocol

**Files:**
- Modify: `src/providers/protocols/codex.rs`

- [ ] **Step 1: Replace hardcoded tool_choice**

Find line 223: `tool_choice: Some("auto".to_string()),`

Replace with:

```rust
tool_choice: payload.tool_choice.as_ref().map(|choice| {
    use crate::providers::adapter::ToolChoice;
    match choice {
        ToolChoice::Auto => "auto".to_string(),
        ToolChoice::Required => "required".to_string(),
        ToolChoice::None => "none".to_string(),
        ToolChoice::Specific(_) => "auto".to_string(),
    }
}).or(Some("auto".to_string())),
```

- [ ] **Step 2: Compile check**

Run: `cargo check -p alephcore`

- [ ] **Step 3: Commit**

```bash
git add src/providers/protocols/codex.rs
git commit -m "feat: add tool_choice support in Codex protocol"
```

---

## Task 8: Build MultiProviderRegistry

**Files:**
- Modify: `src/thinker/mod.rs`
- Modify: `src/providers/presets.rs` (add `resolve_provider_from_model`)
- Modify: `src/providers/mod.rs` (re-export the function)

- [ ] **Step 1: Add `list_providers` to ProviderRegistry trait**

In `src/thinker/mod.rs`, add to the `ProviderRegistry` trait (after line 72):

```rust
    /// List all registered provider names
    fn list_providers(&self) -> Vec<String> { vec![] }
```

- [ ] **Step 2: Add `resolve_provider_from_model` to `src/providers/presets.rs`**

At the end of the file (before any `#[cfg(test)]` block), add:

```rust
/// Resolve provider name from model name using known prefix patterns.
/// Returns None for unknown models — caller falls back to default_provider().
pub fn resolve_provider_from_model(model: &str) -> Option<String> {
    let m = model.to_lowercase();
    if m.starts_with("gpt-") || m.starts_with("o1-") || m.starts_with("o3-") || m.starts_with("o4-") {
        Some("openai".into())
    } else if m.starts_with("claude-") {
        Some("anthropic".into())
    } else if m.starts_with("gemini-") {
        Some("google".into())
    } else if m.starts_with("deepseek-") {
        Some("deepseek".into())
    } else {
        None
    }
}
```

- [ ] **Step 3: Re-export from `src/providers/mod.rs`**

Add to the existing re-exports:

```rust
pub use presets::resolve_provider_from_model;
```

- [ ] **Step 4: Add MultiProviderRegistry to `src/thinker/mod.rs`**

After the existing `SwappableProviderRegistry` tests (end of file), add:

```rust
use std::collections::HashMap;

struct RegistryState {
    providers: HashMap<String, Arc<dyn AiProvider>>,
    default_name: String,
    fallbacks: Vec<String>,
}

/// Multi-provider registry: routes by provider name, supports runtime mutation and fallback.
pub struct MultiProviderRegistry {
    state: std::sync::RwLock<RegistryState>,
}

impl MultiProviderRegistry {
    pub fn new(name: String, provider: Arc<dyn AiProvider>) -> Self {
        let mut providers = HashMap::new();
        providers.insert(name.clone(), provider);
        Self {
            state: std::sync::RwLock::new(RegistryState {
                providers,
                default_name: name,
                fallbacks: vec![],
            }),
        }
    }

    pub fn register(&self, name: String, provider: Arc<dyn AiProvider>) {
        let mut state = self.state.write().unwrap_or_else(|e| e.into_inner());
        state.providers.insert(name, provider);
    }

    pub fn remove(&self, name: &str) -> crate::error::Result<Option<Arc<dyn AiProvider>>> {
        let mut state = self.state.write().unwrap_or_else(|e| e.into_inner());
        if state.providers.len() <= 1 && state.providers.contains_key(name) {
            return Err(crate::error::AlephError::provider("Cannot remove the last provider"));
        }
        let removed = state.providers.remove(name);
        if state.default_name == name {
            if let Some(first) = state.providers.keys().next() {
                state.default_name = first.clone();
            }
        }
        Ok(removed)
    }

    pub fn set_default(&self, name: &str) -> crate::error::Result<()> {
        let mut state = self.state.write().unwrap_or_else(|e| e.into_inner());
        if !state.providers.contains_key(name) {
            return Err(crate::error::AlephError::provider(
                format!("Provider '{}' not found in registry", name),
            ));
        }
        state.default_name = name.to_string();
        Ok(())
    }

    pub fn set_fallbacks(&self, chain: Vec<String>) {
        let mut state = self.state.write().unwrap_or_else(|e| e.into_inner());
        state.fallbacks = chain;
    }

    pub fn fallbacks(&self) -> Vec<String> {
        let state = self.state.read().unwrap_or_else(|e| e.into_inner());
        state.fallbacks.clone()
    }
}

impl ProviderRegistry for MultiProviderRegistry {
    fn get(&self, model_key: &str) -> Option<Arc<dyn AiProvider>> {
        let state = self.state.read().unwrap_or_else(|e| e.into_inner());
        if let Some(provider_name) = model_key.split('/').next() {
            if let Some(p) = state.providers.get(provider_name) {
                return Some(p.clone());
            }
        }
        if let Some(provider_name) = crate::providers::resolve_provider_from_model(model_key) {
            if let Some(p) = state.providers.get(&provider_name) {
                return Some(p.clone());
            }
        }
        None
    }

    fn default_provider(&self) -> Arc<dyn AiProvider> {
        let state = self.state.read().unwrap_or_else(|e| e.into_inner());
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

- [ ] **Step 5: Add tests**

```rust
#[cfg(test)]
mod multi_registry_tests {
    use super::*;

    struct NamedProvider { tag: String }
    impl AiProvider for NamedProvider {
        fn process(
            &self, _: crate::providers::adapter::RequestPayload<'_>,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = crate::error::Result<crate::providers::adapter::ProviderResponse>> + Send + '_>> {
            Box::pin(async { Ok(crate::providers::adapter::ProviderResponse::text_only(String::new())) })
        }
        fn name(&self) -> &str { &self.tag }
        fn color(&self) -> &str { "#000" }
    }

    fn p(name: &str) -> Arc<dyn AiProvider> { Arc::new(NamedProvider { tag: name.into() }) }

    #[test]
    fn test_default() {
        let r = MultiProviderRegistry::new("openai".into(), p("openai"));
        assert_eq!(r.default_provider().name(), "openai");
    }

    #[test]
    fn test_get_by_slash_prefix() {
        let r = MultiProviderRegistry::new("openai".into(), p("openai"));
        r.register("anthropic".into(), p("anthropic"));
        assert_eq!(r.get("anthropic/claude-opus-4-6").unwrap().name(), "anthropic");
    }

    #[test]
    fn test_get_by_model_prefix() {
        let r = MultiProviderRegistry::new("openai".into(), p("openai"));
        r.register("anthropic".into(), p("anthropic"));
        assert_eq!(r.get("claude-opus-4-6").unwrap().name(), "anthropic");
    }

    #[test]
    fn test_unknown_returns_none() {
        let r = MultiProviderRegistry::new("openai".into(), p("openai"));
        assert!(r.get("unknown-xyz").is_none());
    }

    #[test]
    fn test_set_default() {
        let r = MultiProviderRegistry::new("openai".into(), p("openai"));
        r.register("anthropic".into(), p("anthropic"));
        r.set_default("anthropic").unwrap();
        assert_eq!(r.default_provider().name(), "anthropic");
    }

    #[test]
    fn test_remove() {
        let r = MultiProviderRegistry::new("openai".into(), p("openai"));
        r.register("anthropic".into(), p("anthropic"));
        assert!(r.remove("anthropic").unwrap().is_some());
        assert!(r.get("anthropic/x").is_none());
    }

    #[test]
    fn test_cannot_remove_last() {
        let r = MultiProviderRegistry::new("openai".into(), p("openai"));
        assert!(r.remove("openai").is_err());
    }

    #[test]
    fn test_remove_default_auto_switches() {
        let r = MultiProviderRegistry::new("openai".into(), p("openai"));
        r.register("anthropic".into(), p("anthropic"));
        r.remove("openai").unwrap();
        assert_eq!(r.default_provider().name(), "anthropic");
    }

    #[test]
    fn test_list_providers() {
        let r = MultiProviderRegistry::new("openai".into(), p("openai"));
        r.register("anthropic".into(), p("anthropic"));
        let mut list = r.list_providers();
        list.sort();
        assert_eq!(list, vec!["anthropic", "openai"]);
    }
}
```

- [ ] **Step 6: Compile and run tests**

Run: `cargo test -p alephcore --lib multi_registry_tests`
Expected: 9 tests pass

- [ ] **Step 7: Commit**

```bash
git add src/thinker/mod.rs src/providers/presets.rs src/providers/mod.rs
git commit -m "feat: add MultiProviderRegistry with model-key routing"
```

---

## Task 9: Build Fallback Module

**Files:**
- Create: `src/thinker/fallback.rs`
- Modify: `src/thinker/mod.rs` (add `pub mod fallback;`)

- [ ] **Step 1: Create `src/thinker/fallback.rs`**

```rust
//! Provider fallback: try primary, fall back on transient errors.

use crate::error::{AlephError, Result};
use crate::providers::adapter::{ProviderResponse, RequestPayload};
use crate::sync_primitives::Arc;
use tracing::{info, warn};

use super::ProviderRegistry;

/// Call the primary provider; on transient failure, try fallbacks in order.
/// Returns (response, provider_name_used).
pub async fn call_with_fallback(
    registry: &dyn ProviderRegistry,
    primary_name: &str,
    fallbacks: &[String],
    payload: RequestPayload<'_>,
) -> Result<(ProviderResponse, String)> {
    match try_provider(registry, primary_name, &payload).await {
        Ok(resp) => return Ok((resp, primary_name.to_string())),
        Err(e) if e.is_transient() => {
            warn!(provider = primary_name, error = %e, "Primary provider transient failure");
        }
        Err(e) => return Err(e),
    }

    for name in fallbacks {
        match try_provider(registry, name, &payload).await {
            Ok(resp) => {
                info!(provider = %name, primary = primary_name, "Fallback succeeded");
                return Ok((resp, name.clone()));
            }
            Err(e) if e.is_transient() => {
                warn!(provider = %name, error = %e, "Fallback also failed");
                continue;
            }
            Err(e) => return Err(e),
        }
    }

    Err(AlephError::provider(format!(
        "All providers failed: primary '{}' + {} fallback(s)",
        primary_name, fallbacks.len()
    )))
}

async fn try_provider(
    registry: &dyn ProviderRegistry,
    name: &str,
    payload: &RequestPayload<'_>,
) -> Result<ProviderResponse> {
    let provider = registry.get(name).ok_or_else(|| {
        AlephError::provider(format!("Provider '{}' not found in registry", name))
    })?;
    provider.process(RequestPayload {
        messages: payload.messages,
        system_prompt: payload.system_prompt,
        tools: payload.tools,
        think_level: payload.think_level.clone(),
        temperature: payload.temperature,
        max_tokens: payload.max_tokens,
        tool_choice: payload.tool_choice.clone(),
    }).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::adapter::ProviderResponse;
    use crate::providers::message::UnifiedMessage;
    use crate::thinker::MultiProviderRegistry;
    use std::sync::atomic::{AtomicU32, Ordering};

    struct FailProvider { name: String, call_count: AtomicU32, transient: bool }
    impl crate::providers::AiProvider for FailProvider {
        fn process(&self, _: RequestPayload<'_>)
            -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<ProviderResponse>> + Send + '_>>
        {
            self.call_count.fetch_add(1, Ordering::SeqCst);
            let t = self.transient;
            Box::pin(async move {
                if t {
                    Err(AlephError::RateLimitError { message: "429".into(), suggestion: None })
                } else {
                    Err(AlephError::AuthenticationError {
                        message: "invalid".into(), provider: "t".into(), suggestion: None,
                    })
                }
            })
        }
        fn name(&self) -> &str { &self.name }
        fn color(&self) -> &str { "#000" }
    }

    struct OkProvider { name: String }
    impl crate::providers::AiProvider for OkProvider {
        fn process(&self, _: RequestPayload<'_>)
            -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<ProviderResponse>> + Send + '_>>
        {
            Box::pin(async { Ok(ProviderResponse::text_only("ok".into())) })
        }
        fn name(&self) -> &str { &self.name }
        fn color(&self) -> &str { "#000" }
    }

    #[tokio::test]
    async fn test_primary_succeeds() {
        let r = MultiProviderRegistry::new("ok".into(), Arc::new(OkProvider { name: "ok".into() }));
        let msgs = [UnifiedMessage::user("t")];
        let (resp, used) = call_with_fallback(&r, "ok", &[], RequestPayload::new(&msgs)).await.unwrap();
        assert_eq!(resp.text_content(), "ok");
        assert_eq!(used, "ok");
    }

    #[tokio::test]
    async fn test_uses_fallback() {
        let r = MultiProviderRegistry::new(
            "fail".into(),
            Arc::new(FailProvider { name: "fail".into(), call_count: AtomicU32::new(0), transient: true }),
        );
        r.register("ok".into(), Arc::new(OkProvider { name: "ok".into() }));
        let msgs = [UnifiedMessage::user("t")];
        let (_, used) = call_with_fallback(&r, "fail", &["ok".into()], RequestPayload::new(&msgs)).await.unwrap();
        assert_eq!(used, "ok");
    }

    #[tokio::test]
    async fn test_permanent_no_retry() {
        let r = MultiProviderRegistry::new(
            "fail".into(),
            Arc::new(FailProvider { name: "fail".into(), call_count: AtomicU32::new(0), transient: false }),
        );
        r.register("ok".into(), Arc::new(OkProvider { name: "ok".into() }));
        let msgs = [UnifiedMessage::user("t")];
        assert!(call_with_fallback(&r, "fail", &["ok".into()], RequestPayload::new(&msgs)).await.is_err());
    }

    #[tokio::test]
    async fn test_all_fail() {
        let r = MultiProviderRegistry::new(
            "f1".into(),
            Arc::new(FailProvider { name: "f1".into(), call_count: AtomicU32::new(0), transient: true }),
        );
        r.register("f2".into(), Arc::new(FailProvider { name: "f2".into(), call_count: AtomicU32::new(0), transient: true }));
        let msgs = [UnifiedMessage::user("t")];
        let err = call_with_fallback(&r, "f1", &["f2".into()], RequestPayload::new(&msgs)).await.unwrap_err();
        assert!(err.to_string().contains("All providers failed"));
    }
}
```

- [ ] **Step 2: Register the module**

In `src/thinker/mod.rs`, add after existing module declarations:

```rust
pub mod fallback;
```

- [ ] **Step 3: Compile and run tests**

Run: `cargo test -p alephcore --lib fallback::tests`
Expected: 4 tests pass

- [ ] **Step 4: Commit**

```bash
git add src/thinker/fallback.rs src/thinker/mod.rs
git commit -m "feat: add provider fallback module with transient error retry"
```

---

## Task 10: Final Verification

- [ ] **Step 1: Full compile check**

Run: `cargo check -p alephcore`
Expected: Clean

- [ ] **Step 2: Run all tests**

Run: `cargo test -p alephcore --lib 2>&1 | tail -30`
Expected: All tests pass

- [ ] **Step 3: Run clippy**

Run: `cargo clippy -p alephcore -- -D warnings 2>&1 | head -30`
Fix any warnings.

- [ ] **Step 4: Verify deletion stats**

```bash
git diff --stat HEAD~10 -- src/dispatcher/ src/config/types/ | tail -5
```

Expected: Large net deletion (~30K lines removed)
