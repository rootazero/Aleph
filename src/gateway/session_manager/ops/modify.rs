use rusqlite::{params, OptionalExtension};
use tracing::{debug, info};

use super::{SessionIdentityMeta, SessionManager, SessionManagerError, SessionPatch, SessionState};
use crate::gateway::router::SessionKey;

impl SessionManager {
    /// Compact a session by removing old messages
    pub async fn compact_session(&self, key: &SessionKey) -> Result<usize, SessionManagerError> {
        let key_str = key.to_key_string();
        let keep = self.config.compaction_keep as i64;

        let conn = self
            .conn
            .lock()
            .map_err(|e| SessionManagerError::DatabaseError(format!("Lock error: {e}")))?;

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
            // Sync FTS5: remove entries before deleting messages
            conn.execute(
                "DELETE FROM messages_fts WHERE rowid IN (SELECT id FROM messages WHERE session_key = ? AND id < ?)",
                params![&key_str, threshold],
            )
            .ok();

            let deleted = conn
                .execute(
                    "DELETE FROM messages WHERE session_key = ? AND id < ?",
                    params![&key_str, threshold],
                )
                .map_err(|e| SessionManagerError::DatabaseError(e.to_string()))?;

            let new_count: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM messages WHERE session_key = ?",
                    params![&key_str],
                    |row| row.get(0),
                )
                .unwrap_or(0);

            conn.execute(
                "UPDATE sessions SET message_count = ?, compaction_count = compaction_count + 1 WHERE key = ?",
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
            .map_err(|e| SessionManagerError::DatabaseError(format!("Lock error: {e}")))?;

        // Check current state - can't close if already stopped
        let current_state: Option<String> = conn
            .query_row(
                "SELECT state FROM sessions WHERE key = ?",
                params![&key_str],
                |row| row.get(0),
            )
            .ok();

        if current_state.as_deref() == Some("stopped") {
            return Ok(());
        }

        // Spec 1 G3-A: capture the conversation tail for end-of-session digest extraction.
        if let Some(writer) = self.raw_memory_writer.clone() {
            let agent_id = key.agent_id().to_string();
            let session_id = key_str.clone();
            let tail = self
                .read_recent_messages(&conn, &key_str, 64)
                .unwrap_or_default();
            super::emit_session_end_raw(
                writer,
                agent_id,
                session_id,
                tail,
                crate::memory::store::raw_memory::SessionEndReason::Disconnect,
            );
        }

        let existing_json: Option<String> = conn
            .query_row(
                "SELECT metadata FROM sessions WHERE key = ?",
                params![&key_str],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| SessionManagerError::DatabaseError(e.to_string()))?
            .flatten();

        let mut identity_meta = SessionIdentityMeta::from_json_str(existing_json.as_deref());
        identity_meta
            .custom
            .insert("status".to_string(), serde_json::json!("closed"));
        if let Some(t) = topic {
            identity_meta
                .custom
                .insert("topic".to_string(), serde_json::json!(t));
        }

        let meta_json = identity_meta
            .to_json_string()
            .map_err(|e| SessionManagerError::DatabaseError(e.to_string()))?;

        conn.execute(
            "UPDATE sessions SET metadata = ?, state = 'stopped' WHERE key = ?",
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
            .map_err(|e| SessionManagerError::DatabaseError(format!("Lock error: {e}")))?;

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

        let mut identity_meta = SessionIdentityMeta::from_json_str(existing_json.as_deref());
        identity_meta
            .custom
            .insert("topic".to_string(), serde_json::json!(topic));

        let meta_json = identity_meta
            .to_json_string()
            .map_err(|e| SessionManagerError::DatabaseError(e.to_string()))?;

        conn.execute(
            "UPDATE sessions SET metadata = ? WHERE key = ?",
            params![&meta_json, &key_str],
        )
        .map_err(|e| SessionManagerError::DatabaseError(e.to_string()))?;

        Ok(())
    }

    /// Persist the pinned flag into identity_meta.custom["pinned"].
    /// Mirrors `set_topic` exactly so it rides the proven metadata-JSON path.
    pub async fn set_pinned(
        &self,
        key: &SessionKey,
        pinned: bool,
    ) -> Result<(), SessionManagerError> {
        let key_str = key.to_key_string();
        let conn = self
            .conn
            .lock()
            .map_err(|e| SessionManagerError::DatabaseError(format!("Lock error: {e}")))?;

        let existing_json: Option<String> = conn
            .query_row(
                "SELECT metadata FROM sessions WHERE key = ?",
                params![&key_str],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| SessionManagerError::DatabaseError(e.to_string()))?
            .flatten();

        let mut identity_meta = SessionIdentityMeta::from_json_str(existing_json.as_deref());
        identity_meta
            .custom
            .insert("pinned".to_string(), serde_json::json!(pinned));

        let meta_json = identity_meta
            .to_json_string()
            .map_err(|e| SessionManagerError::DatabaseError(e.to_string()))?;

        conn.execute(
            "UPDATE sessions SET metadata = ? WHERE key = ?",
            params![&meta_json, &key_str],
        )
        .map_err(|e| SessionManagerError::DatabaseError(e.to_string()))?;

        Ok(())
    }

    /// Persist the run's project_root into identity_meta.custom["project_root"].
    pub async fn set_project_root(
        &self,
        key: &SessionKey,
        project_root: &str,
    ) -> Result<(), SessionManagerError> {
        let key_str = key.to_key_string();
        let conn = self
            .conn
            .lock()
            .map_err(|e| SessionManagerError::DatabaseError(format!("Lock error: {e}")))?;

        let existing_json: Option<String> = conn
            .query_row(
                "SELECT metadata FROM sessions WHERE key = ?",
                params![&key_str],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| SessionManagerError::DatabaseError(e.to_string()))?
            .flatten();

        let mut identity_meta = SessionIdentityMeta::from_json_str(existing_json.as_deref());
        identity_meta
            .custom
            .insert("project_root".to_string(), serde_json::json!(project_root));

        let meta_json = identity_meta
            .to_json_string()
            .map_err(|e| SessionManagerError::DatabaseError(e.to_string()))?;

        conn.execute(
            "UPDATE sessions SET metadata = ? WHERE key = ?",
            params![&meta_json, &key_str],
        )
        .map_err(|e| SessionManagerError::DatabaseError(e.to_string()))?;

        Ok(())
    }

    /// Record the originating channel of a session onto its identity metadata.
    ///
    /// Idempotent: only writes when the current `source_channel` is unset (empty
    /// or the `"unknown"` sentinel), so a session's real origin is never clobbered
    /// by a later continuation from a different surface. Called once on the first
    /// message of a session (see `execution_engine::execute`). Surfaced through
    /// `sessions.list` (`SessionInfo.channel`) and `SessionChangedEvent.channel`
    /// via `SessionMetadata::origin_channel`.
    pub async fn set_source_channel(
        &self,
        key: &SessionKey,
        channel: &str,
        conversation: Option<&str>,
    ) -> Result<(), SessionManagerError> {
        let channel = channel.trim();
        if channel.is_empty() || channel == "unknown" {
            return Ok(());
        }
        let key_str = key.to_key_string();
        let conn = self
            .conn
            .lock()
            .map_err(|e| SessionManagerError::DatabaseError(format!("Lock error: {e}")))?;

        let existing_json: Option<String> = conn
            .query_row(
                "SELECT metadata FROM sessions WHERE key = ?",
                params![&key_str],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| SessionManagerError::DatabaseError(e.to_string()))?
            .flatten();

        let mut identity_meta = SessionIdentityMeta::from_json_str(existing_json.as_deref());
        let current = identity_meta.source_channel.trim();
        // Never clobber an already-recorded real origin.
        if !current.is_empty() && current != "unknown" {
            return Ok(());
        }
        identity_meta.source_channel = channel.to_string();
        // Capture the origin conversation id (e.g. Telegram chat id) so a later
        // cross-surface continuation (sub-gap (b)) can fan the final reply back
        // to the originating channel.
        if let Some(conv) = conversation.map(str::trim).filter(|c| !c.is_empty()) {
            identity_meta.custom.insert(
                crate::gateway::session_store::types::ORIGIN_CONVERSATION_KEY.to_string(),
                serde_json::json!(conv),
            );
        }

        let meta_json = identity_meta
            .to_json_string()
            .map_err(|e| SessionManagerError::DatabaseError(e.to_string()))?;

        conn.execute(
            "UPDATE sessions SET metadata = ? WHERE key = ?",
            params![&meta_json, &key_str],
        )
        .map_err(|e| SessionManagerError::DatabaseError(e.to_string()))?;

        Ok(())
    }

    /// Trait-facing alias used by the `SessionStore` impl (the inherent method
    /// name is kept distinct from the trait method to avoid recursive
    /// trait-method resolution inside the impl).
    pub async fn stamp_source_channel(
        &self,
        key: &SessionKey,
        channel: &str,
        conversation: Option<&str>,
    ) -> Result<(), SessionManagerError> {
        self.set_source_channel(key, channel, conversation).await
    }

    /// Set session lifecycle state
    pub async fn set_state(
        &self,
        key: &SessionKey,
        state: SessionState,
    ) -> Result<(), SessionManagerError> {
        let key_str = key.to_key_string();
        let old_state = self.get_state(key).await.ok();

        let conn = self
            .conn
            .lock()
            .map_err(|e| SessionManagerError::DatabaseError(format!("Lock error: {e}")))?;

        conn.execute(
            "UPDATE sessions SET state = ? WHERE key = ?",
            params![state.to_string(), &key_str],
        )
        .map_err(|e| SessionManagerError::DatabaseError(format!("Set state failed: {e}")))?;

        drop(conn);

        debug!("Session {} transitioned to {}", key_str, state);
        self.emit_session_lifecycle_changed(&key_str, old_state.as_ref(), &state.to_string(), None);
        self.emit_session_updated(&key_str);
        Ok(())
    }

    /// Patch session metadata (label, status, model, `model_provider`, metadata JSON)
    pub async fn patch_session(
        &self,
        key: &SessionKey,
        patch: &SessionPatch,
    ) -> Result<bool, SessionManagerError> {
        let key_str = key.to_key_string();
        let conn = self
            .conn
            .lock()
            .map_err(|e| SessionManagerError::DatabaseError(format!("Lock error: {e}")))?;

        let existing_json: Option<String> = conn
            .query_row(
                "SELECT metadata FROM sessions WHERE key = ?",
                params![&key_str],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| SessionManagerError::DatabaseError(e.to_string()))?
            .flatten();

        let mut identity_meta = SessionIdentityMeta::from_json_str(existing_json.as_deref());

        if let Some(status) = &patch.status {
            identity_meta
                .custom
                .insert("status".to_string(), serde_json::json!(status));
        }
        if let Some(extra) = &patch.metadata {
            if let Some(extra_obj) = extra.as_object() {
                for (k, v) in extra_obj {
                    identity_meta.custom.insert(k.clone(), v.clone());
                }
            }
        }

        let meta_json = identity_meta
            .to_json_string()
            .map_err(|e| SessionManagerError::DatabaseError(e.to_string()))?;

        let mut sql = String::from("UPDATE sessions SET metadata = ?");
        let mut sql_params: Vec<&dyn rusqlite::ToSql> = vec![&meta_json];

        if let Some(label) = &patch.label {
            sql.push_str(", label = ?");
            sql_params.push(label);
        }
        if let Some(model) = &patch.model {
            sql.push_str(", model = ?");
            sql_params.push(model);
        }
        if let Some(provider) = &patch.model_provider {
            sql.push_str(", model_provider = ?");
            sql_params.push(provider);
        }

        sql.push_str(" WHERE key = ?");
        sql_params.push(&key_str as &dyn rusqlite::ToSql);

        let updated = conn
            .execute(&sql, sql_params.as_slice())
            .map_err(|e| SessionManagerError::DatabaseError(format!("Patch failed: {e}")))?;

        drop(conn);
        self.emit_session_updated(&key_str);
        Ok(updated > 0)
    }

    /// Update session usage statistics (token counts, model info)
    pub async fn update_session_usage(
        &self,
        key: &SessionKey,
        input_tokens: i64,
        output_tokens: i64,
        model: Option<&str>,
        model_provider: Option<&str>,
    ) -> Result<(), SessionManagerError> {
        let key_str = key.to_key_string();
        let conn = self
            .conn
            .lock()
            .map_err(|e| SessionManagerError::DatabaseError(format!("Lock error: {e}")))?;

        let total = input_tokens + output_tokens;
        let model_owned = model.map(|s| s.to_string());
        let provider_owned = model_provider.map(|s| s.to_string());

        let mut sql = String::from(
            "UPDATE sessions SET input_tokens = input_tokens + ?, output_tokens = output_tokens + ?, total_tokens = total_tokens + ?"
        );
        let mut params: Vec<&dyn rusqlite::ToSql> = vec![&input_tokens, &output_tokens, &total];

        if model_owned.is_some() {
            sql.push_str(", model = ?");
        }
        if provider_owned.is_some() {
            sql.push_str(", model_provider = ?");
        }
        sql.push_str(" WHERE key = ?");

        if let Some(ref m) = model_owned {
            params.push(m);
        }
        if let Some(ref mp) = provider_owned {
            params.push(mp);
        }
        params.push(&key_str);

        conn.execute(&sql, params.as_slice())
            .map_err(|e| SessionManagerError::DatabaseError(format!("Usage update failed: {e}")))?;

        drop(conn);
        self.emit_session_updated(&key_str);
        Ok(())
    }

    /// Transition session to error state with optional error message
    pub async fn set_error(
        &self,
        key: &SessionKey,
        _error_msg: Option<&str>,
    ) -> Result<(), SessionManagerError> {
        self.set_state(key, SessionState::Error).await
    }

    /// Transition session to stopped state (permanent)
    pub async fn stop(&self, key: &SessionKey) -> Result<(), SessionManagerError> {
        self.set_state(key, SessionState::Stopped).await
    }

    /// Transition session to idle state (after running completes)
    pub async fn set_idle(&self, key: &SessionKey) -> Result<(), SessionManagerError> {
        self.set_state(key, SessionState::Idle).await
    }

    /// Transition session to running state (agent loop active)
    pub async fn set_running(&self, key: &SessionKey) -> Result<(), SessionManagerError> {
        self.set_state(key, SessionState::Running).await
    }
}
