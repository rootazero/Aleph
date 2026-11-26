// core/src/gateway/security/shared_token.rs

//! Shared Token Management
//!
//! A single shared token used as the "entry key" for UI login and API access.
//! The plaintext token is held in memory; only the hash is stored in `SQLite`.
//! Also manages the encrypted secret vault, using the token as master key.

use super::crypto::{generate_secret, hmac_sign};
use super::store::SecurityStore;
use crate::capability::{CapabilitySlot, MissingSemantics, SlotStatus};
use crate::providers::model_catalog::discovery_authority;
use crate::secrets::crypto::SecretsCrypto;
use crate::secrets::types::{DecryptedSecret, EncryptedEntry, EntryMetadata, SecretError};
use crate::secrets::vault::SecretVault;
use crate::sync_primitives::{Arc, Mutex, RwLock};
use uuid::Uuid;
use zeroize::Zeroizing;

/// `ConsumerDecides`, and the sharpest instance the round has met so far: six
/// production reads, five distinct dispositions, one of which reports success.
///
/// | reader | an uninstalled read becomes |
/// |---|---|
/// | `gateway/server/connection/mod.rs` (connect) | the validator closure returns `false` — every remote token presentation rejected |
/// | `extension/runtime/wasm/secret_resolver.rs` | a `DenyAllSecretResolver`, deliberately, per its own comment |
/// | `gateway/handlers/gateway_token.rs::handle_token_current` | `{"token": null}` returned as **success** |
/// | `gateway/handlers/gateway_token.rs::handle_token_rotate` | `INTERNAL_ERROR "shared token manager unavailable"` |
/// | `extension/plugin_ops.rs` | no resolver — plugin settings ship with secret references unresolved |
/// | `gateway/interfaces/whatsapp/wa_auth/vault_store.rs` (via [`SharedTokenManager::global_crypto`]) | `crypto: None` |
///
/// No single `reads_as` sentence covers a set that contains both "reject every
/// remote login" and "answer `null` and call it success", which is precisely
/// what this variant is for. Adjudicating the arms is Task 15's; recording
/// that there are five of them is this one's.
///
/// ⚠️ `global_crypto` collapses two different absences into one `None` — "no
/// manager installed" and "a manager with no plaintext token loaded" (its
/// `crypto()` returns `Err` for the second). Only the first is this slot's.
static GLOBAL_SHARED_TOKEN_MANAGER: CapabilitySlot<Arc<SharedTokenManager>> = CapabilitySlot::new(
    "gateway/shared-token-manager",
    MissingSemantics::ConsumerDecides,
);

/// The handle above, type-erased for the roster — see
/// [`crate::spend::global_ledger_slot`] for why this shape.
///
/// A free function rather than a third associated item on
/// [`SharedTokenManager`], so that all twelve accessors in this batch have one
/// shape: the roster reads the return type, and a reader comparing files
/// should not have to notice that this one is spelled differently.
pub(crate) const fn global_shared_token_manager_slot() -> &'static dyn SlotStatus {
    &GLOBAL_SHARED_TOKEN_MANAGER
}

/// Manages a single shared token for UI/API authentication.
///
/// On generation, the plaintext is kept in memory and the HMAC hash
/// is persisted to `SQLite` via `SecurityStore`. Validation re-computes
/// the HMAC and compares against the stored hash.
///
/// Also owns a `SecretVault` for encrypted secret storage, using the
/// current token as the master key for encryption/decryption.
pub struct SharedTokenManager {
    store: Arc<SecurityStore>,
    secret: [u8; 32],
    current_token: Mutex<Option<String>>,
    vault: RwLock<SecretVault>,
}

#[derive(Debug, thiserror::Error)]
pub enum SharedTokenError {
    #[error("Storage error: {0}")]
    Storage(String),
}

impl SharedTokenManager {
    /// Create a new manager, restoring persisted HMAC secret if available.
    ///
    /// If the store already has a persisted HMAC secret, it is reused so that
    /// existing tokens remain valid across restarts and updates.
    pub fn new(store: Arc<SecurityStore>, vault_path: impl Into<std::path::PathBuf>) -> Self {
        let secret = match store.get_shared_token_secret() {
            Ok(Some(s)) => {
                tracing::info!("SharedTokenManager: restored persisted HMAC secret from DB");
                s
            }
            Ok(None) => {
                tracing::info!(
                    "SharedTokenManager: no persisted HMAC secret found, generating new one"
                );
                generate_secret()
            }
            Err(e) => {
                tracing::warn!("SharedTokenManager: failed to load HMAC secret from DB: {}, generating new one — THIS WILL INVALIDATE EXISTING TOKENS", e);
                generate_secret()
            }
        };
        let vault_path = vault_path.into();
        // Use `open_or_backup`, not `open().unwrap_or_else(empty)`: the latter
        // silently overwrites an existing-but-unreadable vault on the next
        // save, destroying all stored secrets.
        let vault = SecretVault::open_or_backup(vault_path);
        Self {
            store,
            secret,
            current_token: Mutex::new(None),
            vault: RwLock::new(vault),
        }
    }

    /// Create with a specific secret (for testing).
    pub fn with_secret(
        store: Arc<SecurityStore>,
        secret: [u8; 32],
        vault_path: impl Into<std::path::PathBuf>,
    ) -> Self {
        let vault_path = vault_path.into();
        // Use `open_or_backup`, not `open().unwrap_or_else(empty)`: the latter
        // silently overwrites an existing-but-unreadable vault on the next
        // save, destroying all stored secrets.
        let vault = SecretVault::open_or_backup(vault_path);
        Self {
            store,
            secret,
            current_token: Mutex::new(None),
            vault: RwLock::new(vault),
        }
    }

    /// Generate a new shared token (invalidates any previous one).
    /// Persists hash, HMAC secret, and plaintext so the token survives restarts
    /// even if the token file is lost.
    pub fn generate_token(&self) -> Result<String, SharedTokenError> {
        let token = format!("aleph-{}", Uuid::new_v4());
        self.commit_token(&token, false)?;
        Ok(token)
    }

    /// Persist `token` as the live shared token and make it the in-memory one.
    ///
    /// Validation only ever compares against the persisted HMAC hash, so this
    /// is the moment the previous token stops being accepted. Callers that must
    /// keep the old token working until another store is in sync (see
    /// [`Self::reset_token`]) therefore call this last.
    fn commit_token(&self, token: &str, vault_rewrapped: bool) -> Result<(), SharedTokenError> {
        let hash = hmac_sign(&self.secret, token);

        self.store
            .set_shared_token_with_secret(&hash, &self.secret, Some(token))
            .map_err(|e| SharedTokenError::Storage(e.to_string()))?;

        let mut current = self.current_token.lock().unwrap_or_else(|e| e.into_inner());
        if vault_rewrapped {
            *current = Some(token.to_owned());
        } else {
            let mut fence = discovery_authority::mutation_fence();
            *current = Some(token.to_owned());
            fence.vault_changed(|_| true);
        }
        Ok(())
    }

    /// Validate a token against the stored hash.
    pub fn validate(&self, token: &str) -> Result<bool, SharedTokenError> {
        let hash = hmac_sign(&self.secret, token);
        self.store
            .validate_shared_token_hash(&hash)
            .map_err(|e| SharedTokenError::Storage(e.to_string()))
    }

    /// Try to load the token from the database.
    ///
    /// Returns `Some(token)` if the DB has a plaintext token that passes HMAC validation.
    pub fn try_load_token_from_db(&self) -> Option<String> {
        let token = match self.store.get_shared_token_plaintext() {
            Ok(Some(t)) => t,
            Ok(None) => {
                tracing::info!("SharedTokenManager: no token in DB — first start");
                return None;
            }
            Err(e) => {
                tracing::warn!(
                    "SharedTokenManager: failed to read plaintext token from DB: {}",
                    e
                );
                return None;
            }
        };
        // Validate the recovered token against the stored hash
        match self.validate(&token) {
            Ok(true) => {
                tracing::info!("SharedTokenManager: loaded existing token from DB");
                let mut current = self.current_token.lock().unwrap_or_else(|e| e.into_inner());
                if current.as_deref() != Some(token.as_str()) {
                    let mut fence = discovery_authority::mutation_fence();
                    *current = Some(token.clone());
                    fence.vault_changed(|_| true);
                }
                Some(token)
            }
            Ok(false) => {
                tracing::warn!("SharedTokenManager: DB plaintext token failed HMAC validation (inconsistent state)");
                None
            }
            Err(e) => {
                tracing::warn!(
                    "SharedTokenManager: token validation error during DB recovery: {}",
                    e
                );
                None
            }
        }
    }

    /// Get the current plaintext token (only if this process generated or loaded it).
    pub fn get_current_token(&self) -> Option<String> {
        self.current_token
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub fn set_global(manager: Arc<SharedTokenManager>) {
        let _ = GLOBAL_SHARED_TOKEN_MANAGER.install(manager);
    }

    pub fn global() -> Option<Arc<SharedTokenManager>> {
        GLOBAL_SHARED_TOKEN_MANAGER.get().cloned()
    }

    /// Validate a token against the process-global manager's stored hash.
    ///
    /// `false` when no global manager is installed (hosts without a shared
    /// token) or when validation errors — fail closed. This is the ONE
    /// derivation of "is this the shared gateway token": the WS `connect` arm
    /// calls it directly at both dispatch stations and the MCP face wraps it in
    /// [`crate::gateway::mcp_face::auth::production_shared_token_validator`], so
    /// the three call sites cannot drift apart.
    pub fn global_validate(token: &str) -> bool {
        Self::global()
            .map(|m| m.validate(token).unwrap_or(false))
            .unwrap_or(false)
    }

    pub fn global_crypto() -> Option<SecretsCrypto> {
        GLOBAL_SHARED_TOKEN_MANAGER
            .get()
            .and_then(|mgr| mgr.crypto().ok())
    }

    /// Check if the store already has a shared token hash (regardless of whether
    /// we have the plaintext).
    pub fn has_stored_token(&self) -> bool {
        self.store.has_shared_token().unwrap_or(false)
    }

    /// Get the HMAC secret.
    pub const fn secret(&self) -> &[u8; 32] {
        &self.secret
    }

    /// Get a crypto engine using the current token as master key.
    fn crypto(&self) -> Result<SecretsCrypto, SharedTokenError> {
        let token = self.current_token.lock().unwrap_or_else(|e| e.into_inner());
        let token = token.as_ref().ok_or_else(|| {
            SharedTokenError::Storage("No token set — cannot access vault".into())
        })?;
        Ok(SecretsCrypto::new(token))
    }

    /// Store an encrypted secret in the vault.
    pub fn store_secret(&self, name: &str, value: &str) -> Result<(), SharedTokenError> {
        let mut vault = self.vault.write().unwrap_or_else(|e| e.into_inner());
        let entry = Self::encrypt_secret(&self.crypto()?, name, value)?;
        vault
            .set(name, entry)
            .map_err(|e| SharedTokenError::Storage(e.to_string()))
    }

    /// Store a credential set in one vault write, or leave every old entry intact.
    pub(crate) fn store_secret_batch<'a>(
        &self,
        secrets: impl IntoIterator<Item = (&'a str, &'a str)>,
    ) -> Result<(), SharedTokenError> {
        let mut secrets = secrets.into_iter().peekable();
        if secrets.peek().is_none() {
            return Ok(());
        }
        let mut vault = self.vault.write().unwrap_or_else(|e| e.into_inner());
        let entries = self.encrypt_secret_batch(&vault, secrets)?;
        vault
            .replace_all(entries)
            .map_err(|error| SharedTokenError::Storage(error.to_string()))
    }

    /// Capture the OAuth bearer, grant and scope epoch in one read transaction.
    /// Config writers are excluded by the caller's guard; vault writers must
    /// take the write lock before mutating either credential and its epoch.
    pub(crate) fn oauth_refresh_snapshot(
        &self,
        config: &tokio::sync::RwLockReadGuard<'_, crate::Config>,
        provider: &str,
        bearer_key: &str,
        grant_key: &str,
    ) -> Result<Option<(DecryptedSecret, DecryptedSecret, u64)>, SharedTokenError> {
        let vault = self.vault.read().unwrap_or_else(|e| e.into_inner());
        let epoch = crate::providers::model_catalog::discovery::oauth_route_epoch(provider, config);
        let Some(bearer) = self.decrypt_secret(&vault, bearer_key)? else {
            return Ok(None);
        };
        let Some(grant) = self.decrypt_secret(&vault, grant_key)? else {
            return Ok(None);
        };
        Ok(Some((bearer, grant, epoch)))
    }

    /// Commit a credential refresh and its already-built transport at the same
    /// mutation boundary. The synchronous callback may not read vault or config.
    pub(crate) fn refresh_secrets_if_current<'a, T>(
        &self,
        provider: &str,
        epoch: u64,
        expected: impl IntoIterator<Item = (&'a str, &'a str)>,
        secrets: impl IntoIterator<Item = (&'a str, &'a str)>,
        install: impl FnOnce() -> T,
    ) -> Result<Option<T>, SharedTokenError> {
        let mut vault = self.vault.write().unwrap_or_else(|e| e.into_inner());
        for (name, value) in expected {
            let Some(stored) = self.decrypt_secret(&vault, name)? else {
                return Ok(None);
            };
            if stored.expose() != value {
                return Ok(None);
            }
        }
        let entries = self.encrypt_secret_batch(&vault, secrets)?;
        let mut fence = crate::providers::model_catalog::discovery_authority::mutation_fence();
        if !crate::providers::model_catalog::discovery::scope_epoch_current(provider, epoch) {
            return Ok(None);
        }
        vault
            .replace_credentials(entries, &mut fence)
            .map_err(|error| SharedTokenError::Storage(error.to_string()))?;
        Ok(Some(install()))
    }

    fn encrypt_secret_batch<'a>(
        &self,
        vault: &SecretVault,
        secrets: impl IntoIterator<Item = (&'a str, &'a str)>,
    ) -> Result<std::collections::HashMap<String, EncryptedEntry>, SharedTokenError> {
        // Rotation holds the vault write lock until the current token changes.
        let crypto = self.crypto()?;
        let mut entries = vault.entries().clone();
        for (name, value) in secrets {
            let mut entry = Self::encrypt_secret(&crypto, name, value)?;
            if let Some(previous) = entries.get(name) {
                entry.created_at = previous.created_at;
            }
            entries.insert(name.to_owned(), entry);
        }
        Ok(entries)
    }

    fn encrypt_secret(
        crypto: &SecretsCrypto,
        name: &str,
        value: &str,
    ) -> Result<EncryptedEntry, SharedTokenError> {
        let encrypted = crypto
            .encrypt(value)
            .map_err(|e| SharedTokenError::Storage(format!("Encryption failed: {e}")))?;
        let now = chrono::Utc::now().timestamp();
        Ok(EncryptedEntry {
            ciphertext: encrypted.ciphertext,
            nonce: encrypted.nonce,
            salt: encrypted.salt,
            created_at: now,
            updated_at: now,
            metadata: EntryMetadata {
                description: Some(format!("API key for {name}")),
                provider: Some(name.to_string()),
            },
        })
    }

    /// Get a decrypted secret from the vault.
    pub fn get_secret(&self, name: &str) -> Result<Option<DecryptedSecret>, SharedTokenError> {
        let vault = self.vault.read().unwrap_or_else(|e| e.into_inner());
        self.decrypt_secret(&vault, name)
    }

    fn decrypt_secret(
        &self,
        vault: &SecretVault,
        name: &str,
    ) -> Result<Option<DecryptedSecret>, SharedTokenError> {
        let crypto = self.crypto()?;
        match vault.get(name) {
            Ok(entry) => {
                let decrypted = crypto
                    .decrypt(&entry.ciphertext, &entry.nonce, &entry.salt)
                    .map_err(|e| SharedTokenError::Storage(format!("Decryption failed: {e}")))?;
                Ok(Some(DecryptedSecret::new(decrypted)))
            }
            Err(SecretError::NotFound(_)) => Ok(None),
            Err(e) => Err(SharedTokenError::Storage(e.to_string())),
        }
    }

    /// Delete a secret from the vault.
    pub fn delete_secret(&self, name: &str) -> Result<bool, SharedTokenError> {
        let mut vault = self.vault.write().unwrap_or_else(|e| e.into_inner());
        vault
            .delete(name)
            .map_err(|e| SharedTokenError::Storage(e.to_string()))
    }

    /// List all secret names in the vault.
    pub fn list_secret_names(&self) -> Result<Vec<String>, SharedTokenError> {
        let vault = self.vault.read().unwrap_or_else(|e| e.into_inner());
        Ok(vault.list_names())
    }

    /// Reset the token and re-encrypt all vault entries with the new token.
    ///
    /// Flow:
    /// 1. Decrypt all entries with the current token and re-encrypt them
    ///    with a freshly minted token that is not persisted yet
    /// 2. Commit the re-encrypted vault
    /// 3. Persist the new token; if that fails, roll the vault back to the
    ///    old ciphertexts so the still-live old token can decrypt them
    ///
    /// The vault is committed before the token flips because the vault write
    /// is the failure-prone step (filesystem), and because the rollback must
    /// not depend on the component that just failed (the token store).
    pub fn reset_token(&self) -> Result<String, SharedTokenError> {
        use std::collections::HashMap;

        let new_token = format!("aleph-{}", Uuid::new_v4());
        let new_crypto = SecretsCrypto::new(new_token.clone());

        let mut new_entries = HashMap::new();
        let mut old_entries = HashMap::new();

        // The vault write lock is held for the whole rotation: the replacement
        // below drops every entry it does not carry, so a concurrent
        // `store_secret` must not be able to slip in between the read and the
        // replace — and it must not be able to write an entry under the old
        // key either, after the vault has been re-encrypted.
        let mut vault = self.vault.write().unwrap_or_else(|e| e.into_inner());
        let old_crypto = self.crypto()?;

        // 1. Decrypt all entries with the old token, re-encrypting each one
        //    with the new token.
        // The decrypted value is held in `Zeroizing<String>` so it is wiped
        // from the heap on drop — including every early-return error path
        // below — instead of lingering as a plain `String` until the
        // allocator reuses the pages (audit secrets I-1; same zeroize-on-drop
        // invariant as `DecryptedSecret` and the `Zeroizing<[u8; 32]>` derived
        // keys in `secrets::crypto`).
        for (name, entry) in vault.entries() {
            let decrypted = Zeroizing::new(
                old_crypto
                    .decrypt(&entry.ciphertext, &entry.nonce, &entry.salt)
                    .map_err(|e| {
                        SharedTokenError::Storage(format!("Decrypt failed for '{name}': {e}"))
                    })?,
            );
            let encrypted = new_crypto.encrypt(decrypted.as_str()).map_err(|e| {
                SharedTokenError::Storage(format!("Re-encrypt failed for '{name}': {e}"))
            })?;
            old_entries.insert(name.clone(), entry.clone());
            new_entries.insert(
                name.clone(),
                EncryptedEntry {
                    ciphertext: encrypted.ciphertext,
                    nonce: encrypted.nonce,
                    salt: encrypted.salt,
                    created_at: entry.created_at,
                    updated_at: chrono::Utc::now().timestamp(),
                    metadata: entry.metadata.clone(),
                },
            );
        }

        // 2. Commit the re-encrypted vault. The old token is still the live
        //    credential here, so a failure leaves the vault exactly as it was
        //    and the old token still decrypts it.
        vault
            .rewrap_entries(new_entries)
            .map_err(|e| SharedTokenError::Storage(e.to_string()))?;

        // 3. Persist the new token. If this fails the vault is already
        //    encrypted under the new key, so put the old ciphertexts back: the
        //    old token is still the live one and it can still decrypt them.
        if let Err(e) = self.commit_token(&new_token, true) {
            vault.rewrap_entries(old_entries).map_err(|rollback| {
                discovery_authority::mutation_fence().vault_changed(|_| true);
                SharedTokenError::Storage(format!(
                    "token rotation failed ({e}) and the vault rollback failed as well ({rollback}); the stored secrets may be unreadable"
                ))
            })?;
            return Err(e);
        }

        Ok(new_token)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::security::store::SecurityStore;

    /// See `session::service::tests::the_accessor_exposes_this_handle_to_the_roster`
    /// for why this asserts through the accessor rather than the static.
    #[test]
    fn the_accessor_exposes_this_handle_to_the_roster() {
        let slot = global_shared_token_manager_slot();
        assert_eq!(slot.id(), "gateway/shared-token-manager");
        assert!(matches!(slot.missing(), MissingSemantics::ConsumerDecides));
    }

    #[test]
    fn test_generate_and_validate() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = Arc::new(SecurityStore::in_memory().unwrap());
        let manager = SharedTokenManager::new(store, dir.path().join("test.vault"));
        let token = manager.generate_token().unwrap();
        assert!(!token.is_empty());
        assert!(token.starts_with("aleph-"));
        assert!(manager.validate(&token).unwrap());
    }

    #[test]
    fn test_invalid_token_rejected() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = Arc::new(SecurityStore::in_memory().unwrap());
        let manager = SharedTokenManager::new(store, dir.path().join("test.vault"));
        let _token = manager.generate_token().unwrap();
        assert!(!manager.validate("wrong-token").unwrap());
    }

    #[test]
    fn test_regenerate_invalidates_old() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = Arc::new(SecurityStore::in_memory().unwrap());
        let manager = SharedTokenManager::new(store, dir.path().join("test.vault"));
        let old = manager.generate_token().unwrap();
        let new = manager.generate_token().unwrap();
        assert_ne!(old, new);
        assert!(!manager.validate(&old).unwrap());
        assert!(manager.validate(&new).unwrap());
    }

    #[test]
    fn test_get_current_token() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = Arc::new(SecurityStore::in_memory().unwrap());
        let manager = SharedTokenManager::new(store, dir.path().join("test.vault"));
        assert!(manager.get_current_token().is_none());
        let token = manager.generate_token().unwrap();
        assert_eq!(manager.get_current_token(), Some(token));
    }

    #[test]
    fn test_same_secret_validates() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = Arc::new(SecurityStore::in_memory().unwrap());
        let secret = crate::gateway::security::crypto::generate_secret();
        let manager1 =
            SharedTokenManager::with_secret(store.clone(), secret, dir.path().join("test1.vault"));
        let token = manager1.generate_token().unwrap();

        // Same store + same secret = should validate
        let manager2 =
            SharedTokenManager::with_secret(store, secret, dir.path().join("test2.vault"));
        assert!(manager2.validate(&token).unwrap());
    }

    #[test]
    fn test_token_persists_across_restarts() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = Arc::new(SecurityStore::in_memory().unwrap());

        // First "boot" — generates a token
        let mgr1 = SharedTokenManager::new(store.clone(), dir.path().join("test.vault"));
        let token = mgr1.generate_token().unwrap();
        assert!(mgr1.validate(&token).unwrap());

        // Second "boot" — simulates restart, creates new manager from same store
        let mgr2 = SharedTokenManager::new(store.clone(), dir.path().join("test2.vault"));
        // Should restore the persisted secret and validate the old token
        assert!(mgr2.validate(&token).unwrap());
        // Should load from DB
        let loaded = mgr2.try_load_token_from_db();
        assert_eq!(loaded.as_deref(), Some(token.as_str()));
        assert_eq!(mgr2.get_current_token().as_deref(), Some(token.as_str()));
    }

    #[test]
    fn test_no_regenerate_when_valid_token_exists() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = Arc::new(SecurityStore::in_memory().unwrap());
        let mgr = SharedTokenManager::new(store.clone(), dir.path().join("test.vault"));
        let token = mgr.generate_token().unwrap();

        // After generating, has_stored_token should be true
        assert!(mgr.has_stored_token());

        // New manager from same store — secret restored
        let mgr2 = SharedTokenManager::new(store, dir.path().join("test2.vault"));
        assert!(mgr2.has_stored_token());
        assert!(mgr2.validate(&token).unwrap());
    }

    // --- Secret operations tests ---

    #[test]
    fn test_store_and_get_secret() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = Arc::new(SecurityStore::in_memory().unwrap());
        let mgr = SharedTokenManager::new(store, dir.path().join("test.vault"));
        let _token = mgr.generate_token().unwrap();

        mgr.store_secret("anthropic", "sk-ant-secret").unwrap();
        let secret = mgr.get_secret("anthropic").unwrap().unwrap();
        assert_eq!(secret.expose(), "sk-ant-secret");
    }

    #[test]
    fn test_get_nonexistent_secret() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = Arc::new(SecurityStore::in_memory().unwrap());
        let mgr = SharedTokenManager::new(store, dir.path().join("test.vault"));
        let _token = mgr.generate_token().unwrap();

        assert!(mgr.get_secret("nonexistent").unwrap().is_none());
    }

    #[test]
    fn test_delete_secret() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = Arc::new(SecurityStore::in_memory().unwrap());
        let mgr = SharedTokenManager::new(store, dir.path().join("test.vault"));
        let _token = mgr.generate_token().unwrap();

        mgr.store_secret("openai", "sk-openai-key").unwrap();
        assert!(mgr.delete_secret("openai").unwrap());
        assert!(mgr.get_secret("openai").unwrap().is_none());
    }

    #[test]
    fn test_list_secret_names() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = Arc::new(SecurityStore::in_memory().unwrap());
        let mgr = SharedTokenManager::new(store, dir.path().join("test.vault"));
        let _token = mgr.generate_token().unwrap();

        mgr.store_secret("anthropic", "key1").unwrap();
        mgr.store_secret("openai", "key2").unwrap();
        let mut names = mgr.list_secret_names().unwrap();
        names.sort();
        assert_eq!(names, vec!["anthropic", "openai"]);
    }

    #[test]
    fn concurrent_vault_writes_and_rotations_keep_every_entry_readable() {
        use std::sync::Barrier;
        use std::thread;

        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(SecurityStore::in_memory().unwrap());
        let manager = SharedTokenManager::new(store, dir.path().join("rotate.vault"));
        manager.generate_token().unwrap();
        manager.store_secret("ai:original", "original").unwrap();

        let start = Barrier::new(4);
        thread::scope(|scope| {
            let rotate = || {
                start.wait();
                manager.reset_token().unwrap()
            };
            let first = scope.spawn(rotate);
            let second = scope.spawn(rotate);
            let write = scope.spawn(|| {
                start.wait();
                manager.store_secret("ai:concurrent", "concurrent").unwrap();
            });
            start.wait();
            let first = first.join().unwrap();
            let second = second.join().unwrap();
            write.join().unwrap();
            assert_ne!(first, second);
            assert_ne!(
                manager.validate(&first).unwrap(),
                manager.validate(&second).unwrap()
            );
        });
        for (key, value) in [("ai:original", "original"), ("ai:concurrent", "concurrent")] {
            assert_eq!(manager.get_secret(key).unwrap().unwrap().expose(), value);
        }
    }

    #[test]
    fn test_reset_token_reencrypts_secrets() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = Arc::new(SecurityStore::in_memory().unwrap());
        let mgr = SharedTokenManager::new(store, dir.path().join("test.vault"));

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

    /// A rotation must not be able to destroy the stored secrets when the
    /// vault write fails: the old code had already persisted the new token by
    /// then, which left every entry encrypted under a key nobody held.
    #[test]
    fn a_failed_vault_write_during_rotation_keeps_the_old_token_working() {
        let dir = tempfile::TempDir::new().unwrap();
        let vault_path = dir.path().join("test.vault");
        let store = Arc::new(SecurityStore::in_memory().unwrap());
        let mgr = SharedTokenManager::new(store, vault_path.clone());

        let old_token = mgr.generate_token().unwrap();
        mgr.store_secret("anthropic", "sk-ant-secret").unwrap();

        // Every vault write fails from here on: the vault path is a directory,
        // so the atomic rename cannot land.
        std::fs::remove_file(&vault_path).unwrap();
        std::fs::create_dir(&vault_path).unwrap();

        assert!(mgr.reset_token().is_err());
        assert!(mgr.validate(&old_token).unwrap());
        let secret = mgr.get_secret("anthropic").unwrap().unwrap();
        assert_eq!(secret.expose(), "sk-ant-secret");
    }

    #[test]
    fn test_reset_token_empty_vault() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = Arc::new(SecurityStore::in_memory().unwrap());
        let mgr = SharedTokenManager::new(store, dir.path().join("test.vault"));

        let old_token = mgr.generate_token().unwrap();
        let new_token = mgr.reset_token().unwrap();
        assert_ne!(old_token, new_token);
        assert!(mgr.validate(&new_token).unwrap());
    }

    #[test]
    fn test_no_token_no_secrets() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = Arc::new(SecurityStore::in_memory().unwrap());
        let mgr = SharedTokenManager::new(store, dir.path().join("test.vault"));
        // No token generated — operations should fail
        assert!(mgr.store_secret("x", "y").is_err());
        assert!(mgr.get_secret("x").is_err());
    }

    #[test]
    fn test_secret_persists_with_same_token() {
        let dir = tempfile::TempDir::new().unwrap();
        let vault_path = dir.path().join("persist.vault");
        let store = Arc::new(SecurityStore::in_memory().unwrap());

        // First session: generate token, store secret
        let mgr1 = SharedTokenManager::new(store.clone(), &vault_path);
        let _token = mgr1.generate_token().unwrap();
        mgr1.store_secret("anthropic", "sk-ant-persist").unwrap();

        // Second session: load token from DB, should access same secret
        let mgr2 = SharedTokenManager::new(store, &vault_path);
        assert!(mgr2.try_load_token_from_db().is_some());
        let secret = mgr2.get_secret("anthropic").unwrap().unwrap();
        assert_eq!(secret.expose(), "sk-ant-persist");
    }

    #[test]
    fn test_load_token_from_db() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = Arc::new(SecurityStore::in_memory().unwrap());

        // First session: generate token
        let mgr1 = SharedTokenManager::new(store.clone(), dir.path().join("test.vault"));
        let token = mgr1.generate_token().unwrap();

        // Second session: load from DB
        let mgr2 = SharedTokenManager::new(store.clone(), dir.path().join("test2.vault"));
        let loaded = mgr2.try_load_token_from_db();
        assert_eq!(loaded.as_deref(), Some(token.as_str()));
        assert_eq!(mgr2.get_current_token().as_deref(), Some(token.as_str()));
    }

    #[test]
    fn test_load_token_from_db_empty() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = Arc::new(SecurityStore::in_memory().unwrap());

        // No token generated — DB is empty
        let mgr = SharedTokenManager::new(store, dir.path().join("test.vault"));
        assert!(mgr.try_load_token_from_db().is_none());
    }
}
