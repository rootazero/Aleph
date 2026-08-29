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

        // MAX over every epoch this base has, not "the row created last".
        //
        // Two things were wrong with the previous
        // `LIKE '<base>%' ORDER BY created_at DESC LIMIT 1` + `rsplit(':')`:
        //
        // 1. The pattern was not anchored on a separator, so any base that is
        //    a string PREFIX of this one matched. With `dm_scope = per_peer`,
        //    peer `123`'s base `agent:main:dm:123` matched peer `1234`'s
        //    `agent:main:dm:1234:s2`, and `123`'s inbound messages were routed
        //    into an epoch it had never spoken in — a blank session, with its
        //    real conversation left unreachable.
        // 2. Newest-CREATED is not highest-epoch. Any path that materialises an
        //    older epoch's row later (import, reconcile, backfill) moved the
        //    answer backwards.
        //
        // The file backend already got both of these right; the parse is now
        // literally the same function
        // ([`SessionKey::epoch_after_base`]), so the two backends can no
        // longer answer this differently. LIKE stays only as a prefilter — its
        // `_`/`%` wildcards can only WIDEN the candidate set, and every
        // candidate is re-checked in Rust.
        let like_pattern = format!("{base_key_pattern}%");
        let mut stmt = conn
            .prepare("SELECT key FROM sessions WHERE key LIKE ?")
            .map_err(|e| SessionManagerError::DatabaseError(e.to_string()))?;
        let rows = stmt
            .query_map(params![&like_pattern], |row| row.get::<_, String>(0))
            .map_err(|e| SessionManagerError::DatabaseError(e.to_string()))?;

        let mut max_epoch = 0u32;
        for row in rows {
            let key_str = row.map_err(|e| SessionManagerError::DatabaseError(e.to_string()))?;
            // `:` only. A canonical key string always joins with `:`, and
            // accepting `_` here would misread a sibling whose `main_key`
            // legitimately contains one (`agent:main:main_s5`) as epoch 5 of
            // `agent:main:main`.
            if let Some(n) = SessionKey::epoch_after_base(base_key_pattern, &key_str, &[':']) {
                max_epoch = max_epoch.max(n);
            }
        }
        Ok(max_epoch)
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
