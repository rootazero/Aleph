//! Session Manager
//!
//! Manages sessions with SQLite persistence, automatic compaction,
//! and lifecycle management.

use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tracing::{debug, info};

use super::router::SessionKey;
use aleph_protocol::{IdentityContext, Role, GuestScope};

/// Session message stored in database
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredMessage {
    pub id: i64,
    pub role: String,
    pub content: String,
    pub timestamp: i64,
    pub metadata: Option<String>,
}

/// Session metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMetadata {
    pub key: String,
    pub agent_id: String,
    pub session_type: String,
    pub created_at: i64,
    pub last_active_at: i64,
    pub message_count: i64,
    pub total_tokens: i64,
    pub auto_reset_at: Option<i64>,
}

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
            Role::Anonymous => {
                IdentityContext::anonymous(session_key, self.source_channel.clone())
            }
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
            db_path: dirs::home_dir()
                .unwrap_or_else(|| PathBuf::from("/tmp"))
                .join(".aleph/sessions.db"),
            max_messages: 100,
            compaction_keep: 50,
            auto_reset_hour: Some(4), // 4 AM
            session_expiry_secs: 30 * 24 * 60 * 60, // 30 days
        }
    }
}

/// Session manager with SQLite persistence
///
/// Uses `std::sync::Mutex` for the connection because `rusqlite::Connection`
/// is not `Sync` (it uses `RefCell` internally). This is safe for async use
/// as long as we don't hold the lock across await points.
pub struct SessionManager {
    config: SessionManagerConfig,
    conn: Arc<Mutex<Connection>>,
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
        })
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
                metadata TEXT
            );

            CREATE TABLE IF NOT EXISTS messages (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                session_key TEXT NOT NULL,
                role TEXT NOT NULL,
                content TEXT NOT NULL,
                timestamp INTEGER NOT NULL,
                metadata TEXT,
                FOREIGN KEY (session_key) REFERENCES sessions(key) ON DELETE CASCADE
            );

            CREATE INDEX IF NOT EXISTS idx_messages_session ON messages(session_key);
            CREATE INDEX IF NOT EXISTS idx_sessions_agent ON sessions(agent_id);
            CREATE INDEX IF NOT EXISTS idx_sessions_last_active ON sessions(last_active_at);
            "#,
        )
        .map_err(|e| SessionManagerError::DatabaseError(format!("Schema init failed: {}", e)))?;

        Ok(())
    }

    /// Get or create a session
    pub async fn get_or_create(&self, key: &SessionKey) -> Result<SessionMetadata, SessionManagerError> {
        let key_str = key.to_key_string();
        let agent_id = key.agent_id().to_string();
        let session_type = session_type_str(key);
        let now = chrono::Utc::now().timestamp();

        let conn = self.conn.lock().map_err(|e| SessionManagerError::DatabaseError(format!("Lock error: {}", e)))?;

        // Try to get existing session
        let existing: Option<SessionMetadata> = conn
            .query_row(
                "SELECT key, agent_id, session_type, created_at, last_active_at,
                        message_count, total_tokens, auto_reset_at
                 FROM sessions WHERE key = ?",
                params![&key_str],
                |row| {
                    Ok(SessionMetadata {
                        key: row.get(0)?,
                        agent_id: row.get(1)?,
                        session_type: row.get(2)?,
                        created_at: row.get(3)?,
                        last_active_at: row.get(4)?,
                        message_count: row.get(5)?,
                        total_tokens: row.get(6)?,
                        auto_reset_at: row.get(7)?,
                    })
                },
            )
            .ok();

        if let Some(meta) = existing {
            // Update last_active_at
            conn.execute(
                "UPDATE sessions SET last_active_at = ? WHERE key = ?",
                params![now, &key_str],
            )
            .ok();

            return Ok(meta);
        }

        // Create new session
        conn.execute(
            "INSERT INTO sessions (key, agent_id, session_type, created_at, last_active_at)
             VALUES (?, ?, ?, ?, ?)",
            params![&key_str, &agent_id, &session_type, now, now],
        )
        .map_err(|e| SessionManagerError::DatabaseError(format!("Insert failed: {}", e)))?;

        debug!("Created session: {}", key_str);

        Ok(SessionMetadata {
            key: key_str,
            agent_id,
            session_type,
            created_at: now,
            last_active_at: now,
            message_count: 0,
            total_tokens: 0,
            auto_reset_at: None,
        })
    }

    /// Add a message to a session
    pub async fn add_message(
        &self,
        key: &SessionKey,
        role: &str,
        content: &str,
    ) -> Result<i64, SessionManagerError> {
        let key_str = key.to_key_string();
        let now = chrono::Utc::now().timestamp();

        // Use scope block to ensure lock is released before any await
        let (message_id, needs_compaction) = {
            let conn = self.conn.lock().map_err(|e| SessionManagerError::DatabaseError(format!("Lock error: {}", e)))?;

            // Insert message
            conn.execute(
                "INSERT INTO messages (session_key, role, content, timestamp) VALUES (?, ?, ?, ?)",
                params![&key_str, role, content, now],
            )
            .map_err(|e| SessionManagerError::DatabaseError(format!("Insert message failed: {}", e)))?;

            let message_id = conn.last_insert_rowid();

            // Update session stats
            conn.execute(
                "UPDATE sessions SET last_active_at = ?, message_count = message_count + 1 WHERE key = ?",
                params![now, &key_str],
            )
            .ok();

            // Check if compaction needed
            let count: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM messages WHERE session_key = ?",
                    params![&key_str],
                    |row| row.get(0),
                )
                .unwrap_or(0);

            (message_id, count as usize > self.config.max_messages)
        }; // Lock released here

        if needs_compaction {
            self.compact_session(key).await?;
        }

        Ok(message_id)
    }

    /// Get session history
    pub async fn get_history(
        &self,
        key: &SessionKey,
        limit: Option<usize>,
    ) -> Result<Vec<StoredMessage>, SessionManagerError> {
        let key_str = key.to_key_string();
        let conn = self.conn.lock().map_err(|e| SessionManagerError::DatabaseError(format!("Lock error: {}", e)))?;

        let query = match limit {
            Some(n) => format!(
                "SELECT id, role, content, timestamp, metadata FROM messages
                 WHERE session_key = ? ORDER BY timestamp DESC LIMIT {}",
                n
            ),
            None => "SELECT id, role, content, timestamp, metadata FROM messages
                     WHERE session_key = ? ORDER BY timestamp ASC"
                .to_string(),
        };

        let mut stmt = conn
            .prepare(&query)
            .map_err(|e| SessionManagerError::DatabaseError(e.to_string()))?;

        let messages: Vec<StoredMessage> = stmt
            .query_map(params![&key_str], |row| {
                Ok(StoredMessage {
                    id: row.get(0)?,
                    role: row.get(1)?,
                    content: row.get(2)?,
                    timestamp: row.get(3)?,
                    metadata: row.get(4)?,
                })
            })
            .map_err(|e| SessionManagerError::DatabaseError(e.to_string()))?
            .filter_map(|r| r.ok())
            .collect();

        // If we used DESC for limit, reverse to get chronological order
        if limit.is_some() {
            let reversed: Vec<_> = messages.into_iter().rev().collect();
            return Ok(reversed);
        }

        Ok(messages)
    }

    /// Reset (clear) a session
    pub async fn reset_session(&self, key: &SessionKey) -> Result<bool, SessionManagerError> {
        let key_str = key.to_key_string();
        let conn = self.conn.lock().map_err(|e| SessionManagerError::DatabaseError(format!("Lock error: {}", e)))?;

        let deleted = conn
            .execute("DELETE FROM messages WHERE session_key = ?", params![&key_str])
            .map_err(|e| SessionManagerError::DatabaseError(e.to_string()))?;

        conn.execute(
            "UPDATE sessions SET message_count = 0, last_active_at = ? WHERE key = ?",
            params![chrono::Utc::now().timestamp(), &key_str],
        )
        .ok();

        debug!("Reset session {}: deleted {} messages", key_str, deleted);

        Ok(deleted > 0)
    }

    /// Delete a session entirely
    pub async fn delete_session(&self, key: &SessionKey) -> Result<bool, SessionManagerError> {
        let key_str = key.to_key_string();
        let conn = self.conn.lock().map_err(|e| SessionManagerError::DatabaseError(format!("Lock error: {}", e)))?;

        // Delete messages first
        conn.execute("DELETE FROM messages WHERE session_key = ?", params![&key_str])
            .ok();

        // Delete session
        let deleted = conn
            .execute("DELETE FROM sessions WHERE key = ?", params![&key_str])
            .map_err(|e| SessionManagerError::DatabaseError(e.to_string()))?;

        debug!("Deleted session: {}", key_str);

        Ok(deleted > 0)
    }

    /// List sessions, optionally filtered by agent
    pub async fn list_sessions(
        &self,
        agent_id: Option<&str>,
    ) -> Result<Vec<SessionMetadata>, SessionManagerError> {
        let conn = self.conn.lock().map_err(|e| SessionManagerError::DatabaseError(format!("Lock error: {}", e)))?;

        let sessions = if let Some(id) = agent_id {
            let mut stmt = conn
                .prepare(
                    "SELECT key, agent_id, session_type, created_at, last_active_at,
                            message_count, total_tokens, auto_reset_at
                     FROM sessions WHERE agent_id = ? ORDER BY last_active_at DESC",
                )
                .map_err(|e| SessionManagerError::DatabaseError(e.to_string()))?;

            let rows = stmt
                .query_map(params![id], |row| {
                    Ok(SessionMetadata {
                        key: row.get(0)?,
                        agent_id: row.get(1)?,
                        session_type: row.get(2)?,
                        created_at: row.get(3)?,
                        last_active_at: row.get(4)?,
                        message_count: row.get(5)?,
                        total_tokens: row.get(6)?,
                        auto_reset_at: row.get(7)?,
                    })
                })
                .map_err(|e| SessionManagerError::DatabaseError(e.to_string()))?;

            rows.filter_map(|r| r.ok()).collect()
        } else {
            let mut stmt = conn
                .prepare(
                    "SELECT key, agent_id, session_type, created_at, last_active_at,
                            message_count, total_tokens, auto_reset_at
                     FROM sessions ORDER BY last_active_at DESC",
                )
                .map_err(|e| SessionManagerError::DatabaseError(e.to_string()))?;

            let rows = stmt
                .query_map([], |row| {
                    Ok(SessionMetadata {
                        key: row.get(0)?,
                        agent_id: row.get(1)?,
                        session_type: row.get(2)?,
                        created_at: row.get(3)?,
                        last_active_at: row.get(4)?,
                        message_count: row.get(5)?,
                        total_tokens: row.get(6)?,
                        auto_reset_at: row.get(7)?,
                    })
                })
                .map_err(|e| SessionManagerError::DatabaseError(e.to_string()))?;

            rows.filter_map(|r| r.ok()).collect()
        };

        Ok(sessions)
    }

    /// Compact a session by removing old messages
    pub async fn compact_session(&self, key: &SessionKey) -> Result<usize, SessionManagerError> {
        let key_str = key.to_key_string();
        let keep = self.config.compaction_keep as i64;

        let conn = self.conn.lock().map_err(|e| SessionManagerError::DatabaseError(format!("Lock error: {}", e)))?;

        // Get the ID threshold
        let threshold_id: Option<i64> = conn
            .query_row(
                "SELECT id FROM messages WHERE session_key = ?
                 ORDER BY timestamp DESC LIMIT 1 OFFSET ?",
                params![&key_str, keep],
                |row| row.get(0),
            )
            .ok();

        if let Some(threshold) = threshold_id {
            let deleted = conn
                .execute(
                    "DELETE FROM messages WHERE session_key = ? AND id < ?",
                    params![&key_str, threshold],
                )
                .map_err(|e| SessionManagerError::DatabaseError(e.to_string()))?;

            // Update message count
            let new_count: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM messages WHERE session_key = ?",
                    params![&key_str],
                    |row| row.get(0),
                )
                .unwrap_or(0);

            conn.execute(
                "UPDATE sessions SET message_count = ? WHERE key = ?",
                params![new_count, &key_str],
            )
            .ok();

            info!("Compacted session {}: removed {} messages", key_str, deleted);

            return Ok(deleted);
        }

        Ok(0)
    }

    /// Cleanup expired sessions
    pub async fn cleanup_expired(&self) -> Result<usize, SessionManagerError> {
        if self.config.session_expiry_secs == 0 {
            return Ok(0);
        }

        let expiry_threshold =
            chrono::Utc::now().timestamp() - self.config.session_expiry_secs as i64;

        let conn = self.conn.lock().map_err(|e| SessionManagerError::DatabaseError(format!("Lock error: {}", e)))?;

        // Get sessions to delete
        let keys: Vec<String> = {
            let mut stmt = conn
                .prepare(
                    "SELECT key FROM sessions WHERE last_active_at < ? AND session_type = 'ephemeral'",
                )
                .map_err(|e| SessionManagerError::DatabaseError(e.to_string()))?;

            let rows = stmt
                .query_map(params![expiry_threshold], |row| row.get(0))
                .map_err(|e| SessionManagerError::DatabaseError(e.to_string()))?;

            rows.filter_map(|r| r.ok()).collect()
        };

        let mut deleted = 0;
        for key in &keys {
            conn.execute("DELETE FROM messages WHERE session_key = ?", params![key])
                .ok();
            if conn
                .execute("DELETE FROM sessions WHERE key = ?", params![key])
                .is_ok()
            {
                deleted += 1;
            }
        }

        if deleted > 0 {
            info!("Cleaned up {} expired sessions", deleted);
        }

        Ok(deleted)
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
fn session_type_str(key: &SessionKey) -> String {
    match key {
        SessionKey::Main { .. } => "main".to_string(),
        SessionKey::PerPeer { .. } => "peer".to_string(),
        SessionKey::Task { .. } => "task".to_string(),
        SessionKey::Ephemeral { .. } => "ephemeral".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn test_config(path: PathBuf) -> SessionManagerConfig {
        SessionManagerConfig {
            db_path: path,
            max_messages: 10,
            compaction_keep: 5,
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn test_session_creation() {
        let temp = tempdir().unwrap();
        let config = test_config(temp.path().join("test.db"));
        let manager = SessionManager::new(config).unwrap();

        let key = SessionKey::main("test");
        let meta = manager.get_or_create(&key).await.unwrap();

        assert_eq!(meta.agent_id, "test");
        assert_eq!(meta.session_type, "main");
        assert_eq!(meta.message_count, 0);
    }

    #[tokio::test]
    async fn test_message_operations() {
        let temp = tempdir().unwrap();
        let config = test_config(temp.path().join("test.db"));
        let manager = SessionManager::new(config).unwrap();

        let key = SessionKey::main("test");
        manager.get_or_create(&key).await.unwrap();

        // Add messages
        manager.add_message(&key, "user", "Hello").await.unwrap();
        manager
            .add_message(&key, "assistant", "Hi there!")
            .await
            .unwrap();

        let history = manager.get_history(&key, None).await.unwrap();
        assert_eq!(history.len(), 2);
        assert_eq!(history[0].role, "user");
        assert_eq!(history[1].role, "assistant");
    }

    #[tokio::test]
    async fn test_session_reset() {
        let temp = tempdir().unwrap();
        let config = test_config(temp.path().join("test.db"));
        let manager = SessionManager::new(config).unwrap();

        let key = SessionKey::main("test");
        manager.get_or_create(&key).await.unwrap();
        manager.add_message(&key, "user", "Test").await.unwrap();

        assert!(manager.reset_session(&key).await.unwrap());

        let history = manager.get_history(&key, None).await.unwrap();
        assert!(history.is_empty());
    }

    #[tokio::test]
    async fn test_compaction() {
        let temp = tempdir().unwrap();
        let config = test_config(temp.path().join("test.db"));
        let manager = SessionManager::new(config).unwrap();

        let key = SessionKey::main("test");
        manager.get_or_create(&key).await.unwrap();

        // Add more messages than max_messages
        for i in 0..15 {
            manager
                .add_message(&key, "user", &format!("Message {}", i))
                .await
                .unwrap();
        }

        // Compaction should have happened automatically
        let history = manager.get_history(&key, None).await.unwrap();
        assert!(history.len() <= 10); // Should be at most max_messages after compaction
    }

    #[tokio::test]
    async fn test_list_sessions() {
        let temp = tempdir().unwrap();
        let config = test_config(temp.path().join("test.db"));
        let manager = SessionManager::new(config).unwrap();

        manager
            .get_or_create(&SessionKey::main("agent1"))
            .await
            .unwrap();
        manager
            .get_or_create(&SessionKey::main("agent2"))
            .await
            .unwrap();
        manager
            .get_or_create(&SessionKey::peer("agent1", "peer1"))
            .await
            .unwrap();

        let all = manager.list_sessions(None).await.unwrap();
        assert_eq!(all.len(), 3);

        let agent1_only = manager.list_sessions(Some("agent1")).await.unwrap();
        assert_eq!(agent1_only.len(), 2);
    }

    #[test]
    fn test_session_identity_meta_default() {
        let meta = SessionIdentityMeta::default();
        assert_eq!(meta.role, Role::Owner);
        assert_eq!(meta.identity_id, "owner");
        assert!(meta.scope.is_none());
        assert_eq!(meta.source_channel, "unknown");
    }

    #[test]
    fn test_session_identity_meta_owner_factory() {
        let meta = SessionIdentityMeta::owner("cli");
        assert_eq!(meta.role, Role::Owner);
        assert_eq!(meta.identity_id, "owner");
        assert!(meta.scope.is_none());
        assert_eq!(meta.source_channel, "cli");
    }

    #[test]
    fn test_session_identity_meta_guest_factory() {
        let scope = GuestScope {
            allowed_tools: vec!["translate".to_string()],
            expires_at: Some(2000),
            display_name: Some("Test Guest".to_string()),
        };

        let meta = SessionIdentityMeta::guest("guest-123", scope.clone(), "telegram");
        assert_eq!(meta.role, Role::Guest);
        assert_eq!(meta.identity_id, "guest-123");
        assert_eq!(meta.scope, Some(scope));
        assert_eq!(meta.source_channel, "telegram");
    }

    #[test]
    fn test_session_identity_meta_json_roundtrip() {
        let scope = GuestScope {
            allowed_tools: vec!["tool1".to_string(), "tool2".to_string()],
            expires_at: None,
            display_name: None,
        };

        let meta = SessionIdentityMeta::guest("guest-456", scope, "web");
        let json = meta.to_json_string().unwrap();
        let parsed = SessionIdentityMeta::from_json_str(Some(&json));

        assert_eq!(parsed.role, meta.role);
        assert_eq!(parsed.identity_id, meta.identity_id);
        assert_eq!(parsed.scope, meta.scope);
        assert_eq!(parsed.source_channel, meta.source_channel);
    }

    #[test]
    fn test_session_identity_meta_from_null_json() {
        let meta = SessionIdentityMeta::from_json_str(None);
        assert_eq!(meta.role, Role::Owner); // Default
        assert_eq!(meta.identity_id, "owner");
    }

    #[test]
    fn test_session_identity_meta_from_invalid_json() {
        let meta = SessionIdentityMeta::from_json_str(Some("{invalid json}"));
        assert_eq!(meta.role, Role::Owner); // Fallback to default
    }

    #[test]
    fn test_session_identity_meta_to_identity_context_owner() {
        let meta = SessionIdentityMeta::owner("cli");
        let ctx = meta.to_identity_context("session:main".to_string());

        assert_eq!(ctx.session_key, "session:main");
        assert_eq!(ctx.role, Role::Owner);
        assert_eq!(ctx.identity_id, "owner");
        assert_eq!(ctx.source_channel, "cli");
        assert!(ctx.scope.is_none());
    }

    #[test]
    fn test_session_identity_meta_to_identity_context_guest() {
        let scope = GuestScope {
            allowed_tools: vec!["translate".to_string()],
            expires_at: Some(3000),
            display_name: Some("Guest".to_string()),
        };

        let meta = SessionIdentityMeta::guest("guest-789", scope.clone(), "telegram");
        let ctx = meta.to_identity_context("session:guest".to_string());

        assert_eq!(ctx.session_key, "session:guest");
        assert_eq!(ctx.role, Role::Guest);
        assert_eq!(ctx.identity_id, "guest-789");
        assert_eq!(ctx.source_channel, "telegram");
        assert_eq!(ctx.scope, Some(scope));
    }
}
