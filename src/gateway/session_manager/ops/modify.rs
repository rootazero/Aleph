use rusqlite::{params, OptionalExtension};
use tracing::{debug, info};

use super::{SessionIdentityMeta, SessionManager, SessionManagerError, SessionPatch, SessionState};
use crate::gateway::router::SessionKey;

impl SessionManager {
    /// Stamp the P1 attribution columns onto a row that has neither set.
    ///
    /// The `IS NULL AND IS NULL` guard lives in the statement, so this is safe
    /// to call on any key: a session that already has an owner is left exactly
    /// as it is and the call reports `false`. That is what keeps "session scope
    /// is immutable" true while still letting the legacy-room backfill exist —
    /// it writes only where there is nothing to overwrite.
    pub async fn backfill_attribution(
        &self,
        key: &SessionKey,
        owner_user_id: &str,
        scope_id: &str,
    ) -> Result<bool, SessionManagerError> {
        let key_str = key.to_key_string();
        let conn = self
            .conn
            .lock()
            .map_err(|e| SessionManagerError::DatabaseError(format!("Lock error: {e}")))?;
        let rows = conn
            .execute(
                "UPDATE sessions SET owner_user_id = ?2, scope_id = ?3
                 WHERE key = ?1 AND owner_user_id IS NULL AND scope_id IS NULL",
                params![&key_str, owner_user_id, scope_id],
            )
            .map_err(|e| SessionManagerError::DatabaseError(e.to_string()))?;
        Ok(rows > 0)
    }

    /// Compact a session by removing old messages
    pub async fn compact_session(&self, key: &SessionKey) -> Result<usize, SessionManagerError> {
        let key_str = key.to_key_string();
        let keep = self.config.compaction_keep as i64;

        let conn = self
            .conn
            .lock()
            .map_err(|e| SessionManagerError::DatabaseError(format!("Lock error: {e}")))?;

        // The rows to drop, named by the SAME ranking that decides which ones
        // are "old": everything past the newest `keep` in reader order.
        //
        // Not a threshold id plus `id < threshold`. That spelling asks the
        // ranking for a boundary and then applies it in a DIFFERENT order —
        // `id` is insert order — so the two agree only while the column is the
        // insert clock. It no longer is (`add_message_full` keeps the
        // producer's stamp), and one out-of-order projection is enough to make
        // "everything older than the boundary row" and "everything inserted
        // before it" name different sets: with rows `(id 1, t 300)`,
        // `(id 2, t 100)`, `(id 3, t 200)` and `keep = 2` the boundary lands on
        // id 2 and `id < 2` deletes id 1 — the NEWEST message in the session,
        // while the oldest survives. Silently, with a success return.
        //
        // Ordered by `id` — the order the rows were recorded, which is the
        // transcript's order on both backends (`SessionStore::history_page`).
        //
        // `LIMIT -1` is SQLite's "no limit"; `OFFSET` requires a `LIMIT` beside
        // it. The subquery is correlated to nothing — it re-states the
        // `session_key` predicate — so it is evaluated once.
        let doomed = "SELECT id FROM messages WHERE session_key = ?1
             ORDER BY id DESC LIMIT -1 OFFSET ?2";

        // Sync FTS5: remove entries before deleting messages
        conn.execute(
            &format!("DELETE FROM messages_fts WHERE rowid IN ({doomed})"),
            params![&key_str, keep],
        )
        .ok();

        let deleted = conn
            .execute(
                &format!("DELETE FROM messages WHERE session_key = ?1 AND id IN ({doomed})"),
                params![&key_str, keep],
            )
            .map_err(|e| SessionManagerError::DatabaseError(e.to_string()))?;

        if deleted == 0 {
            // Nothing was over the keep line. Reporting 0 without bumping
            // `compaction_count` is what the threshold spelling did when it
            // found no boundary row to cut at.
            return Ok(0);
        }

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

        Ok(deleted)
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
            // Review fix: the session-end reflector (spawned inside
            // `emit_session_end_raw`) needs this session's P1 scope columns
            // to write OPEN_LOOPS.md through the same composed id the
            // curated-envelope reader resolves — the ambient task-local
            // scope is not reliably live on this session-close path, so
            // derive it from the persisted row instead (a `.ok()` miss on a
            // legacy/pre-P1 row degrades to `(None, None)`, the pre-P1
            // unscoped behaviour).
            let (owner_user_id, scope_id): (Option<String>, Option<String>) = conn
                .query_row(
                    "SELECT owner_user_id, scope_id FROM sessions WHERE key = ?",
                    params![&key_str],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .unwrap_or((None, None));
            super::emit_session_end_raw(
                writer,
                agent_id,
                session_id,
                tail,
                crate::memory::store::raw_memory::SessionEndReason::Disconnect,
                owner_user_id,
                scope_id,
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

        // The scratchpad binding table is process-global and mirrored
        // write-through to disk on every scratchpad tool call, but nothing
        // ever pruned it: a binding left by an epoch that has since been
        // superseded can never be addressed again, yet is rewritten forever.
        // Closing epoch N is the moment epochs < N are known to be dead.
        crate::builtin_tools::scratchpad_registry::clear_superseded_epochs(
            &key.base_key_pattern(),
            key.epoch(),
        );

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

    /// Persist (or clear) the user-chosen project working directory for a
    /// session onto its identity metadata (`custom["project_root"]`). Mirrors
    /// [`Self::set_topic`]'s metadata-merge pattern so no schema column is added.
    ///
    /// `Some(path)` stores the absolute project folder; `None` removes the key
    /// (revert to the default `~/.aleph/workspaces/{agent_id}` workspace). The
    /// value is surfaced through `sessions.list` (`SessionListRow.project_root`) so
    /// the Panel can restore the active project after a reload.
    pub async fn set_project_root(
        &self,
        key: &SessionKey,
        project_root: Option<&str>,
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
        match project_root.map(str::trim).filter(|p| !p.is_empty()) {
            Some(path) => {
                identity_meta
                    .custom
                    .insert("project_root".to_string(), serde_json::json!(path));
            }
            None => {
                identity_meta.custom.remove("project_root");
            }
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

    /// Record the originating channel of a session onto its identity metadata.
    ///
    /// Idempotent: only writes when the current `source_channel` is unset (empty
    /// or the `"unknown"` sentinel), so a session's real origin is never clobbered
    /// by a later continuation from a different surface. Called once on the first
    /// message of a session (see `execution_engine::execute`). Surfaced through
    /// `sessions.list` (`SessionListRow.channel`) and `SessionChangedEvent.channel`
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

    /// Accumulate one run's usage onto the session row: tokens, cost, and the
    /// model that served it.
    ///
    /// `cost_usd` is this run's estimate; pass 0.0 when the run could not be
    /// priced (an unknown provider/model). The column then stays honest-but-low
    /// rather than being poisoned by a fabricated number.
    ///
    /// Until this had a production caller, the session's token columns were
    /// permanently 0 and `estimated_cost_usd` had no column at all — while both
    /// were surfaced to the model (the `sessions` tool) and to the Panel as
    /// facts. The caller is `session_projector`'s `AssistantRunMeta` arm, whose
    /// watermark suppression is what makes this accumulation idempotent under
    /// the reconciler's replay.
    pub async fn update_session_usage(
        &self,
        key: &SessionKey,
        input_tokens: i64,
        output_tokens: i64,
        cost_usd: f64,
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
            "UPDATE sessions SET input_tokens = input_tokens + ?, output_tokens = output_tokens + ?, total_tokens = total_tokens + ?, estimated_cost_usd = estimated_cost_usd + ?"
        );
        let mut params: Vec<&dyn rusqlite::ToSql> =
            vec![&input_tokens, &output_tokens, &total, &cost_usd];

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
}
