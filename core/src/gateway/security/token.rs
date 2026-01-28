//! Bearer Token Management
//!
//! Handles generation, validation, and revocation of bearer tokens
//! for WebSocket authentication.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use rand::Rng;

/// Default token expiry time (24 hours)
const DEFAULT_TOKEN_EXPIRY: Duration = Duration::from_secs(24 * 60 * 60);

/// Information about an issued token
struct TokenInfo {
    /// When the token was created
    created_at: Instant,
    /// Token expiry duration
    expiry: Duration,
    /// Permissions granted to this token
    permissions: Vec<String>,
    /// Device or client identifier
    device_id: Option<String>,
    /// Last time the token was used
    last_used: Instant,
}

impl TokenInfo {
    /// Check if the token has expired
    fn is_expired(&self) -> bool {
        self.created_at.elapsed() > self.expiry
    }
}

/// Token manager for handling bearer token authentication
///
/// Tokens are stored in memory and are lost on restart. For persistent
/// tokens, consider storing them in a database.
#[derive(Clone)]
pub struct TokenManager {
    tokens: Arc<RwLock<HashMap<String, TokenInfo>>>,
    default_expiry: Duration,
}

impl TokenManager {
    /// Create a new token manager with default settings
    pub fn new() -> Self {
        Self {
            tokens: Arc::new(RwLock::new(HashMap::new())),
            default_expiry: DEFAULT_TOKEN_EXPIRY,
        }
    }

    /// Create a token manager with custom default expiry
    pub fn with_expiry(expiry: Duration) -> Self {
        Self {
            tokens: Arc::new(RwLock::new(HashMap::new())),
            default_expiry: expiry,
        }
    }

    /// Generate a new token with given permissions
    ///
    /// # Arguments
    ///
    /// * `permissions` - List of permission strings
    ///
    /// # Returns
    ///
    /// The generated token string
    pub async fn generate_token(&self, permissions: Vec<String>) -> String {
        self.generate_token_with_device(permissions, None).await
    }

    /// Generate a new token with permissions and device ID
    ///
    /// # Arguments
    ///
    /// * `permissions` - List of permission strings
    /// * `device_id` - Optional device identifier
    ///
    /// # Returns
    ///
    /// The generated token string
    pub async fn generate_token_with_device(
        &self,
        permissions: Vec<String>,
        device_id: Option<String>,
    ) -> String {
        let token: String = rand::thread_rng()
            .sample_iter(&rand::distributions::Alphanumeric)
            .take(32)
            .map(char::from)
            .collect();

        let now = Instant::now();
        let mut tokens = self.tokens.write().await;
        tokens.insert(
            token.clone(),
            TokenInfo {
                created_at: now,
                expiry: self.default_expiry,
                permissions,
                device_id,
                last_used: now,
            },
        );

        token
    }

    /// Generate a token with custom expiry
    pub async fn generate_token_with_expiry(
        &self,
        permissions: Vec<String>,
        expiry: Duration,
    ) -> String {
        let token: String = rand::thread_rng()
            .sample_iter(&rand::distributions::Alphanumeric)
            .take(32)
            .map(char::from)
            .collect();

        let now = Instant::now();
        let mut tokens = self.tokens.write().await;
        tokens.insert(
            token.clone(),
            TokenInfo {
                created_at: now,
                expiry,
                permissions: permissions.clone(),
                device_id: None,
                last_used: now,
            },
        );

        token
    }

    /// Validate a token
    ///
    /// # Arguments
    ///
    /// * `token` - The token to validate
    ///
    /// # Returns
    ///
    /// `true` if the token is valid and not expired
    pub async fn validate_token(&self, token: &str) -> bool {
        let mut tokens = self.tokens.write().await;
        if let Some(info) = tokens.get_mut(token) {
            if info.is_expired() {
                tokens.remove(token);
                return false;
            }
            // Update last used time
            info.last_used = Instant::now();
            return true;
        }
        false
    }

    /// Check if a token has a specific permission
    ///
    /// # Arguments
    ///
    /// * `token` - The token to check
    /// * `permission` - The permission to verify
    ///
    /// # Returns
    ///
    /// `true` if the token is valid and has the permission
    pub async fn has_permission(&self, token: &str, permission: &str) -> bool {
        let tokens = self.tokens.read().await;
        if let Some(info) = tokens.get(token) {
            if info.is_expired() {
                return false;
            }
            // Check for wildcard permission
            if info.permissions.contains(&"*".to_string()) {
                return true;
            }
            return info.permissions.iter().any(|p| p == permission);
        }
        false
    }

    /// Get all permissions for a token
    pub async fn get_permissions(&self, token: &str) -> Option<Vec<String>> {
        let tokens = self.tokens.read().await;
        tokens
            .get(token)
            .filter(|info| !info.is_expired())
            .map(|info| info.permissions.clone())
    }

    /// Revoke a token
    pub async fn revoke_token(&self, token: &str) -> bool {
        let mut tokens = self.tokens.write().await;
        tokens.remove(token).is_some()
    }

    /// Revoke all tokens for a device
    pub async fn revoke_device_tokens(&self, device_id: &str) {
        let mut tokens = self.tokens.write().await;
        tokens.retain(|_, info| {
            info.device_id.as_ref().map(|id| id != device_id).unwrap_or(true)
        });
    }

    /// Clean up expired tokens
    ///
    /// This should be called periodically to free memory.
    pub async fn cleanup_expired(&self) -> usize {
        let mut tokens = self.tokens.write().await;
        let before = tokens.len();
        tokens.retain(|_, info| !info.is_expired());
        before - tokens.len()
    }

    /// Get the number of active tokens
    pub async fn active_token_count(&self) -> usize {
        let tokens = self.tokens.read().await;
        tokens.values().filter(|info| !info.is_expired()).count()
    }
}

impl Default for TokenManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_generate_and_validate() {
        let manager = TokenManager::new();
        let token = manager.generate_token(vec!["read".to_string()]).await;

        assert!(manager.validate_token(&token).await);
        assert!(!manager.validate_token("invalid").await);
    }

    #[tokio::test]
    async fn test_permissions() {
        let manager = TokenManager::new();
        let token = manager
            .generate_token(vec!["read".to_string(), "write".to_string()])
            .await;

        assert!(manager.has_permission(&token, "read").await);
        assert!(manager.has_permission(&token, "write").await);
        assert!(!manager.has_permission(&token, "admin").await);
    }

    #[tokio::test]
    async fn test_wildcard_permission() {
        let manager = TokenManager::new();
        let token = manager.generate_token(vec!["*".to_string()]).await;

        assert!(manager.has_permission(&token, "anything").await);
        assert!(manager.has_permission(&token, "read").await);
    }

    #[tokio::test]
    async fn test_revoke() {
        let manager = TokenManager::new();
        let token = manager.generate_token(vec![]).await;

        assert!(manager.validate_token(&token).await);
        assert!(manager.revoke_token(&token).await);
        assert!(!manager.validate_token(&token).await);
    }

    #[tokio::test]
    async fn test_expiry() {
        let manager = TokenManager::with_expiry(Duration::from_millis(10));
        let token = manager.generate_token(vec![]).await;

        assert!(manager.validate_token(&token).await);

        tokio::time::sleep(Duration::from_millis(20)).await;

        assert!(!manager.validate_token(&token).await);
    }
}
