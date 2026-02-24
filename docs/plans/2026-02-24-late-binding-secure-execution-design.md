# Late-Binding Secure Execution Architecture

> Zero-Knowledge Context: LLM must know HOW to use credentials, but never WHAT the credentials are.

**Date**: 2026-02-24
**Status**: Approved
**Scope**: External secret backend integration (SPI) with full async redesign

---

## 1. Design Philosophy

### Problem

Secrets enter the LLM prompt before execution, creating a window where sensitive data exists in the AI context. The current local vault works well for API keys but cannot leverage external enterprise password managers (1Password, Bitwarden) where users already store their credentials.

### Solution: Late-Binding Secure Execution

Introduce a Secret Provider Interface (SPI) that:
1. Abstracts secret sources behind a unified async trait
2. Routes `{{secret:NAME}}` to the correct backend via explicit config mapping
3. Applies hybrid caching (TTL for standard, real-time for high-sensitivity)
4. Triggers JIT authorization for high-sensitivity secrets
5. Never exposes backend details in placeholders

### Key Decisions

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Architecture | Full async redesign (Option C) | Aleph has no external consumers; now is the best time for a clean async-native design |
| Fetch strategy | Hybrid (TTL cache + real-time for high) | Balance performance (~0ms cached) with security (never cache bank passwords) |
| First backend | 1Password CLI (`op`) | Most mature enterprise password manager with comprehensive CLI |
| Mapping model | Explicit configuration | `{{secret:NAME}}` stays abstract; config.toml maps to provider-specific references |
| SPI vs Vault | Vault demoted to one SPI backend | Unified interface; `LocalVaultProvider` wraps existing `SecretVault` |

---

## 2. Core Trait System

### 2.1 AsyncSecretResolver (replaces SecretResolver)

```rust
/// Async trait for resolving secret names to decrypted values.
/// Replaces the synchronous SecretResolver.
#[async_trait]
pub trait AsyncSecretResolver: Send + Sync {
    async fn resolve(&self, name: &str) -> Result<DecryptedSecret, SecretError>;
}
```

### 2.2 SecretProvider (SPI backend abstraction)

```rust
/// Secret Provider Interface — the backend abstraction.
/// Each external secret manager implements this trait.
#[async_trait]
pub trait SecretProvider: Send + Sync {
    /// Provider identifier (e.g., "local_vault", "1password", "bitwarden")
    fn provider_type(&self) -> &str;

    /// Fetch a secret by its provider-specific reference URI.
    /// For local vault: just the name ("anthropic_key")
    /// For 1Password: "op://Personal/GitHub/token"
    async fn get(&self, reference: &str) -> Result<DecryptedSecret, SecretError>;

    /// Check if the provider is available and authenticated.
    async fn health_check(&self) -> Result<ProviderStatus, SecretError>;

    /// List available secret names (for discovery/autocomplete, never values).
    async fn list(&self) -> Result<Vec<SecretMetadata>, SecretError>;
}

#[derive(Debug, Clone)]
pub enum ProviderStatus {
    Ready,
    NeedsAuth { message: String },
    Unavailable { reason: String },
}

#[derive(Debug, Clone)]
pub struct SecretMetadata {
    pub name: String,
    pub provider: String,
    pub updated_at: Option<i64>,
}
```

### 2.3 SecretRouter (routing + caching + AsyncSecretResolver)

```rust
/// Routes secret requests to the correct provider based on config mapping.
/// Implements AsyncSecretResolver as the single entry point for all consumers.
pub struct SecretRouter {
    /// name -> SecretMapping (from config)
    mappings: HashMap<String, SecretMapping>,
    /// provider_type -> Box<dyn SecretProvider>
    providers: HashMap<String, Box<dyn SecretProvider>>,
    /// TTL-based cache for standard-level secrets
    cache: RwLock<HashMap<String, CachedSecret>>,
    /// Default provider key for unmapped secrets
    default_provider: String,
    /// Optional approval manager for high-sensitivity secrets
    approval_manager: Option<Arc<SecretApprovalManager>>,
}

#[async_trait]
impl AsyncSecretResolver for SecretRouter {
    async fn resolve(&self, name: &str) -> Result<DecryptedSecret, SecretError> {
        // 1. Look up mapping (or use default provider with name as reference)
        // 2. If sensitivity == High: trigger JIT approval
        // 3. If sensitivity == Standard: check cache
        //    - Cache hit + not expired -> return cached value
        //    - Cache miss/expired -> call provider.get() -> write cache -> return
        // 4. If High: call provider.get() directly (no cache) -> return
    }
}
```

---

## 3. Configuration Format

### 3.1 config.toml

```toml
# ---- Secret provider definitions ----
[secret_providers.local]
type = "local_vault"

[secret_providers.op]
type = "1password"
account = "my.1password.com"                        # Optional, for multi-account
service_account_token_env = "OP_SERVICE_ACCOUNT_TOKEN"  # Optional, for automation

# ---- Secret mappings ----
[secrets.anthropic_key]
provider = "local"
sensitivity = "standard"
ttl = 3600

[secrets.github_token]
provider = "op"
ref = "op://Personal/GitHub/token"
sensitivity = "standard"

[secrets.bank_password]
provider = "op"
ref = "op://Personal/BankOfChina/password"
sensitivity = "high"
```

### 3.2 Rust Types

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecretProviderConfig {
    #[serde(rename = "type")]
    pub provider_type: String,
    pub account: Option<String>,
    pub service_account_token_env: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecretMapping {
    pub provider: String,
    #[serde(rename = "ref")]
    pub reference: Option<String>,
    #[serde(default = "default_sensitivity")]
    pub sensitivity: Sensitivity,
    #[serde(default = "default_ttl")]
    pub ttl: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Sensitivity {
    Standard,
    High,
}

fn default_sensitivity() -> Sensitivity { Sensitivity::Standard }
fn default_ttl() -> u64 { 3600 }
```

### 3.3 Backwards Compatibility

| Scenario | Behavior |
|----------|----------|
| No `[secret_providers]` in config | Auto-create `local` provider; all secrets use local vault (identical to current) |
| No `[secrets.*]` mappings | All `{{secret:NAME}}` fallback to default provider (local) |
| Existing `provider.secret_name` | Still works, routed through SecretRouter (defaults to local) |

---

## 4. 1Password Provider Implementation

### 4.1 OnePasswordProvider

```rust
pub struct OnePasswordProvider {
    account: Option<String>,
    service_account_token: Option<String>,
}

#[async_trait]
impl SecretProvider for OnePasswordProvider {
    fn provider_type(&self) -> &str { "1password" }

    async fn get(&self, reference: &str) -> Result<DecryptedSecret, SecretError> {
        // Uses `op read <reference> --no-newline`
        let mut cmd = tokio::process::Command::new("op");
        cmd.arg("read").arg(reference).arg("--no-newline");

        if let Some(ref account) = self.account {
            cmd.arg("--account").arg(account);
        }
        if let Some(ref token) = self.service_account_token {
            cmd.env("OP_SERVICE_ACCOUNT_TOKEN", token);
        }

        let output = cmd.output().await?;

        match output.status.success() {
            true => Ok(DecryptedSecret::new(
                String::from_utf8_lossy(&output.stdout).into_owned()
            )),
            false => {
                let stderr = String::from_utf8_lossy(&output.stderr);
                if stderr.contains("not signed in") || stderr.contains("session expired") {
                    Err(SecretError::ProviderAuthRequired {
                        provider: "1password".into(),
                        message: "1Password session expired. Run `op signin`.".into(),
                    })
                } else {
                    Err(SecretError::ProviderError {
                        provider: "1password".into(),
                        message: stderr.to_string(),
                    })
                }
            }
        }
    }

    async fn health_check(&self) -> Result<ProviderStatus, SecretError> {
        let output = tokio::process::Command::new("op")
            .arg("whoami").output().await?;
        if output.status.success() {
            Ok(ProviderStatus::Ready)
        } else {
            Ok(ProviderStatus::NeedsAuth {
                message: "Run `op signin` to authenticate".into(),
            })
        }
    }

    async fn list(&self) -> Result<Vec<SecretMetadata>, SecretError> {
        // `op item list --format=json` -> parse JSON
        todo!()
    }
}
```

### 4.2 Hybrid Cache

```rust
struct CachedSecret {
    value: DecryptedSecret,
    fetched_at: Instant,
    ttl: Duration,
}

impl CachedSecret {
    fn is_expired(&self) -> bool {
        self.fetched_at.elapsed() > self.ttl
    }
}
```

**Resolution flow:**

```
resolve("github_token")
  |
  +-- lookup mapping -> provider="op", sensitivity=Standard, ttl=3600
  |
  +-- sensitivity == Standard?
  |    +-- YES -> check cache
  |    |    +-- cache hit + not expired -> return cached value
  |    |    +-- cache miss/expired -> provider.get() -> write cache -> return
  |    |
  |    +-- NO (High) -> trigger JIT approval -> provider.get() -> no cache -> return
  |
  +-- no mapping found -> fallback to default_provider.get(name)
```

### 4.3 JIT Authorization (High-Sensitivity Secrets)

Reuses existing `SecretApprovalManager`:

```rust
if mapping.sensitivity == Sensitivity::High {
    let request = SecretApprovalRequest {
        secret_name: name.to_string(),
        provider: mapping.provider.clone(),
        tool_context: current_tool_name,
    };
    let decision = self.approval_manager
        .request_approval(request).await?;
    match decision {
        ApprovalDecision::Allow => { /* proceed */ }
        ApprovalDecision::Deny { reason } => {
            return Err(SecretError::AccessDenied { name, reason });
        }
    }
}
```

### 4.4 Extended SecretError

```rust
pub enum SecretError {
    // ... existing variants preserved ...

    #[error("Provider '{provider}' requires authentication: {message}")]
    ProviderAuthRequired { provider: String, message: String },

    #[error("Provider '{provider}' error: {message}")]
    ProviderError { provider: String, message: String },

    #[error("Access denied for secret '{name}': {reason}")]
    AccessDenied { name: String, reason: String },

    #[error("Provider '{provider}' not configured")]
    ProviderNotFound { provider: String },
}
```

---

## 5. File Organization

```
core/src/secrets/
├── mod.rs                  # [MODIFY] Update pub use, add provider/router modules
├── types.rs                # [MODIFY] Add SecretError variants, SecretMapping, Sensitivity
├── crypto.rs               # [NO CHANGE]
├── vault.rs                # [MODIFY] SecretVault impl AsyncSecretResolver
├── injection.rs            # [MODIFY] render_with_secrets -> async, use AsyncSecretResolver
├── leak_detector.rs        # [NO CHANGE]
├── placeholder.rs          # [NO CHANGE]
├── migration.rs            # [NO CHANGE]
├── web3_signer.rs          # [NO CHANGE]
│
├── provider/               # [NEW] SPI backend implementations
│   ├── mod.rs              # SecretProvider trait + ProviderStatus + SecretMetadata
│   ├── local_vault.rs      # LocalVaultProvider (wraps SecretVault, impl SecretProvider)
│   └── onepassword.rs      # OnePasswordProvider
│
├── router.rs               # [NEW] SecretRouter impl AsyncSecretResolver
└── cache.rs                # [NEW] CachedSecret, TTL logic
```

---

## 6. Migration Path (4 Waves)

### Wave 1: Foundation Traits + Provider Skeleton
1. Add `AsyncSecretResolver` trait
2. Add `provider/mod.rs` with `SecretProvider` trait
3. Add `provider/local_vault.rs` wrapping existing `SecretVault`
4. Extend `SecretError` with new variants

### Wave 2: Router + Configuration
5. Add config types (`SecretProviderConfig`, `SecretMapping`, `Sensitivity`)
6. Add `router.rs` (`SecretRouter` implementing `AsyncSecretResolver`)
7. Add `cache.rs` (TTL cache logic)

### Wave 3: Async Migration
8. `render_with_secrets` -> `async fn render_with_secrets`
9. `resolve_provider_secrets` -> `async fn resolve_provider_secrets` (uses `SecretRouter`)
10. Update all callers (server startup, `ExecSecurityGate`)
11. Remove old synchronous `SecretResolver` trait

### Wave 4: 1Password Integration
12. Add `provider/onepassword.rs`
13. Server startup: initialize providers from config, inject `SecretRouter`
14. Add CLI command: `aleph-server secret providers` (list configured provider status)
15. Integration tests

---

## 7. YAGNI — Explicitly Out of Scope

- Bitwarden/Keychain backends (trait is ready; implement when needed)
- Secret rotation/versioning
- LeakDetector modifications (current implementation sufficient)
- New RPC methods (reuse existing `secret.approval.*`)
- Audit logging (future enhancement)

---

## 8. Security Invariants

1. **Zero-Knowledge Context**: `{{secret:NAME}}` never reveals backend details
2. **Memory Safety**: All `DecryptedSecret` values auto-zeroize on drop via `secrecy` crate
3. **No Cache for High**: Secrets with `sensitivity = "high"` are never cached
4. **JIT Authorization**: High-sensitivity secrets require explicit user approval before each use
5. **Leak Prevention**: Existing `LeakDetector` + `SecretMasker` continue to scan inbound/outbound
6. **Session Validation**: `health_check()` detects expired external sessions before operations fail
