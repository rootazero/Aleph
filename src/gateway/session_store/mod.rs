pub mod error;
pub mod file_backend;
pub mod migration;
pub mod sqlite_backend;
pub mod types;

use crate::gateway::router::SessionKey;
use crate::gateway::session_store::error::SessionStoreError;
use crate::gateway::session_store::types::{SessionMetadata, SessionFilter, DeleteResult, MessageRecord, SearchHit, CompactStrategy, CompactResult, TruncateResult, CheckpointSummary, SessionPatch, SessionPreview};
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

    /// History with an optional `before` cursor (unix seconds): only messages
    /// whose timestamp is *strictly older* than `before` are returned; with a
    /// `limit`, the most recent `limit` of those, oldest-first. This is what
    /// powers `chat.history` cursor pagination — fetching successively older
    /// pages by passing the oldest timestamp of the previous page as `before`.
    ///
    /// The default impl fetches the full history once and filters in memory; it
    /// is correct for every store. SQL-backed stores override it to push the
    /// `timestamp <` predicate into the query (see `SessionManager`). When
    /// `before` is `None` the result is identical to [`get_history`].
    ///
    /// [`get_history`]: SessionStore::get_history
    async fn get_history_before(
        &self,
        key: &SessionKey,
        limit: Option<usize>,
        before: Option<i64>,
    ) -> Result<Vec<MessageRecord>, SessionStoreError> {
        let all = self.get_history(key, None).await?;
        Ok(paginate_before(all, limit, before))
    }

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

    /// Drop messages from the tail of a session, keeping only the first
    /// `keep_count` messages by chronological order.
    ///
    /// Used by `/undo` in the TUI to remove the last user+assistant turn pair.
    /// Returning `(0, 0)` when `keep_count >= current_count` is the expected
    /// no-op behaviour; do not return an error in that case.
    ///
    /// Default impl returns `Unsupported` so legacy / test stores need not
    /// re-implement it; production backends override this.
    async fn truncate_messages(
        &self,
        key: &SessionKey,
        keep_count: usize,
    ) -> Result<TruncateResult, SessionStoreError> {
        let _ = (key, keep_count);
        Err(SessionStoreError::Unsupported)
    }
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
    /// Record the originating channel of a session (e.g. "telegram", "gui:chat")
    /// onto its identity metadata, so `sessions.list` / `sessions.changed` can
    /// surface conversation origin for multi-end continuity. Idempotent: must not
    /// clobber an already-recorded real origin. Default is a no-op so backends that
    /// do not persist identity metadata compile unchanged.
    async fn set_source_channel(
        &self,
        key: &SessionKey,
        channel: &str,
        conversation: Option<&str>,
    ) -> Result<(), SessionStoreError> {
        let _ = (key, channel, conversation);
        Ok(())
    }
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

/// Pure pagination core shared by [`SessionStore::get_history_before`]: given a
/// chronological (oldest-first) message list, keep only messages strictly older
/// than `before` (when set), then return the most recent `limit` of those still
/// in oldest-first order.
///
/// Factored out so the predicate/window logic is unit-testable without a store
/// or database, and so every default-impl backend shares one definition.
pub(crate) fn paginate_before(
    all: Vec<MessageRecord>,
    limit: Option<usize>,
    before: Option<i64>,
) -> Vec<MessageRecord> {
    let mut filtered: Vec<MessageRecord> = match before {
        Some(ts) => all.into_iter().filter(|m| m.timestamp < ts).collect(),
        None => all,
    };
    if let Some(n) = limit {
        if filtered.len() > n {
            // Keep the trailing (most-recent) `n`, preserving oldest-first order.
            filtered = filtered.split_off(filtered.len() - n);
        }
    }
    filtered
}

#[cfg(test)]
mod paginate_before_tests {
    use super::*;

    fn rec(id: &str, ts: i64) -> MessageRecord {
        MessageRecord {
            id: id.to_string(),
            role: "user".to_string(),
            content: id.to_string(),
            timestamp: ts,
            metadata: None,
            input_tokens: 0,
            output_tokens: 0,
            model: None,
            model_provider: None,
        }
    }

    fn sample() -> Vec<MessageRecord> {
        vec![rec("a", 10), rec("b", 20), rec("c", 30), rec("d", 40)]
    }

    #[test]
    fn none_before_none_limit_is_identity() {
        let out = paginate_before(sample(), None, None);
        let ids: Vec<&str> = out.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, ["a", "b", "c", "d"]);
    }

    #[test]
    fn before_keeps_only_strictly_older() {
        // Cursor at 30 must exclude the message *at* 30 (strictly older).
        let out = paginate_before(sample(), None, Some(30));
        let ids: Vec<&str> = out.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, ["a", "b"]);
    }

    #[test]
    fn before_with_limit_returns_most_recent_of_the_older_set() {
        // Older-than-40 set is [a,b,c]; the trailing 2 are [b,c], oldest-first.
        let out = paginate_before(sample(), Some(2), Some(40));
        let ids: Vec<&str> = out.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, ["b", "c"]);
    }

    #[test]
    fn limit_without_cursor_returns_trailing_window() {
        let out = paginate_before(sample(), Some(2), None);
        let ids: Vec<&str> = out.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, ["c", "d"]);
    }

    #[test]
    fn cursor_older_than_everything_yields_empty() {
        assert!(paginate_before(sample(), Some(10), Some(5)).is_empty());
    }
}
