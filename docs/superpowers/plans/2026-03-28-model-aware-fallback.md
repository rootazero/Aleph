# Model-Aware Provider Fallback Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `AgentInstanceConfig.model` actually route to the correct provider, add error-aware health tracking with automatic fallback, and surface fallback events to users.

**Architecture:** Extend `MultiProviderRegistry` (in `src/thinker/mod.rs`) with per-provider `ProviderHealth` tracking and a `resolve_with_fallback()` method. Add `model: Option<String>` to `RequestPayload` so protocol adapters use the resolved model instead of `config.default_model()`. Replace the existing `thinker/fallback.rs` with the new health-aware mechanism. Surface fallback info via `ModelInfo` in stream events.

**Tech Stack:** Rust, tokio, serde, Leptos (Panel WASM)

**Spec:** `docs/superpowers/specs/2026-03-28-model-aware-fallback-design.md`

---

## File Map

| File | Action | Responsibility |
|------|--------|----------------|
| `src/providers/health.rs` | Create | `ProviderHealth` enum, `ProviderError` enum, state machine logic |
| `src/providers/adapter.rs` | Modify | Add `model: Option<String>` to `RequestPayload` |
| `src/providers/protocols/openai_chat.rs` | Modify | Use `payload.model` over `config.default_model()` |
| `src/providers/protocols/anthropic.rs` | Modify | Same |
| `src/providers/protocols/gemini.rs` | Modify | Same (model in URL path) |
| `src/providers/protocols/openai_responses.rs` | Modify | Same |
| `src/providers/protocols/template.rs` | Modify | Same |
| `src/providers/mod.rs` | Modify | Re-export health module |
| `src/thinker/mod.rs` | Modify | Add health to `RegistryState`, `resolve_with_fallback()`, `report_outcome()` |
| `src/thinker/streaming/events.rs` | Modify | Add `ModelInfo` to `AssistantStart` |
| `src/gateway/execution_engine/run_loop.rs` | Modify | Use `resolve_with_fallback` instead of `default_provider()` |
| `src/agent_loop/provider_bridge.rs` | Modify | Accept and pass `model` in `RequestPayload` |
| `src/thinker/fallback.rs` | Remove | Replaced by registry-level health-aware fallback |
| `interfaces/webchat/src/views/chat/` | Modify | Parse `ModelInfo`, show fallback indicator |

---

### Task 1: ProviderHealth + ProviderError Types

**Files:**
- Create: `src/providers/health.rs`
- Modify: `src/providers/mod.rs`

- [ ] **Step 1: Create `health.rs` with ProviderHealth and ProviderError**

```rust
// src/providers/health.rs
//! Provider health tracking for error-aware fallback.

use std::time::{Duration, Instant};
use crate::error::AlephError;

/// Maximum cooldown duration for degraded providers
const MAX_COOLDOWN: Duration = Duration::from_secs(300); // 5 minutes
/// Initial cooldown duration after first transient failure
const INITIAL_COOLDOWN: Duration = Duration::from_secs(30);

/// Transient errors that may self-resolve
#[derive(Debug, Clone)]
pub enum TransientError {
    RateLimited { retry_after: Option<Duration> },
    ServerError { status: u16 },
    Timeout,
    ConnectionFailed,
}

/// Permanent errors requiring user intervention
#[derive(Debug, Clone)]
pub enum PermanentError {
    AuthFailed,
    ModelNotFound,
}

/// Provider error classification for health tracking.
/// 400 InvalidRequest is intentionally excluded — it's request-specific, not provider-level.
#[derive(Debug, Clone)]
pub enum ProviderError {
    Transient(TransientError),
    Permanent(PermanentError),
}

/// Per-provider health state
#[derive(Debug, Clone)]
pub enum ProviderHealth {
    Healthy,
    Degraded {
        since: Instant,
        cooldown_until: Instant,
        consecutive_failures: u32,
    },
    Unavailable {
        since: Instant,
        reason: String,
    },
}

impl Default for ProviderHealth {
    fn default() -> Self {
        Self::Healthy
    }
}

impl ProviderHealth {
    /// Whether this provider should be attempted for requests
    pub fn is_usable(&self) -> bool {
        match self {
            Self::Healthy => true,
            Self::Degraded { cooldown_until, .. } => Instant::now() >= *cooldown_until,
            Self::Unavailable { .. } => false,
        }
    }

    /// Record a successful request — resets to Healthy
    pub fn record_success(&mut self) {
        *self = Self::Healthy;
    }

    /// Record a failed request — transitions based on error type
    pub fn record_failure(&mut self, err: &ProviderError) {
        match err {
            ProviderError::Transient(t) => {
                let (consecutive, cooldown) = match self {
                    Self::Degraded { consecutive_failures, .. } => {
                        let next = *consecutive_failures + 1;
                        let backoff = INITIAL_COOLDOWN * 2u32.saturating_pow(next.min(5));
                        (next, backoff.min(MAX_COOLDOWN))
                    }
                    _ => (1, match t {
                        TransientError::RateLimited { retry_after: Some(d) } => (*d).max(INITIAL_COOLDOWN),
                        _ => INITIAL_COOLDOWN,
                    }),
                };
                let now = Instant::now();
                *self = Self::Degraded {
                    since: now,
                    cooldown_until: now + cooldown,
                    consecutive_failures: consecutive,
                };
            }
            ProviderError::Permanent(p) => {
                let reason = match p {
                    PermanentError::AuthFailed => "Authentication failed (401/403)".to_string(),
                    PermanentError::ModelNotFound => "Model not found (404)".to_string(),
                };
                *self = Self::Unavailable {
                    since: Instant::now(),
                    reason,
                };
            }
        }
    }

    /// Reset to healthy (used when user fixes config and re-tests)
    pub fn reset(&mut self) {
        *self = Self::Healthy;
    }
}

/// Classify an AlephError into an optional ProviderError for health tracking.
/// Returns None for errors that should not affect provider health (e.g. 400 bad request).
impl From<&AlephError> for Option<ProviderError> {
    fn from(err: &AlephError) -> Self {
        match err {
            AlephError::RateLimitError { .. } => Some(ProviderError::Transient(
                TransientError::RateLimited { retry_after: None },
            )),
            AlephError::Timeout { .. } | AlephError::ExecutionTimeout { .. } => {
                Some(ProviderError::Transient(TransientError::Timeout))
            }
            AlephError::NetworkError { .. } => {
                Some(ProviderError::Transient(TransientError::ConnectionFailed))
            }
            AlephError::AuthenticationError { .. } => {
                Some(ProviderError::Permanent(PermanentError::AuthFailed))
            }
            // ProviderError with 5xx pattern
            AlephError::ProviderError { message, .. } => {
                if message.contains("500") || message.contains("502") || message.contains("503") {
                    Some(ProviderError::Transient(TransientError::ServerError { status: 500 }))
                } else if message.contains("404") && message.to_lowercase().contains("model") {
                    Some(ProviderError::Permanent(PermanentError::ModelNotFound))
                } else {
                    None // generic provider error, don't affect health
                }
            }
            _ => None,
        }
    }
}

/// Result of model resolution through the fallback chain
#[derive(Debug, Clone)]
pub struct ResolvedModel {
    /// Name of the provider to use
    pub provider_name: String,
    /// Actual model name to send in the request
    pub model: String,
    /// Whether this is a fallback (not the originally requested model)
    pub is_fallback: bool,
    /// The originally requested model name
    pub original_model: String,
}

/// Model info attached to stream responses for user visibility
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ModelInfo {
    pub model: String,
    pub provider: String,
    pub is_fallback: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub original_model: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn healthy_is_usable() {
        assert!(ProviderHealth::Healthy.is_usable());
    }

    #[test]
    fn degraded_not_usable_during_cooldown() {
        let health = ProviderHealth::Degraded {
            since: Instant::now(),
            cooldown_until: Instant::now() + Duration::from_secs(60),
            consecutive_failures: 1,
        };
        assert!(!health.is_usable());
    }

    #[test]
    fn degraded_usable_after_cooldown() {
        let health = ProviderHealth::Degraded {
            since: Instant::now() - Duration::from_secs(120),
            cooldown_until: Instant::now() - Duration::from_secs(1),
            consecutive_failures: 1,
        };
        assert!(health.is_usable());
    }

    #[test]
    fn unavailable_not_usable() {
        let health = ProviderHealth::Unavailable {
            since: Instant::now(),
            reason: "auth failed".into(),
        };
        assert!(!health.is_usable());
    }

    #[test]
    fn record_success_resets_to_healthy() {
        let mut health = ProviderHealth::Degraded {
            since: Instant::now(),
            cooldown_until: Instant::now() + Duration::from_secs(60),
            consecutive_failures: 3,
        };
        health.record_success();
        assert!(matches!(health, ProviderHealth::Healthy));
    }

    #[test]
    fn record_transient_failure_degrades() {
        let mut health = ProviderHealth::Healthy;
        health.record_failure(&ProviderError::Transient(TransientError::Timeout));
        assert!(matches!(health, ProviderHealth::Degraded { consecutive_failures: 1, .. }));
    }

    #[test]
    fn consecutive_failures_increase_cooldown() {
        let mut health = ProviderHealth::Healthy;
        // First failure: 30s cooldown
        health.record_failure(&ProviderError::Transient(TransientError::Timeout));
        let ProviderHealth::Degraded { cooldown_until: c1, .. } = &health else { panic!() };
        let c1 = *c1;

        // Second failure: 60s cooldown (doubled)
        health.record_failure(&ProviderError::Transient(TransientError::Timeout));
        let ProviderHealth::Degraded { cooldown_until: c2, consecutive_failures: 2, .. } = &health else { panic!() };
        assert!(*c2 > c1);
    }

    #[test]
    fn permanent_failure_makes_unavailable() {
        let mut health = ProviderHealth::Healthy;
        health.record_failure(&ProviderError::Permanent(PermanentError::AuthFailed));
        assert!(matches!(health, ProviderHealth::Unavailable { .. }));
    }

    #[test]
    fn reset_restores_healthy() {
        let mut health = ProviderHealth::Unavailable {
            since: Instant::now(),
            reason: "test".into(),
        };
        health.reset();
        assert!(matches!(health, ProviderHealth::Healthy));
    }

    #[test]
    fn rate_limit_with_retry_after() {
        let mut health = ProviderHealth::Healthy;
        health.record_failure(&ProviderError::Transient(
            TransientError::RateLimited { retry_after: Some(Duration::from_secs(90)) },
        ));
        match &health {
            ProviderHealth::Degraded { cooldown_until, .. } => {
                // Should use the retry_after value (90s) since it's > INITIAL_COOLDOWN (30s)
                let elapsed = cooldown_until.duration_since(Instant::now());
                assert!(elapsed.as_secs() >= 85); // roughly 90s
            }
            _ => panic!("Expected Degraded"),
        }
    }
}
```

- [ ] **Step 2: Add health module to `src/providers/mod.rs`**

Add after other `pub mod` declarations:
```rust
pub mod health;
pub use health::{ProviderHealth, ProviderError, ResolvedModel, ModelInfo};
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p alephcore --lib providers::health`
Expected: All tests pass

- [ ] **Step 4: Commit**

```bash
git add src/providers/health.rs src/providers/mod.rs
git commit -m "providers: add ProviderHealth and ProviderError types for fallback"
```

---

### Task 2: Add `model` Field to RequestPayload

**Files:**
- Modify: `src/providers/adapter.rs:36-74`
- Modify: `src/thinker/fallback.rs:53-61` (the `try_provider` field copy)
- Modify: `src/agent_loop/provider_bridge.rs:65-74`

- [ ] **Step 1: Add `model` field to `RequestPayload`**

In `src/providers/adapter.rs`, add to struct (after line 50, before `}`):
```rust
    /// Model override — when set, protocol adapters use this instead of config.default_model()
    pub model: Option<String>,
```

Update `Default` impl (line 54-65) to include `model: None`.

Update `new()` method — no change needed since it uses `..Default::default()`.

Add builder method after `with_tool_choice` (after line 111):
```rust
    /// Set model override
    pub fn with_model(mut self, model: Option<String>) -> Self {
        self.model = model;
        self
    }
```

- [ ] **Step 2: Update `try_provider` in `thinker/fallback.rs`**

In `src/thinker/fallback.rs:53-61`, the `try_provider` function manually constructs `RequestPayload` by copying fields. Add `model` to the copy:

```rust
    provider.process(RequestPayload {
        messages: payload.messages,
        system_prompt: payload.system_prompt,
        tools: payload.tools,
        think_level: payload.think_level,
        temperature: payload.temperature,
        max_tokens: payload.max_tokens,
        tool_choice: payload.tool_choice.clone(),
        model: payload.model.clone(),
    }).await
```

- [ ] **Step 3: Verify compilation**

Run: `cargo check -p alephcore`
Expected: Clean build (no existing code passes `model`, so `Default` fills it with `None`)

- [ ] **Step 4: Commit**

```bash
git add src/providers/adapter.rs src/thinker/fallback.rs
git commit -m "providers: add model field to RequestPayload"
```

---

### Task 3: Protocol Adapters Use `payload.model`

**Files:**
- Modify: `src/providers/protocols/openai_chat.rs:244`
- Modify: `src/providers/protocols/anthropic.rs:313`
- Modify: `src/providers/protocols/gemini.rs:55-58`
- Modify: `src/providers/protocols/openai_responses.rs:204`
- Modify: `src/providers/protocols/template.rs:60`

The pattern for all adapters: `payload.model.as_deref().unwrap_or_else(|| config.default_model())`

- [ ] **Step 1: Update OpenAI Chat adapter**

In `src/providers/protocols/openai_chat.rs:244`, change:
```rust
// Before
"model": config.default_model(),
// After
"model": payload.model.as_deref().unwrap_or_else(|| config.default_model()),
```

- [ ] **Step 2: Update Anthropic adapter**

In `src/providers/protocols/anthropic.rs:313`, change:
```rust
// Before
model: config.default_model().to_string(),
// After
model: payload.model.as_deref().unwrap_or_else(|| config.default_model()).to_string(),
```

Also update the tracing log at line 332 to use the same resolved model for consistency:
```rust
// The model should reflect what was actually used
model = %payload.model.as_deref().unwrap_or_else(|| config.default_model()),
```

- [ ] **Step 3: Update Gemini adapter**

Gemini is special — model is in the URL path, not the body. The `build_endpoint` function at `gemini.rs:44` currently takes `config: &ProviderConfig`. It needs an additional model parameter.

In `src/providers/protocols/gemini.rs`, change `build_endpoint`:
```rust
fn build_endpoint(config: &ProviderConfig, model_override: Option<&str>) -> String {
    let base_url = config.base_url.as_deref()
        .unwrap_or("https://generativelanguage.googleapis.com")
        .trim_end_matches("/v1beta")
        .trim_end_matches("/v1")
        .trim_end_matches('/')
        .to_string();

    let model = model_override.unwrap_or_else(|| config.default_model());
    format!("{}/v1beta/models/{}:streamGenerateContent", base_url, model)
}
```

Update the call site in `build_request` to pass `payload.model.as_deref()`:
```rust
let endpoint = Self::build_endpoint(config, payload.model.as_deref());
```

- [ ] **Step 4: Update OpenAI Responses adapter**

In `src/providers/protocols/openai_responses.rs:204`, change:
```rust
// Before
let request = Self::build_responses_request(payload, config.default_model(), &self.variant, config);
// After
let model = payload.model.as_deref().unwrap_or_else(|| config.default_model());
let request = Self::build_responses_request(payload, model, &self.variant, config);
```

- [ ] **Step 5: Update template adapter**

In `src/providers/protocols/template.rs:60`, change:
```rust
// Before
"model": config.default_model(),
// After
"model": payload.model.as_deref().unwrap_or_else(|| config.default_model()),
```

- [ ] **Step 6: Verify compilation**

Run: `cargo check -p alephcore`
Expected: Clean build

- [ ] **Step 7: Commit**

```bash
git add src/providers/protocols/
git commit -m "protocols: use payload.model override in all adapters"
```

---

### Task 4: Health-Aware `resolve_with_fallback` in MultiProviderRegistry

**Files:**
- Modify: `src/thinker/mod.rs:173-280` (MultiProviderRegistry)

- [ ] **Step 1: Write tests for resolve_with_fallback**

Add to the existing `multi_registry_tests` module in `src/thinker/mod.rs` (after line 282):

```rust
    use crate::providers::health::{ProviderHealth, ProviderError, TransientError, PermanentError};

    #[test]
    fn resolve_returns_requested_model_when_healthy() {
        let r = MultiProviderRegistry::new("claude".into(), Arc::new(NamedProvider { tag: "claude".into() }));
        r.register("openai".into(), Arc::new(NamedProvider { tag: "openai".into() }));

        let resolved = r.resolve_with_fallback("claude-sonnet-4", &[]).unwrap();
        assert_eq!(resolved.provider_name, "claude");
        assert_eq!(resolved.model, "claude-sonnet-4");
        assert!(!resolved.is_fallback);
    }

    #[test]
    fn resolve_uses_agent_fallback_when_primary_degraded() {
        let r = MultiProviderRegistry::new("claude".into(), Arc::new(NamedProvider { tag: "claude".into() }));
        r.register("openai".into(), Arc::new(NamedProvider { tag: "openai".into() }));

        // Mark claude as degraded with future cooldown
        r.report_outcome("claude", Err(ProviderError::Transient(TransientError::Timeout)));

        let resolved = r.resolve_with_fallback(
            "claude-sonnet-4",
            &["gpt-4o".to_string()],
        ).unwrap();
        assert_eq!(resolved.provider_name, "openai");
        assert_eq!(resolved.model, "gpt-4o");
        assert!(resolved.is_fallback);
        assert_eq!(resolved.original_model, "claude-sonnet-4");
    }

    #[test]
    fn resolve_uses_global_fallback_when_agent_fallbacks_exhausted() {
        let r = MultiProviderRegistry::new("claude".into(), Arc::new(NamedProvider { tag: "claude".into() }));
        r.register("openai".into(), Arc::new(NamedProvider { tag: "openai".into() }));
        r.set_fallbacks(vec!["openai".into()]);

        // Mark claude as unavailable
        r.report_outcome("claude", Err(ProviderError::Permanent(PermanentError::AuthFailed)));

        // No agent-level fallbacks, should use global
        let resolved = r.resolve_with_fallback("claude-sonnet-4", &[]).unwrap();
        assert_eq!(resolved.provider_name, "openai");
        assert!(resolved.is_fallback);
    }

    #[test]
    fn resolve_fails_when_all_unavailable() {
        let r = MultiProviderRegistry::new("claude".into(), Arc::new(NamedProvider { tag: "claude".into() }));

        r.report_outcome("claude", Err(ProviderError::Permanent(PermanentError::AuthFailed)));

        let result = r.resolve_with_fallback("claude-sonnet-4", &[]);
        assert!(result.is_err());
    }

    #[test]
    fn resolve_uses_provider_slash_model_syntax() {
        let r = MultiProviderRegistry::new("openai".into(), Arc::new(NamedProvider { tag: "openai".into() }));

        let resolved = r.resolve_with_fallback("openai/gpt-4o", &[]).unwrap();
        assert_eq!(resolved.provider_name, "openai");
        assert_eq!(resolved.model, "openai/gpt-4o");
    }

    #[test]
    fn report_success_resets_health() {
        let r = MultiProviderRegistry::new("claude".into(), Arc::new(NamedProvider { tag: "claude".into() }));

        // Degrade it
        r.report_outcome("claude", Err(ProviderError::Transient(TransientError::Timeout)));
        // Then succeed
        r.report_outcome("claude", Ok(()));

        // Should be usable again immediately
        let resolved = r.resolve_with_fallback("claude-sonnet-4", &[]).unwrap();
        assert_eq!(resolved.provider_name, "claude");
        assert!(!resolved.is_fallback);
    }

    #[test]
    fn resolve_unknown_model_uses_default_provider() {
        let r = MultiProviderRegistry::new("myhost".into(), Arc::new(NamedProvider { tag: "myhost".into() }));

        // "custom-llm-v1" won't match any prefix in resolve_provider_from_model
        let resolved = r.resolve_with_fallback("custom-llm-v1", &[]).unwrap();
        assert_eq!(resolved.provider_name, "myhost"); // falls back to default
        assert_eq!(resolved.model, "custom-llm-v1");
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p alephcore --lib thinker::multi_registry_tests`
Expected: Compilation error — `resolve_with_fallback` and `report_outcome` don't exist yet

- [ ] **Step 3: Add health tracking to RegistryState**

In `src/thinker/mod.rs`, modify `RegistryState` (line 173):
```rust
use crate::providers::health::{ProviderHealth, ProviderError, ResolvedModel};

struct RegistryState {
    providers: HashMap<String, Arc<dyn AiProvider>>,
    default_name: String,
    fallbacks: Vec<String>,
    health: HashMap<String, ProviderHealth>,  // NEW
}
```

Update `MultiProviderRegistry::new()` (line 185) to initialize health:
```rust
    pub fn new(name: String, provider: Arc<dyn AiProvider>) -> Self {
        let mut providers = HashMap::new();
        let mut health = HashMap::new();
        providers.insert(name.clone(), provider);
        health.insert(name.clone(), ProviderHealth::default());
        Self {
            state: std::sync::RwLock::new(RegistryState {
                providers, default_name: name, fallbacks: vec![], health,
            }),
        }
    }
```

Update `register()` (line 195) to also init health:
```rust
    pub fn register(&self, name: String, provider: Arc<dyn AiProvider>) {
        let mut state = self.state.write().unwrap_or_else(|e| e.into_inner());
        state.providers.insert(name.clone(), provider);
        state.health.entry(name).or_insert_with(ProviderHealth::default);
    }
```

- [ ] **Step 4: Implement `resolve_with_fallback`**

Add to `impl MultiProviderRegistry` (after `list_providers` at line 241):

```rust
    /// Resolve a model name to a healthy (provider, model) pair,
    /// trying the fallback chain if the primary is unhealthy.
    ///
    /// Chain order: [requested model's provider] → [agent fallbacks] → [global fallbacks]
    pub fn resolve_with_fallback(
        &self,
        model: &str,
        agent_fallbacks: &[String],
    ) -> crate::error::Result<ResolvedModel> {
        let state = self.state.read().unwrap_or_else(|e| e.into_inner());

        // Build candidate list: (provider_name, model_name)
        let mut candidates: Vec<(String, String)> = Vec::new();

        // 1. Primary: resolve model → provider
        let primary_provider = self.resolve_model_to_provider(&state, model);
        candidates.push((primary_provider, model.to_string()));

        // 2. Agent-level fallbacks
        for fb_model in agent_fallbacks {
            let provider = self.resolve_model_to_provider(&state, fb_model);
            candidates.push((provider, fb_model.clone()));
        }

        // 3. Global fallbacks (use their default model)
        for fb_provider in &state.fallbacks {
            if let Some(p) = state.providers.get(fb_provider) {
                // Use the provider's configured default model
                if let Some(http) = p.as_http_provider() {
                    let default_model = http.config().default_model().to_string();
                    candidates.push((fb_provider.clone(), default_model));
                } else {
                    // Non-HTTP providers: use provider name as model placeholder
                    candidates.push((fb_provider.clone(), fb_provider.clone()));
                }
            }
        }

        // Try each candidate, checking health
        let original_model = model.to_string();
        for (i, (provider_name, candidate_model)) in candidates.iter().enumerate() {
            let health = state.health.get(provider_name)
                .cloned()
                .unwrap_or(ProviderHealth::Healthy);

            if !health.is_usable() {
                continue;
            }

            if state.providers.contains_key(provider_name) {
                return Ok(ResolvedModel {
                    provider_name: provider_name.clone(),
                    model: candidate_model.clone(),
                    is_fallback: i > 0,
                    original_model: original_model.clone(),
                });
            }
        }

        Err(crate::error::AlephError::provider(format!(
            "All providers unavailable for model '{}' ({} candidate(s) checked)",
            model, candidates.len()
        )))
    }

    /// Report the outcome of a request to update provider health
    pub fn report_outcome(&self, provider_name: &str, result: Result<(), ProviderError>) {
        let mut state = self.state.write().unwrap_or_else(|e| e.into_inner());
        let health = state.health.entry(provider_name.to_string())
            .or_insert_with(ProviderHealth::default);

        match result {
            Ok(()) => health.record_success(),
            Err(ref err) => health.record_failure(err),
        }
    }

    /// Reset a provider's health to Healthy (called after test_connection succeeds)
    pub fn reset_health(&self, provider_name: &str) {
        let mut state = self.state.write().unwrap_or_else(|e| e.into_inner());
        if let Some(health) = state.health.get_mut(provider_name) {
            health.reset();
        }
    }

    /// Helper: resolve a model name to a provider name
    fn resolve_model_to_provider(&self, state: &RegistryState, model: &str) -> String {
        // Try "provider/model" syntax
        if let Some(slash_pos) = model.find('/') {
            let provider_name = &model[..slash_pos];
            if state.providers.contains_key(provider_name) {
                return provider_name.to_string();
            }
        }
        // Try prefix-based resolution
        if let Some(name) = crate::providers::resolve_provider_from_model(model) {
            if state.providers.contains_key(&name) {
                return name;
            }
        }
        // Fall back to default provider
        state.default_name.clone()
    }
```

- [ ] **Step 5: Run tests**

Run: `cargo test -p alephcore --lib thinker::multi_registry_tests`
Expected: All tests pass

- [ ] **Step 6: Commit**

```bash
git add src/thinker/mod.rs
git commit -m "thinker: add resolve_with_fallback and health tracking to MultiProviderRegistry"
```

---

### Task 5: Wire Into Execution Engine

**Files:**
- Modify: `src/gateway/execution_engine/run_loop.rs:64-69`
- Modify: `src/agent_loop/provider_bridge.rs:65-74`

- [ ] **Step 1: Update `run_loop.rs` to use resolve_with_fallback**

In `src/gateway/execution_engine/run_loop.rs`, replace lines 64-69:

```rust
// Before:
// TODO(P3): Use call_with_fallback() from thinker::fallback when the registry
// has fallbacks configured. This requires making AiProviderBridge registry-aware
// so it can retry with fallback providers on transient errors.
let provider = self.provider_registry.default_provider();
let bridge = AiProviderBridge::new(provider);

// After:
use crate::providers::health::ProviderError;

let resolved = if let Some(multi_reg) = self.as_multi_registry() {
    let model = &agent.model;
    let fallbacks = &agent.fallback_models;
    multi_reg.resolve_with_fallback(model, fallbacks)?
} else {
    // SingleProvider/Swappable: no fallback, use default
    crate::providers::health::ResolvedModel {
        provider_name: "default".to_string(),
        model: agent.model.clone(),
        is_fallback: false,
        original_model: agent.model.clone(),
    }
};

let provider = self.provider_registry.get(&resolved.provider_name)
    .unwrap_or_else(|| self.provider_registry.default_provider());
let bridge = AiProviderBridge::new(provider)
    .with_model(resolved.model.clone());
```

Note: `self.as_multi_registry()` needs a helper method on `ExecutionEngine` to downcast the registry. If the registry is generic `P: ProviderRegistry`, we need to add `resolve_with_fallback` and `report_outcome` to the `ProviderRegistry` trait with default no-op implementations, so `MultiProviderRegistry` can override them.

**Alternative (simpler):** Add methods to `ProviderRegistry` trait:

In `src/thinker/mod.rs`, extend the trait:
```rust
pub trait ProviderRegistry: Send + Sync {
    fn get(&self, model: &str) -> Option<Arc<dyn AiProvider>>;
    fn default_provider(&self) -> Arc<dyn AiProvider>;
    fn list_providers(&self) -> Vec<String> { vec![] }

    /// Resolve model with health-aware fallback. Default: no health tracking.
    fn resolve_with_fallback(
        &self,
        model: &str,
        _agent_fallbacks: &[String],
    ) -> crate::error::Result<crate::providers::health::ResolvedModel> {
        let provider = self.get(model)
            .unwrap_or_else(|| self.default_provider());
        Ok(crate::providers::health::ResolvedModel {
            provider_name: provider.name().to_string(),
            model: model.to_string(),
            is_fallback: false,
            original_model: model.to_string(),
        })
    }

    /// Report request outcome for health tracking. Default: no-op.
    fn report_outcome(&self, _provider: &str, _result: Result<(), crate::providers::health::ProviderError>) {}

    /// Reset provider health (e.g. after successful test_connection). Default: no-op.
    fn reset_health(&self, _provider: &str) {}
}
```

Then `MultiProviderRegistry` overrides all three.

This way `run_loop.rs` simplifies to:
```rust
let resolved = self.provider_registry
    .resolve_with_fallback(&agent.model, &agent.fallback_models)?;

let provider = self.provider_registry.get(&resolved.provider_name)
    .unwrap_or_else(|| self.provider_registry.default_provider());
let bridge = AiProviderBridge::new(provider)
    .with_model(resolved.model.clone());
```

- [ ] **Step 2: Update `AiProviderBridge` to accept model override**

In `src/agent_loop/provider_bridge.rs`, add a `model` field and builder:

```rust
pub struct AiProviderBridge {
    provider: Arc<dyn AiProvider>,
    model: Option<String>,  // NEW
}

impl AiProviderBridge {
    pub fn new(provider: Arc<dyn AiProvider>) -> Self {
        Self { provider, model: None }
    }

    pub fn with_model(mut self, model: String) -> Self {
        self.model = Some(model);
        self
    }
}
```

Update the `stream()` method payload construction (line 65-74):
```rust
        let payload = RequestPayload {
            messages: &cleaned,
            system_prompt: Some(system_prompt),
            tools: if dispatcher_tools.is_empty() {
                None
            } else {
                Some(&dispatcher_tools)
            },
            model: self.model.clone(),  // NEW — pass model override
            ..Default::default()
        };
```

- [ ] **Step 3: Add `report_outcome` after stream completes**

After the bridge returns the stream result in `run_loop.rs`, add health reporting. This happens after the agent loop processes the response. Find where the stream result is consumed and add:

```rust
// After stream processing completes (success or failure):
if let Some(ref err) = stream_error {
    if let Some(provider_err) = Option::<ProviderError>::from(err) {
        self.provider_registry.report_outcome(&resolved.provider_name, Err(provider_err));
    }
} else {
    self.provider_registry.report_outcome(&resolved.provider_name, Ok(()));
}
```

The exact insertion point depends on the stream consumption code — look for where `AlephError` is caught after `bridge.stream()`.

- [ ] **Step 4: Verify compilation and run tests**

Run: `cargo check -p alephcore && cargo test -p alephcore --lib`
Expected: Clean build, existing tests pass

- [ ] **Step 5: Commit**

```bash
git add src/thinker/mod.rs src/gateway/execution_engine/run_loop.rs src/agent_loop/provider_bridge.rs
git commit -m "engine: wire resolve_with_fallback into agent execution loop"
```

---

### Task 6: ModelInfo in Stream Events

**Files:**
- Modify: `src/thinker/streaming/events.rs:10-14`
- Modify: `src/gateway/execution_engine/run_loop.rs` (pass ModelInfo to stream)

- [ ] **Step 1: Add `model_info` to `AssistantStart` event**

In `src/thinker/streaming/events.rs`, modify `AssistantStart`:

```rust
use crate::providers::health::ModelInfo;

/// Assistant message started
AssistantStart {
    message_index: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    model_info: Option<ModelInfo>,  // NEW
},
```

- [ ] **Step 2: Emit ModelInfo when creating AssistantStart events**

In `run_loop.rs`, when constructing the `AssistantStart` event, include `model_info`:

```rust
let model_info = if resolved.is_fallback {
    Some(ModelInfo {
        model: resolved.model.clone(),
        provider: resolved.provider_name.clone(),
        is_fallback: true,
        original_model: Some(resolved.original_model.clone()),
    })
} else {
    Some(ModelInfo {
        model: resolved.model.clone(),
        provider: resolved.provider_name.clone(),
        is_fallback: false,
        original_model: None,
    })
};
```

Pass `model_info` into whatever code emits `StreamEvent::AssistantStart`. The exact wiring depends on how events are emitted — grep for `AssistantStart` creation sites and update them.

- [ ] **Step 3: Update any existing `AssistantStart` construction sites**

Run: `grep -rn "AssistantStart" src/`

Add `model_info: None` to all existing construction sites that don't have the resolved model info (so they compile without changes).

- [ ] **Step 4: Verify compilation and run tests**

Run: `cargo check -p alephcore && cargo test -p alephcore --lib`
Expected: Clean build

- [ ] **Step 5: Commit**

```bash
git add src/thinker/streaming/events.rs src/gateway/execution_engine/
git commit -m "streaming: add ModelInfo to AssistantStart event"
```

---

### Task 7: Remove Old `fallback.rs`

**Files:**
- Remove: `src/thinker/fallback.rs`
- Modify: `src/thinker/mod.rs:12` (remove `pub mod fallback;`)

- [ ] **Step 1: Check for callers**

Run: `grep -rn "call_with_fallback\|thinker::fallback" src/`

Verify no remaining callers exist (the TODO in run_loop.rs was never wired up).

- [ ] **Step 2: Remove module declaration**

In `src/thinker/mod.rs:12`, remove:
```rust
pub mod fallback;
```

- [ ] **Step 3: Delete the file**

```bash
rm src/thinker/fallback.rs
```

- [ ] **Step 4: Verify compilation**

Run: `cargo check -p alephcore`
Expected: Clean build

- [ ] **Step 5: Commit**

```bash
git add -A src/thinker/fallback.rs src/thinker/mod.rs
git commit -m "thinker: remove old fallback.rs, replaced by registry-level health-aware fallback"
```

---

### Task 8: Panel Fallback Indicator (Leptos WASM)

**Files:**
- Modify: `interfaces/webchat/src/views/chat/` (message bubble component)

This task is intentionally less prescriptive — Panel WASM code requires exploring the exact component structure.

- [ ] **Step 1: Explore Panel chat message components**

```bash
grep -rn "model\|provider" interfaces/webchat/src/views/chat/ | head -30
```

Find where the message bubble renders provider/model info (the small text in top-right).

- [ ] **Step 2: Parse `model_info` from stream events**

In the WebSocket message handler, extract `model_info` from `AssistantStart` events and store it in the message's reactive state.

- [ ] **Step 3: Render fallback indicator**

When `model_info.is_fallback == true`, render the model display as:
```
<span class="model-fallback">
  <span class="model-original">{original_model}</span>
  <span class="model-arrow"> → </span>
  <span class="model-actual">{model}</span>
</span>
```

With CSS: original model gets `text-decoration: line-through; opacity: 0.4`, arrow and actual model get `color: #fde047` (yellow).

When `is_fallback == false`, render normally as current behavior.

- [ ] **Step 4: Build WASM and verify**

Run: `just dev` or the WASM build command
Expected: Panel builds without errors

- [ ] **Step 5: Commit**

```bash
git add interfaces/webchat/
git commit -m "panel: show fallback indicator when model is degraded"
```

---

### Task 9: Integration Test

**Files:**
- The test uses existing `MockProvider` / `NamedProvider` infrastructure

- [ ] **Step 1: Write integration test for full fallback chain**

Add to `src/thinker/mod.rs` tests (or a new test file):

```rust
    #[test]
    fn full_fallback_chain_agent_then_global() {
        let r = MultiProviderRegistry::new("claude".into(), Arc::new(NamedProvider { tag: "claude".into() }));
        r.register("openai".into(), Arc::new(NamedProvider { tag: "openai".into() }));
        r.register("deepseek".into(), Arc::new(NamedProvider { tag: "deepseek".into() }));
        r.set_fallbacks(vec!["deepseek".into()]);

        // Mark claude unavailable (permanent)
        r.report_outcome("claude", Err(ProviderError::Permanent(PermanentError::AuthFailed)));
        // Mark openai degraded (in cooldown)
        r.report_outcome("openai", Err(ProviderError::Transient(TransientError::Timeout)));

        // Agent fallbacks: [gpt-4o], global: [deepseek]
        // claude (unavailable) → gpt-4o/openai (degraded, in cooldown) → deepseek (healthy)
        let resolved = r.resolve_with_fallback(
            "claude-sonnet-4",
            &["gpt-4o".to_string()],
        ).unwrap();

        assert_eq!(resolved.provider_name, "deepseek");
        assert!(resolved.is_fallback);
        assert_eq!(resolved.original_model, "claude-sonnet-4");
    }
```

- [ ] **Step 2: Run full test suite**

Run: `cargo test -p alephcore --lib`
Expected: All tests pass

- [ ] **Step 3: Commit**

```bash
git add src/thinker/mod.rs
git commit -m "test: add integration test for full fallback chain"
```

---

### Task 10: Wire `reset_health` into Provider RPC Handlers

**Files:**
- Modify: `src/gateway/handlers/providers/handlers.rs`

- [ ] **Step 1: Find `handle_set_default` and `test_connection` handlers**

These handlers already live in `src/gateway/handlers/providers/handlers.rs`. When a provider is set as default or test_connection succeeds, call `reset_health`.

- [ ] **Step 2: Add reset_health call after successful test_connection**

In the test_connection handler, after the test succeeds:
```rust
// After test succeeds, reset health so the provider is immediately usable
if let Some(registry) = app_ctx.provider_registry() {
    registry.reset_health(&provider_name);
}
```

- [ ] **Step 3: Add reset_health call in handle_set_default**

When a provider is set as default (implying user wants to use it):
```rust
registry.reset_health(&name);
```

- [ ] **Step 4: Verify compilation**

Run: `cargo check -p alephcore`
Expected: Clean build

- [ ] **Step 5: Commit**

```bash
git add src/gateway/handlers/providers/handlers.rs
git commit -m "gateway: reset provider health on test_connection and set_default"
```
