// core/src/gateway/security/shared_token.rs

//! Shared Token Management
//!
//! A single shared token used as the "entry key" for UI login and API access.
//! The plaintext token is held in memory; only the hash is stored in SQLite.
//! Also manages the encrypted secret vault, using the token as master key.

use std::sync::RwLock;
use crate::sync_primitives::{Arc, Mutex};
use crate::secrets::vault::SecretVault;
use crate::secrets::crypto::SecretsCrypto;
use crate::secrets::types::{DecryptedSecret, EncryptedEntry, EntryMetadata, SecretError};
use super::crypto::{generate_secret, hmac_sign};
use super::store::SecurityStore;
use uuid::Uuid;

/// Manages a single shared token for UI/API authentication.
///
/// On generation, the plaintext is kept in memory and the HMAC hash
/// is persisted to SQLite via `SecurityStore`. Validation re-computes
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
        let secret = store
            .get_shared_token_secret()
            .ok()
            .flatten()
            .unwrap_or_else(generate_secret);
        let vault_path = vault_path.into();
        let vault = SecretVault::open(&vault_path).unwrap_or_else(|_| SecretVault::empty(vault_path));
        Self {
            store,
            secret,
            current_token: Mutex::new(None),
            vault: RwLock::new(vault),
        }
    }

    /// Create with a specific secret (for testing).
    pub fn with_secret(store: Arc<SecurityStore>, secret: [u8; 32], vault_path: impl Into<std::path::PathBuf>) -> Self {
        let vault_path = vault_path.into();
        let vault = SecretVault::open(&vault_path).unwrap_or_else(|_| SecretVault::empty(vault_path));
        Self {
            store,
            secret,
            current_token: Mutex::new(None),
            vault: RwLock::new(vault),
        }
    }

    /// Generate a new shared token (invalidates any previous one).
    /// Persists both the hash and the HMAC secret so the token survives restarts.
    pub fn generate_token(&self) -> Result<String, SharedTokenError> {
        let token = format!("aleph-{}", Uuid::new_v4());
        let hash = hmac_sign(&self.secret, &token);

        self.store
            .set_shared_token_with_secret(&hash, &self.secret)
            .map_err(|e| SharedTokenError::Storage(e.to_string()))?;

        let mut current = self.current_token.lock().unwrap_or_else(|e| e.into_inner());
        *current = Some(token.clone());
        Ok(token)
    }

    /// Validate a token against the stored hash.
    pub fn validate(&self, token: &str) -> Result<bool, SharedTokenError> {
        let hash = hmac_sign(&self.secret, token);
        self.store
            .validate_shared_token_hash(&hash)
            .map_err(|e| SharedTokenError::Storage(e.to_string()))
    }

    /// Try to load and validate an existing token from a file.
    /// Returns `Some(token)` if the file exists and the token validates.
    pub fn try_load_token_from_file(&self, path: &std::path::Path) -> Option<String> {
        let token = std::fs::read_to_string(path).ok()?.trim().to_string();
        if token.is_empty() {
            return None;
        }
        match self.validate(&token) {
            Ok(true) => {
                let mut current = self.current_token.lock().unwrap_or_else(|e| e.into_inner());
                *current = Some(token.clone());
                Some(token)
            }
            _ => None,
        }
    }

    /// Get the current plaintext token (only if this process generated or loaded it).
    pub fn get_current_token(&self) -> Option<String> {
        self.current_token
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Check if the store already has a shared token hash (regardless of whether
    /// we have the plaintext).
    pub fn has_stored_token(&self) -> bool {
        self.store.has_shared_token().unwrap_or(false)
    }

    /// Get the HMAC secret.
    pub fn secret(&self) -> &[u8; 32] {
        &self.secret
    }

    /// Get a crypto engine using the current token as master key.
    fn crypto(&self) -> Result<SecretsCrypto, SharedTokenError> {
        let token = self.current_token.lock().unwrap_or_else(|e| e.into_inner());
        let token = token.as_ref()
            .ok_or_else(|| SharedTokenError::Storage("No token set — cannot access vault".into()))?;
        Ok(SecretsCrypto::new(token))
    }

    /// Store an encrypted secret in the vault.
    pub fn store_secret(&self, name: &str, value: &str) -> Result<(), SharedTokenError> {
        let crypto = self.crypto()?;
        let encrypted = crypto.encrypt(value)
            .map_err(|e| SharedTokenError::Storage(format!("Encryption failed: {}", e)))?;
        let now = chrono::Utc::now().timestamp();
        let entry = EncryptedEntry {
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

    /// Get a decrypted secret from the vault.
    pub fn get_secret(&self, name: &str) -> Result<Option<DecryptedSecret>, SharedTokenError> {
        let crypto = self.crypto()?;
        let vault = self.vault.read().unwrap_or_else(|e| e.into_inner());
        match vault.get(name) {
            Ok(entry) => {
                let decrypted = crypto.decrypt(&entry.ciphertext, &entry.nonce, &entry.salt)
                    .map_err(|e| SharedTokenError::Storage(format!("Decryption failed: {}", e)))?;
                Ok(Some(DecryptedSecret::new(decrypted)))
            }
            Err(SecretError::NotFound(_)) => Ok(None),
            Err(e) => Err(SharedTokenError::Storage(e.to_string())),
        }
    }

    /// Delete a secret from the vault.
    pub fn delete_secret(&self, name: &str) -> Result<bool, SharedTokenError> {
        let mut vault = self.vault.write().unwrap_or_else(|e| e.into_inner());
        vault.delete(name)
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
    /// 1. Decrypt all entries with current token
    /// 2. Generate new token (updates HMAC, current_token)
    /// 3. Re-encrypt all entries with new token
    /// 4. Atomically replace vault entries
    pub fn reset_token(&self) -> Result<String, SharedTokenError> {
        use std::collections::HashMap;

        let old_crypto = self.crypto()?;

        // 1. Decrypt all entries with old token
        let vault = self.vault.read().unwrap_or_else(|e| e.into_inner());
        let mut plaintext_entries: Vec<(String, String, EncryptedEntry)> = Vec::new();
        for (name, entry) in vault.entries() {
            let decrypted = old_crypto
                .decrypt(&entry.ciphertext, &entry.nonce, &entry.salt)
                .map_err(|e| {
                    SharedTokenError::Storage(format!("Decrypt failed for '{}': {}", name, e))
                })?;
            plaintext_entries.push((name.clone(), decrypted, entry.clone()));
        }
        drop(vault);

        // 2. Generate new token (updates HMAC hash, current_token)
        let new_token = self.generate_token()?;
        let new_crypto = self.crypto()?;

        // 3. Re-encrypt all entries with new token
        let mut new_entries = HashMap::new();
        for (name, plaintext, old_entry) in plaintext_entries {
            let encrypted = new_crypto.encrypt(&plaintext).map_err(|e| {
                SharedTokenError::Storage(format!("Re-encrypt failed for '{}': {}", name, e))
            })?;
            new_entries.insert(
                name,
                EncryptedEntry {
                    ciphertext: encrypted.ciphertext,
                    nonce: encrypted.nonce,
                    salt: encrypted.salt,
                    created_at: old_entry.created_at,
                    updated_at: chrono::Utc::now().timestamp(),
                    metadata: old_entry.metadata,
                },
            );
        }

        // 4. Atomic replace
        let mut vault = self.vault.write().unwrap_or_else(|e| e.into_inner());
        vault
            .replace_all(new_entries)
            .map_err(|e| SharedTokenError::Storage(e.to_string()))?;

        Ok(new_token)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::security::store::SecurityStore;

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
        let manager1 = SharedTokenManager::with_secret(store.clone(), secret, dir.path().join("test1.vault"));
        let token = manager1.generate_token().unwrap();

        // Same store + same secret = should validate
        let manager2 = SharedTokenManager::with_secret(store, secret, dir.path().join("test2.vault"));
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

        // Write token to a temp file
        let tmp = dir.path().join("aleph_test_token");
        std::fs::write(&tmp, &token).unwrap();

        // Second "boot" — simulates restart, creates new manager from same store
        let mgr2 = SharedTokenManager::new(store.clone(), dir.path().join("test2.vault"));
        // Should restore the persisted secret and validate the old token
        assert!(mgr2.validate(&token).unwrap());
        // Should load from file
        let loaded = mgr2.try_load_token_from_file(&tmp);
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
        let token_file = dir.path().join("token");
        let store = Arc::new(SecurityStore::in_memory().unwrap());

        // First session: generate token, store secret
        let mgr1 = SharedTokenManager::new(store.clone(), &vault_path);
        let token = mgr1.generate_token().unwrap();
        std::fs::write(&token_file, &token).unwrap();
        mgr1.store_secret("anthropic", "sk-ant-persist").unwrap();

        // Second session: load same token, should access same secret
        let mgr2 = SharedTokenManager::new(store, &vault_path);
        mgr2.try_load_token_from_file(&token_file);
        let secret = mgr2.get_secret("anthropic").unwrap().unwrap();
        assert_eq!(secret.expose(), "sk-ant-persist");
    }
}
