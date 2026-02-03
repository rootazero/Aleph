//! OAuth Token Refresh Manager
//!
//! Automatically refreshes OAuth tokens before they expire for SSE connections.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::RwLock;
use tokio::time::{interval, Instant};

use crate::error::Result;
use crate::mcp::auth::{OAuthProvider, OAuthServerMetadata, OAuthStorage, OAuthTokens};

/// Configuration for token refresh behavior
#[derive(Debug, Clone)]
pub struct TokenRefreshConfig {
    /// Check interval for token expiration
    pub check_interval: Duration,
    /// Refresh tokens this long before expiration
    pub refresh_before_expiry: Duration,
}

impl Default for TokenRefreshConfig {
    fn default() -> Self {
        Self {
            check_interval: Duration::from_secs(60),
            refresh_before_expiry: Duration::from_secs(300), // 5 minutes
        }
    }
}

/// Tracked server for token refresh
struct TrackedServer {
    client_id: String,
    metadata: OAuthServerMetadata,
    #[allow(dead_code)]
    last_refresh: Instant,
}

/// Manages automatic token refresh for multiple servers
pub struct TokenRefreshManager {
    storage: Arc<OAuthStorage>,
    servers: RwLock<HashMap<String, TrackedServer>>,
    config: TokenRefreshConfig,
    shutdown: RwLock<bool>,
}

impl TokenRefreshManager {
    /// Create a new token refresh manager
    pub fn new(storage: Arc<OAuthStorage>, config: TokenRefreshConfig) -> Self {
        Self {
            storage,
            servers: RwLock::new(HashMap::new()),
            config,
            shutdown: RwLock::new(false),
        }
    }

    /// Register a server for token refresh monitoring
    pub async fn register_server(
        &self,
        server_name: &str,
        client_id: &str,
        metadata: OAuthServerMetadata,
    ) {
        let mut servers = self.servers.write().await;
        servers.insert(
            server_name.to_string(),
            TrackedServer {
                client_id: client_id.to_string(),
                metadata,
                last_refresh: Instant::now(),
            },
        );
        tracing::debug!(server = %server_name, "Registered server for token refresh");
    }

    /// Unregister a server from token refresh monitoring
    pub async fn unregister_server(&self, server_name: &str) {
        let mut servers = self.servers.write().await;
        servers.remove(server_name);
        tracing::debug!(server = %server_name, "Unregistered server from token refresh");
    }

    /// Run the refresh loop (call in background task)
    pub async fn run(&self) {
        let mut ticker = interval(self.config.check_interval);

        loop {
            ticker.tick().await;

            if *self.shutdown.read().await {
                tracing::info!("Token refresh manager shutting down");
                break;
            }

            self.check_and_refresh_all().await;
        }
    }

    /// Stop the refresh manager
    pub async fn shutdown(&self) {
        let mut shutdown = self.shutdown.write().await;
        *shutdown = true;
    }

    /// Check all servers and refresh tokens as needed
    async fn check_and_refresh_all(&self) {
        let servers = self.servers.read().await;

        for (name, server) in servers.iter() {
            if let Err(e) = self.check_and_refresh_server(name, server).await {
                tracing::warn!(
                    server = %name,
                    error = %e,
                    "Failed to refresh token"
                );
            }
        }
    }

    /// Check and refresh a single server's token
    async fn check_and_refresh_server(
        &self,
        server_name: &str,
        server: &TrackedServer,
    ) -> Result<()> {
        let tokens = match self.storage.get_tokens(server_name).await? {
            Some(t) => t,
            None => return Ok(()), // No tokens to refresh
        };

        // Check if token needs refresh
        if !self.should_refresh(&tokens) {
            return Ok(());
        }

        // Get refresh token
        let refresh_token = match tokens.refresh_token {
            Some(ref t) => t.clone(),
            None => return Ok(()), // Can't refresh without refresh_token
        };

        // Create provider and refresh
        let provider = OAuthProvider::new(
            self.storage.clone(),
            server_name,
            "", // Server URL not needed for refresh
            "", // Callback URL not needed for refresh
        );

        let _new_tokens = provider
            .refresh_token_with(&server.metadata, &server.client_id, &refresh_token)
            .await?;

        tracing::info!(server = %server_name, "Token refreshed successfully");

        Ok(())
    }

    /// Check if token should be refreshed
    fn should_refresh(&self, tokens: &OAuthTokens) -> bool {
        if let Some(expires_at) = tokens.expires_at {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs() as i64;

            let refresh_threshold = self.config.refresh_before_expiry.as_secs() as i64;
            expires_at - refresh_threshold < now
        } else {
            false // No expiration = no need to refresh
        }
    }

    /// Get list of registered servers
    pub async fn list_servers(&self) -> Vec<String> {
        let servers = self.servers.read().await;
        servers.keys().cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_token_refresh_manager_creation() {
        let dir = tempdir().unwrap();
        let storage = Arc::new(OAuthStorage::new(dir.path().join("auth.json")));
        let config = TokenRefreshConfig::default();

        let manager = TokenRefreshManager::new(storage, config);
        assert!(manager.list_servers().await.is_empty());
    }

    #[tokio::test]
    async fn test_register_unregister_server() {
        let dir = tempdir().unwrap();
        let storage = Arc::new(OAuthStorage::new(dir.path().join("auth.json")));
        let manager = TokenRefreshManager::new(storage, TokenRefreshConfig::default());

        let metadata = OAuthServerMetadata {
            authorization_endpoint: "https://example.com/auth".to_string(),
            token_endpoint: "https://example.com/token".to_string(),
            registration_endpoint: None,
            response_types_supported: vec![],
            grant_types_supported: vec![],
            code_challenge_methods_supported: vec![],
        };

        manager.register_server("test", "client_id", metadata).await;
        assert_eq!(manager.list_servers().await.len(), 1);

        manager.unregister_server("test").await;
        assert!(manager.list_servers().await.is_empty());
    }

    #[test]
    fn test_should_refresh_no_expiry() {
        let dir = tempdir().unwrap();
        let storage = Arc::new(OAuthStorage::new(dir.path().join("auth.json")));
        let manager = TokenRefreshManager::new(storage, TokenRefreshConfig::default());

        let tokens = OAuthTokens {
            access_token: "test".to_string(),
            refresh_token: None,
            expires_at: None,
            scope: None,
        };

        assert!(!manager.should_refresh(&tokens));
    }

    #[test]
    fn test_should_refresh_not_expired() {
        let dir = tempdir().unwrap();
        let storage = Arc::new(OAuthStorage::new(dir.path().join("auth.json")));
        let manager = TokenRefreshManager::new(storage, TokenRefreshConfig::default());

        let tokens = OAuthTokens {
            access_token: "test".to_string(),
            refresh_token: None,
            expires_at: Some(9999999999), // Far future
            scope: None,
        };

        assert!(!manager.should_refresh(&tokens));
    }
}
