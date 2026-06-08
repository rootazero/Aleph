use rusqlite::params;
use tracing::debug;

use super::{
    map_session_metadata, SessionManager, SessionManagerError, SessionMetadata, SessionState,
};
use crate::gateway::router::SessionKey;
use crate::gateway::session_store::types::MessageRecord;

impl SessionManager {
    /// Get or create a session
    pub async fn get_or_create(
        &self,
        key: &SessionKey,
    ) -> Result<SessionMetadata, SessionManagerError> {
        let key_str = key.to_key_string();
        let agent_id = key.agent_id().to_string();
        let session_type = super::session_type_str(key);
        let now = chrono::Utc::now().timestamp();

        let conn = self
            .conn
            .lock()
            .map_err(|e| SessionManagerError::DatabaseError(format!("Lock error: {}", e)))?;

        // Try to get existing session
        let existing: Option<SessionMetadata> = conn
            .query_row(
                "SELECT key, agent_id, session_type, created_at, last_active_at,
                        message_count, total_tokens, auto_reset_at, state, metadata,
                        label, input_tokens, output_tokens, model, model_provider,
                        parent_session_key, compaction_count, derived_title
                 FROM sessions WHERE key = ?",
                params![&key_str],
                map_session_metadata,
            )
            .ok();

        if let Some(mut meta) = existing {
            // Transition Created or Idle -> Active
            let state_update = conn.execute(
                "UPDATE sessions SET last_active_at = ?, state = 'active' WHERE key = ? AND state IN ('created', 'idle')",
                params![now, &key_str],
            );
            if state_update.map(|n| n == 0).unwrap_or(false) {
                // State was not 'created' or 'idle' (e.g., already 'active' or 'running')
                // Just update last_active_at
                conn.execute(
                    "UPDATE sessions SET last_active_at = ? WHERE key = ?",
                    params![now, &key_str],
                )
                .ok();
            } else {
                // State was updated to 'active' - reflect this in returned metadata
                meta.state = Some(SessionState::Active);
            }

            return Ok(meta);
        }

        // Create new session
        conn.execute(
            "INSERT INTO sessions (key, agent_id, session_type, created_at, last_active_at, state)
             VALUES (?, ?, ?, ?, ?, 'created')",
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
            state: Some(SessionState::Created),
            topic: None,
            status: None,
            identity_meta: None,
            label: None,
            input_tokens: 0,
            output_tokens: 0,
            model: None,
            model_provider: None,
            parent_session_key: None,
            compaction_count: 0,
            ..Default::default()
        })
    }

    /// Add a message to a session
    #[allow(deprecated)]
    pub async fn add_message(
        &self,
        key: &SessionKey,
        role: &str,
        content: &str,
    ) -> Result<i64, SessionManagerError> {
        self.add_message_with_meta(key, role, content, None, 0, 0, None, None)
            .await
    }

    /// Add a message to a session with optional metadata and token tracking.
    #[deprecated(note = "SQLite messages table is legacy; new code should use FileSessionStore")]
    #[allow(clippy::too_many_arguments)]
    pub async fn add_message_with_meta(
        &self,
        key: &SessionKey,
        role: &str,
        content: &str,
        metadata: Option<&str>,
        input_tokens: i64,
        output_tokens: i64,
        model: Option<&str>,
        model_provider: Option<&str>,
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
                "INSERT INTO messages (session_key, role, content, timestamp, metadata, input_tokens, output_tokens)
                 VALUES (?, ?, ?, ?, ?, ?, ?)",
                params![&key_str, role, content, now, metadata, input_tokens, output_tokens],
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

            // Only transition to Running if current state allows it
            let valid_transition: bool = conn
                .query_row(
                    "SELECT state FROM sessions WHERE key = ?",
                    params![&key_str],
                    |row| {
                        let state_str: Option<String> = row.get(0)?;
                        Ok(state_str
                            .map(|s| s != "stopped" && s != "error")
                            .unwrap_or(true))
                    },
                )
                .unwrap_or(true);

            let total_delta = input_tokens + output_tokens;
            let model_owned = model.map(|s| s.to_string());
            let provider_owned = model_provider.map(|s| s.to_string());

            let derived_title: Option<String> = if role == "user" {
                conn.query_row(
                    "SELECT derived_title FROM sessions WHERE key = ?",
                    params![&key_str],
                    |row| row.get::<_, Option<String>>(0),
                )
                .ok()
                .flatten()
                .or_else(|| {
                    let title = content.trim();
                    let title = if title.chars().count() > 60 {
                        title.chars().take(60).collect::<String>() + "..."
                    } else {
                        title.to_string()
                    };
                    if title.is_empty() {
                        None
                    } else {
                        Some(title)
                    }
                })
            } else {
                None
            };

            let mut session_update_sql = String::from(
                "UPDATE sessions SET last_active_at = ?, message_count = message_count + 1, input_tokens = input_tokens + ?, output_tokens = output_tokens + ?, total_tokens = total_tokens + ?"
            );
            if valid_transition {
                session_update_sql.push_str(", state = 'running'");
            }
            if model_owned.is_some() {
                session_update_sql.push_str(", model = ?");
            }
            if provider_owned.is_some() {
                session_update_sql.push_str(", model_provider = ?");
            }
            if derived_title.is_some() {
                session_update_sql.push_str(", derived_title = ?");
            }
            session_update_sql.push_str(" WHERE key = ?");

            let mut params: Vec<&dyn rusqlite::ToSql> =
                vec![&now, &input_tokens, &output_tokens, &total_delta];
            if let Some(ref m) = model_owned {
                params.push(m);
            }
            if let Some(ref mp) = provider_owned {
                params.push(mp);
            }
            if let Some(ref dt) = derived_title {
                params.push(dt);
            }
            params.push(&key_str);
            conn.execute(&session_update_sql, params.as_slice()).ok();

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

        self.emit_session_updated(&key_str);

        // Phase 1 dual-write shim: mirror the SQLite write into the
        // append-only session_events log. Errors are logged inside the
        // helper and never propagate — the legacy messages table stays
        // the source of truth until Phase 6.
        if let Some(svc) = self.session_service.as_ref() {
            crate::session::shim::mirror_message_by_role(svc, key, role, content).await;
        }

        Ok(message_id)
    }

    /// Get session history
    pub async fn get_history(
        &self,
        key: &SessionKey,
        limit: Option<usize>,
    ) -> Result<Vec<MessageRecord>, SessionManagerError> {
        let key_str = key.to_key_string();
        let conn = self
            .conn
            .lock()
            .map_err(|e| SessionManagerError::DatabaseError(format!("Lock error: {}", e)))?;

        let query = match limit {
            Some(n) => format!(
                "SELECT id, role, content, timestamp, metadata, input_tokens, output_tokens FROM (
                    SELECT id, role, content, timestamp, metadata, input_tokens, output_tokens
                    FROM messages
                    WHERE session_key = ? ORDER BY timestamp DESC LIMIT {}
                ) ORDER BY timestamp ASC",
                n
            ),
            None => "SELECT id, role, content, timestamp, metadata, input_tokens, output_tokens FROM messages
                     WHERE session_key = ? ORDER BY timestamp ASC"
                .to_string(),
        };

        let mut stmt = conn
            .prepare(&query)
            .map_err(|e| SessionManagerError::DatabaseError(e.to_string()))?;

        let messages: Vec<MessageRecord> = stmt
            .query_map(params![&key_str], |row| {
                Ok(MessageRecord {
                    id: row.get::<_, i64>(0)?.to_string(),
                    role: row.get(1)?,
                    content: row.get(2)?,
                    timestamp: row.get(3)?,
                    metadata: row
                        .get::<_, Option<String>>(4)?
                        .and_then(|s| serde_json::from_str(&s).ok()),
                    input_tokens: row.get(5)?,
                    output_tokens: row.get(6)?,
                    model: None,
                    model_provider: None,
                })
            })
            .map_err(|e| SessionManagerError::DatabaseError(e.to_string()))?
            .filter_map(|r| r.ok())
            .collect();

        Ok(messages)
    }

    /// History with an optional `before` cursor — the efficient, SQL-side
    /// implementation of [`SessionStore::get_history_before`]. Only messages
    /// with `timestamp < before` are scanned, then the most recent `limit` of
    /// those are returned oldest-first (same windowing shape as [`get_history`]).
    ///
    /// When `before` is `None` this is exactly [`get_history`], so the plain
    /// path stays the single source of SQL truth for the non-paginated case.
    ///
    /// [`SessionStore::get_history_before`]: crate::gateway::session_store::SessionStore::get_history_before
    /// [`get_history`]: Self::get_history
    pub async fn get_history_before(
        &self,
        key: &SessionKey,
        limit: Option<usize>,
        before: Option<i64>,
    ) -> Result<Vec<MessageRecord>, SessionManagerError> {
        // No cursor → reuse the plain path; avoids a second SQL variant.
        let Some(before_ts) = before else {
            return self.get_history(key, limit).await;
        };

        let key_str = key.to_key_string();
        let conn = self
            .conn
            .lock()
            .map_err(|e| SessionManagerError::DatabaseError(format!("Lock error: {}", e)))?;

        // Mirror `get_history`'s windowing: take the most-recent `limit` rows
        // that satisfy the cursor (inner DESC LIMIT), then re-sort ASC for
        // chronological display.
        let query = match limit {
            Some(n) => format!(
                "SELECT id, role, content, timestamp, metadata, input_tokens, output_tokens FROM (
                    SELECT id, role, content, timestamp, metadata, input_tokens, output_tokens
                    FROM messages
                    WHERE session_key = ? AND timestamp < ? ORDER BY timestamp DESC LIMIT {}
                ) ORDER BY timestamp ASC",
                n
            ),
            None => "SELECT id, role, content, timestamp, metadata, input_tokens, output_tokens FROM messages
                     WHERE session_key = ? AND timestamp < ? ORDER BY timestamp ASC"
                .to_string(),
        };

        let mut stmt = conn
            .prepare(&query)
            .map_err(|e| SessionManagerError::DatabaseError(e.to_string()))?;

        let messages: Vec<MessageRecord> = stmt
            .query_map(params![&key_str, before_ts], |row| {
                Ok(MessageRecord {
                    id: row.get::<_, i64>(0)?.to_string(),
                    role: row.get(1)?,
                    content: row.get(2)?,
                    timestamp: row.get(3)?,
                    metadata: row
                        .get::<_, Option<String>>(4)?
                        .and_then(|s| serde_json::from_str(&s).ok()),
                    input_tokens: row.get(5)?,
                    output_tokens: row.get(6)?,
                    model: None,
                    model_provider: None,
                })
            })
            .map_err(|e| SessionManagerError::DatabaseError(e.to_string()))?
            .filter_map(|r| r.ok())
            .collect();

        Ok(messages)
    }

    /// Reset (clear) a session
    pub async fn reset_session(&self, key: &SessionKey) -> Result<bool, SessionManagerError> {
        let key_str = key.to_key_string();
        let conn = self
            .conn
            .lock()
            .map_err(|e| SessionManagerError::DatabaseError(format!("Lock error: {}", e)))?;

        // Sync FTS5: remove entries before deleting messages
        conn.execute(
            "DELETE FROM messages_fts WHERE rowid IN (SELECT id FROM messages WHERE session_key = ?)",
            params![&key_str],
        )
        .ok();

        let deleted = conn
            .execute(
                "DELETE FROM messages WHERE session_key = ?",
                params![&key_str],
            )
            .map_err(|e| SessionManagerError::DatabaseError(e.to_string()))?;

        conn.execute(
            "UPDATE sessions SET message_count = 0, last_active_at = ?, state = 'created' WHERE key = ?",
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

        // Sync FTS5: remove entries before deleting messages
        conn.execute(
            "DELETE FROM messages_fts WHERE rowid IN (SELECT id FROM messages WHERE session_key = ?)",
            params![&key_str],
        )
        .ok();

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
}
