//! OAuth Token Refresh Manager
//!
//! Automatically refreshes OAuth tokens before they expire for SSE connections.

use crate::sync_primitives::Arc;
use std::collections::HashMap;
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
    _last_refresh: Instant,
}

/// Manages automatic token refresh for multiple servers
pub struct TokenRefreshManager {
    storage: Arc<OAuthStorage>,
    servers: RwLock<HashMap<String, TrackedServer>>,
    config: TokenRefreshConfig,
    shutdown: tokio_util::sync::CancellationToken,
}

impl TokenRefreshManager {
    /// Create a new token refresh manager
    pub fn new(storage: Arc<OAuthStorage>, config: TokenRefreshConfig) -> Self {
        Self {
            storage,
            servers: RwLock::new(HashMap::new()),
            config,
            shutdown: tokio_util::sync::CancellationToken::new(),
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
                _last_refresh: Instant::now(),
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
            tokio::select! {
                _ = ticker.tick() => {
                    self.check_and_refresh_all().await;
                }
                _ = self.shutdown.cancelled() => {
                    tracing::info!("Token refresh manager shutting down");
                    break;
                }
            }
        }
    }

    /// Stop the refresh manager (returns immediately, loop exits on next select cycle)
    pub fn shutdown(&self) {
        self.shutdown.cancel();
    }

    /// Check all servers and refresh tokens as needed
    async fn check_and_refresh_all(&self) {
        // Snapshot the tracked servers and release the read lock BEFORE doing
        // any network I/O. Holding the lock across `check_and_refresh_server`
        // (which performs a token-endpoint round-trip) would block
        // `register_server`/`unregister_server` writers for the full duration
        // of every refresh — and indefinitely if a refresh hangs.
        let snapshot: Vec<(String, String, OAuthServerMetadata)> = {
            let servers = self.servers.read().await;
            servers
                .iter()
                .map(|(name, server)| {
                    (
                        // rust-doctor-disable-next-line excessive-clone
                        name.clone(),
                        // rust-doctor-disable-next-line excessive-clone
                        server.client_id.clone(),
                        // rust-doctor-disable-next-line excessive-clone
                        server.metadata.clone(),
                    )
                })
                .collect()
        };

        for (name, client_id, metadata) in &snapshot {
            if let Err(e) = self
                .check_and_refresh_server(name, client_id, metadata)
                .await
            {
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
        client_id: &str,
        metadata: &OAuthServerMetadata,
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
        let refresh_token = match tokens.refresh_token.as_ref() {
            Some(t) => t,
            None => return Ok(()), // Can't refresh without refresh_token
        };

        // Create provider and refresh
        let provider = OAuthProvider::new(
            // rust-doctor-disable-next-line excessive-clone
            self.storage.clone(),
            server_name,
            "", // Server URL not needed for refresh
            "", // Callback URL not needed for refresh
        );

        let _new_tokens = provider
            .refresh_token_with(metadata, client_id, refresh_token)
            .await?;

        tracing::info!(server = %server_name, "Token refreshed successfully");

        Ok(())
    }

    /// Check if token should be refreshed
    fn should_refresh(&self, tokens: &OAuthTokens) -> bool {
        if let Some(expires_at) = tokens.expires_at {
            let now: i64 = match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
            {
                Ok(d) => d.as_secs().try_into().unwrap_or(i64::MAX),
                Err(_) => return false,
            };

            let refresh_threshold: i64 = self
                .config
                .refresh_before_expiry
                .as_secs()
                .try_into()
                .unwrap_or(i64::MAX);
            expires_at.saturating_sub(refresh_threshold) < now
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
            issuer: Some("https://example.com".to_string()),
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
