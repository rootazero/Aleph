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
//! let ws = manager.create("project-x", "coding", None).await?;
//!
//! // Set active agent for a channel+peer
//! manager.set_active_agent("telegram", "user-123", "project-x")?;
//!
//! // Get active agent for a channel+peer
//! let agent_id = manager.get_active_agent("telegram", "user-123")?;
//! ```

use chrono::{DateTime, Utc};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use crate::sync_primitives::{Arc, Mutex};
use thiserror::Error;
use tracing::{debug, info};

mod manager_ops;

use crate::config::ProfileConfig;
use crate::memory::namespace::NamespaceScope;
use crate::memory::store::types::SearchFilter;
use crate::routing::SessionKey;

/// Default workspace identifier (maps to the "main" agent)
pub const DEFAULT_WORKSPACE: &str = "main";

// =============================================================================
// WorkspaceFilter
// =============================================================================

/// Filter for selecting workspaces when querying memory facts.
#[derive(Debug, Clone)]
pub enum WorkspaceFilter {
    /// Filter to a single workspace by id
    Single(String),
    /// Filter to multiple workspaces by id
    Multiple(Vec<String>),
    /// No filtering — include all workspaces
    All,
}

impl WorkspaceFilter {
    /// Convert the filter to a SQL WHERE clause fragment.
    ///
    /// Returns a string suitable for use in a SQL `WHERE` clause that filters
    /// on the `agent` column.
    pub fn to_sql_filter(&self) -> String {
        match self {
            WorkspaceFilter::Single(id) => {
                format!("agent = '{}'", id.replace('\'', "''"))
            }
            WorkspaceFilter::Multiple(ids) => {
                if ids.is_empty() {
                    return "1=0".to_string(); // match nothing
                }
                let escaped: Vec<String> = ids
                    .iter()
                    .map(|id| format!("'{}'", id.replace('\'', "''")))
                    .collect();
                format!("agent IN ({})", escaped.join(", "))
            }
            WorkspaceFilter::All => "1=1".to_string(),
        }
    }
}

// =============================================================================
// WorkspaceContext
// =============================================================================

/// Runtime context for the active workspace.
///
/// Created from a Session, flows through the Agent Loop to all memory
/// operations.  Carries the workspace identifier and namespace scope so
/// that every store call is automatically scoped.
#[derive(Debug, Clone)]
pub struct WorkspaceContext {
    /// Active workspace identifier.
    pub workspace_id: String,
    /// Namespace scope for access control.
    pub namespace: NamespaceScope,
}

impl WorkspaceContext {
    /// Create a new workspace context.
    pub fn new(workspace_id: impl Into<String>, namespace: NamespaceScope) -> Self {
        Self {
            workspace_id: workspace_id.into(),
            namespace,
        }
    }

    /// Convenience constructor for the default owner context.
    ///
    /// Uses `DEFAULT_WORKSPACE` ("default") and `NamespaceScope::Owner`.
    pub fn default_owner() -> Self {
        Self {
            workspace_id: DEFAULT_WORKSPACE.to_string(),
            namespace: NamespaceScope::Owner,
        }
    }

    /// Build a `SearchFilter` pre-populated with this context's workspace
    /// and namespace, restricted to valid facts only.
    pub fn to_search_filter(&self) -> SearchFilter {
        SearchFilter::new()
            .with_namespace(self.namespace.clone())
            .with_workspace(WorkspaceFilter::Single(
                self.workspace_id.clone(),
            ))
            .with_valid_only()
    }

    /// Return a reference to the workspace identifier.
    pub fn workspace_id(&self) -> &str {
        &self.workspace_id
    }
}

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

    /// Human-readable display name (defaults to id)
    #[serde(default)]
    pub name: String,

    /// Optional emoji or icon identifier
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,

    /// Whether this workspace is archived (soft-deleted)
    #[serde(default)]
    pub is_archived: bool,

    /// Memory decay rate override (0.0 = no decay, 1.0 = maximum decay)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub decay_rate: Option<f64>,

    /// Fact types that should never decay in this workspace (stored as JSON array of strings)
    #[serde(default)]
    pub permanent_fact_types: Vec<String>,

    /// Default model override (e.g., "claude-sonnet-4-20250514")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_model: Option<String>,

    /// System prompt override for this workspace
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_prompt_override: Option<String>,

    /// Allowlist of tool names available in this workspace (empty = all tools)
    #[serde(default)]
    pub allowed_tools: Vec<String>,
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
            name: id,
            icon: None,
            is_archived: false,
            decay_rate: None,
            permanent_fact_types: Vec::new(),
            default_model: None,
            system_prompt_override: None,
            allowed_tools: Vec::new(),
        }
    }

    /// Get the session key for this workspace
    pub fn session_key(&self, agent_id: &str) -> SessionKey {
        SessionKey::Main {
            agent_id: agent_id.to_string(),
            main_key: format!("ws-{}", self.id),
            epoch: 0,
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
// ActiveWorkspace
// =============================================================================

/// Resolved workspace context for the current execution pipeline.
///
/// This is the central data structure that flows through the entire execution
/// pipeline (Thinker, Memory, Executor). It carries the resolved profile
/// configuration and a memory filter scoped to this workspace.
///
/// Created via `from_manager()` (when a WorkspaceManager is available) or
/// `default_global()` (fallback when no manager exists).
#[derive(Debug, Clone)]
pub struct ActiveWorkspace {
    /// The workspace identifier (e.g., "project-x", "global")
    pub workspace_id: String,

    /// Resolved profile configuration for this workspace
    pub profile: ProfileConfig,

    /// Memory filter scoped to this workspace
    pub memory_filter: WorkspaceFilter,

    /// Filesystem path to the workspace directory (for loading workspace files like SOUL.md)
    pub workspace_path: Option<PathBuf>,
}

impl ActiveWorkspace {
    /// Resolve the active workspace for an agent from the WorkspaceManager.
    ///
    /// Since Agent↔Workspace is 1:1, the agent_id is used directly as workspace_id.
    /// Resolves the workspace's profile binding and builds a memory filter.
    ///
    /// Falls back to a default global workspace if the workspace is not found.
    pub async fn from_manager(
        manager: &WorkspaceManager,
        agent_id: &str,
    ) -> Self {
        let workspace = manager
            .get(agent_id)
            .await
            .ok()
            .flatten()
            .unwrap_or_else(|| {
                debug!("Workspace '{}' not found, falling back to global", agent_id);
                Workspace::new("global", "default")
            });

        let profile = manager
            .get_profile(&workspace.profile)
            .unwrap_or_else(|| {
                debug!(
                    "Profile '{}' not found for workspace '{}', using default",
                    workspace.profile, workspace.id
                );
                ProfileConfig::default()
            });

        let memory_filter =
            WorkspaceFilter::Single(workspace.id.clone());

        let workspace_path = Self::resolve_workspace_path(&workspace.id);

        Self {
            workspace_id: workspace.id,
            profile,
            memory_filter,
            workspace_path,
        }
    }

    /// Build from a specific workspace ID (used by channel→workspace routing).
    ///
    /// Unlike `from_manager()` which reads the user's active workspace,
    /// this directly loads the specified workspace. Falls back to creating
    /// a workspace with the given ID and default profile if not found.
    pub async fn from_workspace_id(manager: &WorkspaceManager, workspace_id: &str) -> Self {
        let workspace = manager
            .get(workspace_id)
            .await
            .ok()
            .flatten()
            .unwrap_or_else(|| Workspace::new(workspace_id, &manager.config.default_profile));

        let profile = manager
            .get_profile(&workspace.profile)
            .unwrap_or_else(|| {
                debug!(
                    "Profile '{}' not found for workspace '{}', using default",
                    workspace.profile, workspace.id
                );
                ProfileConfig::default()
            });

        let memory_filter =
            WorkspaceFilter::Single(workspace.id.clone());

        let workspace_path = Self::resolve_workspace_path(&workspace.id);

        Self {
            workspace_id: workspace.id,
            profile,
            memory_filter,
            workspace_path,
        }
    }

    /// Create a default global workspace when no WorkspaceManager is available.
    ///
    /// Uses "global" as the workspace ID, default profile configuration,
    /// and a memory filter scoped to "global".
    pub fn default_global() -> Self {
        Self {
            workspace_id: "global".to_string(),
            profile: ProfileConfig::default(),
            memory_filter: WorkspaceFilter::Single(
                "global".to_string(),
            ),
            workspace_path: dirs::home_dir().map(|h| h.join(".aleph")),
        }
    }

    /// Resolve the workspace directory path from the workspace ID.
    ///
    /// Convention: `~/.aleph/agents/{workspace_id}`
    fn resolve_workspace_path(workspace_id: &str) -> Option<PathBuf> {
        dirs::home_dir().map(|h| h.join(".aleph/agents").join(workspace_id))
    }
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
                .join(".aleph/data/workspaces.db"),
            default_profile: "default".to_string(),
            archive_after_days: 30,
        }
    }
}

/// Workspace manager with SQLite persistence
pub struct WorkspaceManager {
    pub(super) config: WorkspaceManagerConfig,
    pub(super) conn: Arc<Mutex<Connection>>,
    /// In-memory profile cache (loaded from config)
    pub(super) profiles: Arc<Mutex<HashMap<String, ProfileConfig>>>,
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
                archived INTEGER DEFAULT 0,
                name TEXT NOT NULL DEFAULT '',
                icon TEXT,
                is_archived INTEGER DEFAULT 0,
                decay_rate REAL,
                permanent_fact_types TEXT,
                default_model TEXT,
                system_prompt_override TEXT,
                allowed_tools TEXT
            );

            CREATE TABLE IF NOT EXISTS channel_active_agent (
                channel TEXT NOT NULL,
                peer_id TEXT NOT NULL,
                agent_id TEXT NOT NULL,
                updated_at INTEGER,
                PRIMARY KEY (channel, peer_id)
            );

            CREATE INDEX IF NOT EXISTS idx_workspaces_profile ON workspaces(profile);
            CREATE INDEX IF NOT EXISTS idx_workspaces_last_active ON workspaces(last_active_at);
            "#,
        )
        .map_err(|e| WorkspaceError::Database(format!("Schema init failed: {}", e)))?;

        // Migrate: add columns that may be missing on older databases.
        // SQLite ALTER TABLE ADD COLUMN is a no-op error if column exists,
        // so we simply ignore "duplicate column name" errors.
        let migrations = [
            "ALTER TABLE workspaces ADD COLUMN name TEXT NOT NULL DEFAULT ''",
            "ALTER TABLE workspaces ADD COLUMN icon TEXT",
            "ALTER TABLE workspaces ADD COLUMN is_archived INTEGER DEFAULT 0",
            "ALTER TABLE workspaces ADD COLUMN decay_rate REAL",
            "ALTER TABLE workspaces ADD COLUMN permanent_fact_types TEXT",
            "ALTER TABLE workspaces ADD COLUMN default_model TEXT",
            "ALTER TABLE workspaces ADD COLUMN system_prompt_override TEXT",
            "ALTER TABLE workspaces ADD COLUMN allowed_tools TEXT",
        ];
        for sql in &migrations {
            let _ = conn.execute(sql, []);  // ignore "duplicate column" errors
        }

        // Ensure global workspace exists
        let now = Utc::now().timestamp();
        conn.execute(
            "INSERT OR IGNORE INTO workspaces (id, profile, created_at, last_active_at, description, name)
             VALUES ('global', 'default', ?, ?, 'Default global workspace', 'global')",
            params![now, now],
        )
        .ok();

        Ok(())
    }

    /// Load profiles from config
    pub fn load_profiles(&self, profiles: HashMap<String, ProfileConfig>) {
        let mut cache = self.profiles.lock().unwrap_or_else(|e| e.into_inner());
        *cache = profiles;

        // Ensure default profile exists
        if !cache.contains_key("default") {
            cache.insert("default".to_string(), ProfileConfig::default());
        }
    }

    /// Get a profile by name
    pub fn get_profile(&self, name: &str) -> Option<ProfileConfig> {
        let cache = self.profiles.lock().unwrap_or_else(|e| e.into_inner());
        cache.get(name).cloned()
    }

    /// List all available profiles
    pub fn list_profiles(&self) -> Vec<String> {
        let cache = self.profiles.lock().unwrap_or_else(|e| e.into_inner());
        cache.keys().cloned().collect()
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
            epoch: 0,
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

    #[error("Agent '{agent_id}' is already bound to channel '{channel}'")]
    AgentAlreadyBound { agent_id: String, channel: String },
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
    async fn test_channel_active_agent() {
        let temp = tempdir().unwrap();
        let config = test_config(temp.path().join("test.db"));
        let manager = WorkspaceManager::new(config).unwrap();

        // Set active agent for a channel+peer
        manager.set_active_agent("telegram", "user-123", "my-agent").unwrap();

        // Get active agent
        let agent = manager.get_active_agent("telegram", "user-123").unwrap();
        assert_eq!(agent, Some("my-agent".to_string()));

        // Unknown peer returns None
        let unknown = manager.get_active_agent("telegram", "unknown-user").unwrap();
        assert_eq!(unknown, None);

        // Update active agent (upsert)
        manager.set_active_agent("telegram", "user-123", "new-agent").unwrap();
        let updated = manager.get_active_agent("telegram", "user-123").unwrap();
        assert_eq!(updated, Some("new-agent".to_string()));

        // Different channel returns None
        let other_channel = manager.get_active_agent("discord", "user-123").unwrap();
        assert_eq!(other_channel, None);
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

    #[tokio::test]
    async fn test_active_workspace_from_manager() {
        let temp = tempdir().unwrap();
        let config = test_config(temp.path().join("test.db"));
        let manager = WorkspaceManager::new(config).unwrap();

        // Load profiles with a custom "coding" profile
        let coding_profile = ProfileConfig {
            description: Some("Coding profile".to_string()),
            model: Some("claude-sonnet".to_string()),
            tools: vec!["git_*".to_string()],
            temperature: Some(0.2),
            ..Default::default()
        };
        manager.load_profiles(HashMap::from([
            ("coding".to_string(), coding_profile.clone()),
        ]));

        // Create a workspace bound to the "coding" profile (agent_id = workspace_id in 1:1 model)
        manager.create("project-x", "coding", Some("Test project")).await.unwrap();

        // Resolve active workspace by agent_id — should get "project-x" with "coding" profile
        let active = ActiveWorkspace::from_manager(&manager, "project-x").await;
        assert_eq!(active.workspace_id, "project-x");
        assert_eq!(active.profile.model, Some("claude-sonnet".to_string()));
        assert_eq!(active.profile.temperature, Some(0.2));
        assert_eq!(active.profile.tools, vec!["git_*".to_string()]);

        // Memory filter should be scoped to this workspace
        let filter_sql = active.memory_filter.to_sql_filter();
        assert!(filter_sql.contains("project-x"));
    }

    #[tokio::test]
    async fn test_active_workspace_fallback_to_global() {
        let temp = tempdir().unwrap();
        let config = test_config(temp.path().join("test.db"));
        let manager = WorkspaceManager::new(config).unwrap();

        // Load only default profile (no custom profiles)
        manager.load_profiles(HashMap::new());

        // Non-existent agent_id → should fall back to "global"
        let active = ActiveWorkspace::from_manager(&manager, "nonexistent-agent").await;
        assert_eq!(active.workspace_id, "global");

        // Profile should be the default (since global workspace uses "default" profile)
        assert!(active.profile.model.is_none());
        assert!(active.profile.tools.is_empty());

        // Memory filter should be scoped to "global"
        let filter_sql = active.memory_filter.to_sql_filter();
        assert!(filter_sql.contains("global"));
    }

    #[test]
    fn test_active_workspace_default_global() {
        let active = ActiveWorkspace::default_global();
        assert_eq!(active.workspace_id, "global");
        assert!(active.profile.model.is_none());
        assert!(active.profile.tools.is_empty());

        let filter_sql = active.memory_filter.to_sql_filter();
        assert_eq!(filter_sql, "agent = 'global'");
    }

    // -----------------------------------------------------------------------
    // WorkspaceFilter unit tests (moved from memory::workspace)
    // -----------------------------------------------------------------------

    #[test]
    fn test_workspace_filter_sql() {
        let f = WorkspaceFilter::Single("work".to_string());
        assert_eq!(f.to_sql_filter(), "agent = 'work'");

        let f = WorkspaceFilter::Multiple(vec!["a".to_string(), "b".to_string()]);
        assert_eq!(f.to_sql_filter(), "agent IN ('a', 'b')");

        let f = WorkspaceFilter::Multiple(vec![]);
        assert_eq!(f.to_sql_filter(), "1=0");

        let f = WorkspaceFilter::All;
        assert_eq!(f.to_sql_filter(), "1=1");
    }

    #[test]
    fn test_workspace_filter_sql_injection_escape() {
        let f = WorkspaceFilter::Single("it's".to_string());
        assert_eq!(f.to_sql_filter(), "agent = 'it''s'");

        let f = WorkspaceFilter::Multiple(vec!["o'reilly".to_string()]);
        assert_eq!(f.to_sql_filter(), "agent IN ('o''reilly')");
    }

    // -----------------------------------------------------------------------
    // WorkspaceContext unit tests (moved from memory::workspace)
    // -----------------------------------------------------------------------

    #[test]
    fn test_workspace_context_default_owner() {
        let ctx = WorkspaceContext::default_owner();
        assert_eq!(ctx.workspace_id(), "main");
        assert!(matches!(ctx.namespace, NamespaceScope::Owner));
    }

    #[test]
    fn test_workspace_context_custom() {
        let ctx = WorkspaceContext::new("crypto", NamespaceScope::Owner);
        assert_eq!(ctx.workspace_id(), "crypto");
    }

    #[test]
    fn test_workspace_context_to_search_filter() {
        let ctx = WorkspaceContext::new("crypto", NamespaceScope::Owner);
        let filter = ctx.to_search_filter();
        let sql = filter.to_lance_filter().unwrap();
        assert!(sql.contains("agent = 'crypto'"));
        assert!(sql.contains("is_valid = true"));
        assert!(sql.contains("namespace = 'owner'"));
    }
}
