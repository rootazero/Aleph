use rusqlite::{params, OptionalExtension};
use tracing::info;

use super::{SessionIdentityMeta, SessionManager, SessionManagerError};
use crate::gateway::router::SessionKey;
use aleph_protocol::{GuestScope, IdentityContext, Role};

impl SessionManager {
    /// Get `IdentityContext` for a session
    ///
    /// Retrieves the session's identity metadata from the database and constructs
    /// an `IdentityContext`. Falls back to Owner identity if metadata is missing or invalid.
    pub async fn get_identity_context(
        &self,
        session_key: &str,
        source_channel: &str,
    ) -> Result<IdentityContext, SessionManagerError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| SessionManagerError::DatabaseError(format!("Lock error: {e}")))?;

        let identity_meta: SessionIdentityMeta = {
            let mut stmt = conn
                .prepare("SELECT metadata FROM sessions WHERE key = ?")
                .map_err(|e| SessionManagerError::DatabaseError(e.to_string()))?;

            let metadata_json: Option<String> = stmt
                .query_row(params![session_key], |row| row.get(0))
                .optional()
                .map_err(|e| SessionManagerError::DatabaseError(e.to_string()))?;

            metadata_json
                .and_then(|json| serde_json::from_str(&json).ok())
                .unwrap_or_else(|| SessionIdentityMeta::owner(source_channel))
        };

        let identity = match identity_meta.role {
            Role::Owner => {
                IdentityContext::owner(session_key.to_string(), identity_meta.source_channel)
            }
            Role::Guest => {
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
                // Anonymous is "always denied" (see aleph_protocol::IdentityContext);
                // mirror SessionIdentityMeta::to_identity_context instead of
                // escalating a stored Anonymous role to Owner.
                IdentityContext::anonymous(session_key.to_string(), identity_meta.source_channel)
            }
        };

        Ok(identity)
    }

    /// Read the most recent `limit` messages for a session from the DB connection.
    ///
    /// Returns a newline-separated string in chronological order (oldest first).
    /// Returns an empty string when the session has no messages yet.
    pub(super) fn read_recent_messages(
        &self,
        conn: &rusqlite::Connection,
        key_str: &str,
        limit: usize,
    ) -> Result<String, rusqlite::Error> {
        // `stamp_millis_sql`, not the bare column, and an `id` tiebreak: see
        // `SessionManager::stamp_millis_sql`. Ties had no deterministic order
        // before, so which of two same-second rows landed in the title sample
        // was up to the query planner.
        let stamp = Self::stamp_millis_sql();
        let mut stmt = conn.prepare(&format!(
            "SELECT role, content FROM messages
             WHERE session_key = ?
             ORDER BY {stamp} DESC, id DESC
             LIMIT ?"
        ))?;
        let rows = stmt
            .query_map(params![key_str, limit as i64], |row| {
                Ok(format!(
                    "{role}: {content}",
                    role = row.get::<_, String>(0)?,
                    content = row.get::<_, String>(1)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        let mut v = rows;
        v.reverse();
        Ok(v.join("\n"))
    }

    /// Get current epoch for a base key pattern (e.g. "agent:main:main")
    pub async fn get_current_epoch(
        &self,
        base_key_pattern: &str,
    ) -> Result<u32, SessionManagerError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| SessionManagerError::DatabaseError(format!("Lock error: {e}")))?;

        let like_pattern = format!("{base_key_pattern}%");
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

    /// Get the topic string from a session's metadata
    pub async fn get_session_topic(
        &self,
        key: &SessionKey,
    ) -> Result<Option<String>, SessionManagerError> {
        let key_str = key.to_key_string();
        let conn = self
            .conn
            .lock()
            .map_err(|e| SessionManagerError::DatabaseError(format!("Lock error: {e}")))?;

        let metadata_json: Option<String> = conn
            .query_row(
                "SELECT metadata FROM sessions WHERE key = ?",
                params![&key_str],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| SessionManagerError::DatabaseError(e.to_string()))?
            .flatten();

        let identity_meta = SessionIdentityMeta::from_json_str(metadata_json.as_deref());
        Ok(identity_meta
            .custom
            .get("topic")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()))
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
            .map_err(|e| SessionManagerError::DatabaseError(format!("Lock error: {e}")))?;

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
