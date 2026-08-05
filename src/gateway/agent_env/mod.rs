//! Agent Environment Store
//!
//! Each agent has an environment (`AgentEnv`) that provides context isolation
//! and profile-based configuration. An `AgentEnv` is a runtime instance of a
//! profile, with its own session, cache state, and memory scope.
//!
//! # Architecture
//!
//! ```text
//! Profile (Static Template)     AgentEnv (Runtime Instance)
//! ├── model binding        →    ├── session_key
//! ├── tool whitelist       →    ├── cache_state
//! ├── system_prompt        →    └── env_vars
//! └── temperature
//! ```
//!
//! # Example
//!
//! ```rust,ignore
//! let store = AgentEnvStore::new(config)?;
//!
//! // Create an agent environment from a profile
//! let env = store.create("project-x", "coding", None).await?;
//!
//! // Bind agent to channel (1:1)
//! store.set_active_agent("telegram", "project-x")?;
//!
//! // Get active agent for a channel
//! let agent_id = store.get_active_agent("telegram")?;
//! ```

use crate::sync_primitives::{Arc, Mutex};
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use thiserror::Error;
use tracing::{debug, info};

mod manager_ops;

use crate::config::ProfileConfig;
use crate::memory::namespace::NamespaceScope;
use crate::memory::store::types::SearchFilter;
use crate::routing::SessionKey;

/// Default agent identifier
pub const DEFAULT_AGENT: &str = "main";

// =============================================================================
// AgentEnvFilter
// =============================================================================

/// Filter for selecting agents when querying memory facts.
#[derive(Debug, Clone)]
pub enum AgentEnvFilter {
    /// Filter to a single agent by id
    Single(String),
    /// Filter to multiple agents by id
    Multiple(Vec<String>),
    /// No filtering — include all agents
    All,
}

impl AgentEnvFilter {
    /// Convert the filter to a SQL WHERE clause fragment.
    ///
    /// Returns a string suitable for use in a SQL `WHERE` clause that filters
    /// on the `agent` column.
    #[must_use]
    pub fn to_sql_filter(&self) -> String {
        match self {
            Self::Single(id) => {
                format!("agent = '{}'", id.replace('\'', "''"))
            }
            Self::Multiple(ids) => {
                if ids.is_empty() {
                    return "1=0".to_string(); // match nothing
                }
                let escaped: Vec<String> = ids
                    .iter()
                    .map(|id| format!("'{}'", id.replace('\'', "''")))
                    .collect();
                format!("agent IN ({})", escaped.join(", "))
            }
            Self::All => "1=1".to_string(),
        }
    }
}

// =============================================================================
// AgentEnvContext
// =============================================================================

/// Runtime context for the active agent environment.
///
/// Created from a Session, flows through the Agent Loop to all memory
/// operations.  Carries the agent identifier and namespace scope so
/// that every store call is automatically scoped.
#[derive(Debug, Clone)]
pub struct AgentEnvContext {
    /// Active agent identifier.
    pub agent_id: String,
    /// Namespace scope for access control.
    pub namespace: NamespaceScope,
}

impl AgentEnvContext {
    /// Create a new agent environment context.
    pub fn new(agent_id: impl Into<String>, namespace: NamespaceScope) -> Self {
        Self {
            agent_id: agent_id.into(),
            namespace,
        }
    }

    /// Convenience constructor for the default owner context.
    ///
    /// Uses `DEFAULT_AGENT` ("main") and `NamespaceScope::Owner`.
    #[must_use]
    pub fn default_owner() -> Self {
        Self {
            agent_id: DEFAULT_AGENT.to_string(),
            namespace: NamespaceScope::Owner,
        }
    }

    /// Build a `SearchFilter` pre-populated with this agent context
    /// and namespace, restricted to valid facts only.
    #[must_use]
    pub fn to_search_filter(&self) -> SearchFilter {
        SearchFilter::new()
            .with_namespace(self.namespace.clone())
            .with_agent_filter(AgentEnvFilter::Single(self.agent_id.clone()))
            .with_valid_only()
    }

    /// Return a reference to the agent identifier.
    #[must_use]
    pub fn agent_id(&self) -> &str {
        &self.agent_id
    }
}

// =============================================================================
// AgentEnv
// =============================================================================

/// Agent environment — per-agent runtime state derived from a profile
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentEnv {
    /// Unique agent environment identifier (e.g., "project-aleph")
    pub id: String,

    /// Profile this agent environment inherits from (e.g., "coding")
    pub profile: String,

    /// When this agent environment was created
    pub created_at: DateTime<Utc>,

    /// Last activity timestamp
    pub last_active_at: DateTime<Utc>,

    /// Provider-side context caching state
    pub cache_state: CacheState,

    /// Environment variables specific to this agent
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

    /// Whether this agent environment is archived (soft-deleted)
    #[serde(default)]
    pub is_archived: bool,

    /// Memory decay rate override (0.0 = no decay, 1.0 = maximum decay)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub decay_rate: Option<f64>,

    /// Fact types that should never decay for this agent (stored as JSON array of strings)
    #[serde(default)]
    pub permanent_fact_types: Vec<String>,

    /// Default model override (e.g., "claude-sonnet-4-20250514")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_model: Option<String>,

    /// System prompt override
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_prompt_override: Option<String>,

    /// Allowlist of tool names available (empty = all tools)
    #[serde(default)]
    pub allowed_tools: Vec<String>,
}

impl AgentEnv {
    /// Create a new agent environment
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

    /// Get the session key for this agent environment
    #[must_use]
    pub fn session_key(&self, agent_id: &str) -> SessionKey {
        SessionKey::Main {
            agent_id: agent_id.to_string(),
            main_key: format!("agent-env-{}", self.id),
            epoch: 0,
        }
    }

    /// Get the storage key string
    #[must_use]
    pub fn storage_key(&self, agent_id: &str) -> String {
        self.session_key(agent_id).to_key_string()
    }

    /// Check if this is the global/default agent
    #[must_use]
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
/// - Anthropic: Ephemeral (`cache_control` blocks, no persistent state)
/// - Gemini: Persistent (explicit cache name, stored on provider side)
/// - `OpenAI`: Transparent (automatic, no state needed)
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CacheState {
    /// No caching active
    #[default]
    None,

    /// Anthropic-style ephemeral caching
    /// Only need to track where to insert `cache_control` markers
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
    #[must_use]
    pub fn is_active(&self) -> bool {
        match self {
            Self::None => false,
            Self::Ephemeral {
                cache_breakpoint_index,
                ..
            } => cache_breakpoint_index.is_some(),
            Self::Persistent { expires_at, .. } => *expires_at > Utc::now(),
            Self::Transparent => true,
        }
    }

    /// Get the cached token count
    #[must_use]
    pub const fn tokens_cached(&self) -> Option<u64> {
        match self {
            Self::None => None,
            Self::Ephemeral { tokens_cached, .. } => *tokens_cached,
            Self::Persistent { tokens_cached, .. } => Some(*tokens_cached),
            Self::Transparent => None,
        }
    }
}

// =============================================================================
// ActiveAgentEnv
// =============================================================================

/// Resolved agent environment for the current execution pipeline.
///
/// This is the central data structure that flows through the entire execution
/// pipeline (Thinker, Memory, Executor). It carries the resolved profile
/// configuration and a memory filter scoped to this agent.
///
/// Created via `from_manager()` (when a `AgentEnvStore` is available) or
/// `default_global()` (fallback when no manager exists).
#[derive(Debug, Clone)]
pub struct ActiveAgentEnv {
    /// The agent identifier (e.g., "project-x", "global")
    pub agent_id: String,

    /// Resolved profile configuration
    pub profile: ProfileConfig,

    /// Memory filter scoped to this agent
    pub memory_filter: AgentEnvFilter,

    /// Filesystem path to the agent environment directory (for loading agent files like SOUL.md)
    pub agent_env_path: Option<PathBuf>,
}

impl ActiveAgentEnv {
    /// Resolve the active agent environment from an agent from the `AgentEnvStore`.
    ///
    /// Since Agent↔AgentEnv is 1:1, the `agent_id` is used directly as `agent_id`.
    /// Resolves the profile binding and builds a memory filter.
    ///
    /// Falls back to a default global agent environment if not found.
    pub async fn from_manager(manager: &AgentEnvStore, agent_id: &str) -> Self {
        let env = manager
            .get(agent_id)
            .await
            .ok()
            .flatten()
            .unwrap_or_else(|| {
                debug!("AgentEnv '{}' not found, falling back to global", agent_id);
                AgentEnv::new("global", "default")
            });

        let profile = manager.get_profile(&env.profile).unwrap_or_else(|| {
            debug!(
                "Profile '{}' not found for agent '{}', using default",
                env.profile, env.id
            );
            ProfileConfig::default()
        });

        let memory_filter = AgentEnvFilter::Single(env.id.clone());

        let agent_env_path = Self::resolve_agent_env_path(&env.id);

        Self {
            agent_id: env.id,
            profile,
            memory_filter,
            agent_env_path,
        }
    }

    /// Build from a specific agent ID (used by channel→agent routing).
    ///
    /// Unlike `from_manager()` which reads the active agent environment,
    /// this directly loads the specified agent environment. Falls back to creating
    /// an agent environment with the given ID and default profile if not found.
    pub async fn from_agent_id(manager: &AgentEnvStore, agent_id: &str) -> Self {
        let env = manager
            .get(agent_id)
            .await
            .ok()
            .flatten()
            .unwrap_or_else(|| AgentEnv::new(agent_id, &manager.config.default_profile));

        let profile = manager.get_profile(&env.profile).unwrap_or_else(|| {
            debug!(
                "Profile '{}' not found for agent '{}', using default",
                env.profile, env.id
            );
            ProfileConfig::default()
        });

        let memory_filter = AgentEnvFilter::Single(env.id.clone());

        let agent_env_path = Self::resolve_agent_env_path(&env.id);

        Self {
            agent_id: env.id,
            profile,
            memory_filter,
            agent_env_path,
        }
    }

    /// Create a default global agent environment when no `AgentEnvStore` is available.
    ///
    /// Uses "global" as the agent ID, default profile configuration,
    /// and a memory filter scoped to "global".
    #[must_use]
    pub fn default_global() -> Self {
        Self {
            agent_id: "global".to_string(),
            profile: ProfileConfig::default(),
            memory_filter: AgentEnvFilter::Single("global".to_string()),
            agent_env_path: crate::utils::paths::get_config_dir().ok(),
        }
    }

    /// Resolve the agent environment directory path from the agent ID.
    ///
    /// Convention: `<config_dir>/agents/{agent_id}`.
    ///
    /// ⚠️ This is the SAME directory as `discovery::aleph_agents_dir()`,
    /// `SelfConfigTool`, and `agent_resolver::default_agents_root` — it must
    /// therefore be the same resolution. It used to hand-roll
    /// `dirs::home_dir().join(".aleph/agents")`, making it a fourth
    /// independent answer for one directory, and independent answers only
    /// diverge where nobody is looking (a relocated `ALEPH_HOME`).
    fn resolve_agent_env_path(agent_id: &str) -> Option<PathBuf> {
        crate::utils::paths::get_config_dir()
            .ok()
            .map(|h| h.join("agents").join(agent_id))
    }
}

// =============================================================================
// AgentEnvStore
// =============================================================================

/// Configuration for `AgentEnvStore`
#[derive(Debug, Clone)]
pub struct AgentEnvStoreConfig {
    /// Database path for agent environment storage
    pub db_path: PathBuf,
    /// Default profile for new agent environments
    pub default_profile: String,
    /// Auto-archive agent environments after N days of inactivity (0 = never)
    pub archive_after_days: u32,
}

impl Default for AgentEnvStoreConfig {
    fn default() -> Self {
        Self {
            db_path: dirs::home_dir()
                .unwrap_or_else(|| PathBuf::from("/tmp"))
                .join(".aleph/data/agent_envs.db"),
            default_profile: "default".to_string(),
            archive_after_days: 30,
        }
    }
}

/// `AgentEnv` manager with `SQLite` persistence
pub struct AgentEnvStore {
    pub(super) config: AgentEnvStoreConfig,
    pub(super) conn: Arc<Mutex<Connection>>,
    /// In-memory profile cache (loaded from config)
    pub(super) profiles: Arc<Mutex<HashMap<String, ProfileConfig>>>,
}

impl AgentEnvStore {
    /// Create a new agent environment store
    pub fn new(config: AgentEnvStoreConfig) -> Result<Self, AgentEnvError> {
        // Ensure parent directory exists
        if let Some(parent) = config.db_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                AgentEnvError::Database(format!("Failed to create db directory: {e}"))
            })?;
        }

        let conn = crate::utils::sqlite_open::open_sqlite_safe(&config.db_path)
            .map_err(|e| AgentEnvError::Database(format!("Failed to open database: {e}")))?;

        Self::init_schema(&conn)?;

        info!("AgentEnvStore initialized: {:?}", config.db_path);

        Ok(Self {
            config,
            conn: Arc::new(Mutex::new(conn)),
            profiles: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    /// Create with default configuration
    pub fn with_defaults() -> Result<Self, AgentEnvError> {
        Self::new(AgentEnvStoreConfig::default())
    }

    /// Initialize database schema
    fn init_schema(conn: &Connection) -> Result<(), AgentEnvError> {
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS agent_envs (
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
                channel TEXT NOT NULL PRIMARY KEY,
                agent_id TEXT NOT NULL,
                updated_at INTEGER
            );

            CREATE INDEX IF NOT EXISTS idx_agent_envs_profile ON agent_envs(profile);
            CREATE INDEX IF NOT EXISTS idx_agent_envs_last_active ON agent_envs(last_active_at);
            "#,
        )
        .map_err(|e| AgentEnvError::Database(format!("Schema init failed: {e}")))?;

        // Migrate: add columns that may be missing on older databases.
        // SQLite ALTER TABLE ADD COLUMN is a no-op error if column exists,
        // so we simply ignore "duplicate column name" errors.
        let migrations = [
            "ALTER TABLE agent_envs ADD COLUMN name TEXT NOT NULL DEFAULT ''",
            "ALTER TABLE agent_envs ADD COLUMN icon TEXT",
            "ALTER TABLE agent_envs ADD COLUMN is_archived INTEGER DEFAULT 0",
            "ALTER TABLE agent_envs ADD COLUMN decay_rate REAL",
            "ALTER TABLE agent_envs ADD COLUMN permanent_fact_types TEXT",
            "ALTER TABLE agent_envs ADD COLUMN default_model TEXT",
            "ALTER TABLE agent_envs ADD COLUMN system_prompt_override TEXT",
            "ALTER TABLE agent_envs ADD COLUMN allowed_tools TEXT",
        ];
        for sql in &migrations {
            let _ = conn.execute(sql, []); // ignore "duplicate column" errors
        }

        // Ensure global agent environment exists
        let now = Utc::now().timestamp();
        conn.execute(
            "INSERT OR IGNORE INTO agent_envs (id, profile, created_at, last_active_at, description, name)
             VALUES ('global', 'default', ?, ?, 'Default global agent environment', 'global')",
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
    #[must_use]
    pub fn get_profile(&self, name: &str) -> Option<ProfileConfig> {
        let cache = self.profiles.lock().unwrap_or_else(|e| e.into_inner());
        cache.get(name).cloned()
    }

    /// List all available profiles
    #[must_use]
    pub fn list_profiles(&self) -> Vec<String> {
        let cache = self.profiles.lock().unwrap_or_else(|e| e.into_inner());
        cache.keys().cloned().collect()
    }
}

// =============================================================================
// SessionKey Extension
// =============================================================================

impl SessionKey {
    /// Create a session key for an agent environment
    pub fn agent_env(agent_id: impl Into<String>, env_id: impl Into<String>) -> Self {
        Self::Main {
            agent_id: agent_id.into(),
            main_key: format!("agent-env-{}", env_id.into()),
            epoch: 0,
        }
    }

    /// Check if this is an agent-env session key
    #[must_use]
    pub fn is_agent_env(&self) -> bool {
        match self {
            Self::Main { main_key, .. } => main_key.starts_with("agent-env-"),
            _ => false,
        }
    }

    /// Get the env ID if this is an agent-env session key
    #[must_use]
    pub fn env_id(&self) -> Option<&str> {
        match self {
            Self::Main { main_key, .. } if main_key.starts_with("agent-env-") => {
                Some(&main_key[10..])
            }
            _ => None,
        }
    }
}

// =============================================================================
// Errors
// =============================================================================

#[derive(Debug, Error)]
pub enum AgentEnvError {
    #[error("Database error: {0}")]
    Database(String),

    #[error("Agent environment not found: {0}")]
    NotFound(String),

    #[error("Agent environment already exists: {0}")]
    AlreadyExists(String),

    #[error("Profile not found: {0}")]
    ProfileNotFound(String),

    #[error("Cannot modify global agent")]
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

    fn test_config(path: PathBuf) -> AgentEnvStoreConfig {
        AgentEnvStoreConfig {
            db_path: path,
            default_profile: "default".to_string(),
            archive_after_days: 30,
        }
    }

    #[tokio::test]
    async fn test_agent_env_creation() {
        let temp = tempdir().unwrap();
        let config = test_config(temp.path().join("test.db"));
        let manager = AgentEnvStore::new(config).unwrap();

        // Load default profile
        let mut profiles = HashMap::new();
        profiles.insert(
            "coding".to_string(),
            ProfileConfig {
                description: Some("Coding profile".to_string()),
                model: Some("claude-sonnet".to_string()),
                tools: vec!["git_*".to_string()],
                ..Default::default()
            },
        );
        manager.load_profiles(profiles);

        let ws = manager
            .create("project-x", "coding", Some("My project"))
            .await
            .unwrap();

        assert_eq!(ws.id, "project-x");
        assert_eq!(ws.profile, "coding");
        assert_eq!(ws.description, Some("My project".to_string()));
    }

    #[tokio::test]
    async fn test_agent_env_session_key() {
        let ws = AgentEnv::new("project-x", "coding");
        let key = ws.session_key("main");

        assert_eq!(key.to_key_string(), "agent:main:agent-env-project-x");
        assert!(key.is_agent_env());
        assert_eq!(key.env_id(), Some("project-x"));
    }

    #[tokio::test]
    async fn test_global_agent_env_exists() {
        let temp = tempdir().unwrap();
        let config = test_config(temp.path().join("test.db"));
        let manager = AgentEnvStore::new(config).unwrap();

        let global = manager.get("global").await.unwrap();
        assert!(global.is_some());
        assert!(global.unwrap().is_global());
    }

    #[tokio::test]
    async fn test_channel_active_agent() {
        let temp = tempdir().unwrap();
        let config = test_config(temp.path().join("test.db"));
        let manager = AgentEnvStore::new(config).unwrap();

        // Bind agent to channel
        manager.set_active_agent("telegram", "my-agent").unwrap();

        // Get active agent
        let agent = manager.get_active_agent("telegram").unwrap();
        assert_eq!(agent, Some("my-agent".to_string()));

        // Update binding (rebind to different agent)
        manager.set_active_agent("telegram", "new-agent").unwrap();
        let updated = manager.get_active_agent("telegram").unwrap();
        assert_eq!(updated, Some("new-agent".to_string()));

        // Different channel returns None
        let other_channel = manager.get_active_agent("discord").unwrap();
        assert_eq!(other_channel, None);

        // Clear binding
        manager.clear_active_agent("telegram").unwrap();
        let cleared = manager.get_active_agent("telegram").unwrap();
        assert_eq!(cleared, None);
    }

    #[tokio::test]
    async fn test_cache_state_update() {
        let temp = tempdir().unwrap();
        let config = test_config(temp.path().join("test.db"));
        let manager = AgentEnvStore::new(config).unwrap();

        manager.load_profiles(HashMap::from([(
            "coding".to_string(),
            ProfileConfig::default(),
        )]));

        manager.create("test-ws", "coding", None).await.unwrap();

        let cache_state = CacheState::Persistent {
            cache_name: "cache-123".to_string(),
            content_hash: "abc".to_string(),
            expires_at: Utc::now() + chrono::Duration::hours(1),
            tokens_cached: 50000,
        };

        manager
            .update_cache_state("test-ws", &cache_state)
            .await
            .unwrap();

        let ws = manager.get("test-ws").await.unwrap().unwrap();
        assert!(matches!(ws.cache_state, CacheState::Persistent { .. }));
    }

    #[test]
    fn test_session_key_agent_env_methods() {
        let key = SessionKey::agent_env("main", "coding");
        assert_eq!(key.to_key_string(), "agent:main:agent-env-coding");
        assert!(key.is_agent_env());
        assert_eq!(key.env_id(), Some("coding"));

        let main_key = SessionKey::main("main");
        assert!(!main_key.is_agent_env());
        assert_eq!(main_key.env_id(), None);
    }

    #[tokio::test]
    async fn test_active_agent_env_from_store() {
        let temp = tempdir().unwrap();
        let config = test_config(temp.path().join("test.db"));
        let manager = AgentEnvStore::new(config).unwrap();

        // Load profiles with a custom "coding" profile
        let coding_profile = ProfileConfig {
            description: Some("Coding profile".to_string()),
            model: Some("claude-sonnet".to_string()),
            tools: vec!["git_*".to_string()],
            temperature: Some(0.2),
            ..Default::default()
        };
        manager.load_profiles(HashMap::from([(
            "coding".to_string(),
            coding_profile.clone(),
        )]));

        // Create an agent environment bound to the "coding" profile (agent_id = agent_id in 1:1 model)
        manager
            .create("project-x", "coding", Some("Test project"))
            .await
            .unwrap();

        // Resolve active agent environment by agent_id — should get "project-x" with "coding" profile
        let active = ActiveAgentEnv::from_manager(&manager, "project-x").await;
        assert_eq!(active.agent_id, "project-x");
        assert_eq!(active.profile.model, Some("claude-sonnet".to_string()));
        assert_eq!(active.profile.temperature, Some(0.2));
        assert_eq!(active.profile.tools, vec!["git_*".to_string()]);

        // Memory filter should be scoped to this agent
        let filter_sql = active.memory_filter.to_sql_filter();
        assert!(filter_sql.contains("project-x"));
    }

    #[tokio::test]
    async fn test_active_agent_env_fallback_to_global() {
        let temp = tempdir().unwrap();
        let config = test_config(temp.path().join("test.db"));
        let manager = AgentEnvStore::new(config).unwrap();

        // Load only default profile (no custom profiles)
        manager.load_profiles(HashMap::new());

        // Non-existent agent_id → should fall back to "global"
        let active = ActiveAgentEnv::from_manager(&manager, "nonexistent-agent").await;
        assert_eq!(active.agent_id, "global");

        // Profile should be the default (since global agent environment uses "default" profile)
        assert!(active.profile.model.is_none());
        assert!(active.profile.tools.is_empty());

        // Memory filter should be scoped to "global"
        let filter_sql = active.memory_filter.to_sql_filter();
        assert!(filter_sql.contains("global"));
    }

    #[test]
    fn test_active_agent_env_default_global() {
        let active = ActiveAgentEnv::default_global();
        assert_eq!(active.agent_id, "global");
        assert!(active.profile.model.is_none());
        assert!(active.profile.tools.is_empty());

        let filter_sql = active.memory_filter.to_sql_filter();
        assert_eq!(filter_sql, "agent = 'global'");
    }

    // -----------------------------------------------------------------------
    // AgentEnvFilter unit tests (moved from memory module)
    // -----------------------------------------------------------------------

    #[test]
    fn test_agent_env_filter_sql() {
        let f = AgentEnvFilter::Single("work".to_string());
        assert_eq!(f.to_sql_filter(), "agent = 'work'");

        let f = AgentEnvFilter::Multiple(vec!["a".to_string(), "b".to_string()]);
        assert_eq!(f.to_sql_filter(), "agent IN ('a', 'b')");

        let f = AgentEnvFilter::Multiple(vec![]);
        assert_eq!(f.to_sql_filter(), "1=0");

        let f = AgentEnvFilter::All;
        assert_eq!(f.to_sql_filter(), "1=1");
    }

    #[test]
    fn test_agent_env_filter_sql_injection_escape() {
        let f = AgentEnvFilter::Single("it's".to_string());
        assert_eq!(f.to_sql_filter(), "agent = 'it''s'");

        let f = AgentEnvFilter::Multiple(vec!["o'reilly".to_string()]);
        assert_eq!(f.to_sql_filter(), "agent IN ('o''reilly')");
    }

    // -----------------------------------------------------------------------
    // AgentEnvContext unit tests (moved from memory module)
    // -----------------------------------------------------------------------

    #[test]
    fn test_agent_env_context_default_owner() {
        let ctx = AgentEnvContext::default_owner();
        assert_eq!(ctx.agent_id(), "main");
        assert!(matches!(ctx.namespace, NamespaceScope::Owner));
    }

    #[test]
    fn test_agent_env_context_custom() {
        let ctx = AgentEnvContext::new("crypto", NamespaceScope::Owner);
        assert_eq!(ctx.agent_id(), "crypto");
    }

    #[test]
    fn test_agent_env_context_to_search_filter() {
        let ctx = AgentEnvContext::new("crypto", NamespaceScope::Owner);
        let filter = ctx.to_search_filter();
        let sql = filter.to_lance_filter().unwrap();
        assert!(sql.contains("agent = 'crypto'"));
        assert!(sql.contains("is_valid = true"));
        assert!(sql.contains("namespace = 'owner'"));
    }
}
