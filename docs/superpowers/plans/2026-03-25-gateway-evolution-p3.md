# Gateway Evolution P3: Provider Evolution Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Activate the existing FailoverProvider in production, add OAuth token auto-refresh, and expose dynamic model discovery for Ollama.

**Architecture:** FailoverProvider (380 lines, already complete) + call_with_fallback() exist but aren't wired to production. P3 activates them via config + execution engine integration. OAuth refresh is a new module that plugs into AuthProfileProviderRegistry. ModelDiscovery is a trait + Ollama implementation.

**Tech Stack:** Existing `FailoverProvider`, `MultiProviderRegistry`, `OllamaProvider`, `reqwest`

**Spec:** `docs/superpowers/specs/2026-03-25-gateway-evolution-design.md` (Phase 3)

---

## Key Discovery: 70% Already Built

| Component | Status | Location |
|-----------|--------|----------|
| FailoverProvider | ✅ Complete (380 LOC) | `providers/failover.rs` |
| call_with_fallback() | ✅ Complete | `thinker/fallback.rs` |
| MultiProviderRegistry.set_fallbacks() | ✅ Complete | `thinker/mod.rs` |
| OAuthCredential struct | ✅ Has refresh/expires fields | `providers/auth_profiles/credentials.rs` |
| Ollama /api/tags parsing | ✅ In test code | `providers/ollama.rs` |
| Production wiring | ❌ Not connected | — |
| OAuth auto-refresh logic | ❌ Missing | — |
| ModelDiscovery trait | ❌ Missing | — |

## File Map

| Action | File | Responsibility |
|--------|------|---------------|
| Modify | `src/gateway/execution_engine/run_loop.rs` | Use call_with_fallback() when fallbacks configured |
| Modify | `src/thinker/mod.rs` | Wire fallbacks from config into MultiProviderRegistry |
| Create | `src/providers/oauth_refresh.rs` | OAuth token auto-refresh logic |
| Modify | `src/providers/auth_profiles/credentials.rs` | Add client_secret + token_endpoint to OAuthCredential |
| Modify | `src/providers/auth_profile_registry.rs` | Call refresh before use |
| Modify | `src/providers/mod.rs` | Export new module |
| Create | `src/providers/model_discovery.rs` | ModelDiscovery trait + OllamaDiscovery impl |
| Modify | `src/providers/ollama.rs` | Expose /api/tags as ModelDiscovery |
| Modify | `src/gateway/handlers/providers/handlers.rs` | Merge discovered models into models.list |

---

### Task 1: Wire FailoverProvider fallback chain to production

The FailoverProvider and call_with_fallback() already exist. We need to connect them to the execution path.

**Files:**
- Modify: `src/thinker/mod.rs` — ensure fallbacks are populated from config
- Modify: `src/bin/aleph-server/commands/start/` — read fallback config and call set_fallbacks()

- [ ] **Step 1: Read current MultiProviderRegistry to understand wiring**

Read `src/thinker/mod.rs` lines 160-270 to see MultiProviderRegistry and its set_fallbacks() method. Read `src/bin/aleph-server/commands/start/mod.rs` to see where MultiProviderRegistry is set up.

- [ ] **Step 2: Wire fallbacks from config at server startup**

In the server startup code (likely `commands/start/mod.rs` or `builder/agent_init.rs`), after the MultiProviderRegistry is created and providers are registered, add:

```rust
// Set fallback chain from config (if configured)
if !loaded_app_config.fallback_providers.is_empty() {
    registry.set_fallbacks(loaded_app_config.fallback_providers.clone());
    info!("Fallback provider chain configured: {:?}", loaded_app_config.fallback_providers);
}
```

If `fallback_providers` doesn't exist in the config type, add it:
- In the config type (search for where `bindings` is defined in config), add `pub fallback_providers: Vec<String>`
- Default to empty vec

- [ ] **Step 3: Use call_with_fallback() in the execution path**

In `src/agent_loop/provider_bridge.rs` or where the provider is called, check if fallbacks are available and use `call_with_fallback()`:

Read `src/thinker/fallback.rs` to understand the existing function signature, then find the call site in `provider_bridge.rs` where `self.provider.process(payload)` is called and wrap it:

The simplest approach: make `AiProviderBridge` aware of the registry (not just a single provider), so it can attempt fallbacks. However, since `call_with_fallback()` already exists in `thinker/fallback.rs`, the cleanest integration is at the ExecutionEngine level where the registry is available.

Read `src/gateway/execution_engine/run_loop.rs` to find exactly where the provider is selected (line ~64: `let provider = self.provider_registry.default_provider()`).

The integration point: instead of calling `default_provider()` once, use `call_with_fallback()` when the registry has fallbacks. But since the agent loop calls the provider many times (think → act cycles), the fallback should be per-call, not per-session. The existing `call_with_fallback()` does exactly this.

**Minimal wiring approach**: Modify `AiProviderBridge` to optionally hold the registry reference and fallback list. When `process()` fails with a transient error, retry with next provider from the list.

- [ ] **Step 4: Compile check**

Run: `cargo check -p alephcore`

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "provider: wire FailoverProvider fallback chain to production execution path"
```

---

### Task 2: Add OAuth token auto-refresh

**Files:**
- Modify: `src/providers/auth_profiles/credentials.rs` — add fields
- Create: `src/providers/oauth_refresh.rs` — refresh logic
- Modify: `src/providers/mod.rs` — export module
- Modify: `src/providers/auth_profile_registry.rs` — call refresh before use

- [ ] **Step 1: Add client_secret and token_endpoint to OAuthCredential**

In `credentials.rs`, add to `OAuthCredential` struct:

```rust
    /// OAuth client secret (stored in vault, not logged)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_secret: Option<String>,
    /// Token endpoint URL for refresh (e.g., https://oauth2.googleapis.com/token)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token_endpoint: Option<String>,
```

- [ ] **Step 2: Create oauth_refresh.rs**

```rust
//! OAuth token auto-refresh.
//!
//! Checks if an OAuth credential is near expiry and refreshes it
//! using the refresh_token grant.

use anyhow::Result;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tracing::{debug, info, warn};

use super::auth_profiles::credentials::OAuthCredential;

/// Default refresh margin: refresh 5 minutes before expiry.
const REFRESH_MARGIN_SECS: u64 = 300;

/// Google OAuth2 token endpoint.
const GOOGLE_TOKEN_ENDPOINT: &str = "https://oauth2.googleapis.com/token";

/// Check if an OAuth credential needs refresh.
pub fn needs_refresh(cred: &OAuthCredential) -> bool {
    let Some(expires_ms) = cred.expires else {
        return false; // No expiry set — don't refresh
    };
    // No refresh token — can't refresh
    if cred.refresh.is_none() {
        return false;
    }
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    let margin_ms = REFRESH_MARGIN_SECS * 1000;
    now_ms + margin_ms >= expires_ms
}

/// Refresh an OAuth credential. Returns updated credential on success.
pub async fn refresh_token(cred: &OAuthCredential) -> Result<OAuthCredential> {
    let refresh_token = cred.refresh.as_deref()
        .ok_or_else(|| anyhow::anyhow!("No refresh token available"))?;

    let endpoint = cred.token_endpoint.as_deref()
        .or_else(|| {
            // Auto-detect Google
            if cred.provider.contains("google") || cred.provider.contains("vertex") {
                Some(GOOGLE_TOKEN_ENDPOINT)
            } else {
                None
            }
        })
        .ok_or_else(|| anyhow::anyhow!("No token_endpoint configured for provider '{}'", cred.provider))?;

    let client = reqwest::Client::new();
    let mut form = vec![
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh_token),
    ];
    if let Some(ref client_id) = cred.client_id {
        form.push(("client_id", client_id.as_str()));
    }
    if let Some(ref client_secret) = cred.client_secret {
        form.push(("client_secret", client_secret.as_str()));
    }

    debug!("Refreshing OAuth token for provider '{}' at {}", cred.provider, endpoint);

    let resp = client.post(endpoint)
        .form(&form)
        .send()
        .await?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(anyhow::anyhow!("Token refresh failed: {} — {}", status, body));
    }

    let body: serde_json::Value = resp.json().await?;
    let new_access = body["access_token"].as_str()
        .ok_or_else(|| anyhow::anyhow!("No access_token in refresh response"))?;
    let expires_in = body["expires_in"].as_u64().unwrap_or(3600);
    let new_expires_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
        + expires_in * 1000;

    // Use new refresh token if provided, otherwise keep existing
    let new_refresh = body["refresh_token"].as_str()
        .map(String::from)
        .or_else(|| cred.refresh.clone());

    info!("OAuth token refreshed for provider '{}', expires in {}s", cred.provider, expires_in);

    Ok(OAuthCredential {
        provider: cred.provider.clone(),
        access: new_access.to_string(),
        refresh: new_refresh,
        expires: Some(new_expires_ms),
        client_id: cred.client_id.clone(),
        client_secret: cred.client_secret.clone(),
        token_endpoint: cred.token_endpoint.clone(),
        email: cred.email.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_oauth(expires_ms: Option<u64>, has_refresh: bool) -> OAuthCredential {
        OAuthCredential {
            provider: "test".to_string(),
            access: "access-token".to_string(),
            refresh: if has_refresh { Some("refresh-token".to_string()) } else { None },
            expires: expires_ms,
            client_id: None,
            client_secret: None,
            token_endpoint: None,
            email: None,
        }
    }

    #[test]
    fn test_no_expiry_no_refresh() {
        let cred = make_oauth(None, true);
        assert!(!needs_refresh(&cred));
    }

    #[test]
    fn test_no_refresh_token() {
        let cred = make_oauth(Some(1000), false);
        assert!(!needs_refresh(&cred));
    }

    #[test]
    fn test_expired_needs_refresh() {
        // Expired 1 hour ago
        let now_ms = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() as u64;
        let cred = make_oauth(Some(now_ms - 3_600_000), true);
        assert!(needs_refresh(&cred));
    }

    #[test]
    fn test_near_expiry_needs_refresh() {
        // Expires in 2 minutes (within 5-minute margin)
        let now_ms = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() as u64;
        let cred = make_oauth(Some(now_ms + 120_000), true);
        assert!(needs_refresh(&cred));
    }

    #[test]
    fn test_far_future_no_refresh() {
        // Expires in 1 hour (well outside 5-minute margin)
        let now_ms = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() as u64;
        let cred = make_oauth(Some(now_ms + 3_600_000), true);
        assert!(!needs_refresh(&cred));
    }
}
```

- [ ] **Step 3: Export module in providers/mod.rs**

Add `pub mod oauth_refresh;` to `src/providers/mod.rs`.

- [ ] **Step 4: Integrate into AuthProfileProviderRegistry**

In `auth_profile_registry.rs`, in the `default_provider()` method, before using the credential, check if it needs refresh:

```rust
// Before creating the provider from a credential, check OAuth refresh
if let AuthProfileCredential::OAuth(ref oauth_cred) = credential {
    if crate::providers::oauth_refresh::needs_refresh(oauth_cred) {
        match crate::providers::oauth_refresh::refresh_token(oauth_cred).await {
            Ok(refreshed) => {
                // Update the store with refreshed credential
                self.store.write().unwrap_or_else(|e| e.into_inner())
                    .upsert_profile(profile_id.clone(), AuthProfileCredential::OAuth(refreshed));
                // Rebuild the provider with new credential
                self.refresh_providers();
            }
            Err(e) => {
                warn!("OAuth refresh failed for {}: {}", profile_id, e);
                // Continue with potentially expired token — may still work
            }
        }
    }
}
```

Note: `default_provider()` is currently sync. If making it async is too invasive, add a `try_refresh_oauth()` method that is called externally before `default_provider()`.

- [ ] **Step 5: Compile and test**

Run: `cargo test -p alephcore --lib oauth_refresh -- --nocapture`
Run: `cargo check -p alephcore`

- [ ] **Step 6: Commit**

```bash
git add src/providers/
git commit -m "provider: add OAuth token auto-refresh with Google endpoint auto-detection"
```

---

### Task 3: Add ModelDiscovery trait and Ollama implementation

**Files:**
- Create: `src/providers/model_discovery.rs` — trait + cache
- Modify: `src/providers/ollama.rs` — implement ModelDiscovery
- Modify: `src/providers/mod.rs` — export
- Modify: `src/gateway/handlers/providers/handlers.rs` — merge into models.list

- [ ] **Step 1: Create model_discovery.rs**

```rust
//! Dynamic model discovery for providers that support runtime model listing.
//!
//! Providers like Ollama and LM Studio expose API endpoints to list
//! available models. This module defines a trait and caching layer.

use async_trait::async_trait;
use serde::Serialize;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// A model discovered at runtime from a provider's API.
#[derive(Debug, Clone, Serialize)]
pub struct DiscoveredModel {
    pub id: String,
    pub display_name: Option<String>,
    pub size_bytes: Option<u64>,
    pub modified_at: Option<String>,
}

/// Trait for providers that support runtime model listing.
#[async_trait]
pub trait ModelDiscovery: Send + Sync {
    /// Provider name for this discovery source.
    fn provider_name(&self) -> &str;

    /// Fetch available models from the provider's API.
    async fn discover_models(&self) -> anyhow::Result<Vec<DiscoveredModel>>;
}

/// Cached model discovery wrapper.
/// Caches results for a configurable duration to avoid frequent API calls.
pub struct CachedDiscovery {
    inner: Box<dyn ModelDiscovery>,
    cache: Mutex<Option<(Vec<DiscoveredModel>, Instant)>>,
    ttl: Duration,
}

impl CachedDiscovery {
    pub fn new(inner: Box<dyn ModelDiscovery>, ttl: Duration) -> Self {
        Self {
            inner,
            cache: Mutex::new(None),
            ttl,
        }
    }

    pub async fn discover(&self) -> anyhow::Result<Vec<DiscoveredModel>> {
        // Check cache
        {
            let cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
            if let Some((models, fetched_at)) = cache.as_ref() {
                if fetched_at.elapsed() < self.ttl {
                    return Ok(models.clone());
                }
            }
        }

        // Fetch fresh
        let models = self.inner.discover_models().await?;

        // Update cache
        {
            let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
            *cache = Some((models.clone(), Instant::now()));
        }

        Ok(models)
    }

    pub fn provider_name(&self) -> &str {
        self.inner.provider_name()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct MockDiscovery;

    #[async_trait]
    impl ModelDiscovery for MockDiscovery {
        fn provider_name(&self) -> &str { "mock" }
        async fn discover_models(&self) -> anyhow::Result<Vec<DiscoveredModel>> {
            Ok(vec![DiscoveredModel {
                id: "test-model".to_string(),
                display_name: Some("Test Model".to_string()),
                size_bytes: Some(1_000_000),
                modified_at: None,
            }])
        }
    }

    #[tokio::test]
    async fn test_cached_discovery() {
        let cached = CachedDiscovery::new(
            Box::new(MockDiscovery),
            Duration::from_secs(300),
        );

        let models = cached.discover().await.unwrap();
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].id, "test-model");

        // Second call should use cache
        let models2 = cached.discover().await.unwrap();
        assert_eq!(models2.len(), 1);
    }
}
```

- [ ] **Step 2: Implement ModelDiscovery for OllamaProvider**

In `src/providers/ollama.rs`, add the implementation. The `TagsResponse` struct already exists in tests — move it to non-test code and implement the trait:

```rust
use crate::providers::model_discovery::{ModelDiscovery, DiscoveredModel};

#[derive(Debug, Deserialize)]
struct TagsResponse {
    models: Vec<TagModel>,
}

#[derive(Debug, Deserialize)]
struct TagModel {
    name: String,
    size: Option<u64>,
    modified_at: Option<String>,
}

#[async_trait::async_trait]
impl ModelDiscovery for OllamaProvider {
    fn provider_name(&self) -> &str {
        &self.name
    }

    async fn discover_models(&self) -> anyhow::Result<Vec<DiscoveredModel>> {
        let url = format!("{}/api/tags", self.base_url);
        let resp: TagsResponse = self.client.get(&url)
            .send()
            .await?
            .json()
            .await?;

        Ok(resp.models.into_iter().map(|m| DiscoveredModel {
            id: m.name.clone(),
            display_name: Some(m.name),
            size_bytes: m.size,
            modified_at: m.modified_at,
        }).collect())
    }
}
```

Note: Check that `self.name` and `self.base_url` and `self.client` are accessible (they should be fields on OllamaProvider).

- [ ] **Step 3: Export module in providers/mod.rs**

Add `pub mod model_discovery;`

- [ ] **Step 4: Extend models.list handler to include discovered models**

In `src/gateway/handlers/providers/handlers.rs`, in `handle_list()`, after the static provider listing, check for Ollama providers and add discovered models:

This is optional for the initial implementation — the static list already shows configured models. Discovered models can be surfaced via a new `models.discover` RPC handler instead.

Add a simple new handler:

```rust
pub async fn handle_discover(
    request: JsonRpcRequest,
    config: &Config,
) -> JsonRpcResponse {
    // Find Ollama providers and discover their models
    let mut discovered = Vec::new();
    for (name, cfg) in &config.providers {
        if cfg.protocol() == "ollama" && cfg.enabled {
            if let Ok(provider) = OllamaProvider::new(name.clone(), cfg.clone()) {
                match provider.discover_models().await {
                    Ok(models) => {
                        for model in models {
                            discovered.push(serde_json::json!({
                                "provider": name,
                                "model": model.id,
                                "display_name": model.display_name,
                                "size_bytes": model.size_bytes,
                            }));
                        }
                    }
                    Err(e) => {
                        tracing::warn!("Model discovery failed for {}: {}", name, e);
                    }
                }
            }
        }
    }
    JsonRpcResponse::success(request.id, serde_json::json!({ "models": discovered }))
}
```

Register this handler in the HandlerRegistry.

- [ ] **Step 5: Compile and test**

Run: `cargo test -p alephcore --lib model_discovery -- --nocapture`
Run: `cargo check -p alephcore`

- [ ] **Step 6: Commit**

```bash
git add src/providers/
git commit -m "provider: add ModelDiscovery trait with Ollama implementation and cached wrapper"
```

---

### Task 4: Final validation

- [ ] **Step 1: Run full test suite**

Run: `cargo test -p alephcore --lib`
Expected: All tests pass (except pre-existing failures)

- [ ] **Step 2: Run clippy**

Run: `cargo clippy -p alephcore -- -W clippy::all`
Expected: No new warnings

- [ ] **Step 3: Final commit if needed**

```bash
git add -A && git commit -m "provider: fix clippy warnings in P3 changes"
```
