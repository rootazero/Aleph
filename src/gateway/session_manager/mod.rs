//! Session Manager
//!
//! Manages sessions with SQLite persistence, automatic compaction,
//! and lifecycle management.

pub(crate) mod ops;
#[cfg(test)]
mod tests;

use crate::sync_primitives::{Arc, Mutex};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use tracing::info;

use super::router::SessionKey;
use aleph_protocol::{GuestScope, IdentityContext, Role};

/// A message matched by FTS5 cross-session search.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSearchResult {
    pub message_id: i64,
    pub session_key: String,
    pub role: String,
    pub content: String,
    pub timestamp: i64,
    pub agent_id: String,
    pub topic: Option<String>,
}

/// Session lifecycle state.
///
/// Represents the explicit state machine for session lifecycle management.
/// This enables better session tracking, debugging, and automated cleanup.
///
/// # State Diagram
///
/// ```text
///  Created → Active ↔ Idle ↔ Running → Error → Stopped
///                 ↑                   ↓
///                 └───────────────────┘
/// ```
///
/// - **Created**: Session initialized but not yet used
/// - **Active**: Session is actively processing requests
/// - **Idle**: Session has messages but no recent activity
/// - **Running**: Agent loop is currently executing
/// - **Error**: Session encountered an error (recoverable)
/// - **Stopped**: Session is permanently closed
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[derive(Default)]
pub enum SessionState {
    /// Session initialized but not yet used
    #[default]
    Created,
    /// Session is actively processing requests
    Active,
    /// Session has messages but no recent activity
    Idle,
    /// Agent loop is currently executing
    Running,
    /// Session encountered an error (recoverable)
    Error,
    /// Session is permanently closed
    Stopped,
}

impl std::fmt::Display for SessionState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SessionState::Created => write!(f, "created"),
            SessionState::Active => write!(f, "active"),
            SessionState::Idle => write!(f, "idle"),
            SessionState::Running => write!(f, "running"),
            SessionState::Error => write!(f, "error"),
            SessionState::Stopped => write!(f, "stopped"),
        }
    }
}

/// Session metadata
pub use crate::gateway::session_store::types::SessionMetadata;

/// Patch request for updating session metadata
pub use crate::gateway::session_store::types::SessionPatch;

/// Session identity metadata stored in database
///
/// This structure is serialized to JSON and stored in the `sessions.metadata` column.
/// It contains the frozen identity and permission snapshot for the session.
///
/// # Backward Compatibility
///
/// Old sessions with `metadata=NULL` or unparseable JSON will use `Default::default()`
/// which creates an Owner session. This ensures backward compatibility.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionIdentityMeta {
    /// Role of the session owner
    pub role: Role,

    /// Identity ID ("owner" or guest_id)
    pub identity_id: String,

    /// Guest scope (frozen at session creation)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope: Option<GuestScope>,

    /// Source channel
    pub source_channel: String,

    /// Custom metadata (preserved from old format)
    #[serde(flatten)]
    pub custom: HashMap<String, serde_json::Value>,
}

impl Default for SessionIdentityMeta {
    fn default() -> Self {
        Self {
            role: Role::Owner,
            identity_id: "owner".to_string(),
            scope: None,
            source_channel: "unknown".to_string(),
            custom: HashMap::new(),
        }
    }
}

impl SessionIdentityMeta {
    /// Create identity metadata for an Owner session
    pub fn owner(source_channel: impl Into<String>) -> Self {
        Self {
            role: Role::Owner,
            identity_id: "owner".to_string(),
            scope: None,
            source_channel: source_channel.into(),
            custom: HashMap::new(),
        }
    }

    /// Create identity metadata for a Guest session
    pub fn guest(
        guest_id: impl Into<String>,
        scope: GuestScope,
        source_channel: impl Into<String>,
    ) -> Self {
        Self {
            role: Role::Guest,
            identity_id: guest_id.into(),
            scope: Some(scope),
            source_channel: source_channel.into(),
            custom: HashMap::new(),
        }
    }

    /// Parse from JSON string (with fallback to default)
    pub fn from_json_str(json: Option<&str>) -> Self {
        json.and_then(|s| serde_json::from_str(s).ok())
            .unwrap_or_default()
    }

    /// Serialize to JSON string
    pub fn to_json_string(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }

    /// Convert to IdentityContext for a specific request
    pub fn to_identity_context(&self, session_key: String) -> IdentityContext {
        match self.role {
            Role::Owner => IdentityContext::owner(session_key, self.source_channel.clone()),
            Role::Guest => {
                let scope = self.scope.clone().unwrap_or_else(|| GuestScope {
                    allowed_tools: vec![],
                    expires_at: None,
                    display_name: None,
                });
                IdentityContext::guest(
                    session_key,
                    self.identity_id.clone(),
                    scope,
                    self.source_channel.clone(),
                )
            }
            Role::Anonymous => IdentityContext::anonymous(session_key, self.source_channel.clone()),
        }
    }
}

/// Session manager configuration
#[derive(Debug, Clone)]
pub struct SessionManagerConfig {
    /// Database path
    pub db_path: PathBuf,
    /// Maximum messages per session before compaction
    pub max_messages: usize,
    /// Messages to keep after compaction
    pub compaction_keep: usize,
    /// Auto-reset time (hour of day, 0-23)
    pub auto_reset_hour: Option<u8>,
    /// Session expiry in seconds (0 = never)
    pub session_expiry_secs: u64,
}

impl Default for SessionManagerConfig {
    fn default() -> Self {
        Self {
            db_path: crate::utils::paths::get_sessions_db_path()
                .unwrap_or_else(|_| PathBuf::from("/tmp/aleph_sessions.db")),
            max_messages: 100,
            compaction_keep: 50,
            auto_reset_hour: Some(4),               // 4 AM
            session_expiry_secs: 30 * 24 * 60 * 60, // 30 days
        }
    }
}

/// Session manager with SQLite persistence
///
/// Uses `std::sync::Mutex` for the connection because `rusqlite::Connection`
/// is not `Sync` (it uses `RefCell` internally). This is safe for async use
/// as long as we don't hold the lock across await points.
#[derive(Clone)]
pub struct SessionManager {
    pub(super) config: SessionManagerConfig,
    pub(super) conn: Arc<Mutex<Connection>>,
    /// Optional writer for Spec 1 memory capture hooks.
    pub(super) raw_memory_writer: Option<Arc<dyn crate::memory::store::raw_memory::RawMemoryStore>>,
    /// Optional event bus for publishing session lifecycle events.
    pub(super) event_bus: Option<Arc<super::event_bus::GatewayEventBus>>,
    /// Phase 1 dual-write shim: mirror each `add_message` into the
    /// append-only `session_events` log. Removed in Phase 6 when Gateway
    /// RPC also migrates onto `SessionService`. `None` until wired in
    /// startup, which keeps test fixtures and callers that don't care
    /// about the new log unaffected.
    pub(super) session_service:
        Option<Arc<dyn crate::session::service::SessionService>>,
}

impl SessionManager {
    /// Create a new session manager
    pub fn new(config: SessionManagerConfig) -> Result<Self, SessionManagerError> {
        // Ensure parent directory exists
        if let Some(parent) = config.db_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                SessionManagerError::DatabaseError(format!("Failed to create db directory: {}", e))
            })?;
        }

        let conn = Connection::open(&config.db_path).map_err(|e| {
            SessionManagerError::DatabaseError(format!("Failed to open database: {}", e))
        })?;

        // Initialize schema
        Self::init_schema(&conn)?;

        info!("Session manager initialized: {:?}", config.db_path);

        Ok(Self {
            config,
            conn: Arc::new(Mutex::new(conn)),
            raw_memory_writer: None,
            event_bus: None,
            session_service: None,
        })
    }

    /// Attach a raw-memory writer for Spec 1 memory capture hooks.
    pub fn with_raw_memory_writer(
        mut self,
        writer: Arc<dyn crate::memory::store::raw_memory::RawMemoryStore>,
    ) -> Self {
        self.raw_memory_writer = Some(writer);
        self
    }

    /// Attach an event bus for publishing session lifecycle events.
    pub fn with_event_bus(mut self, bus: Arc<super::event_bus::GatewayEventBus>) -> Self {
        self.event_bus = Some(bus);
        self
    }

    /// Attach a `SessionService` to dual-write every `add_message` into the
    /// append-only `session_events` log. Phase 1 migration scaffolding;
    /// removed in Phase 6 when Gateway RPC also migrates.
    pub fn with_session_service(
        mut self,
        svc: Arc<dyn crate::session::service::SessionService>,
    ) -> Self {
        self.session_service = Some(svc);
        self
    }

    /// Create with default configuration
    pub fn with_defaults() -> Result<Self, SessionManagerError> {
        Self::new(SessionManagerConfig::default())
    }

    /// Initialize database schema
    fn init_schema(conn: &Connection) -> Result<(), SessionManagerError> {
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS sessions (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                key TEXT UNIQUE NOT NULL,
                agent_id TEXT NOT NULL,
                session_type TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                last_active_at INTEGER NOT NULL,
                message_count INTEGER DEFAULT 0,
                total_tokens INTEGER DEFAULT 0,
                auto_reset_at INTEGER,
                state TEXT DEFAULT 'created',
                metadata TEXT,
                label TEXT,
                input_tokens INTEGER DEFAULT 0,
                output_tokens INTEGER DEFAULT 0,
                model TEXT,
                model_provider TEXT,
                parent_session_key TEXT,
                compaction_count INTEGER DEFAULT 0
            );

            CREATE TABLE IF NOT EXISTS messages (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                session_key TEXT NOT NULL,
                role TEXT NOT NULL,
                content TEXT NOT NULL,
                timestamp INTEGER NOT NULL,
                metadata TEXT,
                input_tokens INTEGER DEFAULT 0,
                output_tokens INTEGER DEFAULT 0,
                FOREIGN KEY (session_key) REFERENCES sessions(key) ON DELETE CASCADE
            );

            CREATE INDEX IF NOT EXISTS idx_messages_session ON messages(session_key);
            CREATE INDEX IF NOT EXISTS idx_sessions_agent ON sessions(agent_id);
            CREATE INDEX IF NOT EXISTS idx_sessions_last_active ON sessions(last_active_at);
            CREATE INDEX IF NOT EXISTS idx_sessions_state ON sessions(state);
            CREATE INDEX IF NOT EXISTS idx_sessions_parent ON sessions(parent_session_key);

            CREATE VIRTUAL TABLE IF NOT EXISTS messages_fts USING fts5(
                content,
                content=messages,
                content_rowid=id
            );
            "#,
        )
        .map_err(|e| SessionManagerError::DatabaseError(format!("Schema init failed: {}", e)))?;

        // Backfill FTS5 index for any existing messages not yet indexed.
        let backfill_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM messages WHERE id NOT IN (SELECT rowid FROM messages_fts)",
                [],
                |row| row.get(0),
            )
            .unwrap_or(0);
        if backfill_count > 0 {
            conn.execute_batch(
                "INSERT OR IGNORE INTO messages_fts(rowid, content)
                 SELECT id, content FROM messages
                 WHERE id NOT IN (SELECT rowid FROM messages_fts)",
            )
            .map_err(|e| {
                SessionManagerError::DatabaseError(format!("FTS5 backfill failed: {}", e))
            })?;
            tracing::info!(
                count = backfill_count,
                "Backfilled FTS5 index with existing messages"
            );
        }

        Self::run_migrations(conn)?;

        Ok(())
    }

    /// Run incremental schema migrations for columns added after initial release.
    fn run_migrations(conn: &Connection) -> Result<(), SessionManagerError> {
        let migrations: &[(&str, &str, &str)] = &[
            ("sessions", "state", "TEXT DEFAULT 'created'"),
            ("sessions", "label", "TEXT"),
            ("sessions", "input_tokens", "INTEGER DEFAULT 0"),
            ("sessions", "output_tokens", "INTEGER DEFAULT 0"),
            ("sessions", "model", "TEXT"),
            ("sessions", "model_provider", "TEXT"),
            ("sessions", "parent_session_key", "TEXT"),
            ("sessions", "compaction_count", "INTEGER DEFAULT 0"),
            ("messages", "input_tokens", "INTEGER DEFAULT 0"),
            ("messages", "output_tokens", "INTEGER DEFAULT 0"),
        ];

        for (table, column, def) in migrations {
            let has_col: Result<bool, _> = conn.query_row(
                &format!(
                    "SELECT COUNT(*) > 0 FROM pragma_table_info('{}') WHERE name='{}'",
                    table, column
                ),
                [],
                |row| row.get(0),
            );

            let needs_add = match has_col {
                Ok(true) => false,
                Ok(false) => true,
                Err(e) => {
                    tracing::warn!(
                        table = %table,
                        column = %column,
                        error = %e,
                        "Failed to check column existence - assuming missing"
                    );
                    true
                }
            };

            if needs_add {
                conn.execute(
                    &format!("ALTER TABLE {} ADD COLUMN {} {}", table, column, def),
                    [],
                )
                .map_err(|e| {
                    SessionManagerError::DatabaseError(format!(
                        "Failed to add column {} to {}: {}",
                        column, table, e
                    ))
                })?;
                tracing::info!(table = %table, column = %column, "Migrated database schema");
            }
        }

        Ok(())
    }
}

/// Session manager errors
#[derive(Debug, thiserror::Error)]
pub enum SessionManagerError {
    #[error("Database error: {0}")]
    DatabaseError(String),

    #[error("Session not found: {0}")]
    NotFound(String),
}

/// Get session type string from SessionKey
pub(super) fn session_type_str(key: &SessionKey) -> String {
    match key {
        SessionKey::Main { .. } => "main".to_string(),
        SessionKey::DirectMessage { .. } => "peer".to_string(),
        SessionKey::Task { .. } => "task".to_string(),
        SessionKey::Ephemeral { .. } => "ephemeral".to_string(),
        SessionKey::Group { .. } => "group".to_string(),
        SessionKey::Subagent { .. } => "subagent".to_string(),
    }
}
