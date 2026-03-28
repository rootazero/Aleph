//! Session recorder component - persists session state to SQLite.
//!
//! Subscribes to: All events (for recording)
//! Publishes: None (recording only)

mod event_handler;
mod types;

#[cfg(test)]
mod tests;

pub use types::{RecorderError, SessionRecord};

use rusqlite::{params, Connection};
use std::path::Path;
use crate::sync_primitives::{Arc, Mutex};

use super::{SessionPart};

// ============================================================================
// SessionRecorder Component
// ============================================================================

/// Session Recorder - persists execution state to SQLite
///
/// This component:
/// - Subscribes to ALL events (EventType::All)
/// - Converts events to SessionPart records
/// - Persists parts to SQLite database
/// - Tracks session metadata (iteration count, tokens, timestamps)
pub struct SessionRecorder {
    /// SQLite connection (thread-safe)
    conn: Arc<Mutex<Connection>>,
}

impl SessionRecorder {
    /// Create a new SessionRecorder with an in-memory database
    ///
    /// Useful for testing or ephemeral sessions.
    pub fn new_in_memory() -> Result<Self, RecorderError> {
        let conn =
            Connection::open_in_memory().map_err(|e| RecorderError::Database(e.to_string()))?;

        Self::init_schema(&conn)?;

        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    /// Create a new SessionRecorder with a file-based database
    ///
    /// Creates the database file if it doesn't exist.
    pub fn new<P: AsRef<Path>>(db_path: P) -> Result<Self, RecorderError> {
        let conn = Connection::open(db_path).map_err(|e| RecorderError::Database(e.to_string()))?;

        Self::init_schema(&conn)?;

        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    /// Initialize the database schema
    ///
    /// Creates the following tables:
    /// - `sessions`: Session metadata (id, parent_id, agent_id, status, model, etc.)
    /// - `session_parts`: Fine-grained execution records (id, session_id, part_type, etc.)
    pub fn init_schema(conn: &Connection) -> Result<(), RecorderError> {
        // Create sessions table
        conn.execute(
            r#"
            CREATE TABLE IF NOT EXISTS sessions (
                id TEXT PRIMARY KEY,
                parent_id TEXT,
                agent_id TEXT NOT NULL,
                status TEXT NOT NULL DEFAULT 'running',
                model TEXT NOT NULL,
                iteration_count INTEGER NOT NULL DEFAULT 0,
                total_tokens INTEGER NOT NULL DEFAULT 0,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL
            )
            "#,
            [],
        )
        .map_err(|e| RecorderError::Database(e.to_string()))?;

        // Create session_parts table
        conn.execute(
            r#"
            CREATE TABLE IF NOT EXISTS session_parts (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                session_id TEXT NOT NULL,
                part_type TEXT NOT NULL,
                part_data TEXT NOT NULL,
                sequence INTEGER NOT NULL,
                created_at INTEGER NOT NULL,
                FOREIGN KEY (session_id) REFERENCES sessions(id)
            )
            "#,
            [],
        )
        .map_err(|e| RecorderError::Database(e.to_string()))?;

        // Create index on session_id for efficient lookups
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_session_parts_session_id ON session_parts(session_id)",
            [],
        )
        .map_err(|e| RecorderError::Database(e.to_string()))?;

        // Create index on parent_id for hierarchical queries
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_sessions_parent_id ON sessions(parent_id)",
            [],
        )
        .map_err(|e| RecorderError::Database(e.to_string()))?;

        Ok(())
    }

    /// Create a new session record
    ///
    /// Inserts a new session with the given ID and model.
    /// Sets created_at and updated_at to current timestamp.
    pub fn create_session(&self, session_id: &str, model: &str) -> Result<(), RecorderError> {
        self.create_session_with_options(session_id, model, None, "main")
    }

    /// Create a new session record with full options
    pub fn create_session_with_options(
        &self,
        session_id: &str,
        model: &str,
        parent_id: Option<&str>,
        agent_id: &str,
    ) -> Result<(), RecorderError> {
        let now = chrono::Utc::now().timestamp();

        let conn = self
            .conn
            .lock()
            .map_err(|e| RecorderError::Lock(e.to_string()))?;

        conn.execute(
            r#"
            INSERT INTO sessions (id, parent_id, agent_id, status, model, iteration_count, total_tokens, created_at, updated_at)
            VALUES (?1, ?2, ?3, 'running', ?4, 0, 0, ?5, ?5)
            "#,
            params![session_id, parent_id, agent_id, model, now],
        ).map_err(|e| RecorderError::Database(e.to_string()))?;

        Ok(())
    }

    /// Update session metadata
    ///
    /// Updates the iteration count and updated_at timestamp.
    /// Optionally updates status and token count.
    pub fn update_session(&self, session_id: &str) -> Result<(), RecorderError> {
        let now = chrono::Utc::now().timestamp();

        let conn = self
            .conn
            .lock()
            .map_err(|e| RecorderError::Lock(e.to_string()))?;

        conn.execute(
            r#"
            UPDATE sessions
            SET iteration_count = iteration_count + 1, updated_at = ?1
            WHERE id = ?2
            "#,
            params![now, session_id],
        )
        .map_err(|e| RecorderError::Database(e.to_string()))?;

        Ok(())
    }

    /// Update session with specific values
    pub fn update_session_full(
        &self,
        session_id: &str,
        status: Option<&str>,
        iteration_count: Option<u32>,
        total_tokens: Option<u64>,
    ) -> Result<(), RecorderError> {
        let now = chrono::Utc::now().timestamp();

        let conn = self
            .conn
            .lock()
            .map_err(|e| RecorderError::Lock(e.to_string()))?;

        // Build dynamic update query
        // ?1 = now, then optional fields, last param = session_id
        let mut updates: Vec<String> = vec!["updated_at = ?1".to_string()];
        let mut param_index = 2;

        if status.is_some() {
            updates.push(format!("status = ?{}", param_index));
            param_index += 1;
        }
        if iteration_count.is_some() {
            updates.push(format!("iteration_count = ?{}", param_index));
            param_index += 1;
        }
        if total_tokens.is_some() {
            updates.push(format!("total_tokens = ?{}", param_index));
            param_index += 1;
        }

        // session_id is always the last parameter
        let query = format!(
            "UPDATE sessions SET {} WHERE id = ?{}",
            updates.join(", "),
            param_index
        );

        // Execute with appropriate parameters
        match (status, iteration_count, total_tokens) {
            (Some(s), Some(ic), Some(tt)) => {
                conn.execute(&query, params![now, s, ic, tt, session_id])
            }
            (Some(s), Some(ic), None) => conn.execute(&query, params![now, s, ic, session_id]),
            (Some(s), None, Some(tt)) => conn.execute(&query, params![now, s, tt, session_id]),
            (Some(s), None, None) => conn.execute(&query, params![now, s, session_id]),
            (None, Some(ic), Some(tt)) => conn.execute(&query, params![now, ic, tt, session_id]),
            (None, Some(ic), None) => conn.execute(&query, params![now, ic, session_id]),
            (None, None, Some(tt)) => conn.execute(&query, params![now, tt, session_id]),
            (None, None, None) => conn.execute(&query, params![now, session_id]),
        }
        .map_err(|e| RecorderError::Database(e.to_string()))?;

        Ok(())
    }

    /// Append a session part to the database
    ///
    /// Gets the next sequence number for the session and inserts the part.
    pub fn append_part(&self, session_id: &str, part: &SessionPart) -> Result<i64, RecorderError> {
        let now = chrono::Utc::now().timestamp();

        let conn = self
            .conn
            .lock()
            .map_err(|e| RecorderError::Lock(e.to_string()))?;

        // Get next sequence number for this session
        let sequence: i64 = conn
            .query_row(
                "SELECT COALESCE(MAX(sequence), 0) + 1 FROM session_parts WHERE session_id = ?1",
                params![session_id],
                |row| row.get(0),
            )
            .map_err(|e| RecorderError::Database(e.to_string()))?;

        // Serialize part to JSON
        let part_type = part.type_name();
        let part_data =
            serde_json::to_string(part).map_err(|e| RecorderError::Serialization(e.to_string()))?;

        // Insert the part
        conn.execute(
            r#"
            INSERT INTO session_parts (session_id, part_type, part_data, sequence, created_at)
            VALUES (?1, ?2, ?3, ?4, ?5)
            "#,
            params![session_id, part_type, part_data, sequence, now],
        )
        .map_err(|e| RecorderError::Database(e.to_string()))?;

        let id = conn.last_insert_rowid();

        Ok(id)
    }

    /// Get all parts for a session
    pub fn get_session_parts(&self, session_id: &str) -> Result<Vec<SessionPart>, RecorderError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| RecorderError::Lock(e.to_string()))?;

        let mut stmt = conn
            .prepare("SELECT part_data FROM session_parts WHERE session_id = ?1 ORDER BY sequence")
            .map_err(|e| RecorderError::Database(e.to_string()))?;

        let parts: Result<Vec<SessionPart>, _> = stmt
            .query_map(params![session_id], |row| {
                let data: String = row.get(0)?;
                Ok(data)
            })
            .map_err(|e| RecorderError::Database(e.to_string()))?
            .map(|r| {
                r.map_err(|e| RecorderError::Database(e.to_string()))
                    .and_then(|data| {
                        serde_json::from_str(&data)
                            .map_err(|e| RecorderError::Serialization(e.to_string()))
                    })
            })
            .collect();

        parts
    }

    /// Get session info
    pub fn get_session(&self, session_id: &str) -> Result<Option<SessionRecord>, RecorderError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| RecorderError::Lock(e.to_string()))?;

        let result = conn.query_row(
            r#"
            SELECT id, parent_id, agent_id, status, model, iteration_count, total_tokens, created_at, updated_at
            FROM sessions WHERE id = ?1
            "#,
            params![session_id],
            |row| {
                Ok(SessionRecord {
                    id: row.get(0)?,
                    parent_id: row.get(1)?,
                    agent_id: row.get(2)?,
                    status: row.get(3)?,
                    model: row.get(4)?,
                    iteration_count: row.get(5)?,
                    total_tokens: row.get(6)?,
                    created_at: row.get(7)?,
                    updated_at: row.get(8)?,
                })
            },
        );

        match result {
            Ok(record) => Ok(Some(record)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(RecorderError::Database(e.to_string())),
        }
    }
}
