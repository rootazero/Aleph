// core/src/gateway/security/shared_token.rs

//! Shared Token Management
//!
//! A single shared token used as the "entry key" for UI login and API access.
//! The plaintext token is held in memory; only the hash is stored in SQLite.

use crate::sync_primitives::{Arc, Mutex};
use super::crypto::{generate_secret, hmac_sign};
use super::store::SecurityStore;
use uuid::Uuid;

/// Manages a single shared token for UI/API authentication.
///
/// On generation, the plaintext is kept in memory and the HMAC hash
/// is persisted to SQLite via `SecurityStore`. Validation re-computes
/// the HMAC and compares against the stored hash.
pub struct SharedTokenManager {
    store: Arc<SecurityStore>,
    secret: [u8; 32],
    current_token: Mutex<Option<String>>,
}

#[derive(Debug, thiserror::Error)]
pub enum SharedTokenError {
    #[error("Storage error: {0}")]
    Storage(String),
}

impl SharedTokenManager {
    /// Create a new manager with a random HMAC secret.
    pub fn new(store: Arc<SecurityStore>) -> Self {
        Self {
            store,
            secret: generate_secret(),
            current_token: Mutex::new(None),
        }
    }

    /// Create with a specific secret (for testing or persistence across restarts).
    pub fn with_secret(store: Arc<SecurityStore>, secret: [u8; 32]) -> Self {
        Self {
            store,
            secret,
            current_token: Mutex::new(None),
        }
    }

    /// Generate a new shared token (invalidates any previous one).
    pub fn generate_token(&self) -> Result<String, SharedTokenError> {
        let token = format!("aleph-{}", Uuid::new_v4());
        let hash = hmac_sign(&self.secret, &token);

        self.store
            .set_shared_token_hash(&hash)
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

    /// Get the current plaintext token (only if this process generated it).
    pub fn get_current_token(&self) -> Option<String> {
        self.current_token
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Get the HMAC secret.
    pub fn secret(&self) -> &[u8; 32] {
        &self.secret
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::security::store::SecurityStore;

    #[test]
    fn test_generate_and_validate() {
        let store = Arc::new(SecurityStore::in_memory().unwrap());
        let manager = SharedTokenManager::new(store);
        let token = manager.generate_token().unwrap();
        assert!(!token.is_empty());
        assert!(token.starts_with("aleph-"));
        assert!(manager.validate(&token).unwrap());
    }

    #[test]
    fn test_invalid_token_rejected() {
        let store = Arc::new(SecurityStore::in_memory().unwrap());
        let manager = SharedTokenManager::new(store);
        let _token = manager.generate_token().unwrap();
        assert!(!manager.validate("wrong-token").unwrap());
    }

    #[test]
    fn test_regenerate_invalidates_old() {
        let store = Arc::new(SecurityStore::in_memory().unwrap());
        let manager = SharedTokenManager::new(store);
        let old = manager.generate_token().unwrap();
        let new = manager.generate_token().unwrap();
        assert_ne!(old, new);
        assert!(!manager.validate(&old).unwrap());
        assert!(manager.validate(&new).unwrap());
    }

    #[test]
    fn test_get_current_token() {
        let store = Arc::new(SecurityStore::in_memory().unwrap());
        let manager = SharedTokenManager::new(store);
        assert!(manager.get_current_token().is_none());
        let token = manager.generate_token().unwrap();
        assert_eq!(manager.get_current_token(), Some(token));
    }

    #[test]
    fn test_same_secret_validates() {
        let store = Arc::new(SecurityStore::in_memory().unwrap());
        let secret = crate::gateway::security::crypto::generate_secret();
        let manager1 = SharedTokenManager::with_secret(store.clone(), secret);
        let token = manager1.generate_token().unwrap();

        // Same store + same secret = should validate
        let manager2 = SharedTokenManager::with_secret(store, secret);
        assert!(manager2.validate(&token).unwrap());
    }
}
