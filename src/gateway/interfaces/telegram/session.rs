//! Session state management for Telegram conversations.
//!
//! Stores Telegram-specific session metadata in SQLite using the existing
//! `group_chat_sessions` table, extended with a `telegram_session_metadata` table
//! for per-conversation key/value storage.
//!
//! # Session Lifecycle
//!
//! 1. **Lookup**: On each inbound message, look up session by `(chat_id, thread_id)`
//! 2. **Create**: If no session exists and the message warrants a new session, create one
//! 3. **Update**: After processing, persist metadata changes to SQLite
//! 4. **Expire**: Background task cleans up sessions idle for longer than `timeout_minutes`

use crate::gateway::interfaces::telegram::context::{ConversationKey, SessionState};
use crate::resilience::StateDatabase;
use crate::sync_primitives::Arc;
use chrono::{DateTime, Utc};
use rusqlite::params;
use std::collections::HashMap;
use std::time::Duration;

/// Configuration for session behavior.
#[derive(Debug, Clone)]
pub struct SessionConfig {
    /// Session idle timeout in minutes. Default: 30.
    pub timeout_minutes: u64,
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            timeout_minutes: 30,
        }
    }
}

impl SessionConfig {
    pub fn timeout(&self) -> Duration {
        Duration::from_secs(self.timeout_minutes * 60)
    }
}

/// Manages Telegram session persistence.
///
/// Uses the existing `group_chat_sessions` table (shared schema) with
/// a companion `telegram_session_metadata` table for key/value metadata.
pub struct TelegramSessionManager {
    db: Arc<StateDatabase>,
    config: SessionConfig,
}

impl TelegramSessionManager {
    pub fn new(db: Arc<StateDatabase>, config: SessionConfig) -> Self {
        Self { db, config }
    }

    /// Load or create a session for the given conversation key.
    ///
    /// Returns `(session, is_new)` where `is_new` is `true` if a new session was created.
    pub fn get_or_create_session(
        &self,
        conversation_key: &ConversationKey,
        channel_id: &str,
    ) -> Result<(SessionState, bool), SessionError> {
        let session_key = conversation_key.to_conversation_id_string();

        // Try to load existing session
        if let Some(mut session) = self.load_session_internal(channel_id, &session_key)? {
            // Touch to update last_activity
            session.touch();

            // Check if expired
            if session.is_expired(self.config.timeout()) {
                // Create new session instead
                let new_session = SessionState::new(conversation_key.clone());
                self.save_session(channel_id, &new_session)?;
                return Ok((new_session, true));
            }

            return Ok((session, false));
        }

        // Create new session
        let session = SessionState::new(conversation_key.clone());
        self.save_session(channel_id, &session)?;
        Ok((session, true))
    }

    /// Update an existing session's metadata and persist to database.
    pub fn update_session(
        &self,
        channel_id: &str,
        session: &SessionState,
    ) -> Result<(), SessionError> {
        self.save_session(channel_id, session)
    }

    /// Delete a session by conversation key.
    pub fn delete_session(
        &self,
        channel_id: &str,
        conversation_key: &ConversationKey,
    ) -> Result<(), SessionError> {
        let session_key = conversation_key.to_conversation_id_string();
        let conn = self.db.conn.lock().unwrap_or_else(|e| e.into_inner());

        // Delete metadata first
        conn.execute(
            "DELETE FROM telegram_session_metadata WHERE session_id = ?1",
            [session_key.as_str()],
        )
        .map_err(|e| SessionError::Database(e.to_string()))?;

        // Delete session
        conn.execute(
            "DELETE FROM group_chat_sessions WHERE source_channel = ?1 AND source_session_key = ?2",
            [channel_id, session_key.as_str()],
        )
        .map_err(|e| SessionError::Database(e.to_string()))?;

        Ok(())
    }

    /// List all active sessions for this channel.
    pub fn list_sessions(&self, channel_id: &str) -> Result<Vec<SessionState>, SessionError> {
        let conn = self.db.conn.lock().unwrap_or_else(|e| e.into_inner());

        let mut stmt = conn
            .prepare(
                r#"
                SELECT id, topic, created_at, updated_at
                FROM group_chat_sessions
                WHERE source_channel = ?1 AND status = 'active'
                ORDER BY updated_at DESC
                "#,
            )
            .map_err(|e| SessionError::Database(e.to_string()))?;

        let rows = stmt
            .query_map([channel_id], |row| {
                let session_key: String = row.get(0)?;
                let updated_at: i64 = row.get(3)?;
                Ok((session_key, updated_at))
            })
            .map_err(|e| SessionError::Database(e.to_string()))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| SessionError::Database(e.to_string()))?;

        drop(stmt);
        drop(conn);

        let mut sessions = Vec::new();
        for (session_key, updated_at) in rows {
            if let Some(conv_key) = self.parse_session_key(&session_key) {
                let last_activity =
                    DateTime::from_timestamp(updated_at, 0).unwrap_or_else(Utc::now);
                let metadata = self.load_metadata_internal(&session_key)?;

                sessions.push(SessionState {
                    session_id: uuid::Uuid::new_v4(), // UUID not stored in group_chat_sessions
                    conversation_key: conv_key,
                    last_activity,
                    metadata,
                });
            }
        }

        Ok(sessions)
    }

    /// Clean up expired sessions.
    pub fn cleanup_expired(&self, channel_id: &str) -> Result<usize, SessionError> {
        let cutoff = Utc::now() - self.config.timeout();
        let cutoff_ts = cutoff.timestamp();

        let conn = self.db.conn.lock().unwrap_or_else(|e| e.into_inner());

        // Delete expired sessions
        let deleted = conn
            .execute(
                r#"
                DELETE FROM group_chat_sessions
                WHERE source_channel = ?1
                  AND status = 'active'
                  AND updated_at < ?2
                "#,
                [channel_id, cutoff_ts.to_string().as_str()],
            )
            .map_err(|e| SessionError::Database(e.to_string()))?;

        Ok(deleted)
    }

    // ------------------------------------------------------------------------
    // Internal helpers
    // ------------------------------------------------------------------------

    fn load_session_internal(
        &self,
        channel_id: &str,
        session_key: &str,
    ) -> Result<Option<SessionState>, SessionError> {
        let conn = self.db.conn.lock().unwrap_or_else(|e| e.into_inner());

        let mut stmt = conn
            .prepare(
                r#"
                SELECT id, created_at, updated_at
                FROM group_chat_sessions
                WHERE source_channel = ?1 AND source_session_key = ?2 AND status = 'active'
                "#,
            )
            .map_err(|e| SessionError::Database(e.to_string()))?;

        let result = stmt
            .query_row([channel_id, session_key], |row| {
                let created_at: i64 = row.get(1)?;
                let updated_at: i64 = row.get(2)?;
                Ok((created_at, updated_at))
            })
            .ok();

        drop(stmt);

        if let Some((_created_at, updated_at)) = result {
            let conversation_key = self
                .parse_session_key(session_key)
                .unwrap_or_else(|| ConversationKey::new(0));
            let last_activity = DateTime::from_timestamp(updated_at, 0).unwrap_or_else(Utc::now);
            let metadata = self.load_metadata_internal(session_key)?;

            Ok(Some(SessionState {
                session_id: uuid::Uuid::new_v4(),
                conversation_key,
                last_activity,
                metadata,
            }))
        } else {
            Ok(None)
        }
    }

    fn save_session(&self, channel_id: &str, session: &SessionState) -> Result<(), SessionError> {
        let session_key = session.conversation_key.to_conversation_id_string();
        let now = Utc::now().timestamp();
        let conn = self.db.conn.lock().unwrap_or_else(|e| e.into_inner());

        // Upsert session
        // topic field: None = no thread, Some(i64) = forum thread
        let topic_str = session.conversation_key.thread_id.map(|t| t.to_string());
        conn.execute(
            r#"
            INSERT INTO group_chat_sessions (id, topic, status, source_channel, source_session_key, created_at, updated_at)
            VALUES (?1, ?2, 'active', ?3, ?4, ?5, ?6)
            ON CONFLICT(id) DO UPDATE SET
                updated_at = ?6,
                topic = ?2
            "#,
            params![
                session_key.as_str(),
                topic_str.as_ref(),
                channel_id,
                session_key.as_str(),
                now.to_string(),
                now.to_string(),
            ],
        )
        .map_err(|e| SessionError::Database(e.to_string()))?;

        drop(conn);

        // Save metadata
        self.save_metadata_internal(&session_key, &session.metadata)?;

        Ok(())
    }

    fn load_metadata_internal(
        &self,
        session_key: &str,
    ) -> Result<HashMap<String, String>, SessionError> {
        let conn = self.db.conn.lock().unwrap_or_else(|e| e.into_inner());

        // Create table if not exists (idempotent)
        conn.execute(
            "CREATE TABLE IF NOT EXISTS telegram_session_metadata (
                session_id TEXT NOT NULL,
                key TEXT NOT NULL,
                value TEXT NOT NULL,
                PRIMARY KEY (session_id, key)
            )",
            [],
        )
        .ok(); // Ignore error if table already exists

        let mut stmt = conn
            .prepare("SELECT key, value FROM telegram_session_metadata WHERE session_id = ?1")
            .map_err(|e| SessionError::Database(e.to_string()))?;

        let rows = stmt
            .query_map([session_key], |row| {
                let k: String = row.get(0)?;
                let v: String = row.get(1)?;
                Ok((k, v))
            })
            .map_err(|e| SessionError::Database(e.to_string()))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| SessionError::Database(e.to_string()))?;

        Ok(rows.into_iter().collect())
    }

    fn save_metadata_internal(
        &self,
        session_key: &str,
        metadata: &HashMap<String, String>,
    ) -> Result<(), SessionError> {
        let conn = self.db.conn.lock().unwrap_or_else(|e| e.into_inner());

        // Create table if not exists (idempotent)
        conn.execute(
            "CREATE TABLE IF NOT EXISTS telegram_session_metadata (
                session_id TEXT NOT NULL,
                key TEXT NOT NULL,
                value TEXT NOT NULL,
                PRIMARY KEY (session_id, key)
            )",
            [],
        )
        .ok(); // Ignore error if table already exists

        // Delete existing metadata
        conn.execute(
            "DELETE FROM telegram_session_metadata WHERE session_id = ?1",
            [session_key],
        )
        .map_err(|e| SessionError::Database(e.to_string()))?;

        // Insert new metadata
        for (k, v) in metadata {
            conn.execute(
                "INSERT INTO telegram_session_metadata (session_id, key, value) VALUES (?1, ?2, ?3)",
                [session_key, k, v],
            )
            .map_err(|e| SessionError::Database(e.to_string()))?;
        }

        Ok(())
    }

    fn parse_session_key(&self, session_key: &str) -> Option<ConversationKey> {
        if let Some((chat_str, topic_str)) = session_key.split_once(":topic:") {
            let chat_id = chat_str.parse().ok()?;
            let thread_id = topic_str.parse().ok()?;
            Some(ConversationKey::with_thread(chat_id, thread_id))
        } else {
            let chat_id = session_key.parse().ok()?;
            Some(ConversationKey::new(chat_id))
        }
    }
}

/// Errors that can occur in session operations.
#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    #[error("database error: {0}")]
    Database(String),

    #[error("session not found")]
    NotFound,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_conversation_key_to_session_key() {
        let key = ConversationKey::new(-100123456789);
        assert_eq!(key.to_conversation_id_string(), "-100123456789");

        let key2 = ConversationKey::with_thread(-100123456789, 42);
        assert_eq!(key2.to_conversation_id_string(), "-100123456789:topic:42");
    }

    #[test]
    fn test_parse_session_key() {
        let manager = TelegramSessionManager {
            db: Arc::new(StateDatabase::in_memory().unwrap()),
            config: SessionConfig::default(),
        };

        let key = manager.parse_session_key("-100123456789").unwrap();
        assert_eq!(key.chat_id, -100123456789);
        assert_eq!(key.thread_id, None);

        let key2 = manager.parse_session_key("-100123456789:topic:42").unwrap();
        assert_eq!(key2.chat_id, -100123456789);
        assert_eq!(key2.thread_id, Some(42));
    }

    #[test]
    fn test_session_config_timeout() {
        let config = SessionConfig::default();
        assert_eq!(config.timeout_minutes, 30);
        assert_eq!(config.timeout(), Duration::from_secs(1800));

        let config2 = SessionConfig {
            timeout_minutes: 60,
        };
        assert_eq!(config2.timeout(), Duration::from_secs(3600));
    }
}
