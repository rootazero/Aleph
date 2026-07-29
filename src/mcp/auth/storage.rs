//! OAuth Credential Storage
//!
//! Securely stores OAuth tokens and client information for MCP servers.
//! Credentials are stored in a JSON file with secure permissions (0600 on Unix).

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::SystemTime;

use serde::{Deserialize, Serialize};
use tokio::fs;
use tokio::sync::RwLock;

use crate::error::{AlephError, Result};

/// OAuth tokens received from an authorization server
#[derive(Clone, Serialize, Deserialize)]
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

// Manual Debug impl: never expose token material in logs/panics/{:?}.
impl std::fmt::Debug for OAuthTokens {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OAuthTokens")
            .field("access_token", &"<redacted>")
            .field(
                "refresh_token",
                &self.refresh_token.as_ref().map(|_| "<redacted>"),
            )
            .field("expires_at", &self.expires_at)
            .field("scope", &self.scope)
            .finish()
    }
}

impl OAuthTokens {
    /// Check if the token is expired (with 5 minute buffer)
    #[must_use]
    pub fn is_expired(&self) -> bool {
        if let Some(expires_at) = self.expires_at {
            let now: i64 = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs()
                .try_into()
                .unwrap_or(i64::MAX);
            // Add 5 minute buffer (saturating to prevent overflow on adversarial values)
            expires_at.saturating_sub(300) < now
        } else {
            false
        }
    }

    /// Check if the token can be refreshed
    #[must_use]
    pub const fn can_refresh(&self) -> bool {
        self.refresh_token.is_some()
    }
}

/// Dynamic client registration information
///
/// Some OAuth servers support dynamic client registration, where clients
/// can register themselves at runtime rather than using pre-configured credentials.
#[derive(Clone, Serialize, Deserialize)]
pub struct ClientInfo {
    /// Client ID received from registration
    pub client_id: String,
    /// Client secret (if provided)
    pub client_secret: Option<String>,
    /// Unix timestamp when `client_id` was issued
    pub client_id_issued_at: Option<i64>,
    /// Unix timestamp when `client_secret` expires (0 = never)
    pub client_secret_expires_at: Option<i64>,
    /// The authorization server that issued these credentials.
    ///
    /// Client credentials are scoped to their issuer: presenting them to a
    /// different authorization server leaks a client identity across trust
    /// boundaries, so a change of issuer must force a re-registration rather
    /// than a reuse. `None` on entries written before this field existed, and
    /// on servers whose metadata advertises no `issuer`.
    #[serde(default)]
    pub issuer: Option<String>,
}

// Manual Debug impl: client_secret is a credential — never log it.
impl std::fmt::Debug for ClientInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClientInfo")
            .field("client_id", &self.client_id)
            .field(
                "client_secret",
                &self.client_secret.as_ref().map(|_| "<redacted>"),
            )
            .field("client_id_issued_at", &self.client_id_issued_at)
            .field("client_secret_expires_at", &self.client_secret_expires_at)
            .field("issuer", &self.issuer)
            .finish()
    }
}

/// OAuth entry for a server
///
/// Stores all OAuth-related information for a single MCP server.
#[derive(Clone, Default, Serialize, Deserialize)]
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
    /// The authorization server issuer recorded when the in-flight
    /// authorization began.
    ///
    /// RFC 9207: when the authorization response carries an `iss` parameter the
    /// client must check it against this value *before* redeeming the code, so
    /// a code minted by one authorization server cannot be redeemed as if it
    /// came from another.
    #[serde(default)]
    pub issuer: Option<String>,
}

// Manual Debug impl: code_verifier (PKCE secret) and oauth_state (CSRF token)
// must not leak; tokens/client_info redact themselves via their own Debug.
impl std::fmt::Debug for OAuthEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OAuthEntry")
            .field("tokens", &self.tokens)
            .field("client_info", &self.client_info)
            .field(
                "code_verifier",
                &self.code_verifier.as_ref().map(|_| "<redacted>"),
            )
            .field(
                "oauth_state",
                &self.oauth_state.as_ref().map(|_| "<redacted>"),
            )
            .field("server_url", &self.server_url)
            .field("issuer", &self.issuer)
            .finish()
    }
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
    /// Modified-time of `file_path` when `cache` was last populated. Lets a
    /// long-running process notice tokens refreshed on disk by *another*
    /// process (a cron job, the `aleph` CLI, the desktop app) instead of
    /// serving a stale in-memory copy until restart. Mirrors Claude Code's
    /// `invalidateOAuthCacheIfDiskChanged`.
    cached_mtime: RwLock<Option<SystemTime>>,
}

impl OAuthStorage {
    /// Create new storage at the specified path
    #[must_use]
    pub fn new(file_path: PathBuf) -> Self {
        Self {
            file_path,
            cache: RwLock::new(None),
            cached_mtime: RwLock::new(None),
        }
    }

    /// Current modified-time of the storage file, or `None` if it cannot be
    /// stat'd (missing, permission error). `None` is treated as "do not
    /// invalidate" so a transient stat failure never thrashes the cache.
    async fn file_mtime(&self) -> Option<SystemTime> {
        fs::metadata(&self.file_path)
            .await
            .ok()
            .and_then(|m| m.modified().ok())
    }

    /// Get the default storage location
    ///
    /// Uses the system data directory:
    /// - macOS: ~/Library/Application Support/aleph/mcp-auth.json
    /// - Linux: ~/.local/share/aleph/mcp-auth.json
    /// - Windows: %APPDATA%\aleph\mcp-auth.json
    pub fn default_path() -> PathBuf {
        dirs::data_dir()
            .or_else(dirs::home_dir)
            .unwrap_or_else(|| PathBuf::from("/tmp"))
            .join("aleph")
            .join("mcp-auth.json")
    }

    /// Load storage file
    async fn load(&self) -> Result<StorageFile> {
        // Check cache first — but only trust it if the file on disk has not
        // been rewritten by another process since we cached it. If the stat
        // fails (disk_mtime is None) we keep the cache rather than thrash.
        {
            let cache = self.cache.read().await;
            if let Some(ref storage) = *cache {
                let disk_mtime = self.file_mtime().await;
                let cached_mtime = *self.cached_mtime.read().await;
                if disk_mtime.is_none() || disk_mtime == cached_mtime {
                    // rust-doctor-disable-next-line excessive-clone
                    return Ok(storage.clone());
                }
                tracing::debug!("OAuth storage changed on disk; reloading cached credentials");
            }
        }

        // Load from file (no exists() check — match on error kind to avoid TOCTOU)
        let content = match fs::read_to_string(&self.file_path).await {
            Ok(c) => c,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Ok(StorageFile::default());
            }
            Err(e) => {
                return Err(AlephError::IoError(format!(
                    "Failed to read OAuth storage: {e}"
                )));
            }
        };

        let storage: StorageFile = serde_json::from_str(&content)
            .map_err(|e| AlephError::IoError(format!("Failed to parse OAuth storage: {e}")))?;

        // Update cache, recording the mtime so the next load can detect an
        // out-of-process rewrite.
        let disk_mtime = self.file_mtime().await;
        {
            let mut cache = self.cache.write().await;
            // rust-doctor-disable-next-line excessive-clone
            *cache = Some(storage.clone());
            *self.cached_mtime.write().await = disk_mtime;
        }

        Ok(storage)
    }

    /// Write storage to file without updating cache (caller manages cache lock)
    async fn save_to_file(&self, storage: &StorageFile) -> Result<()> {
        // Create parent directory if needed
        if let Some(parent) = self.file_path.parent() {
            fs::create_dir_all(parent).await.map_err(|e| {
                AlephError::IoError(format!("Failed to create OAuth storage dir: {e}"))
            })?;
        }

        let content = serde_json::to_string_pretty(storage)
            .map_err(|e| AlephError::IoError(format!("Failed to serialize OAuth storage: {e}")))?;

        fs::write(&self.file_path, content)
            .await
            .map_err(|e| AlephError::IoError(format!("Failed to write OAuth storage: {e}")))?;

        // Set file permissions to 0600 on Unix (owner read/write only)
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = std::fs::Permissions::from_mode(0o600);
            if let Err(e) = tokio::fs::set_permissions(&self.file_path, perms).await {
                tracing::warn!(
                    error = %e,
                    "Failed to set secure permissions on OAuth storage"
                );
            }
        }

        // Record the mtime of the file we just wrote so the disk-watch in
        // `load()` does not mistake our own write for an out-of-process change.
        // Covers every writer (save_tokens/client_info/entry, remove) at once.
        *self.cached_mtime.write().await = self.file_mtime().await;

        Ok(())
    }

    /// Load storage from file without using cache
    async fn load_from_file(&self) -> Result<StorageFile> {
        match fs::read_to_string(&self.file_path).await {
            Ok(content) => serde_json::from_str(&content)
                .map_err(|e| AlephError::IoError(format!("Failed to parse OAuth storage: {e}"))),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(StorageFile::default()),
            Err(e) => Err(AlephError::IoError(format!(
                "Failed to read OAuth storage: {e}"
            ))),
        }
    }

    /// Load current storage state while the `cache` write lock is held.
    ///
    /// Trusts the in-memory snapshot only when the on-disk file has not advanced
    /// past the mtime recorded when we cached it; otherwise re-reads from disk so
    /// a concurrent writer's update (another `OAuthStorage` instance sharing the
    /// same file, or another process) is merged rather than clobbered by a stale
    /// snapshot. Mirrors the staleness check in [`Self::load`] — the read path
    /// already guarded against out-of-process rewrites, while every write path
    /// trusted its cache unconditionally and could revert a concurrent update.
    ///
    /// Takes `cached` by ref because the caller already holds the `cache` write
    /// guard; `cached_mtime` is a separate lock, so this does not re-enter it.
    async fn load_for_write(&self, cached: Option<&StorageFile>) -> Result<StorageFile> {
        if let Some(storage) = cached {
            let disk_mtime = self.file_mtime().await;
            let cached_mtime = *self.cached_mtime.read().await;
            if disk_mtime.is_none() || disk_mtime == cached_mtime {
                // rust-doctor-disable-next-line excessive-clone
                return Ok(storage.clone());
            }
            tracing::debug!("OAuth storage changed on disk before write; reloading to merge");
        }
        self.load_from_file().await
    }

    /// Get tokens for a server
    pub async fn get_tokens(&self, server: &str) -> Result<Option<OAuthTokens>> {
        let storage = self.load().await?;
        // rust-doctor-disable-next-line excessive-clone
        Ok(storage.entries.get(server).and_then(|e| e.tokens.clone()))
    }

    /// Save tokens for a server
    ///
    /// Uses the cache write lock to serialize load-modify-save and prevent
    /// concurrent updates from overwriting each other.
    pub async fn save_tokens(&self, server: &str, tokens: &OAuthTokens) -> Result<()> {
        let mut cache = self.cache.write().await;

        // Load current state (from cache or file)
        let mut storage = self.load_for_write(cache.as_ref()).await?;

        let entry = storage
            .entries
            .entry(server.to_string())
            .or_insert_with(OAuthEntry::default);

        // rust-doctor-disable-next-line excessive-clone
        entry.tokens = Some(tokens.clone());
        self.save_to_file(&storage).await?;
        *cache = Some(storage);
        Ok(())
    }

    /// Get client info for a server
    pub async fn get_client_info(&self, server: &str) -> Result<Option<ClientInfo>> {
        let storage = self.load().await?;
        Ok(storage
            .entries
            .get(server)
            // rust-doctor-disable-next-line excessive-clone
            .and_then(|e| e.client_info.clone()))
    }

    /// Save client info for a server
    pub async fn save_client_info(&self, server: &str, client_info: &ClientInfo) -> Result<()> {
        let mut cache = self.cache.write().await;
        let mut storage = self.load_for_write(cache.as_ref()).await?;

        let entry = storage
            .entries
            .entry(server.to_string())
            .or_insert_with(OAuthEntry::default);

        // rust-doctor-disable-next-line excessive-clone
        entry.client_info = Some(client_info.clone());
        self.save_to_file(&storage).await?;
        *cache = Some(storage);
        Ok(())
    }

    /// Get the full OAuth entry for a server
    pub async fn get_entry(&self, server: &str) -> Result<Option<OAuthEntry>> {
        let storage = self.load().await?;
        Ok(storage.entries.get(server).cloned())
    }

    /// Save a full OAuth entry
    pub async fn save_entry(&self, server: &str, entry: &OAuthEntry) -> Result<()> {
        let mut cache = self.cache.write().await;
        let mut storage = self.load_for_write(cache.as_ref()).await?;
        // rust-doctor-disable-next-line excessive-clone
        storage.entries.insert(server.to_string(), entry.clone());
        self.save_to_file(&storage).await?;
        *cache = Some(storage);
        Ok(())
    }

    /// Remove all credentials for a server
    pub async fn remove(&self, server: &str) -> Result<()> {
        let mut cache = self.cache.write().await;
        let mut storage = self.load_for_write(cache.as_ref()).await?;
        storage.entries.remove(server);
        self.save_to_file(&storage).await?;
        *cache = Some(storage);
        Ok(())
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
    async fn external_overwrite_is_picked_up() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("mcp-auth.json");

        let storage = OAuthStorage::new(path.clone());
        let v1 = OAuthTokens {
            access_token: "v1".to_string(),
            refresh_token: None,
            expires_at: None,
            scope: None,
        };
        storage.save_tokens("srv", &v1).await.unwrap();
        assert_eq!(
            storage
                .get_tokens("srv")
                .await
                .unwrap()
                .unwrap()
                .access_token,
            "v1"
        );

        // Ensure the next write lands with a strictly newer mtime.
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;

        // Another process rewrites the same file with fresh tokens.
        let other = OAuthStorage::new(path.clone());
        let v2 = OAuthTokens {
            access_token: "v2".to_string(),
            refresh_token: None,
            expires_at: None,
            scope: None,
        };
        other.save_tokens("srv", &v2).await.unwrap();

        // The original handle must observe the on-disk change, not serve "v1".
        assert_eq!(
            storage
                .get_tokens("srv")
                .await
                .unwrap()
                .unwrap()
                .access_token,
            "v2"
        );
    }

    #[tokio::test]
    async fn concurrent_instance_write_does_not_clobber_other_entry() {
        // Regression: two OAuthStorage handles on the same file (e.g. a remote-MCP
        // connect refresh and a concurrent mcp_login, each constructing its own
        // instance with a private cache lock). Instance A caches a stale snapshot,
        // B writes a new server's tokens, then A writes a third server. A's write
        // must merge B's entry back from disk, not revert it (lost update).
        let dir = tempdir().unwrap();
        let path = dir.path().join("mcp-auth.json");
        let tok = |s: &str| OAuthTokens {
            access_token: s.to_string(),
            refresh_token: None,
            expires_at: None,
            scope: None,
        };

        let a = OAuthStorage::new(path.clone());
        a.save_tokens("srv-a", &tok("a")).await.unwrap(); // a caches {srv-a} @ t0

        // Newer mtime so A's stale cache is detectable on its next write.
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        let b = OAuthStorage::new(path.clone());
        b.save_tokens("srv-b", &tok("b")).await.unwrap(); // disk {srv-a, srv-b} @ t1

        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        a.save_tokens("srv-c", &tok("c")).await.unwrap(); // must merge, not clobber srv-b

        let reader = OAuthStorage::new(path.clone());
        let mut servers = reader.list_servers().await.unwrap();
        servers.sort();
        assert_eq!(servers, vec!["srv-a", "srv-b", "srv-c"]);
    }

    #[tokio::test]
    async fn cache_is_served_when_disk_unchanged() {
        let dir = tempdir().unwrap();
        let storage = OAuthStorage::new(dir.path().join("mcp-auth.json"));
        let tokens = OAuthTokens {
            access_token: "stable".to_string(),
            refresh_token: None,
            expires_at: None,
            scope: None,
        };
        storage.save_tokens("s", &tokens).await.unwrap();

        // Repeated reads with no external writer keep returning the cached value.
        for _ in 0..3 {
            assert_eq!(
                storage.get_tokens("s").await.unwrap().unwrap().access_token,
                "stable"
            );
        }
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
            issuer: Some("https://example.com".to_string()),
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
