pub mod error;
pub mod file_backend;
pub mod migration;
pub mod sqlite_backend;
pub mod types;

use crate::gateway::router::SessionKey;
use crate::gateway::session_store::error::SessionStoreError;
use crate::gateway::session_store::types::*;
use async_trait::async_trait;

#[async_trait]
pub trait SessionStore: Send + Sync {
    async fn get_or_create(&self, key: &SessionKey) -> Result<SessionMetadata, SessionStoreError>;
    async fn get_metadata(
        &self,
        key: &SessionKey,
    ) -> Result<Option<SessionMetadata>, SessionStoreError>;
    async fn list_sessions(
        &self,
        filter: SessionFilter,
    ) -> Result<Vec<SessionMetadata>, SessionStoreError>;
    async fn delete_session(&self, key: &SessionKey) -> Result<DeleteResult, SessionStoreError>;
    async fn reset_session(&self, key: &SessionKey) -> Result<bool, SessionStoreError>;

    async fn append_message(
        &self,
        key: &SessionKey,
        msg: MessageRecord,
    ) -> Result<(), SessionStoreError>;
    async fn get_history(
        &self,
        key: &SessionKey,
        limit: Option<usize>,
    ) -> Result<Vec<MessageRecord>, SessionStoreError>;

    async fn search_messages(
        &self,
        query: &str,
        max_results: usize,
    ) -> Result<Vec<SearchHit>, SessionStoreError>;

    async fn compact(
        &self,
        key: &SessionKey,
        strategy: CompactStrategy,
    ) -> Result<CompactResult, SessionStoreError>;
    async fn list_checkpoints(
        &self,
        key: &SessionKey,
    ) -> Result<Vec<CheckpointSummary>, SessionStoreError>;
    async fn branch_from_checkpoint(
        &self,
        key: &SessionKey,
        checkpoint_id: &str,
        new_key: &SessionKey,
    ) -> Result<SessionMetadata, SessionStoreError>;
    async fn restore_checkpoint(
        &self,
        key: &SessionKey,
        checkpoint_id: &str,
    ) -> Result<SessionMetadata, SessionStoreError>;

    async fn close_session(
        &self,
        key: &SessionKey,
        topic: Option<&str>,
    ) -> Result<(), SessionStoreError>;
    async fn set_topic(&self, key: &SessionKey, topic: &str) -> Result<(), SessionStoreError>;
    async fn set_state(
        &self,
        key: &SessionKey,
        state: crate::gateway::session_manager::SessionState,
    ) -> Result<(), SessionStoreError>;
    async fn get_state(
        &self,
        key: &SessionKey,
    ) -> Result<crate::gateway::session_manager::SessionState, SessionStoreError>;
    async fn get_identity_context(
        &self,
        session_key: &str,
        source_channel: &str,
    ) -> Result<aleph_protocol::IdentityContext, SessionStoreError>;
    async fn get_current_epoch(&self, base_key_pattern: &str) -> Result<u32, SessionStoreError>;
    async fn get_session_topic(
        &self,
        key: &SessionKey,
    ) -> Result<Option<String>, SessionStoreError>;
    async fn cleanup_expired(&self) -> Result<usize, SessionStoreError>;

    async fn patch_session(
        &self,
        key: &SessionKey,
        patch: &SessionPatch,
    ) -> Result<bool, SessionStoreError>;
    async fn update_session_usage(
        &self,
        key: &SessionKey,
        input_tokens: i64,
        output_tokens: i64,
        model: Option<&str>,
        model_provider: Option<&str>,
    ) -> Result<(), SessionStoreError>;
    async fn get_session_preview(
        &self,
        key: &SessionKey,
        message_limit: usize,
    ) -> Result<SessionPreview, SessionStoreError>;
    async fn count_by_state(
        &self,
        state: crate::gateway::session_manager::SessionState,
    ) -> Result<usize, SessionStoreError>;
    async fn list_by_state(
        &self,
        state: crate::gateway::session_manager::SessionState,
    ) -> Result<Vec<SessionMetadata>, SessionStoreError>;
    async fn set_error(
        &self,
        key: &SessionKey,
        error_msg: Option<&str>,
    ) -> Result<(), SessionStoreError>;
    async fn stop(&self, key: &SessionKey) -> Result<(), SessionStoreError>;
    async fn set_idle(&self, key: &SessionKey) -> Result<(), SessionStoreError>;
    async fn set_running(&self, key: &SessionKey) -> Result<(), SessionStoreError>;

    /// Load a windowed slice of the transcript for a session.
    ///
    /// `session_id` is the raw key string as stored in the messages table
    /// (i.e. `SessionKey::to_key_string()`). `agent_id` is accepted for
    /// interface symmetry; the default impl ignores it because `session_id`
    /// already encodes the agent identity.
    ///
    /// Returns up to `max_turns` most-recent messages, further capped at
    /// `max_chars` total content characters, in chronological (oldest-first)
    /// order.
    ///
    /// **Trailing-N semantics**: this default impl relies on `get_history`
    /// returning the *trailing* `max_turns` messages (most recent N) when a
    /// LIMIT is supplied, in chronological order. Both production impls
    /// satisfy this contract:
    ///   * `SessionManager` (sqlite) wraps the LIMIT in
    ///     `SELECT ... FROM (... ORDER BY timestamp DESC LIMIT N) ORDER BY timestamp ASC`
    ///     (`session_manager/ops.rs:302`).
    ///   * `FileSessionStore::read_transcript` reads the full transcript and
    ///     `split_off(messages.len() - n)` (`file_backend/mod.rs:247`).
    ///
    /// New impls MUST honor this contract — returning the leading N would
    /// summarize stale context and break the lazy synthesizer.
    async fn load_window(
        &self,
        agent_id: &str,
        session_id: &str,
        max_turns: usize,
        max_chars: usize,
    ) -> Result<Vec<(String, String)>, SessionStoreError> {
        use crate::gateway::router::SessionKey;
        // Reconstruct the SessionKey from the stored key string, falling back
        // to a Main key if parsing fails (e.g. legacy or test key formats).
        let key = SessionKey::parse(session_id).unwrap_or_else(|| SessionKey::Main {
            agent_id: agent_id.to_string(),
            main_key: session_id.to_string(),
            epoch: 0,
        });
        let records = self.get_history(&key, Some(max_turns)).await?;
        let mut pairs: Vec<(String, String)> = Vec::with_capacity(records.len());
        let mut total_chars = 0usize;
        for msg in records {
            let chars = msg.content.len();
            if total_chars + chars > max_chars && !pairs.is_empty() {
                break;
            }
            total_chars += chars;
            pairs.push((msg.role, msg.content));
        }
        Ok(pairs)
    }
}
