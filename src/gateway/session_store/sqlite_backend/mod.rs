use async_trait::async_trait;
use rusqlite::{params, OptionalExtension};

use crate::gateway::router::SessionKey;
use crate::gateway::session_manager::{SessionManager, SessionManagerConfig, SessionState};
use crate::gateway::session_store::error::SessionStoreError;
use crate::gateway::session_store::types::{
    CheckpointSummary, DeleteResult, MessageRecord, SearchHit, SessionFilter, SessionMetadata,
    SessionPatch, SessionPreview, TruncateResult,
};
use crate::gateway::session_store::SessionStore;

pub type SqliteSessionStore = SessionManager;
pub type SqliteSessionStoreConfig = SessionManagerConfig;

fn map_err(e: crate::gateway::session_manager::SessionManagerError) -> SessionStoreError {
    match e {
        crate::gateway::session_manager::SessionManagerError::DatabaseError(msg) => {
            SessionStoreError::DatabaseError(msg)
        }
        crate::gateway::session_manager::SessionManagerError::NotFound(msg) => {
            SessionStoreError::NotFound(msg)
        }
        // A stopped session surfaces through the store as `Stopped` — the
        // caller must explicitly reopen rather than retry the same call.
        crate::gateway::session_manager::SessionManagerError::SessionStopped(msg) => {
            SessionStoreError::Stopped(msg)
        }
    }
}

fn search_result_to_hit(r: crate::gateway::session_manager::SessionSearchResult) -> SearchHit {
    SearchHit {
        session_key: r.session_key,
        agent_id: r.agent_id,
        role: r.role,
        content: r.content,
        timestamp: r.timestamp,
        topic: r.topic,
    }
}

fn map_session_metadata(
    row: &rusqlite::Row,
) -> Result<crate::gateway::session_store::types::SessionMetadata, rusqlite::Error> {
    let state_str: Option<String> = row.get(8)?;
    let state = state_str
        .and_then(|s| serde_json::from_str(&format!("\"{s}\"")).ok())
        .unwrap_or_default();
    let metadata_json: Option<String> = row.get(9)?;
    let (topic, status, identity_meta) =
        crate::gateway::session_store::types::SessionMetadata::parse_legacy_metadata_json(
            metadata_json.as_deref(),
        );
    Ok(crate::gateway::session_store::types::SessionMetadata {
        key: row.get(0)?,
        agent_id: row.get(1)?,
        session_type: row.get(2)?,
        created_at: row.get(3)?,
        last_active_at: row.get(4)?,
        message_count: row.get(5)?,
        total_tokens: row.get(6)?,
        auto_reset_at: row.get(7)?,
        state: Some(state),
        topic,
        status,
        identity_meta,
        label: row.get(10)?,
        // Legacy rows: ALTER TABLE ADD COLUMN without DEFAULT leaves NULL for
        // pre-migration data. Coerce to 0 so historical sessions load instead
        // of panicking on `Invalid column type Null at index: 11`.
        input_tokens: row.get::<_, Option<i64>>(11)?.unwrap_or(0),
        output_tokens: row.get::<_, Option<i64>>(12)?.unwrap_or(0),
        model: row.get(13)?,
        model_provider: row.get(14)?,
        parent_session_key: row.get(15)?,
        compaction_count: row.get(16)?,
        derived_title: row.get(17).ok(),
        // Column added later; `.ok()` + default keeps a pre-migration row (or a
        // NULL) reading as 0.0 rather than panicking.
        estimated_cost_usd: row.get::<_, Option<f64>>(18).ok().flatten().unwrap_or(0.0),
        // P1 columns; `.ok()` keeps a pre-migration row reading as `None`
        // (legacy, adoption-by-absence) rather than panicking.
        owner_user_id: row.get(19).ok(),
        scope_id: row.get(20).ok(),
        ..Default::default()
    })
}

#[async_trait]
impl SessionStore for SessionManager {
    async fn get_or_create(&self, key: &SessionKey) -> Result<SessionMetadata, SessionStoreError> {
        self.get_or_create(key).await.map_err(map_err)
    }

    async fn get_metadata(
        &self,
        key: &SessionKey,
    ) -> Result<Option<SessionMetadata>, SessionStoreError> {
        let key_str = key.to_key_string();
        let conn = self
            .conn
            .lock()
            .map_err(|e| SessionStoreError::DatabaseError(format!("Lock error: {e}")))?;

        let meta = conn
            .query_row(
                "SELECT key, agent_id, session_type, created_at, last_active_at,
                        message_count, total_tokens, auto_reset_at, state, metadata,
                        label, input_tokens, output_tokens, model, model_provider,
                        parent_session_key, compaction_count, derived_title,
                        estimated_cost_usd, owner_user_id, scope_id
                 FROM sessions WHERE key = ?",
                params![&key_str],
                map_session_metadata,
            )
            .optional()
            .map_err(|e| SessionStoreError::DatabaseError(e.to_string()))?;
        Ok(meta)
    }

    async fn list_sessions(
        &self,
        filter: SessionFilter,
    ) -> Result<Vec<SessionMetadata>, SessionStoreError> {
        let sessions = self
            .list_sessions(filter.agent_id.as_deref())
            .await
            .map_err(map_err)?;
        let mut sessions = sessions;
        if let Some(threshold) = filter.active_minutes {
            let cutoff = chrono::Utc::now().timestamp() - (i64::from(threshold) * 60);
            sessions.retain(|s| s.last_active_at >= cutoff);
        }
        // Deliberate deviation from the P1 plan's "SQL COALESCE(owner_user_id, ?)"
        // instruction: filtering in memory (like `active_minutes` above) keeps the
        // visibility rule single-sourced in `visibility::session_visible_to`
        // instead of re-deriving it a second time in SQL. That predicate also
        // covers project rooms, where the roster — not the creator — decides.
        if let Some(ref owner) = filter.owner_visible_to {
            sessions.retain(|s| crate::gateway::visibility::session_visible_to(s, owner));
        }
        if let Some(limit) = filter.limit {
            sessions.truncate(limit);
        }
        Ok(sessions)
    }

    async fn delete_session(&self, key: &SessionKey) -> Result<DeleteResult, SessionStoreError> {
        self.delete_session(key)
            .await
            .map_err(map_err)
            .map(|deleted| DeleteResult { deleted })
    }

    async fn reset_session(&self, key: &SessionKey) -> Result<bool, SessionStoreError> {
        self.reset_session(key).await.map_err(map_err)
    }

    async fn append_message(
        &self,
        key: &SessionKey,
        msg: MessageRecord,
    ) -> Result<(), SessionStoreError> {
        let metadata_str = msg
            .metadata
            .as_ref()
            .and_then(|v| serde_json::to_string(v).ok());
        // This table keys rows by an autoincrement rowid, so the projector's
        // `"{key}:{seq}"` id would otherwise be dropped on the floor — and with
        // it the only link back to the source event. Persist the seq so a
        // rewind can delete exactly the rows whose events it retired.
        let source_seq =
            crate::session::projection::parse_source_seq(&msg.id, &key.to_key_string())
                .and_then(|seq| i64::try_from(seq).ok());
        self.add_message_full(
            key,
            &msg.role,
            &msg.content,
            metadata_str.as_deref(),
            msg.input_tokens,
            msg.output_tokens,
            msg.tool_call_id.as_deref(),
            msg.tool_name.as_deref(),
            source_seq,
        )
        .await
        .map_err(map_err)?;
        Ok(())
    }

    async fn get_history(
        &self,
        key: &SessionKey,
        limit: Option<usize>,
    ) -> Result<Vec<MessageRecord>, SessionStoreError> {
        self.get_history(key, limit).await.map_err(map_err)
    }

    async fn get_history_before(
        &self,
        key: &SessionKey,
        limit: Option<usize>,
        before: Option<i64>,
    ) -> Result<Vec<MessageRecord>, SessionStoreError> {
        // Push the cursor into SQL rather than filtering in memory.
        self.get_history_before(key, limit, before)
            .await
            .map_err(map_err)
    }

    async fn search_messages(
        &self,
        query: &str,
        max_results: usize,
    ) -> Result<Vec<SearchHit>, SessionStoreError> {
        self.search_messages(query, max_results)
            .await
            .map_err(map_err)
            .map(|results| results.into_iter().map(search_result_to_hit).collect())
    }

    async fn truncate_messages(
        &self,
        key: &SessionKey,
        keep_count: usize,
    ) -> Result<TruncateResult, SessionStoreError> {
        let key_str = key.to_key_string();
        let conn = self
            .conn
            .lock()
            .map_err(|e| SessionStoreError::DatabaseError(format!("Lock error: {e}")))?;

        let total: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM messages WHERE session_key = ?",
                params![&key_str],
                |row| row.get(0),
            )
            .unwrap_or(0);
        if (keep_count as i64) >= total {
            return Ok(TruncateResult::default());
        }

        // Find the id of the keep_count-th message in chronological order
        // (i.e. the last message we want to keep). When keep_count == 0 we
        // delete all messages for the session, mirroring reset_session.
        let threshold_id: Option<i64> = if keep_count == 0 {
            None
        } else {
            conn.query_row(
                "SELECT id FROM messages WHERE session_key = ?
                 ORDER BY timestamp ASC, id ASC LIMIT 1 OFFSET ?",
                params![&key_str, (keep_count - 1) as i64],
                |row| row.get(0),
            )
            .ok()
        };

        // Sum the tokens we are about to drop (best-effort; saturates on err).
        let tokens_removed: i64 = match threshold_id {
            Some(threshold) => conn
                .query_row(
                    "SELECT COALESCE(SUM(input_tokens + output_tokens), 0)
                     FROM messages WHERE session_key = ? AND id > ?",
                    params![&key_str, threshold],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap_or(0),
            None => conn
                .query_row(
                    "SELECT COALESCE(SUM(input_tokens + output_tokens), 0)
                     FROM messages WHERE session_key = ?",
                    params![&key_str],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap_or(0),
        };

        // Sync FTS index inside the same transaction as the source delete so a
        // crash / FTS error rolls back the source rows (the FTS would
        // otherwise over-count messages the source delete already removed).
        let tx = conn
            .unchecked_transaction()
            .map_err(|e| SessionStoreError::DatabaseError(e.to_string()))?;
        match threshold_id {
            Some(threshold) => {
                tx.execute(
                    "DELETE FROM messages_fts WHERE rowid IN (
                        SELECT id FROM messages WHERE session_key = ? AND id > ?
                    )",
                    params![&key_str, threshold],
                )
                .map_err(|e| SessionStoreError::DatabaseError(e.to_string()))?;
            }
            None => {
                tx.execute(
                    "DELETE FROM messages_fts WHERE rowid IN (
                        SELECT id FROM messages WHERE session_key = ?
                    )",
                    params![&key_str],
                )
                .map_err(|e| SessionStoreError::DatabaseError(e.to_string()))?;
            }
        }

        // **Audit fix**: FTS delete + source delete + count update are wrapped in
        // a single transaction so a failure in any step rolls back the others
        // (see `delete_messages_from_seq` for the full rationale).
        let tx = conn
            .unchecked_transaction()
            .map_err(|e| SessionStoreError::DatabaseError(e.to_string()))?;

        let deleted = match threshold_id {
            Some(threshold) => tx
                .execute(
                    "DELETE FROM messages WHERE session_key = ? AND id > ?",
                    params![&key_str, threshold],
                )
                .map_err(|e| SessionStoreError::DatabaseError(e.to_string()))?,
            None => tx
                .execute(
                    "DELETE FROM messages WHERE session_key = ?",
                    params![&key_str],
                )
                .map_err(|e| SessionStoreError::DatabaseError(e.to_string()))?,
        };

        let new_count: i64 = tx
            .query_row(
                "SELECT COUNT(*) FROM messages WHERE session_key = ?",
                params![&key_str],
                |row| row.get(0),
            )
            .map_err(|e| SessionStoreError::DatabaseError(e.to_string()))?;

        // Keep the session row's message_count consistent. We do NOT bump
        // compaction_count — this is a manual revert, not a compaction.
        tx.execute(
            "UPDATE sessions SET message_count = ? WHERE key = ?",
            params![new_count, &key_str],
        )
        .map_err(|e| SessionStoreError::DatabaseError(e.to_string()))?;

        tx.commit()
            .map_err(|e| SessionStoreError::DatabaseError(e.to_string()))?;
        Ok(TruncateResult {
            messages_removed: deleted,
            tokens_removed_estimate: tokens_removed.max(0) as u64,
        })
    }

    async fn delete_messages_from_seq(
        &self,
        key: &SessionKey,
        from_seq: u64,
    ) -> Result<usize, SessionStoreError> {
        let key_str = key.to_key_string();
        // A seq beyond i64 range can never have been stored; saturating high
        // makes the predicate match nothing rather than wrapping into a range
        // that deletes rows the caller never asked for.
        let from = i64::try_from(from_seq).unwrap_or(i64::MAX);

        let conn = self
            .conn
            .lock()
            .map_err(|e| SessionStoreError::DatabaseError(format!("Lock error: {e}")))?;

        // **Audit fix**: wrap the FTS delete + source delete + count
        // update in a single transaction so a failure in any step rolls back
        // the others. The previous code ran them as separate `conn.execute`
        // calls with `.ok()` on the FTS path: a failed FTS cleanup left the
        // index over-counting the rows the source delete removed, so a
        // subsequent cross-session `search_messages` returned hits for
        // content the user had explicitly rewound. The session_count UPDATE
        // is included in the same transaction so a crash mid-way leaves the
        // table consistent.
        //
        // `source_seq IS NULL` rows (legacy transcripts, boot-time orphan
        // notices) are not event-sourced and survive untouched.
        let tx = conn
            .unchecked_transaction()
            .map_err(|e| SessionStoreError::DatabaseError(e.to_string()))?;

        tx.execute(
            "DELETE FROM messages_fts WHERE rowid IN (
                SELECT id FROM messages WHERE session_key = ? AND source_seq >= ?
            )",
            params![&key_str, from],
        )
        .map_err(|e| SessionStoreError::DatabaseError(e.to_string()))?;

        let deleted = tx
            .execute(
                "DELETE FROM messages WHERE session_key = ? AND source_seq >= ?",
                params![&key_str, from],
            )
            .map_err(|e| SessionStoreError::DatabaseError(e.to_string()))?;

        let new_count: i64 = tx
            .query_row(
                "SELECT COUNT(*) FROM messages WHERE session_key = ?",
                params![&key_str],
                |row| row.get(0),
            )
            .map_err(|e| SessionStoreError::DatabaseError(e.to_string()))?;
        tx.execute(
            "UPDATE sessions SET message_count = ? WHERE key = ?",
            params![new_count, &key_str],
        )
        .map_err(|e| SessionStoreError::DatabaseError(e.to_string()))?;

        tx.commit()
            .map_err(|e| SessionStoreError::DatabaseError(e.to_string()))?;
        Ok(deleted)
    }

    async fn list_checkpoints(
        &self,
        _key: &SessionKey,
    ) -> Result<Vec<CheckpointSummary>, SessionStoreError> {
        Err(SessionStoreError::Unsupported)
    }

    async fn branch_from_checkpoint(
        &self,
        _key: &SessionKey,
        _checkpoint_id: &str,
        _new_key: &SessionKey,
    ) -> Result<SessionMetadata, SessionStoreError> {
        Err(SessionStoreError::Unsupported)
    }

    async fn restore_checkpoint(
        &self,
        _key: &SessionKey,
        _checkpoint_id: &str,
    ) -> Result<SessionMetadata, SessionStoreError> {
        Err(SessionStoreError::Unsupported)
    }

    async fn close_session(
        &self,
        key: &SessionKey,
        topic: Option<&str>,
    ) -> Result<(), SessionStoreError> {
        self.close_session(key, topic.map(|s| s.to_string()))
            .await
            .map_err(map_err)
    }

    async fn set_topic(&self, key: &SessionKey, topic: &str) -> Result<(), SessionStoreError> {
        self.set_topic(key, topic).await.map_err(map_err)
    }

    async fn set_project_root(
        &self,
        key: &SessionKey,
        project_root: Option<&str>,
    ) -> Result<(), SessionStoreError> {
        self.set_project_root(key, project_root)
            .await
            .map_err(map_err)
    }

    async fn backfill_attribution(
        &self,
        key: &SessionKey,
        owner_user_id: &str,
        scope_id: &str,
    ) -> Result<bool, SessionStoreError> {
        self.backfill_attribution(key, owner_user_id, scope_id)
            .await
            .map_err(map_err)
    }

    /// See the trait doc. Written directly against `self.conn` — like
    /// `stamp_last_assistant_metadata` above — rather than as an inherent
    /// `SessionManager` method plus `map_err`: `SessionManagerError` has no
    /// variant for "this key is not rescopable", because that is a
    /// store-level concept (`SessionStoreError::Unsupported`), not a
    /// manager-level one. There is no scope-kind check here: `project_id` is
    /// not a rendered scope string, so there is no "wrong kind of scope"
    /// value for this verb to reject — it only ever renders `Project`.
    async fn rescope_attribution(
        &self,
        key: &SessionKey,
        project_id: &str,
    ) -> Result<bool, SessionStoreError> {
        crate::gateway::session_store::require_conversation_key(key)?;
        let scope_id = crate::scope::ScopeId::Project(project_id.to_string()).render();
        let key_str = key.to_key_string();
        let conn = self
            .conn
            .lock()
            .map_err(|e| SessionStoreError::DatabaseError(format!("Lock error: {e}")))?;
        let rows = conn
            .execute(
                "UPDATE sessions SET scope_id = ?2 WHERE key = ?1",
                params![&key_str, scope_id],
            )
            .map_err(|e| SessionStoreError::DatabaseError(e.to_string()))?;
        Ok(rows > 0)
    }

    async fn set_source_channel(
        &self,
        key: &SessionKey,
        channel: &str,
        conversation: Option<&str>,
    ) -> Result<(), SessionStoreError> {
        self.stamp_source_channel(key, channel, conversation)
            .await
            .map_err(map_err)
    }

    async fn set_state(
        &self,
        key: &SessionKey,
        state: SessionState,
    ) -> Result<(), SessionStoreError> {
        self.set_state(key, state).await.map_err(map_err)
    }

    async fn get_state(&self, key: &SessionKey) -> Result<SessionState, SessionStoreError> {
        self.get_state(key).await.map_err(map_err)
    }

    async fn get_total_tokens(&self, key: &SessionKey) -> Result<Option<u64>, SessionStoreError> {
        // Resolves to the inherent SessionManager::get_total_tokens (inherent
        // methods shadow trait methods on the concrete type — same pattern as
        // get_state above).
        self.get_total_tokens(key).await.map_err(map_err)
    }

    async fn get_identity_context(
        &self,
        session_key: &str,
        source_channel: &str,
    ) -> Result<aleph_protocol::IdentityContext, SessionStoreError> {
        self.get_identity_context(session_key, source_channel)
            .await
            .map_err(map_err)
    }

    async fn get_current_epoch(&self, base_key_pattern: &str) -> Result<u32, SessionStoreError> {
        self.get_current_epoch(base_key_pattern)
            .await
            .map_err(map_err)
    }

    async fn get_session_topic(
        &self,
        key: &SessionKey,
    ) -> Result<Option<String>, SessionStoreError> {
        self.get_session_topic(key).await.map_err(map_err)
    }

    async fn cleanup_expired(&self) -> Result<usize, SessionStoreError> {
        self.cleanup_expired().await.map_err(map_err)
    }

    async fn patch_session(
        &self,
        key: &SessionKey,
        patch: &SessionPatch,
    ) -> Result<bool, SessionStoreError> {
        self.patch_session(key, patch).await.map_err(map_err)
    }

    async fn update_session_usage(
        &self,
        key: &SessionKey,
        input_tokens: i64,
        output_tokens: i64,
        cost_usd: f64,
        model: Option<&str>,
        model_provider: Option<&str>,
    ) -> Result<(), SessionStoreError> {
        self.update_session_usage(
            key,
            input_tokens,
            output_tokens,
            cost_usd,
            model,
            model_provider,
        )
        .await
        .map_err(map_err)
    }

    async fn get_session_preview(
        &self,
        key: &SessionKey,
        message_limit: usize,
    ) -> Result<SessionPreview, SessionStoreError> {
        self.get_session_preview(key, message_limit)
            .await
            .map_err(map_err)
    }

    async fn count_by_state(&self, state: SessionState) -> Result<usize, SessionStoreError> {
        self.count_by_state(state).await.map_err(map_err)
    }

    async fn list_by_state(
        &self,
        state: SessionState,
    ) -> Result<Vec<SessionMetadata>, SessionStoreError> {
        self.list_by_state(state).await.map_err(map_err)
    }

    async fn set_error(
        &self,
        key: &SessionKey,
        error_msg: Option<&str>,
    ) -> Result<(), SessionStoreError> {
        self.set_error(key, error_msg).await.map_err(map_err)
    }

    async fn stop(&self, key: &SessionKey) -> Result<(), SessionStoreError> {
        self.stop(key).await.map_err(map_err)
    }

    async fn set_idle(&self, key: &SessionKey) -> Result<(), SessionStoreError> {
        self.set_idle(key).await.map_err(map_err)
    }

    async fn stamp_last_assistant_metadata(
        &self,
        key: &SessionKey,
        metadata: &serde_json::Value,
    ) -> Result<(), SessionStoreError> {
        let key_str = key.to_key_string();
        let metadata_json = serde_json::to_string(metadata)
            .map_err(|e| SessionStoreError::DatabaseError(format!("serialize metadata: {e}")))?;
        let conn = self
            .conn
            .lock()
            .map_err(|e| SessionStoreError::DatabaseError(format!("Lock error: {e}")))?;
        conn.execute(
            "UPDATE messages SET metadata = ?1
             WHERE id = (SELECT MAX(id) FROM messages WHERE session_key = ?2 AND role = 'assistant')",
            params![metadata_json, key_str],
        )
        .map_err(|e| SessionStoreError::DatabaseError(e.to_string()))?;
        Ok(())
    }
}

#[async_trait]
impl crate::session::epoch_registrar::SessionEpochRegistrar for SessionManager {
    async fn register_epoch(&self, key: &crate::session::service::SessionId) -> anyhow::Result<()> {
        self.get_or_create(key)
            .await
            .map(|_| ())
            .map_err(|e| anyhow::anyhow!("register_epoch: get_or_create failed: {e}"))
    }

    async fn retire_superseded(&self, superseded: &crate::session::service::SessionId) {
        // Awaited rather than spawned: the caller is already a background
        // compaction, so there is no user-facing latency to protect, and
        // awaiting means the work cannot be lost to a runtime shutdown.
        crate::gateway::continuation_lifecycle::retire_side_session_now(
            superseded,
            "context-split",
            self,
        )
        .await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn test_store(temp: &tempfile::TempDir) -> SessionManager {
        let config = SqliteSessionStoreConfig {
            db_path: temp.path().join("test_sessions.db"),
            ..Default::default()
        };
        SessionManager::new(config).expect("test session store")
    }

    #[tokio::test]
    async fn register_epoch_registers_child_session_row() {
        use crate::routing::session_key::SessionKey;
        use crate::session::epoch_registrar::SessionEpochRegistrar;

        let temp = tempdir().unwrap();
        let store = test_store(&temp);

        let base = SessionKey::Main {
            agent_id: "a".into(),
            main_key: "k".into(),
            epoch: 0,
        };
        store.get_or_create(&base).await.unwrap();

        let child = base.with_next_epoch(); // epoch 1
        assert_eq!(child.epoch(), 1);

        // Before registration the child generation does not exist.
        assert!(
            store.get_metadata(&child).await.unwrap().is_none(),
            "child session must not exist before register_epoch"
        );

        store.register_epoch(&child).await.unwrap();

        // register_epoch must have created the child row (it delegates to
        // get_or_create). Assert directly via a metadata lookup — no reliance
        // on get_current_epoch ordering.
        let meta = store
            .get_metadata(&child)
            .await
            .unwrap()
            .expect("register_epoch must create the child session row");
        assert_eq!(meta.key, child.to_key_string());
    }

    /// The one exception to "session scope is immutable once set": an operator
    /// binding a channel conversation to a room. Everything else must keep
    /// getting the create-only behaviour.
    #[tokio::test]
    async fn rescoping_a_group_row_to_a_room_moves_only_the_scope() {
        use crate::routing::session_key::PeerKind;
        use crate::scope::{with_scope, ScopeAttribution};

        let temp = tempdir().unwrap();
        let store = test_store(&temp);
        let key = SessionKey::group("main", "telegram", PeerKind::Group, "C1");
        with_scope(
            Some(ScopeAttribution::personal("u-alice")),
            store.get_or_create(&key),
        )
        .await
        .unwrap();

        let changed = store
            .rescope_attribution(&key, "p-1")
            .await
            .expect("a sqlite backend supports rescoping");
        assert!(changed, "the row moved");

        let meta = store.get_metadata(&key).await.unwrap().unwrap();
        assert_eq!(meta.scope_id.as_deref(), Some("project:p-1"));
        assert_eq!(
            meta.owner_user_id.as_deref(),
            Some("u-alice"),
            "the owner still names whoever spoke first — the room's visibility is \
             decided by the roster, so overwriting the owner would only lose the byline"
        );
    }

    /// A non-group key must be refused. Rescoping is a visibility grant, and a
    /// DM has exactly one human on the far side: there is no roster to grant to.
    #[tokio::test]
    async fn rescoping_refuses_a_key_that_is_not_a_conversation() {
        let temp = tempdir().unwrap();
        let store = test_store(&temp);
        let key = SessionKey::main("main");
        let result = store.rescope_attribution(&key, "p-1").await;
        assert!(
            result.is_err(),
            "only a group conversation may be rescoped into a room"
        );
    }

    /// A group nobody has spoken in yet has no row. That is not an error —
    /// it is the common case for a freshly bound room, and the trait doc
    /// pins `Ok(false)` for it specifically so a store that cannot tell the
    /// difference between "unsupported" and "nothing to move" cannot use it
    /// to mean either.
    #[tokio::test]
    async fn rescoping_a_row_that_does_not_exist_yet_reports_no_change() {
        use crate::routing::session_key::PeerKind;

        let temp = tempdir().unwrap();
        let store = test_store(&temp);
        let key = SessionKey::group("main", "telegram", PeerKind::Group, "C-unspoken");
        let changed = store
            .rescope_attribution(&key, "p-1")
            .await
            .expect("a sqlite backend supports rescoping");
        assert!(
            !changed,
            "there is no row yet for a group nobody has spoken in"
        );
    }
}
