use async_trait::async_trait;
use rusqlite::{OptionalExtension, params};

use crate::gateway::router::SessionKey;
use crate::gateway::session_manager::{SessionManager, SessionManagerConfig, SessionState};
use crate::gateway::session_store::error::SessionStoreError;
use crate::gateway::session_store::types::*;
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
    }
}

fn search_result_to_hit(
    r: crate::gateway::session_manager::SessionSearchResult,
) -> SearchHit {
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
        .and_then(|s| serde_json::from_str(&format!("\"{}\"", s)).ok())
        .unwrap_or_default();
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
        metadata_json: row.get(9)?,
        label: row.get(10)?,
        input_tokens: row.get(11)?,
        output_tokens: row.get(12)?,
        model: row.get(13)?,
        model_provider: row.get(14)?,
        parent_session_key: row.get(15)?,
        compaction_count: row.get(16)?,
        ..Default::default()
    })
}

#[async_trait]
impl SessionStore for SessionManager {
    async fn get_or_create(
        &self,
        key: &SessionKey,
    ) -> Result<SessionMetadata, SessionStoreError> {
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
            .map_err(|e| SessionStoreError::DatabaseError(format!("Lock error: {}", e)))?;

        let meta = conn
            .query_row(
                "SELECT key, agent_id, session_type, created_at, last_active_at,
                        message_count, total_tokens, auto_reset_at, state, metadata,
                        label, input_tokens, output_tokens, model, model_provider,
                        parent_session_key, compaction_count
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
            let cutoff = chrono::Utc::now().timestamp() - (threshold as i64 * 60);
            sessions.retain(|s| s.last_active_at >= cutoff);
        }
        if let Some(limit) = filter.limit {
            sessions.truncate(limit);
        }
        Ok(sessions)
    }

    async fn delete_session(
        &self,
        key: &SessionKey,
    ) -> Result<DeleteResult, SessionStoreError> {
        self.delete_session(key)
            .await
            .map_err(map_err)
            .map(|deleted| DeleteResult { deleted })
    }

    async fn reset_session(&self, key: &SessionKey) -> Result<bool, SessionStoreError> {
        self.reset_session(key).await.map_err(map_err)
    }

    #[allow(deprecated)]
    async fn append_message(
        &self,
        key: &SessionKey,
        msg: MessageRecord,
    ) -> Result<(), SessionStoreError> {
        let metadata_str = msg.metadata.as_ref().and_then(|v| serde_json::to_string(v).ok());
        self.add_message_with_meta(
            key,
            &msg.role,
            &msg.content,
            metadata_str.as_deref(),
            msg.input_tokens,
            msg.output_tokens,
            msg.model.as_deref(),
            msg.model_provider.as_deref(),
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

    async fn compact(
        &self,
        key: &SessionKey,
        strategy: CompactStrategy,
    ) -> Result<CompactResult, SessionStoreError> {
        match strategy {
            CompactStrategy::KeepLastN { n } => {
                let key_str = key.to_key_string();
                let keep = n as i64;
                let conn = self
                    .conn
                    .lock()
                    .map_err(|e| SessionStoreError::DatabaseError(format!("Lock error: {}", e)))?;

                let threshold_id: Option<i64> = conn
                    .query_row(
                        "SELECT id FROM messages WHERE session_key = ?
                         ORDER BY timestamp DESC LIMIT 1 OFFSET ?",
                        params![&key_str, keep],
                        |row| row.get(0),
                    )
                    .ok();

                if let Some(threshold) = threshold_id {
                    conn.execute(
                        "DELETE FROM messages_fts WHERE rowid IN (
                            SELECT id FROM messages WHERE session_key = ? AND id < ?
                        )",
                        params![&key_str, threshold],
                    )
                    .ok();

                    let deleted = conn
                        .execute(
                            "DELETE FROM messages WHERE session_key = ? AND id < ?",
                            params![&key_str, threshold],
                        )
                        .map_err(|e| SessionStoreError::DatabaseError(e.to_string()))?;

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

                    return Ok(CompactResult {
                        compacted: true,
                        deleted,
                    });
                }
                Ok(CompactResult {
                    compacted: false,
                    deleted: 0,
                })
            }
        }
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

    async fn get_identity_context(
        &self,
        session_key: &str,
        source_channel: &str,
    ) -> Result<aleph_protocol::IdentityContext, SessionStoreError> {
        self.get_identity_context(session_key, source_channel)
            .await
            .map_err(map_err)
    }

    async fn get_current_epoch(
        &self,
        base_key_pattern: &str,
    ) -> Result<u32, SessionStoreError> {
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
        model: Option<&str>,
        model_provider: Option<&str>,
    ) -> Result<(), SessionStoreError> {
        self.update_session_usage(
            key,
            input_tokens,
            output_tokens,
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

    async fn count_by_state(
        &self,
        state: SessionState,
    ) -> Result<usize, SessionStoreError> {
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

    async fn set_running(&self, key: &SessionKey) -> Result<(), SessionStoreError> {
        self.set_running(key).await.map_err(map_err)
    }
}


