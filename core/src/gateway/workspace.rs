//! Workspace Manager
//!
//! Workspaces provide context isolation and profile-based configuration.
//! Each workspace is an instance of a profile, with its own session and cache state.
//!
//! # Architecture
//!
//! ```text
//! Profile (Static Template)     Workspace (Runtime Instance)
//! ├── model binding        →    ├── session_key (ws-{id})
//! ├── tool whitelist       →    ├── cache_state
//! ├── system_prompt        →    └── env_vars
//! └── temperature
//! ```
//!
//! # Example
//!
//! ```rust,ignore
//! let manager = WorkspaceManager::new(config)?;
//!
//! // Create a workspace from a profile
//! let ws = manager.create("project-x", "coding").await?;
//!
//! // Switch user's active workspace
//! manager.set_active("user-123", "project-x").await?;
//!
//! // Get current workspace for user
//! let active = manager.get_active("user-123").await?;
//! ```

use chrono::{DateTime, Utc};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use thiserror::Error;
use tracing::{debug, info};

use crate::config::ProfileConfig;
use crate::routing::SessionKey;

// =============================================================================
// Workspace
// =============================================================================

/// A workspace instance - runtime state derived from a profile
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Workspace {
    /// Unique workspace identifier (e.g., "project-aleph")
    pub id: String,

    /// Profile this workspace inherits from (e.g., "coding")
    pub profile: String,

    /// When this workspace was created
    pub created_at: DateTime<Utc>,

    /// Last activity timestamp
    pub last_active_at: DateTime<Utc>,

    /// Provider-side context caching state
    pub cache_state: CacheState,

    /// Environment variables specific to this workspace
    #[serde(default)]
    pub env_vars: HashMap<String, String>,

    /// Optional description
    pub description: Option<String>,
}

impl Workspace {
    /// Create a new workspace
    pub fn new(id: impl Into<String>, profile: impl Into<String>) -> Self {
        let id = id.into();
        let now = Utc::now();

        Self {
            id: id.clone(),
            profile: profile.into(),
            created_at: now,
            last_active_at: now,
            cache_state: CacheState::None,
            env_vars: HashMap::new(),
            description: None,
        }
    }

    /// Get the session key for this workspace
    pub fn session_key(&self, agent_id: &str) -> SessionKey {
        SessionKey::Main {
            agent_id: agent_id.to_string(),
            main_key: format!("ws-{}", self.id),
        }
    }

    /// Get the storage key string
    pub fn storage_key(&self, agent_id: &str) -> String {
        self.session_key(agent_id).to_key_string()
    }

    /// Check if this is the global/default workspace
    pub fn is_global(&self) -> bool {
        self.id == "global" || self.id == "main"
    }
}

// =============================================================================
// CacheState
// =============================================================================

/// Provider-side context caching state
///
/// Different providers have different caching mechanisms:
/// - Anthropic: Ephemeral (cache_control blocks, no persistent state)
/// - Gemini: Persistent (explicit cache name, stored on provider side)
/// - OpenAI: Transparent (automatic, no state needed)
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CacheState {
    /// No caching active
    #[default]
    None,

    /// Anthropic-style ephemeral caching
    /// Only need to track where to insert cache_control markers
    Ephemeral {
        /// Index in message history where cache breakpoint is set
        cache_breakpoint_index: Option<usize>,
        /// Token count at the breakpoint
        tokens_cached: Option<u64>,
    },

    /// Gemini-style persistent caching
    /// Need to store cache name and track expiry
    Persistent {
        /// Provider-assigned cache name
        cache_name: String,
        /// Hash of cached content (to detect changes)
        content_hash: String,
        /// When the cache expires
        expires_at: DateTime<Utc>,
        /// Token count in cache
        tokens_cached: u64,
    },

    /// OpenAI-style transparent caching
    /// No state needed, provider handles automatically
    Transparent,
}

impl CacheState {
    /// Check if cache is active and valid
    pub fn is_active(&self) -> bool {
        match self {
            CacheState::None => false,
            CacheState::Ephemeral { cache_breakpoint_index, .. } => cache_breakpoint_index.is_some(),
            CacheState::Persistent { expires_at, .. } => *expires_at > Utc::now(),
            CacheState::Transparent => true,
        }
    }

    /// Get the cached token count
    pub fn tokens_cached(&self) -> Option<u64> {
        match self {
            CacheState::None => None,
            CacheState::Ephemeral { tokens_cached, .. } => *tokens_cached,
            CacheState::Persistent { tokens_cached, .. } => Some(*tokens_cached),
            CacheState::Transparent => None,
        }
    }
}

// =============================================================================
// UserActiveWorkspace
// =============================================================================

/// Tracks which workspace a user is currently in
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserActiveWorkspace {
    /// User identifier
    pub user_id: String,
    /// Currently active workspace ID
    pub workspace_id: String,
    /// When this was last updated
    pub updated_at: DateTime<Utc>,
}

// =============================================================================
// WorkspaceManager
// =============================================================================

/// Configuration for WorkspaceManager
#[derive(Debug, Clone)]
pub struct WorkspaceManagerConfig {
    /// Database path for workspace storage
    pub db_path: PathBuf,
    /// Default profile for new workspaces
    pub default_profile: String,
    /// Auto-archive workspaces after N days of inactivity (0 = never)
    pub archive_after_days: u32,
}

impl Default for WorkspaceManagerConfig {
    fn default() -> Self {
        Self {
            db_path: dirs::home_dir()
                .unwrap_or_else(|| PathBuf::from("/tmp"))
                .join(".aleph/workspaces.db"),
            default_profile: "default".to_string(),
            archive_after_days: 30,
        }
    }
}

/// Workspace manager with SQLite persistence
pub struct WorkspaceManager {
    config: WorkspaceManagerConfig,
    conn: Arc<Mutex<Connection>>,
    /// In-memory profile cache (loaded from config)
    profiles: Arc<Mutex<HashMap<String, ProfileConfig>>>,
}

impl WorkspaceManager {
    /// Create a new workspace manager
    pub fn new(config: WorkspaceManagerConfig) -> Result<Self, WorkspaceError> {
        // Ensure parent directory exists
        if let Some(parent) = config.db_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                WorkspaceError::Database(format!("Failed to create db directory: {}", e))
            })?;
        }

        let conn = Connection::open(&config.db_path).map_err(|e| {
            WorkspaceError::Database(format!("Failed to open database: {}", e))
        })?;

        Self::init_schema(&conn)?;

        info!("Workspace manager initialized: {:?}", config.db_path);

        Ok(Self {
            config,
            conn: Arc::new(Mutex::new(conn)),
            profiles: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    /// Create with default configuration
    pub fn with_defaults() -> Result<Self, WorkspaceError> {
        Self::new(WorkspaceManagerConfig::default())
    }

    /// Initialize database schema
    fn init_schema(conn: &Connection) -> Result<(), WorkspaceError> {
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS workspaces (
                id TEXT PRIMARY KEY,
                profile TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                last_active_at INTEGER NOT NULL,
                cache_state TEXT,
                env_vars TEXT,
                description TEXT,
                archived INTEGER DEFAULT 0
            );

            CREATE TABLE IF NOT EXISTS user_active_workspace (
                user_id TEXT PRIMARY KEY,
                workspace_id TEXT NOT NULL,
                updated_at INTEGER NOT NULL,
                FOREIGN KEY (workspace_id) REFERENCES workspaces(id)
            );

            CREATE INDEX IF NOT EXISTS idx_workspaces_profile ON workspaces(profile);
            CREATE INDEX IF NOT EXISTS idx_workspaces_last_active ON workspaces(last_active_at);
            "#,
        )
        .map_err(|e| WorkspaceError::Database(format!("Schema init failed: {}", e)))?;

        // Ensure global workspace exists
        let now = Utc::now().timestamp();
        conn.execute(
            "INSERT OR IGNORE INTO workspaces (id, profile, created_at, last_active_at, description)
             VALUES ('global', 'default', ?, ?, 'Default global workspace')",
            params![now, now],
        )
        .ok();

        Ok(())
    }

    /// Load profiles from config
    pub fn load_profiles(&self, profiles: HashMap<String, ProfileConfig>) {
        let mut cache = self.profiles.lock().unwrap();
        *cache = profiles;

        // Ensure default profile exists
        if !cache.contains_key("default") {
            cache.insert("default".to_string(), ProfileConfig::default());
        }
    }

    /// Get a profile by name
    pub fn get_profile(&self, name: &str) -> Option<ProfileConfig> {
        let cache = self.profiles.lock().unwrap();
        cache.get(name).cloned()
    }

    /// List all available profiles
    pub fn list_profiles(&self) -> Vec<String> {
        let cache = self.profiles.lock().unwrap();
        cache.keys().cloned().collect()
    }

    // =========================================================================
    // Workspace CRUD
    // =========================================================================

    /// Create a new workspace
    pub async fn create(
        &self,
        id: &str,
        profile: &str,
        description: Option<&str>,
    ) -> Result<Workspace, WorkspaceError> {
        // Validate profile exists
        if self.get_profile(profile).is_none() {
            return Err(WorkspaceError::ProfileNotFound(profile.to_string()));
        }

        let now = Utc::now();
        let workspace = Workspace {
            id: id.to_string(),
            profile: profile.to_string(),
            created_at: now,
            last_active_at: now,
            cache_state: CacheState::None,
            env_vars: HashMap::new(),
            description: description.map(String::from),
        };

        let conn = self.conn.lock().map_err(|e| {
            WorkspaceError::Database(format!("Lock error: {}", e))
        })?;

        conn.execute(
            "INSERT INTO workspaces (id, profile, created_at, last_active_at, description)
             VALUES (?, ?, ?, ?, ?)",
            params![
                &workspace.id,
                &workspace.profile,
                now.timestamp(),
                now.timestamp(),
                &workspace.description,
            ],
        )
        .map_err(|e| {
            if e.to_string().contains("UNIQUE constraint") {
                WorkspaceError::AlreadyExists(id.to_string())
            } else {
                WorkspaceError::Database(format!("Insert failed: {}", e))
            }
        })?;

        info!("Created workspace '{}' with profile '{}'", id, profile);

        Ok(workspace)
    }

    /// Get a workspace by ID
    pub async fn get(&self, id: &str) -> Result<Option<Workspace>, WorkspaceError> {
        let conn = self.conn.lock().map_err(|e| {
            WorkspaceError::Database(format!("Lock error: {}", e))
        })?;

        let result = conn.query_row(
            "SELECT id, profile, created_at, last_active_at, cache_state, env_vars, description
             FROM workspaces WHERE id = ? AND archived = 0",
            params![id],
            |row| {
                let cache_state_json: Option<String> = row.get(4)?;
                let env_vars_json: Option<String> = row.get(5)?;

                Ok(Workspace {
                    id: row.get(0)?,
                    profile: row.get(1)?,
                    created_at: DateTime::from_timestamp(row.get::<_, i64>(2)?, 0)
                        .unwrap_or_else(Utc::now),
                    last_active_at: DateTime::from_timestamp(row.get::<_, i64>(3)?, 0)
                        .unwrap_or_else(Utc::now),
                    cache_state: cache_state_json
                        .and_then(|j| serde_json::from_str(&j).ok())
                        .unwrap_or_default(),
                    env_vars: env_vars_json
                        .and_then(|j| serde_json::from_str(&j).ok())
                        .unwrap_or_default(),
                    description: row.get(6)?,
                })
            },
        );

        match result {
            Ok(ws) => Ok(Some(ws)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(WorkspaceError::Database(e.to_string())),
        }
    }

    /// List all workspaces
    pub async fn list(&self, include_archived: bool) -> Result<Vec<Workspace>, WorkspaceError> {
        let conn = self.conn.lock().map_err(|e| {
            WorkspaceError::Database(format!("Lock error: {}", e))
        })?;

        let query = if include_archived {
            "SELECT id, profile, created_at, last_active_at, cache_state, env_vars, description
             FROM workspaces ORDER BY last_active_at DESC"
        } else {
            "SELECT id, profile, created_at, last_active_at, cache_state, env_vars, description
             FROM workspaces WHERE archived = 0 ORDER BY last_active_at DESC"
        };

        let mut stmt = conn.prepare(query)
            .map_err(|e| WorkspaceError::Database(e.to_string()))?;

        let workspaces = stmt
            .query_map([], |row| {
                let cache_state_json: Option<String> = row.get(4)?;
                let env_vars_json: Option<String> = row.get(5)?;

                Ok(Workspace {
                    id: row.get(0)?,
                    profile: row.get(1)?,
                    created_at: DateTime::from_timestamp(row.get::<_, i64>(2)?, 0)
                        .unwrap_or_else(Utc::now),
                    last_active_at: DateTime::from_timestamp(row.get::<_, i64>(3)?, 0)
                        .unwrap_or_else(Utc::now),
                    cache_state: cache_state_json
                        .and_then(|j| serde_json::from_str(&j).ok())
                        .unwrap_or_default(),
                    env_vars: env_vars_json
                        .and_then(|j| serde_json::from_str(&j).ok())
                        .unwrap_or_default(),
                    description: row.get(6)?,
                })
            })
            .map_err(|e| WorkspaceError::Database(e.to_string()))?
            .filter_map(|r| r.ok())
            .collect();

        Ok(workspaces)
    }

    /// Update workspace's last active timestamp
    pub async fn touch(&self, id: &str) -> Result<(), WorkspaceError> {
        let conn = self.conn.lock().map_err(|e| {
            WorkspaceError::Database(format!("Lock error: {}", e))
        })?;

        conn.execute(
            "UPDATE workspaces SET last_active_at = ? WHERE id = ?",
            params![Utc::now().timestamp(), id],
        )
        .map_err(|e| WorkspaceError::Database(e.to_string()))?;

        Ok(())
    }

    /// Update workspace's cache state
    pub async fn update_cache_state(
        &self,
        id: &str,
        cache_state: &CacheState,
    ) -> Result<(), WorkspaceError> {
        let conn = self.conn.lock().map_err(|e| {
            WorkspaceError::Database(format!("Lock error: {}", e))
        })?;

        let cache_json = serde_json::to_string(cache_state)
            .map_err(|e| WorkspaceError::Database(format!("Serialize error: {}", e)))?;

        conn.execute(
            "UPDATE workspaces SET cache_state = ?, last_active_at = ? WHERE id = ?",
            params![cache_json, Utc::now().timestamp(), id],
        )
        .map_err(|e| WorkspaceError::Database(e.to_string()))?;

        debug!("Updated cache state for workspace '{}'", id);

        Ok(())
    }

    /// Archive a workspace (soft delete)
    pub async fn archive(&self, id: &str) -> Result<bool, WorkspaceError> {
        if id == "global" {
            return Err(WorkspaceError::CannotModifyGlobal);
        }

        let conn = self.conn.lock().map_err(|e| {
            WorkspaceError::Database(format!("Lock error: {}", e))
        })?;

        let affected = conn
            .execute("UPDATE workspaces SET archived = 1 WHERE id = ?", params![id])
            .map_err(|e| WorkspaceError::Database(e.to_string()))?;

        if affected > 0 {
            info!("Archived workspace '{}'", id);
        }

        Ok(affected > 0)
    }

    /// Delete a workspace permanently
    pub async fn delete(&self, id: &str) -> Result<bool, WorkspaceError> {
        if id == "global" {
            return Err(WorkspaceError::CannotModifyGlobal);
        }

        let conn = self.conn.lock().map_err(|e| {
            WorkspaceError::Database(format!("Lock error: {}", e))
        })?;

        // First remove any user active workspace references
        conn.execute(
            "UPDATE user_active_workspace SET workspace_id = 'global' WHERE workspace_id = ?",
            params![id],
        )
        .ok();

        let affected = conn
            .execute("DELETE FROM workspaces WHERE id = ?", params![id])
            .map_err(|e| WorkspaceError::Database(e.to_string()))?;

        if affected > 0 {
            info!("Deleted workspace '{}'", id);
        }

        Ok(affected > 0)
    }

    // =========================================================================
    // User Active Workspace
    // =========================================================================

    /// Set the active workspace for a user
    pub async fn set_active(&self, user_id: &str, workspace_id: &str) -> Result<(), WorkspaceError> {
        // Verify workspace exists
        if self.get(workspace_id).await?.is_none() {
            return Err(WorkspaceError::NotFound(workspace_id.to_string()));
        }

        let conn = self.conn.lock().map_err(|e| {
            WorkspaceError::Database(format!("Lock error: {}", e))
        })?;

        conn.execute(
            "INSERT OR REPLACE INTO user_active_workspace (user_id, workspace_id, updated_at)
             VALUES (?, ?, ?)",
            params![user_id, workspace_id, Utc::now().timestamp()],
        )
        .map_err(|e| WorkspaceError::Database(e.to_string()))?;

        // Touch the workspace
        conn.execute(
            "UPDATE workspaces SET last_active_at = ? WHERE id = ?",
            params![Utc::now().timestamp(), workspace_id],
        )
        .ok();

        debug!("Set active workspace for user '{}' to '{}'", user_id, workspace_id);

        Ok(())
    }

    /// Get the active workspace for a user
    pub async fn get_active(&self, user_id: &str) -> Result<Workspace, WorkspaceError> {
        let workspace_id = {
            let conn = self.conn.lock().map_err(|e| {
                WorkspaceError::Database(format!("Lock error: {}", e))
            })?;

            conn.query_row(
                "SELECT workspace_id FROM user_active_workspace WHERE user_id = ?",
                params![user_id],
                |row| row.get::<_, String>(0),
            )
            .unwrap_or_else(|_| "global".to_string())
        };

        self.get(&workspace_id)
            .await?
            .ok_or_else(|| WorkspaceError::NotFound(workspace_id))
    }

    /// Get the active workspace ID for a user (lightweight, no full workspace fetch)
    pub async fn get_active_id(&self, user_id: &str) -> String {
        let conn = match self.conn.lock() {
            Ok(c) => c,
            Err(_) => return "global".to_string(),
        };

        conn.query_row(
            "SELECT workspace_id FROM user_active_workspace WHERE user_id = ?",
            params![user_id],
            |row| row.get::<_, String>(0),
        )
        .unwrap_or_else(|_| "global".to_string())
    }

    // =========================================================================
    // Maintenance
    // =========================================================================

    /// Archive inactive workspaces
    pub async fn archive_inactive(&self) -> Result<usize, WorkspaceError> {
        if self.config.archive_after_days == 0 {
            return Ok(0);
        }

        let threshold = Utc::now().timestamp()
            - (self.config.archive_after_days as i64 * 24 * 60 * 60);

        let conn = self.conn.lock().map_err(|e| {
            WorkspaceError::Database(format!("Lock error: {}", e))
        })?;

        let affected = conn
            .execute(
                "UPDATE workspaces SET archived = 1
                 WHERE last_active_at < ? AND id != 'global' AND archived = 0",
                params![threshold],
            )
            .map_err(|e| WorkspaceError::Database(e.to_string()))?;

        if affected > 0 {
            info!("Archived {} inactive workspaces", affected);
        }

        Ok(affected)
    }
}

// =============================================================================
// SessionKey Extension
// =============================================================================

impl SessionKey {
    /// Create a session key for a workspace
    pub fn workspace(agent_id: impl Into<String>, workspace_id: impl Into<String>) -> Self {
        Self::Main {
            agent_id: agent_id.into(),
            main_key: format!("ws-{}", workspace_id.into()),
        }
    }

    /// Check if this is a workspace session key
    pub fn is_workspace(&self) -> bool {
        match self {
            Self::Main { main_key, .. } => main_key.starts_with("ws-"),
            _ => false,
        }
    }

    /// Get the workspace ID if this is a workspace session key
    pub fn workspace_id(&self) -> Option<&str> {
        match self {
            Self::Main { main_key, .. } if main_key.starts_with("ws-") => {
                Some(&main_key[3..])
            }
            _ => None,
        }
    }
}

// =============================================================================
// Errors
// =============================================================================

#[derive(Debug, Error)]
pub enum WorkspaceError {
    #[error("Database error: {0}")]
    Database(String),

    #[error("Workspace not found: {0}")]
    NotFound(String),

    #[error("Workspace already exists: {0}")]
    AlreadyExists(String),

    #[error("Profile not found: {0}")]
    ProfileNotFound(String),

    #[error("Cannot modify global workspace")]
    CannotModifyGlobal,
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn test_config(path: PathBuf) -> WorkspaceManagerConfig {
        WorkspaceManagerConfig {
            db_path: path,
            default_profile: "default".to_string(),
            archive_after_days: 30,
        }
    }

    #[tokio::test]
    async fn test_workspace_creation() {
        let temp = tempdir().unwrap();
        let config = test_config(temp.path().join("test.db"));
        let manager = WorkspaceManager::new(config).unwrap();

        // Load default profile
        let mut profiles = HashMap::new();
        profiles.insert("coding".to_string(), ProfileConfig {
            description: Some("Coding profile".to_string()),
            model: Some("claude-sonnet".to_string()),
            tools: vec!["git_*".to_string()],
            ..Default::default()
        });
        manager.load_profiles(profiles);

        let ws = manager.create("project-x", "coding", Some("My project")).await.unwrap();

        assert_eq!(ws.id, "project-x");
        assert_eq!(ws.profile, "coding");
        assert_eq!(ws.description, Some("My project".to_string()));
    }

    #[tokio::test]
    async fn test_workspace_session_key() {
        let ws = Workspace::new("project-x", "coding");
        let key = ws.session_key("main");

        assert_eq!(key.to_key_string(), "agent:main:ws-project-x");
        assert!(key.is_workspace());
        assert_eq!(key.workspace_id(), Some("project-x"));
    }

    #[tokio::test]
    async fn test_global_workspace_exists() {
        let temp = tempdir().unwrap();
        let config = test_config(temp.path().join("test.db"));
        let manager = WorkspaceManager::new(config).unwrap();

        let global = manager.get("global").await.unwrap();
        assert!(global.is_some());
        assert!(global.unwrap().is_global());
    }

    #[tokio::test]
    async fn test_user_active_workspace() {
        let temp = tempdir().unwrap();
        let config = test_config(temp.path().join("test.db"));
        let manager = WorkspaceManager::new(config).unwrap();

        // Load profiles
        manager.load_profiles(HashMap::from([
            ("coding".to_string(), ProfileConfig::default()),
        ]));

        // Create workspace
        manager.create("my-project", "coding", None).await.unwrap();

        // Set active
        manager.set_active("user-123", "my-project").await.unwrap();

        // Get active
        let active = manager.get_active("user-123").await.unwrap();
        assert_eq!(active.id, "my-project");

        // Unknown user defaults to global
        let default = manager.get_active("unknown-user").await.unwrap();
        assert_eq!(default.id, "global");
    }

    #[tokio::test]
    async fn test_cannot_delete_global() {
        let temp = tempdir().unwrap();
        let config = test_config(temp.path().join("test.db"));
        let manager = WorkspaceManager::new(config).unwrap();

        let result = manager.delete("global").await;
        assert!(matches!(result, Err(WorkspaceError::CannotModifyGlobal)));
    }

    #[tokio::test]
    async fn test_cache_state_update() {
        let temp = tempdir().unwrap();
        let config = test_config(temp.path().join("test.db"));
        let manager = WorkspaceManager::new(config).unwrap();

        manager.load_profiles(HashMap::from([
            ("coding".to_string(), ProfileConfig::default()),
        ]));

        manager.create("test-ws", "coding", None).await.unwrap();

        let cache_state = CacheState::Persistent {
            cache_name: "cache-123".to_string(),
            content_hash: "abc".to_string(),
            expires_at: Utc::now() + chrono::Duration::hours(1),
            tokens_cached: 50000,
        };

        manager.update_cache_state("test-ws", &cache_state).await.unwrap();

        let ws = manager.get("test-ws").await.unwrap().unwrap();
        assert!(matches!(ws.cache_state, CacheState::Persistent { .. }));
    }

    #[test]
    fn test_session_key_workspace_methods() {
        let key = SessionKey::workspace("main", "coding");
        assert_eq!(key.to_key_string(), "agent:main:ws-coding");
        assert!(key.is_workspace());
        assert_eq!(key.workspace_id(), Some("coding"));

        let main_key = SessionKey::main("main");
        assert!(!main_key.is_workspace());
        assert_eq!(main_key.workspace_id(), None);
    }
}
