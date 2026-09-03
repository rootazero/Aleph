//! Session Manager — compatibility layer for Gateway `session.*` RPC.
//!
//! Agent execution (`agent_loop`) no longer reads or writes `SessionManager`
//! directly; it uses `crate::session::SessionService`. Messages are
//! materialized from `session_events` by `MessageProjector`.
//!
//! This module manages sessions with `SQLite` persistence, automatic
//! compaction, and lifecycle management.

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
            Self::Created => write!(f, "created"),
            Self::Active => write!(f, "active"),
            Self::Idle => write!(f, "idle"),
            Self::Running => write!(f, "running"),
            Self::Error => write!(f, "error"),
            Self::Stopped => write!(f, "stopped"),
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

    /// Identity ID ("owner" or `guest_id`)
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
    #[must_use]
    pub fn from_json_str(json: Option<&str>) -> Self {
        json.and_then(|s| serde_json::from_str(s).ok())
            .unwrap_or_default()
    }

    /// Serialize to JSON string
    pub fn to_json_string(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }

    /// Convert to `IdentityContext` for a specific request
    #[must_use]
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

/// Session manager with `SQLite` persistence
///
#[derive(Clone)]
pub struct SessionManager {
    pub(super) config: SessionManagerConfig,
    pub(super) conn: Arc<Mutex<Connection>>,
    /// Optional writer for Spec 1 memory capture hooks.
    pub(super) raw_memory_writer: Option<Arc<dyn crate::memory::store::raw_memory::RawMemoryStore>>,
    /// Optional event bus for publishing session lifecycle events.
    pub(super) event_bus: Option<Arc<super::event_bus::GatewayEventBus>>,
}

impl SessionManager {
    /// Create a new session manager
    pub fn new(config: SessionManagerConfig) -> Result<Self, SessionManagerError> {
        // Ensure parent directory exists
        if let Some(parent) = config.db_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                SessionManagerError::DatabaseError(format!("Failed to create db directory: {e}"))
            })?;
        }

        let conn = crate::utils::sqlite_open::open_sqlite_safe(&config.db_path).map_err(|e| {
            SessionManagerError::DatabaseError(format!("Failed to open database: {e}"))
        })?;

        // Initialize schema
        Self::init_schema(&conn)?;

        info!("Session manager initialized: {:?}", config.db_path);

        Ok(Self {
            config,
            conn: Arc::new(Mutex::new(conn)),
            raw_memory_writer: None,
            event_bus: None,
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
    #[must_use]
    pub fn with_event_bus(mut self, bus: Arc<super::event_bus::GatewayEventBus>) -> Self {
        self.event_bus = Some(bus);
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
                source_seq INTEGER,
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
        .map_err(|e| SessionManagerError::DatabaseError(format!("Schema init failed: {e}")))?;

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
                SessionManagerError::DatabaseError(format!("FTS5 backfill failed: {e}"))
            })?;
            tracing::info!(
                count = backfill_count,
                "Backfilled FTS5 index with existing messages"
            );
        }

        Self::run_migrations(conn)?;

        // AFTER the migrations, not in the schema batch above: `source_seq` is
        // added by `run_migrations` on a database created before it existed, so
        // an index declared alongside `CREATE TABLE messages` would fail on
        // every legacy install and take the whole store open down with it.
        //
        // Serves the projector's seq-ranged reads — `stamp_assistant_metadata_in_range`
        // and `delete_messages_from_seq` both scan `(session_key, source_seq)`.
        conn.execute_batch(
            "CREATE INDEX IF NOT EXISTS idx_messages_source_seq
               ON messages(session_key, source_seq);",
        )
        .map_err(|e| {
            SessionManagerError::DatabaseError(format!("source_seq index failed: {e}"))
        })?;

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
            ("sessions", "derived_title", "TEXT"),
            // `SessionMetadata.estimated_cost_usd` has been a struct field with
            // no column and no writer since it was introduced — it read back 0.0
            // forever while being surfaced to the model (the `sessions` tool) and
            // to the Panel as this session's cost. Give it somewhere to live.
            ("sessions", "estimated_cost_usd", "REAL DEFAULT 0"),
            // P1 data isolation: authenticated owner + scope stamp, set once at
            // creation (`SessionMetadata::stamp_attribution`). NULL on legacy
            // rows — read as owned by `OWNER_USER_ID` via
            // `gateway::visibility::effective_owner` (adoption-by-absence).
            ("sessions", "owner_user_id", "TEXT"),
            ("sessions", "scope_id", "TEXT"),
            // `SessionMetadata.last_message_preview` had a writer only in the
            // FILE backend (`FileSessionStore::append_message`), no column
            // here, and two readers that surface it regardless of backend
            // (`sessions.preview` and the `sessions` tool row). So a
            // `session_store_backend = "sqlite"` install rendered a null
            // preview for every conversation forever — indistinguishable from
            // "this conversation has no messages yet".
            ("sessions", "last_message_preview", "TEXT"),
            ("messages", "input_tokens", "INTEGER DEFAULT 0"),
            ("messages", "output_tokens", "INTEGER DEFAULT 0"),
            ("messages", "tool_call_id", "TEXT"),
            ("messages", "tool_name", "TEXT"),
            // Seq of the `session_events` event this row was projected from.
            // NULL = not event-sourced (legacy rows, boot-time orphan notices),
            // which is what keeps `delete_messages_from_seq` off them.
            ("messages", "source_seq", "INTEGER"),
        ];

        fn is_safe_identifier(s: &str) -> bool {
            !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
        }

        fn is_safe_column_def(s: &str) -> bool {
            !s.is_empty()
                && s.bytes().all(|b| {
                    b.is_ascii_alphanumeric()
                        || b.is_ascii_whitespace()
                        || b == b'_'
                        || b == b'\''
                        || b == b'('
                        || b == b')'
                        || b == b','
                })
        }

        for (table, column, def) in migrations {
            if !is_safe_identifier(table) || !is_safe_identifier(column) || !is_safe_column_def(def)
            {
                return Err(SessionManagerError::DatabaseError(format!(
                    "Refusing unsafe migration identifier: {table}.{column} {def}"
                )));
            }

            let has_col: Result<bool, _> = conn.query_row(
                "SELECT COUNT(*) > 0 FROM pragma_table_info(?1) WHERE name = ?2",
                [*table, *column],
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
                // Identifiers were validated above; quote table/column names to avoid
                // keyword collisions. `def` is a validated column definition.
                let sql = format!(r#"ALTER TABLE "{table}" ADD COLUMN "{column}" {def}"#);
                conn.execute(&sql, []).map_err(|e| {
                    SessionManagerError::DatabaseError(format!(
                        "Failed to add column {column} to {table}: {e}"
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

    /// The session exists but is in a terminal state (stopped / ended) and
    /// cannot be silently resumed by `get_or_create`. The caller must
    /// explicitly reopen it (e.g. via the operator IPC) before new messages
    /// can be appended.
    #[error("Session is stopped or ended: {0}")]
    SessionStopped(String),
}

/// Get session type string from `SessionKey`
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
