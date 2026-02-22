# Agent Secret Management Design

> Phase 1: LLM Provider API Key Protection
> Inspired by NEAR AI IronClaw Architecture

## Background

Aleph currently stores LLM Provider API Keys (Anthropic, OpenAI, Gemini, etc.) in plaintext within `~/.aleph/config.toml`. This is a critical security gap — keys are exposed on disk, in backups, and potentially in version control.

This design introduces a **SecretVault** system inspired by IronClaw's `SecretsCrypto` + `SecretsStore` pattern, providing encrypted-at-rest storage with memory safety guarantees.

### IronClaw Core Insight

> "Secrets are encrypted in the Host layer, decrypted in the Host layer, and scanned for leaks in the Host layer. WASM code can only say 'I need access to api.slack.com' but never knows what the credentials are."

This Phase 1 focuses on the foundation: encrypted storage and config migration. Phases 2-3 will add WASM sandbox isolation, placeholder injection, and leak detection.

## Scope

### In Scope (Phase 1)

- AES-256-GCM encrypted vault file (`~/.aleph/secrets.vault`)
- HKDF-SHA256 per-entry key derivation
- `SecretString` (secrecy crate) for memory-safe secret handling
- Forced migration from plaintext `api_key` in config.toml
- CLI commands for secret management (`aleph secret set/list/delete/verify`)
- `DecryptedSecret` wrapper with Debug/Display redaction

### Out of Scope (Future Phases)

- WASM plugin secret isolation (Phase 2)
- Tool parameter placeholder injection (Phase 2)
- Bidirectional leak detection (Phase 2)
- Web3 private key signing (Phase 3)
- User approval workflow for sensitive operations (Phase 3)

## Architecture

### Storage Layer: SecretVault

```
┌─────────────────────────────────────────────┐
│  config.toml                                 │
│  api_key field removed                       │
│  [providers.anthropic]                       │
│  secret_name = "anthropic_api_key"           │
└───────────────┬─────────────────────────────┘
                │
┌───────────────▼─────────────────────────────┐
│  SecretVault                                 │
│  ~/.aleph/secrets.vault (encrypted file)     │
│  AES-256-GCM + HKDF-SHA256                  │
│  Master Key: env var or interactive input     │
└───────────────┬─────────────────────────────┘
                │
┌───────────────▼─────────────────────────────┐
│  ProviderFactory                             │
│  Decrypts key from vault on provider init    │
│  Key held via SecretString (auto-zeroize)    │
└─────────────────────────────────────────────┘
```

### Core Types

```rust
// core/src/secrets/vault.rs
pub struct SecretVault {
    store: HashMap<String, EncryptedEntry>,
    crypto: SecretsCrypto,
    path: PathBuf,
}

// core/src/secrets/types.rs
struct EncryptedEntry {
    ciphertext: Vec<u8>,     // AES-256-GCM ciphertext
    nonce: [u8; 12],         // GCM nonce
    salt: [u8; 32],          // HKDF salt (per-entry)
    created_at: i64,
    updated_at: i64,
    metadata: EntryMetadata, // non-sensitive: provider name, description
}

// core/src/secrets/crypto.rs
pub struct SecretsCrypto {
    master_key: SecretString, // secrecy crate, zeroized on drop
}

// core/src/secrets/types.rs
pub struct DecryptedSecret {
    value: SecretString,  // zeroized on drop
}
```

### Encryption Scheme

| Component | Algorithm | Notes |
|-----------|-----------|-------|
| **Encryption** | AES-256-GCM | Authenticated encryption, tamper-proof |
| **Key Derivation** | HKDF-SHA256 | Per-entry independent salt → independent encryption key |
| **Master Key** | SecretString | `secrecy` crate wrapper, memory zeroization guarantee |
| **File Format** | bincode | Compact, fast, non-human-readable |
| **Info Label** | `b"aleph-secrets-v1"` | HKDF info parameter for domain separation |

### Master Key Sources (priority order)

1. Environment variable `ALEPH_MASTER_KEY`
2. Interactive generation on first launch (user saves it)

### SecretStore Trait

```rust
pub trait SecretStore: Send + Sync {
    async fn get(&self, name: &str) -> Result<DecryptedSecret, SecretError>;
    async fn set(&self, name: &str, value: &str) -> Result<()>;
    async fn delete(&self, name: &str) -> Result<bool>;
    async fn exists(&self, name: &str) -> bool;
    async fn list(&self) -> Vec<String>; // names only, never values
}
```

### DecryptedSecret Safety

```rust
impl DecryptedSecret {
    pub fn expose(&self) -> &str { ... }  // only way to get plaintext
}

impl fmt::Debug for DecryptedSecret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[REDACTED, {} bytes]", self.value.expose_secret().len())
    }
}

impl fmt::Display for DecryptedSecret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[REDACTED]")
    }
}
```

## Config System Changes

### config.toml Before/After

**Before (current — insecure):**
```toml
[providers.anthropic]
api_key = "sk-ant-api03-xxxxx"
model = "claude-sonnet-4-20250514"
```

**After (Phase 1):**
```toml
[providers.anthropic]
secret_name = "anthropic_api_key"
model = "claude-sonnet-4-20250514"
```

### ProviderConfig Change

```rust
// Before
pub struct ProviderConfig {
    pub api_key: Option<String>,  // REMOVED
    // ...
}

// After
pub struct ProviderConfig {
    pub secret_name: Option<String>,  // reference to SecretVault entry
    // ... other fields unchanged
}
```

### Forced Migration

On server startup, a one-time migration runs:

```
Startup: scan config.toml
    │
    ├─ Found api_key = "sk-xxx" (plaintext)
    │   ├─ Store in SecretVault (encrypted)
    │   ├─ Replace with secret_name = "xxx_api_key"
    │   ├─ Rewrite config.toml (plaintext removed)
    │   └─ Log: "Migrated api_key for provider 'xxx' to SecretVault"
    │
    └─ Found secret_name = "xxx" (already migrated)
        └─ Normal startup
```

### Provider Creation Flow

```rust
async fn create_provider(
    config: &ProviderConfig,
    vault: &SecretVault,
) -> Result<Box<dyn Provider>> {
    let api_key = match &config.secret_name {
        Some(name) => vault.get(name).await?,
        None => return Err(Error::NoApiKeyConfigured),
    };

    // api_key is DecryptedSecret, expose() only during HTTP client creation
    let client = HttpProvider::new(api_key.expose(), &config.model, ...);
    // api_key drops here → memory zeroized
    Ok(Box::new(client))
}
```

## Module Organization

### New Module

```
core/src/
├── secrets/                    # NEW
│   ├── mod.rs                  # pub exports
│   ├── crypto.rs               # SecretsCrypto (AES-256-GCM + HKDF)
│   ├── vault.rs                # SecretVault (file storage + CRUD)
│   ├── types.rs                # DecryptedSecret, EncryptedEntry, SecretRef
│   └── migration.rs            # config.toml migration logic
```

### Dependency Graph

```
                    ┌──────────┐
                    │ secrets/ │
                    │  vault   │
                    │  crypto  │
                    │  types   │
                    └────┬─────┘
                         │
            ┌────────────┼────────────┐
            ▼            ▼            ▼
      ┌──────────┐ ┌──────────┐ ┌──────────┐
      │ config/  │ │providers/│ │  cli/    │
      │migration │ │ factory  │ │ secret   │
      │ check    │ │ create   │ │ commands │
      └──────────┘ └──────────┘ └──────────┘
```

### New Rust Dependencies

```toml
aes-gcm = "0.10"         # AES-256-GCM
hkdf = "0.12"             # HKDF-SHA256
sha2 = "0.10"             # SHA-256 (HKDF hash function)
secrecy = "0.8"           # SecretString (memory zeroization)
bincode = "1.3"           # vault file serialization
zeroize = "1.8"           # additional memory zeroization
rpassword = "5.0"         # CLI hidden password input
```

### Error Types

```rust
#[derive(Debug, thiserror::Error)]
pub enum SecretError {
    #[error("Secret '{0}' not found")]
    NotFound(String),

    #[error("Master key not configured. Set ALEPH_MASTER_KEY or run `aleph secret init`")]
    MasterKeyMissing,

    #[error("Decryption failed: vault may be corrupted or master key is wrong")]
    DecryptionFailed,

    #[error("Vault I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Migration failed for provider '{provider}': {reason}")]
    MigrationFailed { provider: String, reason: String },
}
```

## CLI Commands

```bash
# Initialize vault (first time)
aleph secret init
> Enter master key: ****
> Confirm master key: ****
> Vault initialized at ~/.aleph/secrets.vault

# Add a secret
aleph secret set anthropic_api_key
> Enter value: ****

# List secrets (names only)
aleph secret list
  anthropic_api_key  (created: 2026-02-22)
  openai_api_key     (created: 2026-02-22)

# Delete a secret
aleph secret delete openai_api_key

# Verify a secret works
aleph secret verify anthropic_api_key
  ✓ Valid (Anthropic Claude)
```

## Data Flow Summary

```
User sets secret
    │
    ▼
aleph secret set anthropic_api_key
    │
    ▼
SecretsCrypto.encrypt(value, master_key)
    │  AES-256-GCM + HKDF per-entry salt
    ▼
~/.aleph/secrets.vault (encrypted file)
    │
    ▼
Server starts
    │
    ▼
Config loads → secret_name = "anthropic_api_key"
    │
    ▼
ProviderFactory → vault.get("anthropic_api_key")
    │
    ▼
SecretsCrypto.decrypt() → DecryptedSecret
    │
    ▼
HttpProvider::new(secret.expose()) → HTTP Client
    │  secret dropped → memory zeroized
    ▼
Provider works normally, key only lives inside HTTP client
```

## Testing Strategy

| Type | Coverage |
|------|----------|
| **Unit** | crypto encrypt/decrypt roundtrip, vault CRUD, migration detection |
| **Integration** | Full config load → vault decrypt → provider creation flow |
| **Security** | Wrong master key rejection, vault tamper detection, memory zeroization |

## Future Extension Points

### Phase 2 Readiness

- `SecretStore` is a trait, can add `access_audit()` and `get_for_tool(name, tool)` with permission checks
- `EncryptedEntry.metadata` reserved for: `allowed_tools`, `scope` (Provider/Tool/Extension)
- Existing `SecretMasker` can be enhanced with bidirectional scanning (IronClaw's `LeakDetector` pattern)
- WASM `PermissionChecker` can add `can_access_secret(name)` method

### Phase 3 Readiness

- `SecretsCrypto` can be extended with signing operations for Web3 keys
- Approval workflow already exists in `exec/` — pattern reusable for secret access approval
