# Token-Vault Fusion Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Merge SharedTokenManager and SecretVault so the panel UI token becomes the vault master key, eliminating plaintext API key storage and all manual vault configuration.

**Architecture:** Token is used as HKDF input key material. SecretVault becomes an internal component of SharedTokenManager. Provider name maps implicitly to vault entry. All keychain/env-var/migration/vault-RPC code is deleted.

**Tech Stack:** Rust, AES-256-GCM, HKDF-SHA256, bincode, secrecy crate

**Design Doc:** `docs/plans/2026-03-13-token-vault-fusion-design.md`

---

## Task 1: Simplify SecretVault — Remove Self-Held Master Key

Remove `crypto` field from `SecretVault`. Instead of holding a `SecretsCrypto` internally, accept the master key as a parameter for each encrypt/decrypt operation. Delete all keychain, env var, VaultStatus, and resolve_master_key code.

**Files:**
- Modify: `src/secrets/vault.rs`
- Modify: `src/secrets/types.rs` (remove `MigrationFailed` variant if present)
- Modify: `src/secrets/mod.rs` (remove deleted re-exports)

**Step 1: Refactor SecretVault struct and methods**

In `src/secrets/vault.rs`:

```rust
// Remove these:
// - KEYRING_SERVICE, KEYRING_ACCOUNT constants
// - crypto field from struct
// - resolve_master_key()
// - get_master_key_from_keyring()
// - store_master_key_to_keyring()
// - delete_master_key_from_keyring()
// - VaultStatus struct
// - vault_status() function
// - resolve_provider_secrets() function
// - impl AsyncSecretResolver for SecretVault

// New SecretVault struct:
pub struct SecretVault {
    data: VaultData,
    path: PathBuf,
}

impl SecretVault {
    /// Open or create a vault at the given path.
    /// Does NOT take a master key — encryption is caller's responsibility.
    pub fn open(path: impl Into<PathBuf>) -> Result<Self, SecretError> {
        let path = path.into();
        let data = if path.exists() {
            let bytes = std::fs::read(&path)?;
            bincode::deserialize(&bytes).map_err(|e| {
                SecretError::Serialization(format!("Failed to deserialize vault: {}", e))
            })?
        } else {
            VaultData { version: VAULT_VERSION, entries: HashMap::new() }
        };
        Ok(Self { data, path })
    }

    /// Store encrypted entry (caller provides pre-encrypted data).
    pub fn set(&mut self, name: &str, entry: EncryptedEntry) -> Result<(), SecretError> {
        // Preserve created_at if overwriting
        if let Some(existing) = self.data.entries.get(name) {
            let mut entry = entry;
            entry.created_at = existing.created_at;
            self.data.entries.insert(name.to_string(), entry);
        } else {
            self.data.entries.insert(name.to_string(), entry);
        }
        self.save()
    }

    /// Get raw encrypted entry (caller decrypts).
    pub fn get(&self, name: &str) -> Result<&EncryptedEntry, SecretError> {
        self.data.entries.get(name)
            .ok_or_else(|| SecretError::NotFound(name.to_string()))
    }

    /// Delete an entry.
    pub fn delete(&mut self, name: &str) -> Result<bool, SecretError> {
        let removed = self.data.entries.remove(name).is_some();
        if removed { self.save()?; }
        Ok(removed)
    }

    /// List all entry names.
    pub fn list_names(&self) -> Vec<String> {
        self.data.entries.keys().cloned().collect()
    }

    /// Get all entries (for re-encryption).
    pub fn entries(&self) -> &HashMap<String, EncryptedEntry> {
        &self.data.entries
    }

    /// Replace all entries (for re-encryption).
    pub fn replace_all(&mut self, entries: HashMap<String, EncryptedEntry>) -> Result<(), SecretError> {
        self.data.entries = entries;
        self.save()
    }

    // Keep: save(), path(), len(), is_empty(), default_path()
}
```

**Step 2: Update `src/secrets/mod.rs`**

Remove deleted re-exports:
```rust
// DELETE these lines:
pub use vault::{
    resolve_master_key, SecretVault,
    get_master_key_from_keyring, store_master_key_to_keyring, delete_master_key_from_keyring,
    vault_status, VaultStatus,
};
pub use provider::{ProviderStatus, SecretMetadata, SecretProvider};
pub use provider::local_vault::LocalVaultProvider;
pub use router::{AsyncSecretResolver, SecretRouter};

// KEEP only:
pub use vault::SecretVault;
```

**Step 3: Run tests**

Run: `cargo check -p alephcore 2>&1 | head -50`
Expected: Compilation errors from downstream consumers — that's expected, we fix them in later tasks.

**Step 4: Update vault.rs tests**

Update all tests in `vault.rs` to use new API (no master_key parameter in `open()`). Tests that test encrypt/decrypt directly should move to SharedTokenManager tests in Task 2.

**Step 5: Commit**

```
git commit -m "secrets: simplify SecretVault — remove master key, keychain, env var"
```

---

## Task 2: Extend SharedTokenManager with Vault + Secret Operations

Add vault as an internal component of SharedTokenManager. Implement `store_secret`, `get_secret`, `delete_secret`, `list_secret_names`.

**Files:**
- Modify: `src/gateway/security/shared_token.rs`
- Reference: `src/secrets/crypto.rs` (SecretsCrypto — reuse for encrypt/decrypt)

**Step 1: Write failing tests**

In `src/gateway/security/shared_token.rs`, add tests:

```rust
#[test]
fn test_store_and_get_secret() {
    let dir = tempfile::TempDir::new().unwrap();
    let store = Arc::new(SecurityStore::in_memory().unwrap());
    let vault_path = dir.path().join("test.vault");
    let mgr = SharedTokenManager::new(store, vault_path);
    let token = mgr.generate_token().unwrap();

    mgr.store_secret("anthropic", "sk-ant-secret").unwrap();
    let secret = mgr.get_secret("anthropic").unwrap().unwrap();
    assert_eq!(secret.expose(), "sk-ant-secret");
}

#[test]
fn test_delete_secret() {
    let dir = tempfile::TempDir::new().unwrap();
    let store = Arc::new(SecurityStore::in_memory().unwrap());
    let vault_path = dir.path().join("test.vault");
    let mgr = SharedTokenManager::new(store, vault_path);
    let _token = mgr.generate_token().unwrap();

    mgr.store_secret("openai", "sk-openai-key").unwrap();
    mgr.delete_secret("openai").unwrap();
    assert!(mgr.get_secret("openai").unwrap().is_none());
}

#[test]
fn test_list_secret_names() {
    let dir = tempfile::TempDir::new().unwrap();
    let store = Arc::new(SecurityStore::in_memory().unwrap());
    let vault_path = dir.path().join("test.vault");
    let mgr = SharedTokenManager::new(store, vault_path);
    let _token = mgr.generate_token().unwrap();

    mgr.store_secret("anthropic", "key1").unwrap();
    mgr.store_secret("openai", "key2").unwrap();
    let mut names = mgr.list_secret_names().unwrap();
    names.sort();
    assert_eq!(names, vec!["anthropic", "openai"]);
}

#[test]
fn test_no_token_no_secrets() {
    let dir = tempfile::TempDir::new().unwrap();
    let store = Arc::new(SecurityStore::in_memory().unwrap());
    let vault_path = dir.path().join("test.vault");
    let mgr = SharedTokenManager::new(store, vault_path);
    // No token generated — operations should fail
    assert!(mgr.store_secret("x", "y").is_err());
}
```

**Step 2: Run tests to verify they fail**

Run: `cargo test -p alephcore --lib shared_token -- --nocapture 2>&1 | tail -20`
Expected: FAIL — methods don't exist yet.

**Step 3: Implement SharedTokenManager vault integration**

```rust
use std::sync::RwLock;
use crate::secrets::vault::SecretVault;
use crate::secrets::crypto::SecretsCrypto;
use crate::secrets::types::{DecryptedSecret, EntryMetadata, SecretError};

pub struct SharedTokenManager {
    store: Arc<SecurityStore>,
    secret: [u8; 32],
    current_token: Mutex<Option<String>>,
    vault: RwLock<SecretVault>,  // NEW
}

impl SharedTokenManager {
    pub fn new(store: Arc<SecurityStore>, vault_path: impl Into<std::path::PathBuf>) -> Self {
        let secret = store
            .get_shared_token_secret()
            .ok()
            .flatten()
            .unwrap_or_else(generate_secret);
        let vault = SecretVault::open(vault_path).unwrap_or_else(|e| {
            tracing::warn!("Failed to open vault, creating empty: {}", e);
            SecretVault::empty(SecretVault::default_path())
        });
        Self {
            store,
            secret,
            current_token: Mutex::new(None),
            vault: RwLock::new(vault),
        }
    }

    /// Get the crypto engine using current token as master key.
    fn crypto(&self) -> Result<SecretsCrypto, SharedTokenError> {
        let token = self.current_token.lock().unwrap_or_else(|e| e.into_inner());
        let token = token.as_ref()
            .ok_or_else(|| SharedTokenError::Storage("No token set — cannot access vault".into()))?;
        Ok(SecretsCrypto::new(token))
    }

    pub fn store_secret(&self, name: &str, value: &str) -> Result<(), SharedTokenError> {
        let crypto = self.crypto()?;
        let encrypted = crypto.encrypt(value.as_bytes())
            .map_err(|e| SharedTokenError::Storage(format!("Encryption failed: {}", e)))?;
        let now = chrono::Utc::now().timestamp();
        let entry = crate::secrets::types::EncryptedEntry {
            ciphertext: encrypted.ciphertext,
            nonce: encrypted.nonce,
            salt: encrypted.salt,
            created_at: now,
            updated_at: now,
            metadata: EntryMetadata {
                description: Some(format!("API key for {}", name)),
                provider: Some(name.to_string()),
            },
        };
        let mut vault = self.vault.write().unwrap_or_else(|e| e.into_inner());
        vault.set(name, entry)
            .map_err(|e| SharedTokenError::Storage(e.to_string()))
    }

    pub fn get_secret(&self, name: &str) -> Result<Option<DecryptedSecret>, SharedTokenError> {
        let crypto = self.crypto()?;
        let vault = self.vault.read().unwrap_or_else(|e| e.into_inner());
        match vault.get(name) {
            Ok(entry) => {
                let decrypted = crypto.decrypt(&entry.ciphertext, &entry.nonce, &entry.salt)
                    .map_err(|e| SharedTokenError::Storage(format!("Decryption failed: {}", e)))?;
                let secret_str = String::from_utf8(decrypted)
                    .map_err(|_| SharedTokenError::Storage("Invalid UTF-8 in secret".into()))?;
                Ok(Some(DecryptedSecret::new(secret_str)))
            }
            Err(SecretError::NotFound(_)) => Ok(None),
            Err(e) => Err(SharedTokenError::Storage(e.to_string())),
        }
    }

    pub fn delete_secret(&self, name: &str) -> Result<bool, SharedTokenError> {
        let mut vault = self.vault.write().unwrap_or_else(|e| e.into_inner());
        vault.delete(name)
            .map_err(|e| SharedTokenError::Storage(e.to_string()))
    }

    pub fn list_secret_names(&self) -> Result<Vec<String>, SharedTokenError> {
        let vault = self.vault.read().unwrap_or_else(|e| e.into_inner());
        Ok(vault.list_names())
    }
}
```

**Step 4: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib shared_token -- --nocapture`
Expected: All tests PASS.

**Step 5: Commit**

```
git commit -m "security: add vault integration to SharedTokenManager"
```

---

## Task 3: Implement reset_token with Re-Encryption

**Files:**
- Modify: `src/gateway/security/shared_token.rs`

**Step 1: Write failing test**

```rust
#[test]
fn test_reset_token_reencrypts_secrets() {
    let dir = tempfile::TempDir::new().unwrap();
    let store = Arc::new(SecurityStore::in_memory().unwrap());
    let vault_path = dir.path().join("test.vault");
    let mgr = SharedTokenManager::new(store, vault_path);

    let old_token = mgr.generate_token().unwrap();
    mgr.store_secret("anthropic", "sk-ant-secret").unwrap();
    mgr.store_secret("openai", "sk-openai-key").unwrap();

    let new_token = mgr.reset_token().unwrap();
    assert_ne!(old_token, new_token);

    // Secrets still accessible with new token
    let s1 = mgr.get_secret("anthropic").unwrap().unwrap();
    assert_eq!(s1.expose(), "sk-ant-secret");
    let s2 = mgr.get_secret("openai").unwrap().unwrap();
    assert_eq!(s2.expose(), "sk-openai-key");

    // Old token no longer validates
    assert!(!mgr.validate(&old_token).unwrap());
    assert!(mgr.validate(&new_token).unwrap());
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib test_reset_token_reencrypts -- --nocapture`
Expected: FAIL — `reset_token` method doesn't exist.

**Step 3: Implement reset_token**

```rust
pub fn reset_token(&self) -> Result<String, SharedTokenError> {
    let old_crypto = self.crypto()?;

    // 1. Decrypt all entries with old token
    let vault = self.vault.read().unwrap_or_else(|e| e.into_inner());
    let mut plaintext_entries: Vec<(String, Vec<u8>, EncryptedEntry)> = Vec::new();
    for (name, entry) in vault.entries() {
        let decrypted = old_crypto.decrypt(&entry.ciphertext, &entry.nonce, &entry.salt)
            .map_err(|e| SharedTokenError::Storage(format!("Decrypt failed for {}: {}", name, e)))?;
        plaintext_entries.push((name.clone(), decrypted, entry.clone()));
    }
    drop(vault);

    // 2. Generate new token (updates HMAC, current_token)
    let new_token = self.generate_token()?;
    let new_crypto = self.crypto()?;

    // 3. Re-encrypt all entries with new token
    let mut new_entries = HashMap::new();
    for (name, plaintext, old_entry) in plaintext_entries {
        let encrypted = new_crypto.encrypt(&plaintext)
            .map_err(|e| SharedTokenError::Storage(format!("Re-encrypt failed for {}: {}", name, e)))?;
        new_entries.insert(name, EncryptedEntry {
            ciphertext: encrypted.ciphertext,
            nonce: encrypted.nonce,
            salt: encrypted.salt,
            created_at: old_entry.created_at,
            updated_at: chrono::Utc::now().timestamp(),
            metadata: old_entry.metadata,
        });
    }

    // 4. Atomic replace
    let mut vault = self.vault.write().unwrap_or_else(|e| e.into_inner());
    vault.replace_all(new_entries)
        .map_err(|e| SharedTokenError::Storage(e.to_string()))?;

    Ok(new_token)
}
```

**Step 4: Run tests**

Run: `cargo test -p alephcore --lib shared_token -- --nocapture`
Expected: All PASS.

**Step 5: Commit**

```
git commit -m "security: implement reset_token with vault re-encryption"
```

---

## Task 4: Delete Vault RPC Handlers and Migration System

**Files:**
- Delete: `src/gateway/handlers/vault_config.rs`
- Delete: `src/secrets/migration.rs`
- Delete: `src/secrets/router.rs`
- Delete: `src/secrets/cache.rs`
- Delete: `src/secrets/provider/local_vault.rs`
- Modify: `src/gateway/handlers/mod.rs` (remove vault handler registrations + module declaration)
- Modify: `src/secrets/mod.rs` (remove deleted module declarations)
- Modify: `src/secrets/provider/mod.rs` (remove local_vault module)

**Step 1: Delete files**

```bash
rm src/gateway/handlers/vault_config.rs
rm src/secrets/migration.rs
rm src/secrets/router.rs
rm src/secrets/cache.rs
rm src/secrets/provider/local_vault.rs
```

**Step 2: Remove vault handler registrations from `src/gateway/handlers/mod.rs`**

Delete lines registering `vault.status`, `vault.storeKey`, `vault.deleteKey`, `vault.verify`, `vault.migrateKeys`, `vault.disableVault`. Remove `pub mod vault_config;` declaration.

**Step 3: Clean up `src/secrets/mod.rs`**

```rust
// Remove:
pub mod cache;
pub mod migration;
pub mod router;

// Remove re-exports of deleted items:
// pub use provider::local_vault::LocalVaultProvider;
// pub use router::{AsyncSecretResolver, SecretRouter};
// pub use provider::{ProviderStatus, SecretMetadata, SecretProvider};

// Remove from vault re-exports:
// resolve_master_key, get_master_key_from_keyring, store_master_key_to_keyring,
// delete_master_key_from_keyring, vault_status, VaultStatus
```

**Step 4: Clean up `src/secrets/provider/mod.rs`**

Remove `pub mod local_vault;` declaration. Keep provider trait if used by 1Password provider or web3_signer. If SecretProvider trait is only used by LocalVaultProvider and 1Password, evaluate whether to keep or simplify.

**Step 5: Compile check**

Run: `cargo check -p alephcore 2>&1 | head -80`
Expected: Errors from remaining consumers (providers.rs, embedding_providers.rs, etc.) — fixed in Task 5.

**Step 6: Commit**

```
git commit -m "secrets: delete vault RPC, migration, router, cache, local vault provider"
```

---

## Task 5: Remove `api_key` / `secret_name` from ProviderConfig

**Files:**
- Modify: `src/config/types/provider.rs` — remove `api_key` and `secret_name` fields
- Modify: all files that reference `provider.api_key` or `provider.secret_name`

**Step 1: Remove fields from ProviderConfig**

In `src/config/types/provider.rs`:
- Delete `pub api_key: Option<String>` field
- Delete `pub secret_name: Option<String>` field
- Update `test_config()` — remove `api_key` and `secret_name` from constructor
- Update all Default/builder patterns

**Step 2: Fix all compilation errors**

Key files that need updating (search for `api_key` and `secret_name` in `src/`):

- `src/providers/auth_profile_registry.rs` — remove `api_key` / `secret_name` from ProviderConfig construction
- `src/providers/failover.rs` — remove `api_key = None` in test configs
- `src/providers/mod.rs` — remove `api_key = None` in test configs
- `src/providers/ollama.rs` — remove `api_key = None`
- `src/providers/profile_config.rs` — update ProfileApiConfig to not write `api_key` to ProviderConfig
- `src/config/patcher.rs` — remove vault field from ConfigPatcher, update constructor
- `src/builtin_tools/config_update.rs` — remove secrets/vault references

For provider factory code that resolves API keys at runtime, the key now comes from `SharedTokenManager.get_secret(provider_name)` instead of `config.api_key`. This is wired up in Task 6.

**Step 3: Compile check**

Run: `cargo check -p alephcore 2>&1 | head -80`

**Step 4: Commit**

```
git commit -m "config: remove api_key and secret_name from ProviderConfig"
```

---

## Task 6: Update Provider Handlers to Use SharedTokenManager

The RPC handlers that store/retrieve API keys (providers.rs, embedding_providers.rs, generation_providers.rs, search_config.rs) currently open SecretVault directly with `resolve_master_key()`. Replace with `SharedTokenManager`.

**Files:**
- Modify: `src/gateway/handlers/providers.rs`
- Modify: `src/gateway/handlers/embedding_providers.rs`
- Modify: `src/gateway/handlers/generation_providers.rs`
- Modify: `src/gateway/handlers/search_config.rs`

**Step 1: Determine how handlers access SharedTokenManager**

Handlers receive `JsonRpcRequest` which has access to `AppContext` or similar shared state. Find how SharedTokenManager is currently injected/accessible in the handler context.

Check: `src/gateway/handlers/mod.rs` for `HandlerRegistry` and how it provides dependencies to handlers.

**Step 2: Replace vault access pattern**

In each handler file, replace this pattern:
```rust
// OLD:
let master_key = resolve_master_key().map_err(|e| ...)?;
let mut vault = SecretVault::open(SecretVault::default_path(), &master_key).map_err(|e| ...)?;
vault.set(&secret_name, api_key, metadata).map_err(|e| ...)?;

// NEW:
// Access SharedTokenManager from handler context
token_manager.store_secret(&provider_name, &api_key).map_err(|e| ...)?;
```

And for reads:
```rust
// OLD:
let vault = SecretVault::open(...)?;
let secret = vault.get(&secret_name)?;

// NEW:
let secret = token_manager.get_secret(&provider_name)?;
```

**Step 3: Remove `store_provider_api_key`, `resolve_test_api_key`, `store_embedding_api_key` helper functions**

These are no longer needed — `SharedTokenManager` handles everything.

**Step 4: Simplify `build_provider_config_for_persistence` / `prepare_embedding_config_for_persistence`**

No more branching on `secret_name` vs `api_key`. When user provides an API key:
1. Store in vault via `token_manager.store_secret(provider_name, key)`
2. Save ProviderConfig without any key fields (just model, url, etc.)

**Step 5: Compile and test**

Run: `cargo check -p alephcore`
Run: `cargo test -p alephcore --lib`

**Step 6: Commit**

```
git commit -m "handlers: replace direct vault access with SharedTokenManager"
```

---

## Task 7: Update Startup Code

**Files:**
- Modify: `src/bin/aleph/commands/start/builder/subsystems.rs`

**Step 1: Rewrite `load_app_config()`**

The function currently resolves master key, opens vault, runs migration, builds SecretRouter, resolves provider secrets. All of this is replaced by SharedTokenManager.

```rust
pub(in crate::commands::start) fn load_app_config() -> alephcore::Config {
    // Just load config — no vault, no migration, no secret resolution
    match alephcore::Config::load() {
        Ok(cfg) => cfg,
        Err(e) => {
            eprintln!("Error loading application config: {}", e);
            std::process::exit(1);
        }
    }
}
```

Note: `load_app_config` no longer needs to be `async` since we removed vault/migration.

**Step 2: Update SharedTokenManager construction**

Where SharedTokenManager is constructed (in `build_auth_bundle` or nearby), pass vault_path:

```rust
let vault_path = dirs::home_dir()
    .unwrap_or_else(|| std::path::PathBuf::from("/tmp"))
    .join(".aleph/data/secrets.vault");
let shared_token_mgr = Arc::new(SharedTokenManager::new(security_store.clone(), vault_path));
```

**Step 3: Wire SharedTokenManager into handler context**

Ensure the `SharedTokenManager` Arc is accessible from RPC handlers. Check how existing dependencies are injected (likely through `AppContext` or `ServerState`).

**Step 4: Ensure provider factory uses SharedTokenManager**

At provider initialization time, resolve API keys from vault:
```rust
for (name, _config) in &config.providers {
    if let Ok(Some(secret)) = shared_token_mgr.get_secret(name) {
        // Pass secret to provider factory
    }
}
```

**Step 5: Compile and test full startup**

Run: `cargo check -p alephcore`
Run: `cargo run --bin aleph -- --help` (sanity check)

**Step 6: Commit**

```
git commit -m "startup: wire SharedTokenManager vault into server initialization"
```

---

## Task 8: Delete Integration Tests and Unused Modules

**Files:**
- Delete: `tests/secret_router_integration.rs`
- Delete: `tests/secret_boundary_integration.rs` (if exists and only tests deleted code)
- Modify: `src/secrets/provider/mod.rs` — evaluate if SecretProvider trait is still needed
- Modify: `src/bin/aleph/commands/secret.rs` — update or remove CLI secret commands

**Step 1: Delete integration tests for removed code**

```bash
rm tests/secret_router_integration.rs
# Check if secret_boundary_integration.rs exists and is only for deleted code
```

**Step 2: Clean up provider module**

If `SecretProvider` trait is only used by deleted `LocalVaultProvider` and `OnePasswordProvider`, and OnePasswordProvider is no longer wired (since SecretRouter is deleted), remove `src/secrets/provider/` entirely or simplify.

**Step 3: Update CLI secret commands**

Check `src/bin/aleph/commands/secret.rs` — if it uses `resolve_master_key` or `SecretVault` directly, update to use SharedTokenManager or remove CLI vault commands.

**Step 4: Remove `keyring` dependency from Cargo.toml if no longer used**

Check if `keyring` crate is only used by deleted vault keychain code. If so, remove from `Cargo.toml`.

**Step 5: Full compile and test**

Run: `cargo check -p alephcore`
Run: `cargo test -p alephcore --lib`

**Step 6: Commit**

```
git commit -m "cleanup: remove vault integration tests and unused secret provider modules"
```

---

## Task 9: Update ConfigPatcher Vault References

**Files:**
- Modify: `src/config/patcher.rs` — remove `vault: Option<Arc<Mutex<SecretVault>>>` field
- Modify: `src/builtin_tools/config_update.rs` — remove secrets/vault references

**Step 1: Remove vault field from ConfigPatcher**

In `src/config/patcher.rs`:
- Remove `vault` field from `ConfigPatcher` struct
- Remove `vault` parameter from `ConfigPatcher::new()`
- Update any methods that use vault for secret storage — they should delegate to SharedTokenManager instead (or remove if config_update tool no longer handles secrets)

**Step 2: Update config_update tool**

In `src/builtin_tools/config_update.rs`:
- Remove `secrets` field from `ConfigUpdateArgs` if it directly stores vault entries
- Or redirect to SharedTokenManager for secret storage
- Update tool description to not mention SecretVault

**Step 3: Fix all callers of `ConfigPatcher::new()`**

Search for `ConfigPatcher::new(` and remove the vault argument.

**Step 4: Compile and test**

Run: `cargo check -p alephcore`
Run: `cargo test -p alephcore --lib`

**Step 5: Commit**

```
git commit -m "config: remove vault dependency from ConfigPatcher and config_update tool"
```

---

## Task 10: Final Cleanup and Verification

**Step 1: Search for any remaining references to deleted items**

```bash
cargo check -p alephcore 2>&1 | grep "error"
```

Search for orphaned references:
- `resolve_master_key`
- `ALEPH_MASTER_KEY`
- `VaultStatus`
- `vault_status`
- `LocalVaultProvider`
- `SecretRouter`
- `AsyncSecretResolver`
- `needs_migration`
- `migrate_api_keys`
- `secret_name` (in config context)

**Step 2: Run full test suite**

Run: `cargo test -p alephcore --lib`

**Step 3: Run clippy**

Run: `cargo clippy -p alephcore -- -D warnings 2>&1 | head -30`

**Step 4: Manual smoke test**

Run: `cargo run --bin aleph`
- Verify token is generated and displayed
- Verify vault file is created at `~/.aleph/data/secrets.vault`
- Verify panel UI can connect with token

**Step 5: Commit**

```
git commit -m "cleanup: final token-vault fusion verification"
```

---

## Dependency Graph

```
Task 1 (Simplify SecretVault) ─┐
                                ├─ Task 2 (Extend SharedTokenManager) ─── Task 3 (reset_token)
Task 4 (Delete vault RPC)     ─┤
                                ├─ Task 5 (Remove api_key/secret_name) ── Task 6 (Update handlers)
                                │                                          │
                                └─ Task 7 (Update startup) ───────────────┘
                                                                           │
                                Task 8 (Delete integration tests) ─────────┤
                                Task 9 (Update ConfigPatcher) ─────────────┤
                                                                           │
                                Task 10 (Final cleanup) ───────────────────┘
```

Tasks 1 and 4 can run in parallel. Tasks 2 depends on 1. Tasks 5 depends on 4. Task 6 depends on 2+5. Task 7 depends on 2+5. Tasks 8 and 9 can run after 6+7. Task 10 is last.
