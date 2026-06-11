// core/src/gateway/security/token.rs

//! HMAC-Signed Token Management
//!
//! Tokens are signed with HMAC-SHA256 and stored in SQLite.
//! The original token value is never stored - only the hash.

use crate::sync_primitives::Arc;
use thiserror::Error;
use uuid::Uuid;

use super::crypto::{generate_secret, hmac_sign, hmac_verify};
use super::device::DeviceRole;
use super::store::SecurityStore;

/// Default token expiry (24 hours in milliseconds)
const DEFAULT_TOKEN_EXPIRY_MS: i64 = 24 * 60 * 60 * 1000;

/// Token-related errors
#[derive(Debug, Error)]
pub enum TokenError {
    #[error("Invalid token")]
    InvalidToken,
    #[error("Token expired")]
    TokenExpired,
    #[error("Token revoked")]
    TokenRevoked,
    #[error("Signature verification failed")]
    SignatureInvalid,
    #[error("Database error: {0}")]
    DatabaseError(String),
}

/// A signed token with its signature
#[derive(Debug, Clone)]
pub struct SignedToken {
    pub token: String,
    pub signature: String,
    pub token_id: String,
    pub expires_at: i64,
}

/// Token validation result
#[derive(Debug, Clone)]
pub struct TokenValidation {
    pub token_id: String,
    pub device_id: String,
    pub role: DeviceRole,
    pub scopes: Vec<String>,
    pub remaining_ms: i64,
}

/// Token manager with HMAC signing
pub struct TokenManager {
    store: Arc<SecurityStore>,
    secret: [u8; 32],
    default_expiry_ms: i64,
}

impl TokenManager {
    /// Create a new token manager, restoring persisted HMAC secret if available.
    ///
    /// If the store already has a persisted secret, it is reused so that
    /// existing device tokens remain valid across restarts and updates.
    pub fn new(store: Arc<SecurityStore>) -> Self {
        let secret = match store.get_token_manager_secret() {
            Ok(Some(s)) => s,
            _ => {
                let s = generate_secret();
                // Persist the new secret for future restarts
                if let Err(e) = store.set_token_manager_secret(&s) {
                    tracing::warn!("Failed to persist token manager secret: {}", e);
                }
                s
            }
        };
        Self {
            store,
            secret,
            default_expiry_ms: DEFAULT_TOKEN_EXPIRY_MS,
        }
    }

    /// Create with a specific secret (for testing or persistence)
    pub const fn with_secret(store: Arc<SecurityStore>, secret: [u8; 32]) -> Self {
        Self {
            store,
            secret,
            default_expiry_ms: DEFAULT_TOKEN_EXPIRY_MS,
        }
    }

    /// Create with custom expiry
    pub fn with_expiry(store: Arc<SecurityStore>, expiry_ms: i64) -> Self {
        let secret = match store.get_token_manager_secret() {
            Ok(Some(s)) => s,
            _ => {
                let s = generate_secret();
                let _ = store.set_token_manager_secret(&s);
                s
            }
        };
        Self {
            store,
            secret,
            default_expiry_ms: expiry_ms,
        }
    }

    /// Issue a new signed token for a device
    pub fn issue_token(
        &self,
        device_id: &str,
        role: DeviceRole,
        scopes: Vec<String>,
    ) -> Result<SignedToken, TokenError> {
        self.issue_token_with_expiry(device_id, role, scopes, self.default_expiry_ms)
    }

    /// Issue a token with custom expiry
    pub fn issue_token_with_expiry(
        &self,
        device_id: &str,
        role: DeviceRole,
        scopes: Vec<String>,
        expiry_ms: i64,
    ) -> Result<SignedToken, TokenError> {
        let token_id = Uuid::new_v4().to_string();
        let token = Uuid::new_v4().to_string();
        let signature = hmac_sign(&self.secret, &token);
        let token_hash = hmac_sign(&self.secret, &token); // Store hash, not token

        let now = current_timestamp_ms();
        let expires_at = now + expiry_ms;

        self.store
            .insert_token(
                &token_id,
                device_id,
                &token_hash,
                role.as_str(),
                &scopes,
                expires_at,
            )
            .map_err(|e| TokenError::DatabaseError(e.to_string()))?;

        Ok(SignedToken {
            token,
            signature,
            token_id,
            expires_at,
        })
    }

    /// Validate a token and its signature
    pub fn validate_token(
        &self,
        token: &str,
        signature: &str,
    ) -> Result<TokenValidation, TokenError> {
        // Verify HMAC signature
        hmac_verify(&self.secret, token, signature).map_err(|_| TokenError::SignatureInvalid)?;

        // Compute hash to look up in database
        let token_hash = hmac_sign(&self.secret, token);

        // Look up token in database
        let token_row = self
            .store
            .get_token_by_hash(&token_hash)
            .map_err(|e| TokenError::DatabaseError(e.to_string()))?
            .ok_or(TokenError::InvalidToken)?;

        // Check if revoked (shouldn't happen as query filters, but be safe)
        if token_row.revoked_at.is_some() {
            return Err(TokenError::TokenRevoked);
        }

        // Check expiry
        let now = current_timestamp_ms();
        if token_row.expires_at <= now {
            return Err(TokenError::TokenExpired);
        }

        // Update last_used_at
        let _ = self.store.touch_token(&token_row.token_id);

        Ok(TokenValidation {
            token_id: token_row.token_id,
            device_id: token_row.device_id,
            role: DeviceRole::from_str_opt(&token_row.role).unwrap_or_default(),
            scopes: token_row.scopes,
            remaining_ms: token_row.expires_at - now,
        })
    }

    /// Rotate a token (invalidate old, issue new)
    pub fn rotate_token(
        &self,
        old_token: &str,
        old_signature: &str,
    ) -> Result<SignedToken, TokenError> {
        // Validate the old token first
        let validation = self.validate_token(old_token, old_signature)?;

        // Revoke the old token
        self.store
            .revoke_token(&validation.token_id)
            .map_err(|e| TokenError::DatabaseError(e.to_string()))?;

        // Issue a new token with same permissions
        self.issue_token(&validation.device_id, validation.role, validation.scopes)
    }

    /// Revoke a specific token
    pub fn revoke_token(&self, token_id: &str) -> Result<bool, TokenError> {
        self.store
            .revoke_token(token_id)
            .map_err(|e| TokenError::DatabaseError(e.to_string()))
    }

    /// Revoke all tokens for a device
    pub fn revoke_device_tokens(&self, device_id: &str) -> Result<u64, TokenError> {
        self.store
            .revoke_device_tokens(device_id)
            .map_err(|e| TokenError::DatabaseError(e.to_string()))
    }

    /// Stamp a device's `last_seen_at` (now). Cluster node lifecycle seams call
    /// this on connect/disconnect so an offline node carries an honest
    /// "last seen" in `environments.list`. Best-effort for callers.
    pub fn touch_device(&self, device_id: &str) -> Result<(), TokenError> {
        self.store
            .touch_device(device_id)
            .map_err(|e| TokenError::DatabaseError(e.to_string()))
    }

    /// Clean up expired tokens
    pub fn cleanup_expired(&self) -> Result<u64, TokenError> {
        self.store
            .delete_expired_tokens()
            .map_err(|e| TokenError::DatabaseError(e.to_string()))
    }

    /// Get the HMAC secret (for persistence)
    #[must_use]
    pub const fn secret(&self) -> &[u8; 32] {
        &self.secret
    }

    /// HMAC hash of a raw token, in the same form stored in the token table.
    ///
    /// Lets a caller that only holds the raw bearer (e.g. the WebSocket
    /// dispatch loop, which captures the token at `connect` time) derive the
    /// lookup key for [`is_token_hash_revoked`] without re-running full
    /// signature validation on every request.
    #[must_use]
    pub fn token_hash(&self, token: &str) -> String {
        hmac_sign(&self.secret, token)
    }

    /// Whether the token identified by `token_hash` is no longer valid —
    /// revoked, or its row removed (e.g. the device was revoked, which also
    /// revokes its tokens).
    ///
    /// `get_token_by_hash` already filters `revoked_at IS NULL`, so `Ok(None)`
    /// means revoked-or-gone. Returns `false` on a transient store error so a
    /// DB blip never disconnects a legitimate client — the next dispatch
    /// re-checks. Expiry is intentionally NOT treated as revocation here
    /// (mid-session expiry is a separate concern).
    #[must_use]
    pub fn is_token_hash_revoked(&self, token_hash: &str) -> bool {
        match self.store.get_token_by_hash(token_hash) {
            Ok(Some(_)) => false,
            Ok(None) => true,
            Err(_) => false,
        }
    }
}

fn current_timestamp_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_manager() -> TokenManager {
        let store = Arc::new(SecurityStore::in_memory().unwrap());
        // Create a device for tokens
        store
            .upsert_device(&crate::gateway::security::store::DeviceUpsertData {
                device_id: "dev-1",
                device_name: "Test",
                device_type: None,
                public_key: &[1u8; 32],
                fingerprint: "fp",
                role: "operator",
                scopes: &[],
            })
            .unwrap();
        TokenManager::new(store)
    }

    #[test]
    fn test_issue_and_validate() {
        let manager = create_test_manager();

        let signed = manager
            .issue_token("dev-1", DeviceRole::Operator, vec!["*".into()])
            .unwrap();

        assert!(!signed.token.is_empty());
        assert!(!signed.signature.is_empty());

        let validation = manager
            .validate_token(&signed.token, &signed.signature)
            .unwrap();
        assert_eq!(validation.device_id, "dev-1");
        assert_eq!(validation.role, DeviceRole::Operator);
    }

    #[test]
    fn test_invalid_signature() {
        let manager = create_test_manager();

        let signed = manager
            .issue_token("dev-1", DeviceRole::Operator, vec![])
            .unwrap();

        let result = manager.validate_token(&signed.token, "wrong-signature");
        assert!(matches!(result, Err(TokenError::SignatureInvalid)));
    }

    #[test]
    fn test_token_rotation() {
        let manager = create_test_manager();

        let old_token = manager
            .issue_token("dev-1", DeviceRole::Operator, vec!["*".into()])
            .unwrap();

        let new_token = manager
            .rotate_token(&old_token.token, &old_token.signature)
            .unwrap();

        // Old token should be invalid
        let old_result = manager.validate_token(&old_token.token, &old_token.signature);
        assert!(old_result.is_err());

        // New token should be valid
        let new_result = manager.validate_token(&new_token.token, &new_token.signature);
        assert!(new_result.is_ok());
    }

    #[test]
    fn test_revoke_token() {
        let manager = create_test_manager();

        let signed = manager
            .issue_token("dev-1", DeviceRole::Operator, vec![])
            .unwrap();

        assert!(manager
            .validate_token(&signed.token, &signed.signature)
            .is_ok());

        manager.revoke_token(&signed.token_id).unwrap();

        assert!(manager
            .validate_token(&signed.token, &signed.signature)
            .is_err());
    }

    #[test]
    fn test_revoke_device_tokens() {
        let manager = create_test_manager();

        // Issue multiple tokens
        let t1 = manager
            .issue_token("dev-1", DeviceRole::Operator, vec![])
            .unwrap();
        let t2 = manager
            .issue_token("dev-1", DeviceRole::Operator, vec![])
            .unwrap();

        // Revoke all
        let count = manager.revoke_device_tokens("dev-1").unwrap();
        assert_eq!(count, 2);

        // Both should be invalid
        assert!(manager.validate_token(&t1.token, &t1.signature).is_err());
        assert!(manager.validate_token(&t2.token, &t2.signature).is_err());
    }

    #[test]
    fn test_token_expiry() {
        let store = Arc::new(SecurityStore::in_memory().unwrap());
        store
            .upsert_device(&crate::gateway::security::store::DeviceUpsertData {
                device_id: "dev-1",
                device_name: "Test",
                device_type: None,
                public_key: &[1u8; 32],
                fingerprint: "fp",
                role: "operator",
                scopes: &[],
            })
            .unwrap();

        // Create manager with very short expiry (10ms)
        let manager = TokenManager::with_expiry(store, 10);

        let signed = manager
            .issue_token("dev-1", DeviceRole::Operator, vec![])
            .unwrap();

        // Wait for expiry (50ms should be enough)
        std::thread::sleep(std::time::Duration::from_millis(50));

        let result = manager.validate_token(&signed.token, &signed.signature);
        // Token should fail validation - either expired or not found (database query filters expired)
        assert!(result.is_err(), "Expected error but got: {:?}", result);
        match result {
            Err(TokenError::TokenExpired) | Err(TokenError::InvalidToken) => {
                // Both are acceptable - database query may filter expired tokens
            }
            other => panic!("Unexpected result: {:?}", other),
        }
    }

    #[test]
    fn token_hash_then_revocation_check_roundtrip() {
        let manager = create_test_manager();
        let signed = manager
            .issue_token("dev-1", DeviceRole::Operator, vec!["*".into()])
            .unwrap();

        let hash = manager.token_hash(&signed.token);
        // Freshly issued → not revoked.
        assert!(!manager.is_token_hash_revoked(&hash));

        // After revoking this token → revoked.
        manager.revoke_token(&signed.token_id).unwrap();
        assert!(manager.is_token_hash_revoked(&hash));
    }

    #[test]
    fn revocation_check_true_for_device_token_revocation() {
        let manager = create_test_manager();
        let signed = manager
            .issue_token("dev-1", DeviceRole::Operator, vec![])
            .unwrap();
        let hash = manager.token_hash(&signed.token);
        assert!(!manager.is_token_hash_revoked(&hash));

        // Revoking all the device's tokens (the path taken on device removal)
        // also flips the hash to revoked.
        manager.revoke_device_tokens("dev-1").unwrap();
        assert!(manager.is_token_hash_revoked(&hash));
    }

    #[test]
    fn revocation_check_true_for_unknown_hash() {
        let manager = create_test_manager();
        assert!(manager.is_token_hash_revoked("deadbeef_not_a_real_hash"));
    }

    #[test]
    fn token_hash_is_stable_and_distinct() {
        let manager = create_test_manager();
        let a = manager
            .issue_token("dev-1", DeviceRole::Operator, vec![])
            .unwrap();
        // Stable: same token hashes the same.
        assert_eq!(manager.token_hash(&a.token), manager.token_hash(&a.token));
        // Distinct tokens hash differently.
        let b = manager
            .issue_token("dev-1", DeviceRole::Operator, vec![])
            .unwrap();
        assert_ne!(manager.token_hash(&a.token), manager.token_hash(&b.token));
    }
}
