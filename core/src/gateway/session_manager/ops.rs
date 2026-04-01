//! Session Manager operations: CRUD, query, compaction, and cleanup methods.

use rusqlite::{params, OptionalExtension};
use tracing::{debug, info};

use super::{
    session_type_str, SessionIdentityMeta, SessionManager, SessionManagerError, SessionMetadata,
    SessionSearchResult, StoredMessage,
};
use crate::gateway::router::SessionKey;
use aleph_protocol::{GuestScope, IdentityContext, Role};

impl SessionManager {
    /// Get or create a session
    pub async fn get_or_create(
        &self,
        key: &SessionKey,
    ) -> Result<SessionMetadata, SessionManagerError> {
        let key_str = key.to_key_string();
        let agent_id = key.agent_id().to_string();
        let session_type = session_type_str(key);
        let now = chrono::Utc::now().timestamp();

        let conn = self
            .conn
            .lock()
            .map_err(|e| SessionManagerError::DatabaseError(format!("Lock error: {}", e)))?;

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
                        metadata_json: None,
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
            metadata_json: None,
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
            let conn = self
                .conn
                .lock()
                .map_err(|e| SessionManagerError::DatabaseError(format!("Lock error: {}", e)))?;

            // Insert message
            conn.execute(
                "INSERT INTO messages (session_key, role, content, timestamp) VALUES (?, ?, ?, ?)",
                params![&key_str, role, content, now],
            )
            .map_err(|e| {
                SessionManagerError::DatabaseError(format!("Insert message failed: {}", e))
            })?;

            let message_id = conn.last_insert_rowid();

            // Sync FTS5 index (non-fatal — search degrades gracefully)
            conn.execute(
                "INSERT INTO messages_fts(rowid, content) VALUES (?, ?)",
                params![message_id, content],
            )
            .ok();

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
        let conn = self
            .conn
            .lock()
            .map_err(|e| SessionManagerError::DatabaseError(format!("Lock error: {}", e)))?;

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
        let conn = self
            .conn
            .lock()
            .map_err(|e| SessionManagerError::DatabaseError(format!("Lock error: {}", e)))?;

        let deleted = conn
            .execute(
                "DELETE FROM messages WHERE session_key = ?",
                params![&key_str],
            )
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
        let conn = self
            .conn
            .lock()
            .map_err(|e| SessionManagerError::DatabaseError(format!("Lock error: {}", e)))?;

        // Delete messages first
        conn.execute(
            "DELETE FROM messages WHERE session_key = ?",
            params![&key_str],
        )
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
        let conn = self
            .conn
            .lock()
            .map_err(|e| SessionManagerError::DatabaseError(format!("Lock error: {}", e)))?;

        let sessions = if let Some(id) = agent_id {
            let mut stmt = conn
                .prepare(
                    "SELECT key, agent_id, session_type, created_at, last_active_at,
                            message_count, total_tokens, auto_reset_at, metadata
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
                        metadata_json: row.get(8)?,
                    })
                })
                .map_err(|e| SessionManagerError::DatabaseError(e.to_string()))?;

            rows.filter_map(|r| r.ok()).collect()
        } else {
            let mut stmt = conn
                .prepare(
                    "SELECT key, agent_id, session_type, created_at, last_active_at,
                            message_count, total_tokens, auto_reset_at, metadata
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
                        metadata_json: row.get(8)?,
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

        let conn = self
            .conn
            .lock()
            .map_err(|e| SessionManagerError::DatabaseError(format!("Lock error: {}", e)))?;

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

            info!(
                "Compacted session {}: removed {} messages",
                key_str, deleted
            );

            return Ok(deleted);
        }

        Ok(0)
    }

    /// Get IdentityContext for a session
    ///
    /// Retrieves the session's identity metadata from the database and constructs
    /// an IdentityContext. Falls back to Owner identity if metadata is missing or invalid.
    pub async fn get_identity_context(
        &self,
        session_key: &str,
        source_channel: &str,
    ) -> Result<IdentityContext, SessionManagerError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| SessionManagerError::DatabaseError(format!("Lock error: {}", e)))?;

        // Query session metadata
        let metadata_json: Option<String> = {
            let mut stmt = conn
                .prepare("SELECT metadata FROM sessions WHERE key = ?")
                .map_err(|e| SessionManagerError::DatabaseError(e.to_string()))?;

            stmt.query_row(params![session_key], |row| row.get(0))
                .optional()
                .map_err(|e| SessionManagerError::DatabaseError(e.to_string()))?
        };

        // Parse metadata or use default (Owner)
        let identity_meta: SessionIdentityMeta = metadata_json
            .and_then(|json| serde_json::from_str(&json).ok())
            .unwrap_or_else(|| SessionIdentityMeta::owner(source_channel));

        // Construct IdentityContext
        let identity = match identity_meta.role {
            Role::Owner => {
                IdentityContext::owner(session_key.to_string(), identity_meta.source_channel)
            }
            Role::Guest => {
                // Guest must have a scope; if missing, create minimal scope
                let scope = identity_meta.scope.unwrap_or_else(|| GuestScope {
                    allowed_tools: vec![],
                    expires_at: None,
                    display_name: None,
                });
                IdentityContext::guest(
                    session_key.to_string(),
                    identity_meta.identity_id,
                    scope,
                    identity_meta.source_channel,
                )
            }
            Role::Anonymous => {
                // Anonymous sessions are treated as Owner for backward compatibility
                IdentityContext::owner(session_key.to_string(), identity_meta.source_channel)
            }
        };

        Ok(identity)
    }

    /// Close a session: set status=closed, store topic in metadata JSON
    pub async fn close_session(
        &self,
        key: &SessionKey,
        topic: Option<String>,
    ) -> Result<(), SessionManagerError> {
        let key_str = key.to_key_string();
        let conn = self
            .conn
            .lock()
            .map_err(|e| SessionManagerError::DatabaseError(format!("Lock error: {}", e)))?;

        // Get existing metadata
        let existing_json: Option<String> = conn
            .query_row(
                "SELECT metadata FROM sessions WHERE key = ?",
                params![&key_str],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| SessionManagerError::DatabaseError(e.to_string()))?
            .flatten();

        let mut meta: serde_json::Value = existing_json
            .as_deref()
            .and_then(|s| serde_json::from_str(s).ok())
            .unwrap_or_else(|| serde_json::json!({}));

        if let Some(obj) = meta.as_object_mut() {
            obj.insert("status".to_string(), serde_json::json!("closed"));
            if let Some(t) = &topic {
                obj.insert("topic".to_string(), serde_json::json!(t));
            }
        }

        let meta_json = serde_json::to_string(&meta)
            .map_err(|e| SessionManagerError::DatabaseError(e.to_string()))?;

        conn.execute(
            "UPDATE sessions SET metadata = ? WHERE key = ?",
            params![&meta_json, &key_str],
        )
        .map_err(|e| SessionManagerError::DatabaseError(e.to_string()))?;

        Ok(())
    }

    /// Set the topic for a session without closing it.
    /// Used by async topic generation on first message.
    pub async fn set_topic(
        &self,
        key: &SessionKey,
        topic: &str,
    ) -> Result<(), SessionManagerError> {
        let key_str = key.to_key_string();
        let conn = self
            .conn
            .lock()
            .map_err(|e| SessionManagerError::DatabaseError(format!("Lock error: {}", e)))?;

        // Get existing metadata
        let existing_json: Option<String> = conn
            .query_row(
                "SELECT metadata FROM sessions WHERE key = ?",
                params![&key_str],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| SessionManagerError::DatabaseError(e.to_string()))?
            .flatten();

        let mut meta: serde_json::Value = existing_json
            .as_deref()
            .and_then(|s| serde_json::from_str(s).ok())
            .unwrap_or_else(|| serde_json::json!({}));

        if let Some(obj) = meta.as_object_mut() {
            obj.insert("topic".to_string(), serde_json::json!(topic));
        }

        let meta_json = serde_json::to_string(&meta)
            .map_err(|e| SessionManagerError::DatabaseError(e.to_string()))?;

        conn.execute(
            "UPDATE sessions SET metadata = ? WHERE key = ?",
            params![&meta_json, &key_str],
        )
        .map_err(|e| SessionManagerError::DatabaseError(e.to_string()))?;

        Ok(())
    }

    /// Get current epoch for a base key pattern (e.g. "agent:main:main")
    pub async fn get_current_epoch(
        &self,
        base_key_pattern: &str,
    ) -> Result<u32, SessionManagerError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| SessionManagerError::DatabaseError(format!("Lock error: {}", e)))?;

        let like_pattern = format!("{}%", base_key_pattern);
        let latest_key: Option<String> = conn
            .query_row(
                "SELECT key FROM sessions WHERE key LIKE ? ORDER BY created_at DESC LIMIT 1",
                params![&like_pattern],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| SessionManagerError::DatabaseError(e.to_string()))?;

        match latest_key {
            Some(key_str) => {
                if let Some(suffix) = key_str.rsplit(':').next() {
                    if let Some(n_str) = suffix.strip_prefix('s') {
                        if let Ok(n) = n_str.parse::<u32>() {
                            return Ok(n);
                        }
                    }
                }
                Ok(0)
            }
            None => Ok(0),
        }
    }

    /// Get the topic string from a session's metadata JSON
    pub async fn get_session_topic(
        &self,
        key: &SessionKey,
    ) -> Result<Option<String>, SessionManagerError> {
        let key_str = key.to_key_string();
        let conn = self
            .conn
            .lock()
            .map_err(|e| SessionManagerError::DatabaseError(format!("Lock error: {}", e)))?;

        let metadata_json: Option<String> = conn
            .query_row(
                "SELECT metadata FROM sessions WHERE key = ?",
                params![&key_str],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| SessionManagerError::DatabaseError(e.to_string()))?
            .flatten();

        Ok(metadata_json
            .and_then(|json| serde_json::from_str::<serde_json::Value>(&json).ok())
            .and_then(|v| v.get("topic")?.as_str().map(|s| s.to_string())))
    }

    /// Cleanup expired sessions
    pub async fn cleanup_expired(&self) -> Result<usize, SessionManagerError> {
        if self.config.session_expiry_secs == 0 {
            return Ok(0);
        }

        let expiry_threshold =
            chrono::Utc::now().timestamp() - self.config.session_expiry_secs as i64;

        let conn = self
            .conn
            .lock()
            .map_err(|e| SessionManagerError::DatabaseError(format!("Lock error: {}", e)))?;

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

    /// Search messages across all sessions using FTS5 full-text search.
    pub async fn search_messages(
        &self,
        query: &str,
        max_results: usize,
    ) -> Result<Vec<SessionSearchResult>, SessionManagerError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| SessionManagerError::DatabaseError(format!("Lock error: {}", e)))?;

        // Graceful degradation for old databases without FTS5
        let fts_exists: bool = conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='messages_fts'",
                [],
                |row| row.get(0),
            )
            .unwrap_or(false);

        if !fts_exists {
            return Ok(Vec::new());
        }

        let mut stmt = conn
            .prepare(
                "SELECT m.id, m.session_key, m.role, m.content, m.timestamp,
                        s.agent_id, s.metadata
                 FROM messages_fts f
                 JOIN messages m ON m.id = f.rowid
                 JOIN sessions s ON s.key = m.session_key
                 WHERE messages_fts MATCH ?
                 ORDER BY rank
                 LIMIT ?",
            )
            .map_err(|e| SessionManagerError::DatabaseError(e.to_string()))?;

        let results: Vec<SessionSearchResult> = stmt
            .query_map(params![query, max_results as i64], |row| {
                let metadata_json: Option<String> = row.get(6)?;
                let topic = metadata_json
                    .and_then(|json| serde_json::from_str::<serde_json::Value>(&json).ok())
                    .and_then(|v| v.get("topic")?.as_str().map(|s| s.to_string()));

                Ok(SessionSearchResult {
                    message_id: row.get(0)?,
                    session_key: row.get(1)?,
                    role: row.get(2)?,
                    content: row.get(3)?,
                    timestamp: row.get(4)?,
                    agent_id: row.get(5)?,
                    topic,
                })
            })
            .map_err(|e| SessionManagerError::DatabaseError(e.to_string()))?
            .filter_map(|r| r.ok())
            .collect();

        Ok(results)
    }
}
