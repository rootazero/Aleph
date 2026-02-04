//! OAuth Credential Storage
//!
//! Securely stores OAuth tokens and client information for MCP servers.
//! Credentials are stored in a JSON file with secure permissions (0600 on Unix).

use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use tokio::fs;
use tokio::sync::RwLock;

use crate::error::{AlephError, Result};

/// OAuth tokens received from an authorization server
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuthTokens {
    /// The access token for API requests
    pub access_token: String,
    /// Refresh token for obtaining new access tokens
    pub refresh_token: Option<String>,
    /// Unix timestamp when the access token expires
    pub expires_at: Option<i64>,
    /// Granted scopes
    pub scope: Option<String>,
}

impl OAuthTokens {
    /// Check if the token is expired (with 5 minute buffer)
    pub fn is_expired(&self) -> bool {
        if let Some(expires_at) = self.expires_at {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs() as i64;
            // Add 5 minute buffer
            expires_at - 300 < now
        } else {
            false
        }
    }

    /// Check if the token can be refreshed
    pub fn can_refresh(&self) -> bool {
        self.refresh_token.is_some()
    }
}

/// Dynamic client registration information
///
/// Some OAuth servers support dynamic client registration, where clients
/// can register themselves at runtime rather than using pre-configured credentials.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientInfo {
    /// Client ID received from registration
    pub client_id: String,
    /// Client secret (if provided)
    pub client_secret: Option<String>,
    /// Unix timestamp when client_id was issued
    pub client_id_issued_at: Option<i64>,
    /// Unix timestamp when client_secret expires (0 = never)
    pub client_secret_expires_at: Option<i64>,
}

/// OAuth entry for a server
///
/// Stores all OAuth-related information for a single MCP server.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OAuthEntry {
    /// OAuth tokens
    pub tokens: Option<OAuthTokens>,
    /// Dynamic client registration info
    pub client_info: Option<ClientInfo>,
    /// PKCE code verifier (stored during authorization flow)
    pub code_verifier: Option<String>,
    /// OAuth state parameter (for CSRF protection)
    pub oauth_state: Option<String>,
    /// The server URL this entry is for
    pub server_url: Option<String>,
}

/// Storage file structure
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct StorageFile {
    entries: HashMap<String, OAuthEntry>,
}

/// OAuth credential storage
///
/// Provides persistent storage for OAuth credentials across sessions.
/// Credentials are stored in a JSON file with restricted permissions.
///
/// # Example
///
/// ```ignore
/// let storage = OAuthStorage::new(PathBuf::from("/path/to/auth.json"));
///
/// // Save tokens
/// let tokens = OAuthTokens {
///     access_token: "abc123".to_string(),
///     refresh_token: Some("refresh456".to_string()),
///     expires_at: Some(1234567890),
///     scope: Some("read write".to_string()),
/// };
/// storage.save_tokens("my-server", &tokens).await?;
///
/// // Load tokens
/// if let Some(tokens) = storage.get_tokens("my-server").await? {
///     println!("Token: {}", tokens.access_token);
/// }
/// ```
pub struct OAuthStorage {
    file_path: PathBuf,
    cache: RwLock<Option<StorageFile>>,
}

impl OAuthStorage {
    /// Create new storage at the specified path
    pub fn new(file_path: PathBuf) -> Self {
        Self {
            file_path,
            cache: RwLock::new(None),
        }
    }

    /// Get the default storage location
    ///
    /// Uses the system data directory:
    /// - macOS: ~/Library/Application Support/aether/mcp-auth.json
    /// - Linux: ~/.local/share/aether/mcp-auth.json
    /// - Windows: %APPDATA%\aether\mcp-auth.json
    pub fn default_path() -> PathBuf {
        dirs::data_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("aether")
            .join("mcp-auth.json")
    }

    /// Load storage file
    async fn load(&self) -> Result<StorageFile> {
        // Check cache first
        {
            let cache = self.cache.read().await;
            if let Some(ref storage) = *cache {
                return Ok(storage.clone());
            }
        }

        // Load from file
        if !self.file_path.exists() {
            return Ok(StorageFile::default());
        }

        let content = fs::read_to_string(&self.file_path).await.map_err(|e| {
            AlephError::IoError(format!("Failed to read OAuth storage: {}", e))
        })?;

        let storage: StorageFile = serde_json::from_str(&content).map_err(|e| {
            AlephError::IoError(format!("Failed to parse OAuth storage: {}", e))
        })?;

        // Update cache
        {
            let mut cache = self.cache.write().await;
            *cache = Some(storage.clone());
        }

        Ok(storage)
    }

    /// Save storage file with secure permissions
    async fn save(&self, storage: &StorageFile) -> Result<()> {
        // Create parent directory if needed
        if let Some(parent) = self.file_path.parent() {
            fs::create_dir_all(parent).await.map_err(|e| {
                AlephError::IoError(format!("Failed to create OAuth storage dir: {}", e))
            })?;
        }

        let content = serde_json::to_string_pretty(storage).map_err(|e| {
            AlephError::IoError(format!("Failed to serialize OAuth storage: {}", e))
        })?;

        fs::write(&self.file_path, content).await.map_err(|e| {
            AlephError::IoError(format!("Failed to write OAuth storage: {}", e))
        })?;

        // Set file permissions to 0600 on Unix (owner read/write only)
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = std::fs::Permissions::from_mode(0o600);
            if let Err(e) = std::fs::set_permissions(&self.file_path, perms) {
                tracing::warn!(
                    error = %e,
                    "Failed to set secure permissions on OAuth storage"
                );
            }
        }

        // Update cache
        {
            let mut cache = self.cache.write().await;
            *cache = Some(storage.clone());
        }

        Ok(())
    }

    /// Get tokens for a server
    pub async fn get_tokens(&self, server: &str) -> Result<Option<OAuthTokens>> {
        let storage = self.load().await?;
        Ok(storage.entries.get(server).and_then(|e| e.tokens.clone()))
    }

    /// Save tokens for a server
    pub async fn save_tokens(&self, server: &str, tokens: &OAuthTokens) -> Result<()> {
        let mut storage = self.load().await?;

        let entry = storage
            .entries
            .entry(server.to_string())
            .or_insert_with(OAuthEntry::default);

        entry.tokens = Some(tokens.clone());
        self.save(&storage).await
    }

    /// Get client info for a server
    pub async fn get_client_info(&self, server: &str) -> Result<Option<ClientInfo>> {
        let storage = self.load().await?;
        Ok(storage
            .entries
            .get(server)
            .and_then(|e| e.client_info.clone()))
    }

    /// Save client info for a server
    pub async fn save_client_info(&self, server: &str, client_info: &ClientInfo) -> Result<()> {
        let mut storage = self.load().await?;

        let entry = storage
            .entries
            .entry(server.to_string())
            .or_insert_with(OAuthEntry::default);

        entry.client_info = Some(client_info.clone());
        self.save(&storage).await
    }

    /// Get the full OAuth entry for a server
    pub async fn get_entry(&self, server: &str) -> Result<Option<OAuthEntry>> {
        let storage = self.load().await?;
        Ok(storage.entries.get(server).cloned())
    }

    /// Save a full OAuth entry
    pub async fn save_entry(&self, server: &str, entry: &OAuthEntry) -> Result<()> {
        let mut storage = self.load().await?;
        storage.entries.insert(server.to_string(), entry.clone());
        self.save(&storage).await
    }

    /// Remove all credentials for a server
    pub async fn remove(&self, server: &str) -> Result<()> {
        let mut storage = self.load().await?;
        storage.entries.remove(server);
        self.save(&storage).await
    }

    /// List all servers with stored credentials
    pub async fn list_servers(&self) -> Result<Vec<String>> {
        let storage = self.load().await?;
        Ok(storage.entries.keys().cloned().collect())
    }

    /// Clear the in-memory cache
    pub async fn clear_cache(&self) {
        let mut cache = self.cache.write().await;
        *cache = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_oauth_storage_save_and_load() {
        let dir = tempdir().unwrap();
        let storage = OAuthStorage::new(dir.path().join("mcp-auth.json"));

        let tokens = OAuthTokens {
            access_token: "test_token".to_string(),
            refresh_token: Some("refresh".to_string()),
            expires_at: Some(1234567890),
            scope: None,
        };

        storage.save_tokens("test-server", &tokens).await.unwrap();

        let loaded = storage.get_tokens("test-server").await.unwrap();
        assert!(loaded.is_some());
        assert_eq!(loaded.unwrap().access_token, "test_token");
    }

    #[tokio::test]
    async fn test_oauth_storage_remove() {
        let dir = tempdir().unwrap();
        let storage = OAuthStorage::new(dir.path().join("mcp-auth.json"));

        let tokens = OAuthTokens {
            access_token: "test".to_string(),
            refresh_token: None,
            expires_at: None,
            scope: None,
        };

        storage.save_tokens("server1", &tokens).await.unwrap();
        storage.remove("server1").await.unwrap();

        let loaded = storage.get_tokens("server1").await.unwrap();
        assert!(loaded.is_none());
    }

    #[tokio::test]
    async fn test_oauth_storage_nonexistent_server() {
        let dir = tempdir().unwrap();
        let storage = OAuthStorage::new(dir.path().join("mcp-auth.json"));

        let loaded = storage.get_tokens("nonexistent").await.unwrap();
        assert!(loaded.is_none());
    }

    #[tokio::test]
    async fn test_oauth_storage_client_info() {
        let dir = tempdir().unwrap();
        let storage = OAuthStorage::new(dir.path().join("mcp-auth.json"));

        let client_info = ClientInfo {
            client_id: "client123".to_string(),
            client_secret: Some("secret456".to_string()),
            client_id_issued_at: Some(1234567890),
            client_secret_expires_at: None,
        };

        storage
            .save_client_info("server1", &client_info)
            .await
            .unwrap();

        let loaded = storage.get_client_info("server1").await.unwrap();
        assert!(loaded.is_some());
        assert_eq!(loaded.unwrap().client_id, "client123");
    }

    #[tokio::test]
    async fn test_oauth_storage_list_servers() {
        let dir = tempdir().unwrap();
        let storage = OAuthStorage::new(dir.path().join("mcp-auth.json"));

        let tokens = OAuthTokens {
            access_token: "test".to_string(),
            refresh_token: None,
            expires_at: None,
            scope: None,
        };

        storage.save_tokens("server1", &tokens).await.unwrap();
        storage.save_tokens("server2", &tokens).await.unwrap();

        let servers = storage.list_servers().await.unwrap();
        assert_eq!(servers.len(), 2);
        assert!(servers.contains(&"server1".to_string()));
        assert!(servers.contains(&"server2".to_string()));
    }

    #[test]
    fn test_oauth_tokens_is_expired() {
        // Token that expired in the past
        let expired = OAuthTokens {
            access_token: "test".to_string(),
            refresh_token: None,
            expires_at: Some(0), // Unix epoch
            scope: None,
        };
        assert!(expired.is_expired());

        // Token that expires far in the future
        let valid = OAuthTokens {
            access_token: "test".to_string(),
            refresh_token: None,
            expires_at: Some(9999999999),
            scope: None,
        };
        assert!(!valid.is_expired());

        // Token without expiration
        let no_expiry = OAuthTokens {
            access_token: "test".to_string(),
            refresh_token: None,
            expires_at: None,
            scope: None,
        };
        assert!(!no_expiry.is_expired());
    }

    #[test]
    fn test_oauth_tokens_can_refresh() {
        let with_refresh = OAuthTokens {
            access_token: "test".to_string(),
            refresh_token: Some("refresh".to_string()),
            expires_at: None,
            scope: None,
        };
        assert!(with_refresh.can_refresh());

        let without_refresh = OAuthTokens {
            access_token: "test".to_string(),
            refresh_token: None,
            expires_at: None,
            scope: None,
        };
        assert!(!without_refresh.can_refresh());
    }
}
