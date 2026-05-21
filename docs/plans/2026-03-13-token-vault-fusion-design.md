# Token-Vault Fusion Design

**Date**: 2026-03-13
**Status**: Approved

## Summary

Merge SharedTokenManager and SecretVault into a single module. The panel UI token becomes the vault master key. API keys are always encrypted — no plaintext storage, no migration, no manual vault configuration.

## Core Decisions

1. **Token = master key**: Token used as HKDF-SHA256 input key material to derive vault encryption keys
2. **Token reset = vault re-encrypt**: Old token decrypts all entries, new token re-encrypts (atomic file swap)
3. **No env var / keychain**: Master key source is exclusively the token
4. **No vault RPC**: Vault is an internal implementation detail, invisible to users
5. **Implicit vault key**: Provider name maps directly to vault entry name (no `api_key` / `secret_name` fields)
6. **Auto-init**: Token generation = vault initialization, no "vault-less" state possible

## Data Model

### SharedTokenManager (extended)

```rust
pub struct SharedTokenManager {
    store: Arc<SecurityStore>,            // token hash + HMAC in SQLite
    vault: RwLock<SecretVault>,           // embedded encrypted storage
    current_token: Mutex<Option<String>>, // in-memory plaintext token
}
```

### SecretVault (simplified)

```rust
pub struct SecretVault {
    entries: HashMap<String, EncryptedEntry>,  // key = provider name
    vault_path: PathBuf,                        // ~/.aleph/data/secrets.vault
}
```

No `master_key` field — token passed in per operation by SharedTokenManager.

### ProviderConfig (simplified)

```rust
pub struct ProviderConfig {
    // REMOVED: api_key, secret_name
    pub model: String,
    pub protocol: Option<String>,
    pub base_url: Option<String>,
    pub enabled: bool,
    // ... generation parameters
}
```

## Lifecycle Flows

### Server Startup

```
Server start
  ├─ Load token from file → HMAC validation pass
  │   ├─ HKDF(token) → derive vault master key
  │   └─ Load secrets.vault (decrypt & verify)
  └─ No valid token → generate new token
      ├─ Store HMAC hash in SQLite
      ├─ Write token file (0o600)
      ├─ HKDF(new token) → derive vault master key
      └─ Initialize empty vault
```

### API Key Store/Retrieve

```
Store: SharedTokenManager.store_secret("anthropic", "sk-...")
  → HKDF(token, salt=random_32bytes) → derive entry key
  → AES-256-GCM encrypt
  → Write secrets.vault

Retrieve: SharedTokenManager.get_secret("anthropic")
  → Read EncryptedEntry
  → HKDF(token, salt=entry.salt) → derive entry key
  → AES-256-GCM decrypt
  → Return SecretString (auto-zeroize)
```

### Token Reset

```
SharedTokenManager.reset_token()
  1. old_token = current_token (held in memory)
  2. Decrypt all vault entries with old_token
  3. new_token = generate_token()
  4. Re-encrypt all entries with new_token (new salt per entry)
  5. Write new secrets.vault (atomic: tmp file + rename)
  6. Update SQLite (new HMAC hash)
  7. Update token file
  8. Update current_token
```

Steps 2-5 are atomic — write to temp file, rename on success, preserve old vault on failure.

## New SharedTokenManager API

```rust
impl SharedTokenManager {
    // Existing
    pub fn generate_token(&self) -> Result<String>;
    pub fn validate_token(&self, token: &str) -> bool;
    pub fn try_load_token_from_file(&self, path: &Path) -> Option<String>;

    // New
    pub fn store_secret(&self, name: &str, value: &str) -> Result<()>;
    pub fn get_secret(&self, name: &str) -> Result<Option<SecretString>>;
    pub fn delete_secret(&self, name: &str) -> Result<()>;
    pub fn list_secret_names(&self) -> Result<Vec<String>>;
    pub fn reset_token(&self) -> Result<String>;  // re-encrypt + return new token
}
```

## Deletion List

| Target | Reason |
|--------|--------|
| `gateway/handlers/vault_config.rs` | Vault RPC handlers |
| `vault.status/storeKey/deleteKey/verify` RPC routes | No vault RPC |
| `SecretVault` keychain methods | No keychain master key |
| `SecretVault` env var parsing | No `ALEPH_MASTER_KEY` |
| `MasterKeySource` enum | Single source: token |
| `SecretRouter` / `LocalVaultProvider` | Vault wrapped by SharedTokenManager |
| `AsyncSecretResolver` trait | Same |
| `ProviderConfig.api_key` field | No plaintext storage |
| `ProviderConfig.secret_name` field | Implicit mapping |
| Plaintext→encrypted migration logic | No migration |
| `VaultStatus` struct | No vault RPC |

## Provider Factory Change

```rust
// Before:
let api_key = if let Some(secret_name) = &config.secret_name {
    vault.get(secret_name)?
} else {
    config.api_key.clone()
};

// After:
let api_key = token_manager.get_secret(&provider_name)?;
```

## Panel UI Impact

Minimal — vault fusion is a pure backend change:
- API key input flow unchanged (SecretInput → RPC → backend encrypts)
- Remove any vault management UI (master key config, migration status)
- Token set/reset flow unchanged
