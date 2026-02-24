# Late-Binding Secure Execution — Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add Secret Provider Interface (SPI) with 1Password CLI backend, full async redesign of secret resolution, hybrid caching, and JIT authorization for high-sensitivity secrets.

**Architecture:** Introduce `AsyncSecretResolver` and `SecretProvider` traits, refactor existing `SecretVault` into a `LocalVaultProvider`, add `SecretRouter` as the unified entry point. All secret resolution becomes async-native. Config gains `[secret_providers]` and `[secrets]` sections.

**Tech Stack:** Rust, async-trait, tokio::process (for 1Password CLI), secrecy crate (memory safety), existing AES-256-GCM vault infrastructure.

**Design Doc:** `docs/plans/2026-02-24-late-binding-secure-execution-design.md`

---

## Wave 1: Foundation Traits + Provider Skeleton

### Task 1: Add new SecretError variants

**Files:**
- Modify: `core/src/secrets/types.rs:89-113` (SecretError enum)
- Test: `core/src/secrets/types.rs` (inline tests)

**Step 1: Write failing tests for new error variants**

Add to the `#[cfg(test)] mod tests` block at bottom of `core/src/secrets/types.rs`:

```rust
#[test]
fn test_provider_auth_required_error() {
    let err = SecretError::ProviderAuthRequired {
        provider: "1password".into(),
        message: "Session expired".into(),
    };
    assert!(format!("{}", err).contains("1password"));
    assert!(format!("{}", err).contains("authentication"));
}

#[test]
fn test_provider_error() {
    let err = SecretError::ProviderError {
        provider: "1password".into(),
        message: "item not found".into(),
    };
    assert!(format!("{}", err).contains("1password"));
}

#[test]
fn test_access_denied_error() {
    let err = SecretError::AccessDenied {
        name: "bank_password".into(),
        reason: "User denied".into(),
    };
    assert!(format!("{}", err).contains("bank_password"));
}

#[test]
fn test_provider_not_found_error() {
    let err = SecretError::ProviderNotFound {
        provider: "bitwarden".into(),
    };
    assert!(format!("{}", err).contains("bitwarden"));
}
```

**Step 2: Run tests to verify they fail**

Run: `cd /Volumes/TBU4/Workspace/Aleph/core && cargo test --lib secrets::types::tests -- --nocapture 2>&1 | tail -20`
Expected: Compilation failure — variants don't exist yet.

**Step 3: Add error variants to SecretError**

In `core/src/secrets/types.rs`, add these variants to the `SecretError` enum (after the existing `Serialization` variant):

```rust
    #[error("Provider '{provider}' requires authentication: {message}")]
    ProviderAuthRequired { provider: String, message: String },

    #[error("Provider '{provider}' error: {message}")]
    ProviderError { provider: String, message: String },

    #[error("Access denied for secret '{name}': {reason}")]
    AccessDenied { name: String, reason: String },

    #[error("Provider '{provider}' not configured")]
    ProviderNotFound { provider: String },
```

**Step 4: Run tests to verify they pass**

Run: `cd /Volumes/TBU4/Workspace/Aleph/core && cargo test --lib secrets::types::tests -- --nocapture`
Expected: All tests PASS.

**Step 5: Commit**

```bash
git add core/src/secrets/types.rs
git commit -m "secrets: add provider-related error variants to SecretError"
```

---

### Task 2: Create SecretProvider trait and provider types

**Files:**
- Create: `core/src/secrets/provider/mod.rs`
- Create: `core/src/secrets/provider/local_vault.rs`
- Modify: `core/src/secrets/mod.rs:1-21` (add module declaration)

**Step 1: Write failing test for SecretProvider trait**

Create `core/src/secrets/provider/mod.rs`:

```rust
//! Secret Provider Interface (SPI)
//!
//! Abstracts secret backends behind a unified async trait.
//! Implementations: LocalVaultProvider (built-in), OnePasswordProvider, etc.

pub mod local_vault;

use async_trait::async_trait;
use super::types::{DecryptedSecret, SecretError};

/// Provider health status.
#[derive(Debug, Clone, PartialEq)]
pub enum ProviderStatus {
    Ready,
    NeedsAuth { message: String },
    Unavailable { reason: String },
}

/// Non-sensitive metadata for a secret entry.
#[derive(Debug, Clone)]
pub struct SecretMetadata {
    pub name: String,
    pub provider: String,
    pub updated_at: Option<i64>,
}

/// Secret Provider Interface — the backend abstraction.
///
/// Each external secret manager (1Password, Bitwarden, local vault, etc.)
/// implements this trait.
#[async_trait]
pub trait SecretProvider: Send + Sync {
    /// Provider identifier (e.g., "local_vault", "1password").
    fn provider_type(&self) -> &str;

    /// Fetch a secret by its provider-specific reference.
    /// For local vault: the secret name ("anthropic_key").
    /// For 1Password: an op:// URI ("op://Personal/GitHub/token").
    async fn get(&self, reference: &str) -> Result<DecryptedSecret, SecretError>;

    /// Check if the provider is available and authenticated.
    async fn health_check(&self) -> Result<ProviderStatus, SecretError>;

    /// List available secret names (never values).
    async fn list(&self) -> Result<Vec<SecretMetadata>, SecretError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    struct MockProvider;

    #[async_trait]
    impl SecretProvider for MockProvider {
        fn provider_type(&self) -> &str { "mock" }
        async fn get(&self, reference: &str) -> Result<DecryptedSecret, SecretError> {
            if reference == "exists" {
                Ok(DecryptedSecret::new("mock-value"))
            } else {
                Err(SecretError::NotFound(reference.to_string()))
            }
        }
        async fn health_check(&self) -> Result<ProviderStatus, SecretError> {
            Ok(ProviderStatus::Ready)
        }
        async fn list(&self) -> Result<Vec<SecretMetadata>, SecretError> {
            Ok(vec![SecretMetadata {
                name: "exists".into(),
                provider: "mock".into(),
                updated_at: None,
            }])
        }
    }

    #[tokio::test]
    async fn test_mock_provider_get() {
        let p = MockProvider;
        let secret = p.get("exists").await.unwrap();
        assert_eq!(secret.expose(), "mock-value");
    }

    #[tokio::test]
    async fn test_mock_provider_not_found() {
        let p = MockProvider;
        let result = p.get("nope").await;
        assert!(matches!(result, Err(SecretError::NotFound(_))));
    }

    #[tokio::test]
    async fn test_mock_provider_health() {
        let p = MockProvider;
        let status = p.health_check().await.unwrap();
        assert_eq!(status, ProviderStatus::Ready);
    }

    #[tokio::test]
    async fn test_mock_provider_list() {
        let p = MockProvider;
        let items = p.list().await.unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].name, "exists");
    }
}
```

**Step 2: Run test to verify it compiles and passes**

Run: `cd /Volumes/TBU4/Workspace/Aleph/core && cargo test --lib secrets::provider::tests -- --nocapture`
Expected: May fail — module not registered in mod.rs yet.

**Step 3: Register provider module**

In `core/src/secrets/mod.rs`, add after line 13 (`pub mod web3_signer;`):

```rust
pub mod provider;
```

Also add to the pub use block:

```rust
pub use provider::{SecretProvider, ProviderStatus, SecretMetadata};
```

**Step 4: Run tests to verify they pass**

Run: `cd /Volumes/TBU4/Workspace/Aleph/core && cargo test --lib secrets::provider::tests -- --nocapture`
Expected: All 4 tests PASS.

**Step 5: Commit**

```bash
git add core/src/secrets/provider/mod.rs core/src/secrets/mod.rs
git commit -m "secrets: add SecretProvider trait and provider module"
```

---

### Task 3: Implement LocalVaultProvider

**Files:**
- Create: `core/src/secrets/provider/local_vault.rs`

**Step 1: Write failing test for LocalVaultProvider**

Create `core/src/secrets/provider/local_vault.rs`:

```rust
//! Local vault provider — wraps the existing SecretVault as a SecretProvider.

use async_trait::async_trait;
use super::{SecretProvider, ProviderStatus, SecretMetadata};
use crate::secrets::types::{DecryptedSecret, SecretError};
use crate::secrets::vault::SecretVault;
use std::sync::RwLock;

/// Wraps the existing file-based encrypted SecretVault as a SecretProvider.
pub struct LocalVaultProvider {
    vault: RwLock<SecretVault>,
}

impl LocalVaultProvider {
    /// Create a new LocalVaultProvider wrapping an existing SecretVault.
    pub fn new(vault: SecretVault) -> Self {
        Self {
            vault: RwLock::new(vault),
        }
    }
}

#[async_trait]
impl SecretProvider for LocalVaultProvider {
    fn provider_type(&self) -> &str {
        "local_vault"
    }

    async fn get(&self, reference: &str) -> Result<DecryptedSecret, SecretError> {
        let vault = self.vault.read().map_err(|e| {
            SecretError::ProviderError {
                provider: "local_vault".into(),
                message: format!("Lock poisoned: {}", e),
            }
        })?;
        vault.get(reference)
    }

    async fn health_check(&self) -> Result<ProviderStatus, SecretError> {
        // Local vault is always ready if it was opened successfully.
        Ok(ProviderStatus::Ready)
    }

    async fn list(&self) -> Result<Vec<SecretMetadata>, SecretError> {
        let vault = self.vault.read().map_err(|e| {
            SecretError::ProviderError {
                provider: "local_vault".into(),
                message: format!("Lock poisoned: {}", e),
            }
        })?;
        Ok(vault
            .list()
            .into_iter()
            .map(|(name, meta)| SecretMetadata {
                name,
                provider: "local_vault".into(),
                updated_at: meta.provider.as_ref().map(|_| 0), // simplified
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::secrets::types::EntryMetadata;
    use tempfile::TempDir;

    fn test_provider(dir: &TempDir) -> LocalVaultProvider {
        let path = dir.path().join("test.vault");
        let mut vault = SecretVault::open(path, "test-master-key").unwrap();
        vault.set("api_key", "sk-ant-secret", EntryMetadata::default()).unwrap();
        LocalVaultProvider::new(vault)
    }

    #[tokio::test]
    async fn test_local_vault_get() {
        let dir = TempDir::new().unwrap();
        let provider = test_provider(&dir);
        let secret = provider.get("api_key").await.unwrap();
        assert_eq!(secret.expose(), "sk-ant-secret");
    }

    #[tokio::test]
    async fn test_local_vault_get_not_found() {
        let dir = TempDir::new().unwrap();
        let provider = test_provider(&dir);
        let result = provider.get("nonexistent").await;
        assert!(matches!(result, Err(SecretError::NotFound(_))));
    }

    #[tokio::test]
    async fn test_local_vault_health() {
        let dir = TempDir::new().unwrap();
        let provider = test_provider(&dir);
        let status = provider.health_check().await.unwrap();
        assert_eq!(status, ProviderStatus::Ready);
    }

    #[tokio::test]
    async fn test_local_vault_list() {
        let dir = TempDir::new().unwrap();
        let provider = test_provider(&dir);
        let items = provider.list().await.unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].name, "api_key");
        assert_eq!(items[0].provider, "local_vault");
    }

    #[tokio::test]
    async fn test_local_vault_provider_type() {
        let dir = TempDir::new().unwrap();
        let provider = test_provider(&dir);
        assert_eq!(provider.provider_type(), "local_vault");
    }
}
```

**Step 2: Run tests**

Run: `cd /Volumes/TBU4/Workspace/Aleph/core && cargo test --lib secrets::provider::local_vault::tests -- --nocapture`
Expected: All 5 tests PASS.

**Step 3: Commit**

```bash
git add core/src/secrets/provider/local_vault.rs
git commit -m "secrets: implement LocalVaultProvider wrapping SecretVault"
```

---

## Wave 2: Router + Configuration

### Task 4: Add config types for secret providers and mappings

**Files:**
- Create: `core/src/config/types/secrets.rs`
- Modify: `core/src/config/types/mod.rs:20-58` (add module + re-export)
- Modify: `core/src/config/structs.rs:16-95` (add fields to Config)

**Step 1: Create config types**

Create `core/src/config/types/secrets.rs`:

```rust
//! Secret provider and mapping configuration types.
//!
//! Defines [secret_providers] and [secrets] sections in config.toml.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Configuration for a secret provider backend.
///
/// Example in config.toml:
/// ```toml
/// [secret_providers.op]
/// type = "1password"
/// account = "my.1password.com"
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SecretProviderConfig {
    /// Provider type: "local_vault", "1password", "bitwarden"
    #[serde(rename = "type")]
    pub provider_type: String,
    /// Account identifier (1Password: account domain)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
    /// Environment variable name holding service account token
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service_account_token_env: Option<String>,
}

/// Sensitivity level for a secret.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Sensitivity {
    /// Cached with TTL, no extra approval needed.
    Standard,
    /// Never cached, requires JIT approval before each use.
    High,
}

impl Default for Sensitivity {
    fn default() -> Self {
        Sensitivity::Standard
    }
}

/// Mapping from a logical secret name to a provider-specific reference.
///
/// Example in config.toml:
/// ```toml
/// [secrets.github_token]
/// provider = "op"
/// ref = "op://Personal/GitHub/token"
/// sensitivity = "standard"
/// ttl = 3600
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SecretMapping {
    /// Which secret_providers entry to use.
    pub provider: String,
    /// Provider-specific reference URI.
    /// For local_vault: omit (uses the secret name as key).
    /// For 1Password: "op://Vault/Item/Field".
    #[serde(default, rename = "ref", skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
    /// Sensitivity level.
    #[serde(default)]
    pub sensitivity: Sensitivity,
    /// Cache TTL in seconds (only for Standard sensitivity).
    #[serde(default = "default_ttl")]
    pub ttl: u64,
}

fn default_ttl() -> u64 {
    3600
}

/// Top-level secrets configuration section.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct SecretsConfig {
    /// Default provider key (falls back to "local" if not set).
    #[serde(default = "default_provider")]
    pub default_provider: String,
}

fn default_provider() -> String {
    "local".into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sensitivity_default() {
        assert_eq!(Sensitivity::default(), Sensitivity::Standard);
    }

    #[test]
    fn test_sensitivity_serde_roundtrip() {
        let high: Sensitivity = serde_json::from_str("\"high\"").unwrap();
        assert_eq!(high, Sensitivity::High);
        let standard: Sensitivity = serde_json::from_str("\"standard\"").unwrap();
        assert_eq!(standard, Sensitivity::Standard);
    }

    #[test]
    fn test_secret_mapping_defaults() {
        let json = r#"{"provider": "local"}"#;
        let mapping: SecretMapping = serde_json::from_str(json).unwrap();
        assert_eq!(mapping.provider, "local");
        assert_eq!(mapping.sensitivity, Sensitivity::Standard);
        assert_eq!(mapping.ttl, 3600);
        assert!(mapping.reference.is_none());
    }

    #[test]
    fn test_secret_provider_config_serde() {
        let toml_str = r#"
            type = "1password"
            account = "my.1password.com"
        "#;
        let config: SecretProviderConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.provider_type, "1password");
        assert_eq!(config.account.as_deref(), Some("my.1password.com"));
    }

    #[test]
    fn test_secrets_config_default() {
        let config = SecretsConfig::default();
        assert_eq!(config.default_provider, "local");
    }
}
```

**Step 2: Register module in config/types/mod.rs**

In `core/src/config/types/mod.rs`, add after line 36 (`pub mod video;`):

```rust
pub mod secrets;
```

And add to the re-exports after line 57 (`pub use video::*;`):

```rust
pub use secrets::*;
```

**Step 3: Add fields to Config struct**

In `core/src/config/structs.rs`, add these fields to the `Config` struct (after the `profiles` field, before the closing `}`):

```rust
    /// Secret provider backend configurations
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub secret_providers: HashMap<String, SecretProviderConfig>,
    /// Secret name-to-provider mappings
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub secrets: HashMap<String, SecretMapping>,
    /// Secrets subsystem settings
    #[serde(default)]
    pub secrets_config: SecretsConfig,
```

**Step 4: Run tests**

Run: `cd /Volumes/TBU4/Workspace/Aleph/core && cargo test --lib config::types::secrets::tests -- --nocapture`
Expected: All 5 tests PASS.

Also verify the full build compiles:
Run: `cd /Volumes/TBU4/Workspace/Aleph/core && cargo check 2>&1 | tail -5`
Expected: No errors.

**Step 5: Commit**

```bash
git add core/src/config/types/secrets.rs core/src/config/types/mod.rs core/src/config/structs.rs
git commit -m "config: add secret_providers and secrets mapping configuration types"
```

---

### Task 5: Implement TTL cache

**Files:**
- Create: `core/src/secrets/cache.rs`
- Modify: `core/src/secrets/mod.rs` (add module)

**Step 1: Create cache module with tests**

Create `core/src/secrets/cache.rs`:

```rust
//! TTL-based in-memory cache for resolved secrets.
//!
//! Standard-sensitivity secrets are cached to avoid repeated CLI calls.
//! High-sensitivity secrets bypass the cache entirely.

use std::collections::HashMap;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

use super::types::DecryptedSecret;

/// A cached secret value with TTL tracking.
struct CachedEntry {
    value: DecryptedSecret,
    fetched_at: Instant,
    ttl: Duration,
}

impl CachedEntry {
    fn is_expired(&self) -> bool {
        self.fetched_at.elapsed() > self.ttl
    }
}

/// Thread-safe TTL cache for secret values.
pub struct SecretCache {
    entries: RwLock<HashMap<String, CachedEntry>>,
}

impl SecretCache {
    pub fn new() -> Self {
        Self {
            entries: RwLock::new(HashMap::new()),
        }
    }

    /// Get a cached secret if it exists and hasn't expired.
    /// Returns None on cache miss or expiration.
    pub async fn get(&self, name: &str) -> Option<DecryptedSecret> {
        let entries = self.entries.read().await;
        entries.get(name).and_then(|entry| {
            if entry.is_expired() {
                None
            } else {
                Some(DecryptedSecret::new(entry.value.expose().to_string()))
            }
        })
    }

    /// Store a secret in the cache with the given TTL.
    pub async fn put(&self, name: String, value: DecryptedSecret, ttl: Duration) {
        let mut entries = self.entries.write().await;
        entries.insert(
            name,
            CachedEntry {
                value,
                fetched_at: Instant::now(),
                ttl,
            },
        );
    }

    /// Remove a specific entry from the cache.
    pub async fn invalidate(&self, name: &str) {
        let mut entries = self.entries.write().await;
        entries.remove(name);
    }

    /// Remove all expired entries.
    pub async fn evict_expired(&self) {
        let mut entries = self.entries.write().await;
        entries.retain(|_, entry| !entry.is_expired());
    }

    /// Clear the entire cache.
    pub async fn clear(&self) {
        let mut entries = self.entries.write().await;
        entries.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_cache_put_and_get() {
        let cache = SecretCache::new();
        cache
            .put(
                "key".into(),
                DecryptedSecret::new("value"),
                Duration::from_secs(60),
            )
            .await;
        let result = cache.get("key").await;
        assert!(result.is_some());
        assert_eq!(result.unwrap().expose(), "value");
    }

    #[tokio::test]
    async fn test_cache_miss() {
        let cache = SecretCache::new();
        let result = cache.get("nonexistent").await;
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_cache_expiration() {
        let cache = SecretCache::new();
        cache
            .put(
                "key".into(),
                DecryptedSecret::new("value"),
                Duration::from_millis(1),
            )
            .await;
        // Wait for expiration
        tokio::time::sleep(Duration::from_millis(10)).await;
        let result = cache.get("key").await;
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_cache_invalidate() {
        let cache = SecretCache::new();
        cache
            .put(
                "key".into(),
                DecryptedSecret::new("value"),
                Duration::from_secs(60),
            )
            .await;
        cache.invalidate("key").await;
        let result = cache.get("key").await;
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_cache_clear() {
        let cache = SecretCache::new();
        cache
            .put(
                "k1".into(),
                DecryptedSecret::new("v1"),
                Duration::from_secs(60),
            )
            .await;
        cache
            .put(
                "k2".into(),
                DecryptedSecret::new("v2"),
                Duration::from_secs(60),
            )
            .await;
        cache.clear().await;
        assert!(cache.get("k1").await.is_none());
        assert!(cache.get("k2").await.is_none());
    }

    #[tokio::test]
    async fn test_evict_expired() {
        let cache = SecretCache::new();
        cache
            .put(
                "short".into(),
                DecryptedSecret::new("v1"),
                Duration::from_millis(1),
            )
            .await;
        cache
            .put(
                "long".into(),
                DecryptedSecret::new("v2"),
                Duration::from_secs(60),
            )
            .await;
        tokio::time::sleep(Duration::from_millis(10)).await;
        cache.evict_expired().await;
        assert!(cache.get("short").await.is_none());
        assert!(cache.get("long").await.is_some());
    }
}
```

**Step 2: Register module**

In `core/src/secrets/mod.rs`, add:

```rust
pub mod cache;
```

**Step 3: Run tests**

Run: `cd /Volumes/TBU4/Workspace/Aleph/core && cargo test --lib secrets::cache::tests -- --nocapture`
Expected: All 6 tests PASS.

**Step 4: Commit**

```bash
git add core/src/secrets/cache.rs core/src/secrets/mod.rs
git commit -m "secrets: add TTL-based in-memory secret cache"
```

---

### Task 6: Implement SecretRouter

**Files:**
- Create: `core/src/secrets/router.rs`
- Modify: `core/src/secrets/mod.rs` (add module + re-export)

**Step 1: Create SecretRouter with tests**

Create `core/src/secrets/router.rs`:

```rust
//! Secret Router — routes secret resolution to the correct provider.
//!
//! Implements AsyncSecretResolver as the single entry point for all consumers.
//! Handles TTL caching for standard secrets, JIT approval for high secrets.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tracing::{debug, warn};

use super::cache::SecretCache;
use super::provider::SecretProvider;
use super::types::{DecryptedSecret, SecretError};
use crate::config::types::secrets::{SecretMapping, Sensitivity};

/// Async trait for resolving secret names to decrypted values.
/// Replaces the synchronous SecretResolver.
#[async_trait]
pub trait AsyncSecretResolver: Send + Sync {
    async fn resolve(&self, name: &str) -> Result<DecryptedSecret, SecretError>;
}

/// Routes secret requests to the correct provider based on config mapping.
pub struct SecretRouter {
    /// name -> mapping (from config)
    mappings: HashMap<String, SecretMapping>,
    /// provider_key -> provider impl
    providers: HashMap<String, Arc<dyn SecretProvider>>,
    /// TTL cache for standard-sensitivity secrets
    cache: SecretCache,
    /// Default provider key for unmapped secrets
    default_provider: String,
}

impl SecretRouter {
    pub fn new(
        mappings: HashMap<String, SecretMapping>,
        providers: HashMap<String, Arc<dyn SecretProvider>>,
        default_provider: String,
    ) -> Self {
        Self {
            mappings,
            providers,
            cache: SecretCache::new(),
            default_provider,
        }
    }

    /// Get a provider by key.
    fn get_provider(&self, key: &str) -> Result<&Arc<dyn SecretProvider>, SecretError> {
        self.providers.get(key).ok_or_else(|| SecretError::ProviderNotFound {
            provider: key.to_string(),
        })
    }
}

#[async_trait]
impl AsyncSecretResolver for SecretRouter {
    async fn resolve(&self, name: &str) -> Result<DecryptedSecret, SecretError> {
        // 1. Look up mapping
        if let Some(mapping) = self.mappings.get(name) {
            let provider = self.get_provider(&mapping.provider)?;
            let reference = mapping
                .reference
                .as_deref()
                .unwrap_or(name);

            match mapping.sensitivity {
                Sensitivity::Standard => {
                    // Check cache first
                    if let Some(cached) = self.cache.get(name).await {
                        debug!(name = name, "Secret resolved from cache");
                        return Ok(cached);
                    }
                    // Fetch from provider
                    let secret = provider.get(reference).await?;
                    // Cache the result
                    self.cache
                        .put(
                            name.to_string(),
                            DecryptedSecret::new(secret.expose().to_string()),
                            Duration::from_secs(mapping.ttl),
                        )
                        .await;
                    debug!(name = name, provider = mapping.provider.as_str(), "Secret resolved from provider");
                    Ok(secret)
                }
                Sensitivity::High => {
                    // High sensitivity: no cache, always fetch fresh
                    // JIT approval would be injected here (see Task 10)
                    debug!(name = name, "High-sensitivity secret: fetching fresh");
                    provider.get(reference).await
                }
            }
        } else {
            // No mapping: fallback to default provider using name as reference
            let provider = self.get_provider(&self.default_provider)?;
            debug!(name = name, provider = self.default_provider.as_str(), "Secret resolved via default provider");
            provider.get(name).await
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::secrets::provider::{ProviderStatus, SecretMetadata};

    struct InMemoryProvider {
        secrets: HashMap<String, String>,
    }

    impl InMemoryProvider {
        fn new(secrets: Vec<(&str, &str)>) -> Self {
            Self {
                secrets: secrets.into_iter().map(|(k, v)| (k.into(), v.into())).collect(),
            }
        }
    }

    #[async_trait]
    impl SecretProvider for InMemoryProvider {
        fn provider_type(&self) -> &str { "in_memory" }
        async fn get(&self, reference: &str) -> Result<DecryptedSecret, SecretError> {
            self.secrets
                .get(reference)
                .map(|v| DecryptedSecret::new(v.clone()))
                .ok_or_else(|| SecretError::NotFound(reference.to_string()))
        }
        async fn health_check(&self) -> Result<ProviderStatus, SecretError> {
            Ok(ProviderStatus::Ready)
        }
        async fn list(&self) -> Result<Vec<SecretMetadata>, SecretError> {
            Ok(vec![])
        }
    }

    fn test_router() -> SecretRouter {
        let local = Arc::new(InMemoryProvider::new(vec![
            ("local_key", "local-value"),
        ])) as Arc<dyn SecretProvider>;

        let external = Arc::new(InMemoryProvider::new(vec![
            ("op://Vault/Item/field", "external-value"),
        ])) as Arc<dyn SecretProvider>;

        let mut providers: HashMap<String, Arc<dyn SecretProvider>> = HashMap::new();
        providers.insert("local".into(), local);
        providers.insert("op".into(), external);

        let mut mappings = HashMap::new();
        mappings.insert("my_key".into(), SecretMapping {
            provider: "local".into(),
            reference: Some("local_key".into()),
            sensitivity: Sensitivity::Standard,
            ttl: 3600,
        });
        mappings.insert("ext_key".into(), SecretMapping {
            provider: "op".into(),
            reference: Some("op://Vault/Item/field".into()),
            sensitivity: Sensitivity::Standard,
            ttl: 60,
        });
        mappings.insert("high_key".into(), SecretMapping {
            provider: "local".into(),
            reference: Some("local_key".into()),
            sensitivity: Sensitivity::High,
            ttl: 0,
        });

        SecretRouter::new(mappings, providers, "local".into())
    }

    #[tokio::test]
    async fn test_resolve_mapped_local() {
        let router = test_router();
        let secret = router.resolve("my_key").await.unwrap();
        assert_eq!(secret.expose(), "local-value");
    }

    #[tokio::test]
    async fn test_resolve_mapped_external() {
        let router = test_router();
        let secret = router.resolve("ext_key").await.unwrap();
        assert_eq!(secret.expose(), "external-value");
    }

    #[tokio::test]
    async fn test_resolve_unmapped_falls_back_to_default() {
        let router = test_router();
        let secret = router.resolve("local_key").await.unwrap();
        assert_eq!(secret.expose(), "local-value");
    }

    #[tokio::test]
    async fn test_resolve_unmapped_not_found() {
        let router = test_router();
        let result = router.resolve("nonexistent").await;
        assert!(matches!(result, Err(SecretError::NotFound(_))));
    }

    #[tokio::test]
    async fn test_resolve_unknown_provider() {
        let mut mappings = HashMap::new();
        mappings.insert("bad".into(), SecretMapping {
            provider: "nonexistent_provider".into(),
            reference: None,
            sensitivity: Sensitivity::Standard,
            ttl: 60,
        });
        let router = SecretRouter::new(mappings, HashMap::new(), "local".into());
        let result = router.resolve("bad").await;
        assert!(matches!(result, Err(SecretError::ProviderNotFound { .. })));
    }

    #[tokio::test]
    async fn test_standard_caching() {
        let router = test_router();
        // First call: from provider
        let s1 = router.resolve("my_key").await.unwrap();
        assert_eq!(s1.expose(), "local-value");
        // Second call: should come from cache (same result)
        let s2 = router.resolve("my_key").await.unwrap();
        assert_eq!(s2.expose(), "local-value");
    }

    #[tokio::test]
    async fn test_high_sensitivity_no_cache() {
        let router = test_router();
        // High sensitivity always fetches fresh
        let s1 = router.resolve("high_key").await.unwrap();
        assert_eq!(s1.expose(), "local-value");
        // Verify cache is not populated for high sensitivity
        let cached = router.cache.get("high_key").await;
        assert!(cached.is_none());
    }
}
```

**Step 2: Register module and re-export**

In `core/src/secrets/mod.rs`, add:

```rust
pub mod router;
```

And add to re-exports:

```rust
pub use router::{AsyncSecretResolver, SecretRouter};
```

**Step 3: Run tests**

Run: `cd /Volumes/TBU4/Workspace/Aleph/core && cargo test --lib secrets::router::tests -- --nocapture`
Expected: All 7 tests PASS.

**Step 4: Commit**

```bash
git add core/src/secrets/router.rs core/src/secrets/mod.rs
git commit -m "secrets: implement SecretRouter with async resolution and TTL caching"
```

---

## Wave 3: Async Migration

### Task 7: Migrate injection.rs to async

**Files:**
- Modify: `core/src/secrets/injection.rs` (full file)
- Modify: `core/src/secrets/mod.rs` (update re-exports)

**Step 1: Convert render_with_secrets to async**

Replace the sync `SecretResolver` trait usage in `core/src/secrets/injection.rs` with `AsyncSecretResolver` from the router module. The full updated file:

1. Replace the `SecretResolver` trait definition (lines 13-15) — **delete it** (now lives in `router.rs` as `AsyncSecretResolver`)
2. Change `render_with_secrets` signature (line 45-67) to:

```rust
/// Render a string by replacing all `{{secret:NAME}}` placeholders.
///
/// Returns the rendered string and a list of injected secrets
/// (with hashes, never plaintext) for downstream leak detection.
pub async fn render_with_secrets(
    input: &str,
    resolver: &dyn AsyncSecretResolver,
) -> Result<(String, Vec<InjectedSecret>), SecretError> {
    let refs = extract_secret_refs(input)?;

    if refs.is_empty() {
        return Ok((input.to_string(), vec![]));
    }

    let mut result = input.to_string();
    let mut injected = Vec::with_capacity(refs.len());

    for secret_ref in &refs {
        let decrypted = resolver.resolve(&secret_ref.name).await?;
        let value = decrypted.expose();

        injected.push(InjectedSecret::from_value(&secret_ref.name, value));
        result = result.replace(&secret_ref.raw, value);
    }

    Ok((result, injected))
}
```

3. Add import at top: `use super::router::AsyncSecretResolver;`
4. Remove the import of `SecretResolver` from the old location
5. Update tests to use `#[tokio::test]` and async `MockResolver` (implementing `AsyncSecretResolver` instead of `SecretResolver`)

**Step 2: Update mod.rs re-exports**

In `core/src/secrets/mod.rs`, change:
- Remove: `pub use injection::{render_with_secrets, InjectedSecret, SecretResolver};`
- Add: `pub use injection::{render_with_secrets, InjectedSecret};`
(AsyncSecretResolver is already re-exported from router)

**Step 3: Update vault.rs SecretResolver impl**

In `core/src/secrets/vault.rs`, replace the `impl SecretResolver for SecretVault` block (lines 159-163) with an `AsyncSecretResolver` impl:

```rust
#[async_trait::async_trait]
impl super::router::AsyncSecretResolver for SecretVault {
    async fn resolve(&self, name: &str) -> Result<super::types::DecryptedSecret, super::types::SecretError> {
        self.get(name)
    }
}
```

**Step 4: Run all secrets tests**

Run: `cd /Volumes/TBU4/Workspace/Aleph/core && cargo test --lib secrets -- --nocapture`
Expected: All tests PASS.

**Step 5: Commit**

```bash
git add core/src/secrets/injection.rs core/src/secrets/vault.rs core/src/secrets/mod.rs
git commit -m "secrets: migrate render_with_secrets and SecretResolver to async"
```

---

### Task 8: Migrate resolve_provider_secrets to async + use SecretRouter

**Files:**
- Modify: `core/src/secrets/vault.rs:170-204` (resolve_provider_secrets function)
- Modify: `core/src/bin/aleph_server/commands/start/mod.rs:551-593` (load_app_config caller)

**Step 1: Make resolve_provider_secrets async**

In `core/src/secrets/vault.rs`, change `resolve_provider_secrets` (lines 170-204) to:

```rust
/// Resolve secret_name references in provider configs using a SecretRouter.
///
/// For each provider with a secret_name, resolves the secret via the router
/// and populates the in-memory api_key field. The config.toml file is
/// never modified — this is a runtime-only operation.
pub async fn resolve_provider_secrets(
    config: &mut Config,
    resolver: &dyn super::router::AsyncSecretResolver,
) -> Result<(), SecretError> {
    for (name, provider) in config.providers.iter_mut() {
        if let Some(ref secret_name) = provider.secret_name {
            if provider.api_key.is_none() {
                match resolver.resolve(secret_name).await {
                    Ok(secret) => {
                        provider.api_key = Some(secret.expose().to_string());
                        debug!(
                            provider = %name,
                            secret_name = %secret_name,
                            "Resolved API key from secret provider"
                        );
                    }
                    Err(SecretError::NotFound(ref s)) => {
                        warn!(
                            provider = %name,
                            secret_name = %s,
                            "Secret not found, provider will fail on use"
                        );
                    }
                    Err(e) => return Err(e),
                }
            }
        }
    }
    Ok(())
}
```

**Step 2: Update the test for resolve_provider_secrets**

Update the test in `vault.rs` (lines 389-414) to use async and AsyncSecretResolver.

**Step 3: Update server startup caller**

In `core/src/bin/aleph_server/commands/start/mod.rs`, change `load_app_config()` (line 553) from sync to async:

```rust
async fn load_app_config() -> alephcore::Config {
```

Build the `SecretRouter` from config before resolving:

```rust
    // After vault is opened successfully:
    // Build SecretRouter with LocalVaultProvider as default
    use alephcore::secrets::provider::local_vault::LocalVaultProvider;
    use alephcore::secrets::router::SecretRouter;
    use alephcore::secrets::SecretProvider;
    use std::sync::Arc;

    let local_provider = Arc::new(LocalVaultProvider::new(vault))
        as Arc<dyn SecretProvider>;
    let mut providers = std::collections::HashMap::new();
    providers.insert("local".into(), local_provider);

    // TODO: In future, initialize external providers from config.secret_providers here

    let router = SecretRouter::new(
        config.secrets.clone(),
        providers,
        config.secrets_config.default_provider.clone(),
    );

    if let Err(e) = alephcore::secrets::vault::resolve_provider_secrets(&mut config, &router).await {
        warn!(error = %e, "Failed to resolve provider secrets");
    }
```

Update the call site at line 807 to `.await` the async function.

**Step 4: Run full build**

Run: `cd /Volumes/TBU4/Workspace/Aleph/core && cargo check 2>&1 | tail -20`
Expected: No errors (may need to fix other callers that emerged).

**Step 5: Run tests**

Run: `cd /Volumes/TBU4/Workspace/Aleph/core && cargo test --lib secrets -- --nocapture`
Expected: All tests PASS.

**Step 6: Commit**

```bash
git add core/src/secrets/vault.rs core/src/bin/aleph_server/commands/start/mod.rs
git commit -m "secrets: migrate resolve_provider_secrets to async with SecretRouter"
```

---

### Task 9: Update remaining SecretVault direct callers

**Files:**
- Modify: `core/src/gateway/handlers/providers.rs:191,233` (vault construction)
- Modify: `core/src/gateway/handlers/generation_providers.rs:72,141` (vault construction)

These files construct `SecretVault` directly for provider CRUD operations. They should continue to work because `SecretVault` is still the underlying storage for the local provider. However, verify they still compile and work.

**Step 1: Run full build**

Run: `cd /Volumes/TBU4/Workspace/Aleph/core && cargo build 2>&1 | tail -20`

Fix any remaining compilation errors from the sync→async migration. The gateway handlers that directly use `SecretVault::open()` for write operations (set/delete secrets) should remain unchanged — they are CRUD operations on the local vault, not secret resolution.

**Step 2: Run full test suite**

Run: `cd /Volumes/TBU4/Workspace/Aleph/core && cargo test 2>&1 | tail -30`
Expected: All tests PASS.

**Step 3: Commit (if changes needed)**

```bash
git add -A
git commit -m "secrets: fix remaining callers after async migration"
```

---

## Wave 4: 1Password Integration

### Task 10: Implement OnePasswordProvider

**Files:**
- Create: `core/src/secrets/provider/onepassword.rs`
- Modify: `core/src/secrets/provider/mod.rs` (add module)

**Step 1: Create OnePasswordProvider with tests**

Create `core/src/secrets/provider/onepassword.rs`:

```rust
//! 1Password CLI (`op`) secret provider.
//!
//! Uses `op read` to fetch secrets by their op:// URI references.
//! Requires 1Password CLI v2+ to be installed and authenticated.

use async_trait::async_trait;
use tracing::{debug, warn};

use super::{ProviderStatus, SecretMetadata, SecretProvider};
use crate::secrets::types::{DecryptedSecret, SecretError};

/// 1Password CLI-based secret provider.
pub struct OnePasswordProvider {
    account: Option<String>,
    service_account_token: Option<String>,
}

impl OnePasswordProvider {
    /// Create a new 1Password provider.
    ///
    /// - `account`: Optional account domain (for multi-account setups).
    /// - `service_account_token`: Optional token for headless/CI environments.
    pub fn new(account: Option<String>, service_account_token: Option<String>) -> Self {
        Self {
            account,
            service_account_token,
        }
    }

    /// Build the base `op` command with common arguments.
    fn base_command(&self) -> tokio::process::Command {
        let mut cmd = tokio::process::Command::new("op");
        if let Some(ref account) = self.account {
            cmd.arg("--account").arg(account);
        }
        if let Some(ref token) = self.service_account_token {
            cmd.env("OP_SERVICE_ACCOUNT_TOKEN", token);
        }
        cmd
    }

    /// Parse stderr from `op` CLI to determine error type.
    fn classify_error(stderr: &str) -> SecretError {
        if stderr.contains("not signed in")
            || stderr.contains("session expired")
            || stderr.contains("authorization prompt")
            || stderr.contains("sign in")
        {
            SecretError::ProviderAuthRequired {
                provider: "1password".into(),
                message: format!(
                    "1Password session expired or not signed in. Run `op signin`. Details: {}",
                    stderr.trim()
                ),
            }
        } else if stderr.contains("not found")
            || stderr.contains("doesn't exist")
            || stderr.contains("no item")
        {
            SecretError::NotFound(stderr.trim().to_string())
        } else {
            SecretError::ProviderError {
                provider: "1password".into(),
                message: stderr.trim().to_string(),
            }
        }
    }
}

#[async_trait]
impl SecretProvider for OnePasswordProvider {
    fn provider_type(&self) -> &str {
        "1password"
    }

    async fn get(&self, reference: &str) -> Result<DecryptedSecret, SecretError> {
        let mut cmd = self.base_command();
        cmd.arg("read").arg(reference).arg("--no-newline");

        debug!(reference = reference, "Fetching secret from 1Password");

        let output = cmd.output().await.map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                SecretError::ProviderError {
                    provider: "1password".into(),
                    message: "1Password CLI (`op`) not found. Install from https://1password.com/downloads/command-line/".into(),
                }
            } else {
                SecretError::ProviderError {
                    provider: "1password".into(),
                    message: format!("Failed to execute `op`: {}", e),
                }
            }
        })?;

        if output.status.success() {
            let value = String::from_utf8_lossy(&output.stdout).into_owned();
            Ok(DecryptedSecret::new(value))
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            Err(Self::classify_error(&stderr))
        }
    }

    async fn health_check(&self) -> Result<ProviderStatus, SecretError> {
        let mut cmd = self.base_command();
        cmd.arg("whoami");

        let output = cmd.output().await.map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                SecretError::ProviderError {
                    provider: "1password".into(),
                    message: "1Password CLI (`op`) not found".into(),
                }
            } else {
                SecretError::ProviderError {
                    provider: "1password".into(),
                    message: format!("Failed to execute `op whoami`: {}", e),
                }
            }
        })?;

        if output.status.success() {
            Ok(ProviderStatus::Ready)
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            Ok(ProviderStatus::NeedsAuth {
                message: format!("Run `op signin` to authenticate. Error: {}", stderr.trim()),
            })
        }
    }

    async fn list(&self) -> Result<Vec<SecretMetadata>, SecretError> {
        let mut cmd = self.base_command();
        cmd.arg("item").arg("list").arg("--format=json");

        let output = cmd.output().await.map_err(|e| SecretError::ProviderError {
            provider: "1password".into(),
            message: format!("Failed to execute `op item list`: {}", e),
        })?;

        if output.status.success() {
            // Parse JSON array of items
            let stdout = String::from_utf8_lossy(&output.stdout);
            let items: Vec<serde_json::Value> =
                serde_json::from_str(&stdout).unwrap_or_default();

            Ok(items
                .iter()
                .filter_map(|item| {
                    let name = item.get("title")?.as_str()?.to_string();
                    let updated = item
                        .get("updated_at")
                        .and_then(|v| v.as_str())
                        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                        .map(|dt| dt.timestamp());
                    Some(SecretMetadata {
                        name,
                        provider: "1password".into(),
                        updated_at: updated,
                    })
                })
                .collect())
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            Err(Self::classify_error(&stderr))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_classify_error_auth() {
        let err = OnePasswordProvider::classify_error("You are not signed in");
        assert!(matches!(err, SecretError::ProviderAuthRequired { .. }));
    }

    #[test]
    fn test_classify_error_not_found() {
        let err = OnePasswordProvider::classify_error("item not found in vault");
        assert!(matches!(err, SecretError::NotFound(_)));
    }

    #[test]
    fn test_classify_error_generic() {
        let err = OnePasswordProvider::classify_error("some random error");
        assert!(matches!(err, SecretError::ProviderError { .. }));
    }

    #[test]
    fn test_provider_type() {
        let provider = OnePasswordProvider::new(None, None);
        assert_eq!(provider.provider_type(), "1password");
    }

    // NOTE: Integration tests with actual `op` CLI require 1Password to be
    // installed and authenticated. These should be run manually or in CI
    // with a service account. Mark with #[ignore] for standard test runs.

    #[tokio::test]
    #[ignore]
    async fn test_health_check_live() {
        let provider = OnePasswordProvider::new(None, None);
        let status = provider.health_check().await.unwrap();
        println!("1Password status: {:?}", status);
    }
}
```

**Step 2: Register module**

In `core/src/secrets/provider/mod.rs`, add after `pub mod local_vault;`:

```rust
pub mod onepassword;
```

**Step 3: Run tests**

Run: `cd /Volumes/TBU4/Workspace/Aleph/core && cargo test --lib secrets::provider::onepassword::tests -- --nocapture`
Expected: 4 sync tests PASS, 1 ignored (live test).

**Step 4: Commit**

```bash
git add core/src/secrets/provider/onepassword.rs core/src/secrets/provider/mod.rs
git commit -m "secrets: implement OnePasswordProvider using op CLI"
```

---

### Task 11: Wire up provider initialization in server startup

**Files:**
- Modify: `core/src/bin/aleph_server/commands/start/mod.rs:551-593` (load_app_config)

**Step 1: Update load_app_config to build providers from config**

In the `load_app_config` function, after opening the vault, build the full provider set from `config.secret_providers`:

```rust
    // Build providers from config
    use alephcore::secrets::provider::local_vault::LocalVaultProvider;
    use alephcore::secrets::provider::onepassword::OnePasswordProvider;
    use alephcore::secrets::provider::SecretProvider;
    use alephcore::secrets::router::SecretRouter;

    let mut providers: HashMap<String, Arc<dyn SecretProvider>> = HashMap::new();

    // Always register local vault as "local"
    providers.insert(
        "local".into(),
        Arc::new(LocalVaultProvider::new(vault)) as Arc<dyn SecretProvider>,
    );

    // Register providers from config
    for (key, provider_config) in &config.secret_providers {
        match provider_config.provider_type.as_str() {
            "local_vault" => {
                // "local" already registered above
                debug!(key = key.as_str(), "Local vault provider already registered");
            }
            "1password" => {
                let token = provider_config
                    .service_account_token_env
                    .as_ref()
                    .and_then(|env_name| std::env::var(env_name).ok());
                let op = OnePasswordProvider::new(
                    provider_config.account.clone(),
                    token,
                );
                providers.insert(key.clone(), Arc::new(op) as Arc<dyn SecretProvider>);
                info!(key = key.as_str(), "Registered 1Password provider");
            }
            other => {
                warn!(key = key.as_str(), provider_type = other, "Unknown secret provider type, skipping");
            }
        }
    }

    let router = SecretRouter::new(
        config.secrets.clone(),
        providers,
        config.secrets_config.default_provider.clone(),
    );

    if let Err(e) = resolve_provider_secrets(&mut config, &router).await {
        warn!(error = %e, "Failed to resolve provider secrets");
    }
```

**Step 2: Verify build**

Run: `cd /Volumes/TBU4/Workspace/Aleph/core && cargo build 2>&1 | tail -10`
Expected: Successful build.

**Step 3: Run full test suite**

Run: `cd /Volumes/TBU4/Workspace/Aleph/core && cargo test 2>&1 | tail -30`
Expected: All tests PASS.

**Step 4: Commit**

```bash
git add core/src/bin/aleph_server/commands/start/mod.rs
git commit -m "secrets: wire up provider initialization from config in server startup"
```

---

### Task 12: Add CLI command to list provider status

**Files:**
- Modify: `core/src/bin/aleph_server/commands/secret.rs` (add `providers` subcommand)

**Step 1: Add `providers` subcommand**

In the secret CLI commands file, add a new subcommand that:
1. Reads config to find `[secret_providers]`
2. Initializes each provider
3. Calls `health_check()` on each
4. Prints status table

```rust
// Add to the match in the secret command handler:
"providers" => {
    // Load config
    let config = alephcore::Config::load().unwrap_or_default();

    println!("Secret Providers:");
    println!("{:<15} {:<15} {}", "KEY", "TYPE", "STATUS");
    println!("{}", "-".repeat(50));

    // Always show local vault
    println!("{:<15} {:<15} {}", "local", "local_vault", "Ready (built-in)");

    // Show configured external providers
    for (key, provider_config) in &config.secret_providers {
        match provider_config.provider_type.as_str() {
            "1password" => {
                let token = provider_config
                    .service_account_token_env
                    .as_ref()
                    .and_then(|env_name| std::env::var(env_name).ok());
                let op = OnePasswordProvider::new(provider_config.account.clone(), token);
                // Use a small runtime for the health check
                let rt = tokio::runtime::Runtime::new().unwrap();
                match rt.block_on(op.health_check()) {
                    Ok(ProviderStatus::Ready) => {
                        println!("{:<15} {:<15} {}", key, "1password", "Ready");
                    }
                    Ok(ProviderStatus::NeedsAuth { message }) => {
                        println!("{:<15} {:<15} {}", key, "1password", format!("Needs Auth: {}", message));
                    }
                    Ok(ProviderStatus::Unavailable { reason }) => {
                        println!("{:<15} {:<15} {}", key, "1password", format!("Unavailable: {}", reason));
                    }
                    Err(e) => {
                        println!("{:<15} {:<15} {}", key, "1password", format!("Error: {}", e));
                    }
                }
            }
            other => {
                println!("{:<15} {:<15} {}", key, other, "Unknown type");
            }
        }
    }
}
```

**Step 2: Verify build**

Run: `cd /Volumes/TBU4/Workspace/Aleph/core && cargo build --bin aleph-server 2>&1 | tail -5`
Expected: Successful build.

**Step 3: Test manually**

Run: `cd /Volumes/TBU4/Workspace/Aleph/core && cargo run --bin aleph-server -- secret providers`
Expected: Shows provider status table (at minimum the local vault).

**Step 4: Commit**

```bash
git add core/src/bin/aleph_server/commands/secret.rs
git commit -m "cli: add 'secret providers' command to show provider status"
```

---

### Task 13: Integration tests

**Files:**
- Create: `core/tests/secret_router_integration.rs`

**Step 1: Write integration test**

Create `core/tests/secret_router_integration.rs`:

```rust
//! Integration tests for the secret routing system.
//!
//! Tests the full flow: Config -> SecretRouter -> Provider -> Resolution.

use std::collections::HashMap;
use std::sync::Arc;

use alephcore::config::types::secrets::{SecretMapping, Sensitivity};
use alephcore::secrets::cache::SecretCache;
use alephcore::secrets::provider::local_vault::LocalVaultProvider;
use alephcore::secrets::provider::SecretProvider;
use alephcore::secrets::router::{AsyncSecretResolver, SecretRouter};
use alephcore::secrets::types::EntryMetadata;
use alephcore::secrets::vault::SecretVault;
use tempfile::TempDir;

fn setup_router(dir: &TempDir) -> SecretRouter {
    let vault_path = dir.path().join("test.vault");
    let mut vault = SecretVault::open(&vault_path, "test-key").unwrap();

    vault
        .set("anthropic_key", "sk-ant-test-123", EntryMetadata::default())
        .unwrap();
    vault
        .set("github_token", "ghp_test456", EntryMetadata::default())
        .unwrap();

    let provider = Arc::new(LocalVaultProvider::new(vault)) as Arc<dyn SecretProvider>;
    let mut providers = HashMap::new();
    providers.insert("local".into(), provider);

    let mut mappings = HashMap::new();
    mappings.insert(
        "anthropic_key".into(),
        SecretMapping {
            provider: "local".into(),
            reference: None,
            sensitivity: Sensitivity::Standard,
            ttl: 3600,
        },
    );
    mappings.insert(
        "github_token".into(),
        SecretMapping {
            provider: "local".into(),
            reference: None,
            sensitivity: Sensitivity::High,
            ttl: 0,
        },
    );

    SecretRouter::new(mappings, providers, "local".into())
}

#[tokio::test]
async fn test_full_resolution_flow() {
    let dir = TempDir::new().unwrap();
    let router = setup_router(&dir);

    // Standard secret
    let secret = router.resolve("anthropic_key").await.unwrap();
    assert_eq!(secret.expose(), "sk-ant-test-123");

    // High-sensitivity secret
    let secret = router.resolve("github_token").await.unwrap();
    assert_eq!(secret.expose(), "ghp_test456");
}

#[tokio::test]
async fn test_unmapped_secret_fallback() {
    let dir = TempDir::new().unwrap();
    let router = setup_router(&dir);

    // This secret exists in vault but has no explicit mapping.
    // Should fallback to default provider with the name as reference.
    let secret = router.resolve("anthropic_key").await.unwrap();
    assert_eq!(secret.expose(), "sk-ant-test-123");
}

#[tokio::test]
async fn test_render_with_secrets_integration() {
    let dir = TempDir::new().unwrap();
    let router = setup_router(&dir);

    let input = "Authorization: Bearer {{secret:anthropic_key}}";
    let (rendered, injected) = alephcore::secrets::render_with_secrets(input, &router)
        .await
        .unwrap();

    assert_eq!(rendered, "Authorization: Bearer sk-ant-test-123");
    assert_eq!(injected.len(), 1);
    assert_eq!(injected[0].name, "anthropic_key");
    // Verify no plaintext in the InjectedSecret record
    assert_ne!(injected[0].value_hash, 0);
}

#[tokio::test]
async fn test_nonexistent_secret_returns_error() {
    let dir = TempDir::new().unwrap();
    let router = setup_router(&dir);

    let result = router.resolve("does_not_exist").await;
    assert!(result.is_err());
}
```

**Step 2: Run integration tests**

Run: `cd /Volumes/TBU4/Workspace/Aleph/core && cargo test --test secret_router_integration -- --nocapture`
Expected: All 4 tests PASS.

**Step 3: Commit**

```bash
git add core/tests/secret_router_integration.rs
git commit -m "test: add integration tests for secret routing system"
```

---

### Task 14: Final verification and cleanup

**Step 1: Run full test suite**

Run: `cd /Volumes/TBU4/Workspace/Aleph/core && cargo test 2>&1 | tail -30`
Expected: All tests PASS with no warnings.

**Step 2: Run clippy**

Run: `cd /Volumes/TBU4/Workspace/Aleph/core && cargo clippy 2>&1 | tail -20`
Expected: No errors (warnings acceptable).

**Step 3: Verify the old sync SecretResolver is fully removed**

Search for any remaining references:
Run: `rg "SecretResolver" core/src/ --type rust`
Expected: Only `AsyncSecretResolver` references remain. No orphaned sync `SecretResolver` usage.

**Step 4: Final commit (if any cleanup needed)**

```bash
git add -A
git commit -m "secrets: final cleanup after SPI migration"
```

---

## Summary

| Wave | Tasks | Description |
|------|-------|-------------|
| **Wave 1** | 1-3 | Foundation: error types, SecretProvider trait, LocalVaultProvider |
| **Wave 2** | 4-6 | Infrastructure: config types, TTL cache, SecretRouter |
| **Wave 3** | 7-9 | Migration: async injection, async provider resolution, fix callers |
| **Wave 4** | 10-14 | Integration: 1Password provider, server wiring, CLI, integration tests |

**Total: 14 tasks across 4 waves**
**Estimated commits: 14**
