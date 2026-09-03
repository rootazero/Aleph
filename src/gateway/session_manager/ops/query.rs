use rusqlite::{params, OptionalExtension};

use super::{
    map_session_metadata, SessionIdentityMeta, SessionManager, SessionManagerError,
    SessionMetadata, SessionSearchResult, SessionState, SESSION_COLUMNS,
};
use crate::gateway::router::SessionKey;
use crate::gateway::session_store::types::{MessageRecord, SessionPreview};

impl SessionManager {
    /// List sessions, optionally filtered by agent
    pub async fn list_sessions(
        &self,
        agent_id: Option<&str>,
    ) -> Result<Vec<SessionMetadata>, SessionManagerError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| SessionManagerError::DatabaseError(format!("Lock error: {e}")))?;

        let sql = format!("SELECT {SESSION_COLUMNS} FROM sessions");

        let sessions = if let Some(id) = agent_id {
            let mut stmt = conn
                .prepare(&format!(
                    "{sql} WHERE agent_id = ? ORDER BY last_active_at DESC"
                ))
                .map_err(|e| SessionManagerError::DatabaseError(e.to_string()))?;

            let rows = stmt
                .query_map(params![id], map_session_metadata)
                .map_err(|e| SessionManagerError::DatabaseError(e.to_string()))?;

            rows.filter_map(|r| r.ok()).collect()
        } else {
            let mut stmt = conn
                .prepare(&format!("{sql} ORDER BY last_active_at DESC"))
                .map_err(|e| SessionManagerError::DatabaseError(e.to_string()))?;

            let rows = stmt
                .query_map([], map_session_metadata)
                .map_err(|e| SessionManagerError::DatabaseError(e.to_string()))?;

            rows.filter_map(|r| r.ok()).collect()
        };

        Ok(sessions)
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
            .map_err(|e| SessionManagerError::DatabaseError(format!("Lock error: {e}")))?;

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

        // Sanitize query: wrap in double-quotes to treat as literal phrase,
        // preventing FTS5 syntax errors from malformed user input.
        let sanitized_query = format!("\"{}\"", query.replace('"', ""));

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

        let results: Vec<SessionSearchResult> =
            match stmt.query_map(params![&sanitized_query, max_results as i64], |row| {
                let metadata_json: Option<String> = row.get(6)?;
                let identity_meta = SessionIdentityMeta::from_json_str(metadata_json.as_deref());
                let topic = identity_meta
                    .custom
                    .get("topic")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());

                Ok(SessionSearchResult {
                    message_id: row.get(0)?,
                    session_key: row.get(1)?,
                    role: row.get(2)?,
                    content: row.get(3)?,
                    timestamp: row.get(4)?,
                    agent_id: row.get(5)?,
                    topic,
                })
            }) {
                Ok(rows) => rows.filter_map(|r| r.ok()).collect(),
                Err(_) => {
                    // Graceful degradation: malformed FTS5 query returns empty results
                    return Ok(Vec::new());
                }
            };

        Ok(results)
    }

    /// Get a preview of the session (last N messages + summary metadata)
    pub async fn get_session_preview(
        &self,
        key: &SessionKey,
        message_limit: usize,
    ) -> Result<SessionPreview, SessionManagerError> {
        let key_str = key.to_key_string();
        let conn = self
            .conn
            .lock()
            .map_err(|e| SessionManagerError::DatabaseError(format!("Lock error: {e}")))?;

        let meta = conn
            .query_row(
                &format!("SELECT {SESSION_COLUMNS} FROM sessions WHERE key = ?"),
                params![&key_str],
                map_session_metadata,
            )
            .optional()
            .map_err(|e| SessionManagerError::DatabaseError(e.to_string()))?;

        let mut messages = Vec::new();
        if message_limit > 0 {
            // The seq anchor — the order the conversation happened, which is
            // the transcript's order everywhere
            // (`SessionStore::history_page`). The same spelling `history_sql`
            // uses, and for the same reason: two spellings of "the trailing N
            // rows" that order differently is a divergence between two views of
            // one conversation. This one was `ORDER BY id` alongside its twin,
            // and moving only the twin would have created exactly that.
            //
            // `source_seq` is not in this preview's column list and does not
            // need to be — the window reads it off the table, and the outer
            // projection drops it again.
            let sql = format!(
                "SELECT id, role, content, timestamp, metadata, input_tokens, output_tokens, \
                 tool_call_id, tool_name FROM ( \
                    SELECT * FROM ( \
                        SELECT id, role, content, timestamp, metadata, input_tokens, \
                        output_tokens, tool_call_id, tool_name, {anchor} AS anchor \
                        FROM messages WHERE session_key = ? \
                    ) ORDER BY anchor DESC, id DESC LIMIT ? \
                 ) ORDER BY anchor ASC, id ASC",
                anchor = crate::session::projection::TRANSCRIPT_ANCHOR_SQL,
            );
            let mut stmt = conn
                .prepare(&sql)
                .map_err(|e| SessionManagerError::DatabaseError(e.to_string()))?;
            messages = stmt
                .query_map(params![&key_str, message_limit as i64], |row| {
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
                        tool_call_id: row.get(7)?,
                        tool_name: row.get(8)?,
                    })
                })
                .map_err(|e| SessionManagerError::DatabaseError(e.to_string()))?
                .filter_map(|r| r.ok())
                .collect();
        }

        Ok(SessionPreview { meta, messages })
    }

    /// Read a session's cumulative `total_tokens` (input + output across its
    /// lifetime). Used by the autonomous-goal continuation hook to enforce a
    /// goal's token budget against the live session counter. Returns `None` when
    /// the session row does not exist yet. Mirrors `get_state`'s single-row
    /// pattern (synchronous `query_row` under the conn lock).
    pub async fn get_total_tokens(
        &self,
        key: &SessionKey,
    ) -> Result<Option<u64>, SessionManagerError> {
        let key_str = key.to_key_string();
        let conn = self
            .conn
            .lock()
            .map_err(|e| SessionManagerError::DatabaseError(format!("Lock error: {e}")))?;

        let total: Option<i64> = conn
            .query_row(
                "SELECT total_tokens FROM sessions WHERE key = ?",
                params![&key_str],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| SessionManagerError::DatabaseError(e.to_string()))?;

        // SQLite stores the counter as a signed INTEGER; clamp any negative
        // sentinel to 0 before widening to u64 (token counts are non-negative).
        Ok(total.map(|t| u64::try_from(t).unwrap_or(0)))
    }

    /// Get current session state
    pub async fn get_state(&self, key: &SessionKey) -> Result<SessionState, SessionManagerError> {
        let key_str = key.to_key_string();
        let conn = self
            .conn
            .lock()
            .map_err(|e| SessionManagerError::DatabaseError(format!("Lock error: {e}")))?;

        let state_str: Option<String> = conn
            .query_row(
                "SELECT state FROM sessions WHERE key = ?",
                params![&key_str],
                |row| row.get(0),
            )
            .ok();

        match state_str {
            Some(s) => match serde_json::from_str::<SessionState>(&format!("\"{s}\"")) {
                Ok(state) => Ok(state),
                Err(e) => {
                    tracing::warn!(
                        key = %key_str,
                        state = %s,
                        error = %e,
                        "Failed to parse session state, returning Created"
                    );
                    Ok(SessionState::Created)
                }
            },
            None => {
                tracing::debug!(
                    key = %key_str,
                    "Session state is NULL, returning Created"
                );
                Ok(SessionState::Created)
            }
        }
    }

    /// Count sessions by state
    pub async fn count_by_state(&self, state: SessionState) -> Result<usize, SessionManagerError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| SessionManagerError::DatabaseError(format!("Lock error: {e}")))?;

        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sessions WHERE state = ?",
                params![state.to_string()],
                |row| row.get(0),
            )
            .unwrap_or(0);

        Ok(count as usize)
    }

    /// List sessions filtered by state
    pub async fn list_by_state(
        &self,
        state: SessionState,
    ) -> Result<Vec<SessionMetadata>, SessionManagerError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| SessionManagerError::DatabaseError(format!("Lock error: {e}")))?;

        let mut stmt = conn
            .prepare(
                &format!("SELECT {SESSION_COLUMNS} FROM sessions WHERE state = ? ORDER BY last_active_at DESC"),
            )
            .map_err(|e| SessionManagerError::DatabaseError(e.to_string()))?;

        let rows = stmt
            .query_map(params![state.to_string()], map_session_metadata)
            .map_err(|e| SessionManagerError::DatabaseError(e.to_string()))?;

        Ok(rows.filter_map(|r| r.ok()).collect())
    }
}
