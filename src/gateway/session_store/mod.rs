//! Read invariant (P1 SSOT): `messages` is the Panel read surface. Legacy
//! sessions retain their rows here; new sessions are materialized by
//! `MessageProjector` from `session_events`. No dual-read branch is needed —
//! both classes read the same `messages` table.

pub mod error;
pub mod file_backend;
pub mod migration;
pub mod sqlite_backend;
pub mod types;

use crate::gateway::router::SessionKey;
use crate::gateway::session_store::error::SessionStoreError;
use crate::gateway::session_store::types::{
    CheckpointSummary, DeleteResult, MessageRecord, SearchHit, SessionFilter, SessionMetadata,
    SessionPatch, SessionPreview, TruncateResult,
};
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

    // NOTE: there is deliberately no `compact` here. It existed to delete the
    // oldest rows of THIS table, which is a read projection for the Panel — the
    // agent's prompt is rebuilt from `session_events`, so the operation cost
    // the user their scrollback and freed exactly zero context. User-driven
    // `/compact` now lives in `context::compact::manual`, which compacts the
    // event log and deletes nothing.

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

    /// Delete every projection row materialised from a source event with
    /// `seq >= from_seq`, and return how many rows were removed.
    ///
    /// This is the projection half of `chat.rewind`: `retire_from` retires the
    /// event-log suffix, this removes the rows those events produced. Deleting
    /// by ROW COUNT instead would be wrong — `messages` is not a 1:1 image of
    /// the live event log (boot-time orphan notices and other writers append
    /// rows with no source event), so a count-derived ordinal cuts the wrong
    /// range. The correlation is the projector's row id (`"{key}:{seq}"`, see
    /// [`crate::session::projection::parse_source_seq`]); rows with no source
    /// seq are NOT event-sourced and are left in place.
    ///
    /// Default impl returns `Unsupported`, mirroring [`truncate_messages`];
    /// production backends override it.
    ///
    /// [`truncate_messages`]: SessionStore::truncate_messages
    async fn delete_messages_from_seq(
        &self,
        key: &SessionKey,
        from_seq: u64,
    ) -> Result<usize, SessionStoreError> {
        let _ = (key, from_seq);
        Err(SessionStoreError::Unsupported)
    }
    /// Stamp `owner_user_id` / `scope_id` onto a session row that has **both**
    /// columns unset. Returns whether a row was actually written.
    ///
    /// This is the one write that is allowed to touch columns the spec calls
    /// immutable, and it is narrow on purpose: the `IS NULL AND IS NULL`
    /// predicate is part of the statement, not the caller's job, so this can
    /// never re-scope a session somebody already owns no matter who calls it.
    /// `SessionPatch` deliberately cannot express these columns, which is why
    /// this exists as its own verb rather than a flag on an existing one.
    ///
    /// Its only caller is the legacy-room backfill
    /// (`projects::attribution_backfill`): a room created before P1 has a
    /// `scope_id` of `NULL`, which reads as "org-era personal session", so
    /// `project_visible` never fires and the room's own conversation is
    /// invisible to every member of its roster. `owner_user_id` alone is
    /// already handled by adoption-by-absence — `scope_id` is the column with
    /// no default that means the right thing.
    ///
    /// Default is `Unsupported` rather than `Ok(false)`: a store that cannot do
    /// this has not "found nothing to backfill", and the migration counts the
    /// two outcomes separately.
    async fn backfill_attribution(
        &self,
        key: &SessionKey,
        owner_user_id: &str,
        scope_id: &str,
    ) -> Result<bool, SessionStoreError> {
        let _ = (key, owner_user_id, scope_id);
        Err(SessionStoreError::Unsupported)
    }

    /// Move an EXISTING session row's scope into a project room.
    ///
    /// This is the one exception to spec §10 ("session scope is immutable once
    /// set"), and it is deliberately narrow. `stamp_attribution` stays
    /// create-only for every other path; this verb exists because binding a
    /// channel conversation to a room is a decision a human makes, with a
    /// reason, that is recorded in the audit log — while `backfill_attribution`
    /// can only heal rows that were never stamped at all.
    ///
    /// Without it a bound conversation splits in two: the RUN takes the room
    /// scope (memory partition, roster, room context) while the ROW keeps
    /// `personal:<first speaker>`, so every other member's `session_visible_to`
    /// says false and the group stays invisible in their session list.
    ///
    /// `key` must be a `SessionKey::Group`; refuse anything else in the verb
    /// itself, not at the call site, so "only a conversation may be rescoped"
    /// is a property of the method rather than a rule each caller has to
    /// remember. A DM has one human on the far side and no roster to grant
    /// visibility to. Implementors: check this with
    /// [`require_conversation_key`] rather than re-deriving the match, so the
    /// property has one author across every backend, not one per
    /// implementor.
    ///
    /// `project_id` — not a rendered `scope_id` — deliberately. This is the
    /// one place the two types diverge, and it is not an inconsistency to
    /// "harmonize" back: `backfill_attribution` can legitimately stamp *any*
    /// scope, because it heals a NULL row to whatever the run's ambient
    /// attribution happened to be — personal, org, or project — so its
    /// `scope_id: &str` has to accept all three. This verb can only ever
    /// target a project: there is no such thing as rescoping a conversation
    /// into someone's personal scope or into org scope. Accepting a project
    /// id — not a rendered scope string of any kind — makes "which kind of
    /// scope" the parameter's meaning rather than a value it can carry
    /// wrongly: there is no longer a rendered-personal-scope string for a
    /// caller to pass here, so this verb has nothing left to detect and
    /// reject at runtime. The check was deleted, not centralized.
    ///
    /// Move ONLY the scope. `owner_user_id` still names whoever spoke first:
    /// for a project-scoped row, visibility is decided by the roster, so
    /// overwriting the owner would buy nothing and lose the byline.
    ///
    /// Its only caller will be the channel-binding handler
    /// (`handlers::projects_channel::handle_bind`, Task 9), which pins it via
    /// `session_store::caller_census::SOLE_CALLERS` in the same commit that
    /// creates that file — see this method's introducing commit for why that
    /// row is not added yet.
    ///
    /// `Ok(false)` means there was no such row — a conversation nobody has
    /// spoken in yet, which is the common case for a freshly bound group and
    /// is not an error. Default is `Unsupported` rather than `Ok(false)` for
    /// the same reason `backfill_attribution`'s is: a store that cannot do
    /// this has not "found nothing to move".
    async fn rescope_attribution(
        &self,
        key: &SessionKey,
        project_id: &str,
    ) -> Result<bool, SessionStoreError> {
        let _ = (key, project_id);
        Err(SessionStoreError::Unsupported)
    }

    async fn list_checkpoints(
        &self,
        key: &SessionKey,
    ) -> Result<Vec<CheckpointSummary>, SessionStoreError>;
    /// Implementors: the returned `SessionMetadata` is a freshly-created
    /// session (`new_key`), so it must be owner-stamped exactly like
    /// [`get_or_create`](SessionStore::get_or_create)'s CREATE branch — call
    /// `SessionMetadata::stamp_attribution()` on it before persisting. Skip
    /// this and the branched session silently reads as legacy/owner-owned
    /// under `visibility::session_visible`, invisible to the member who just
    /// created it (P1 data isolation).
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

    /// Persist (or clear) the user-chosen project working directory for a
    /// session onto its identity metadata (`custom["project_root"]`).
    /// `Some(path)` stores it; `None` reverts to the default agent workspace.
    /// Surfaced through `sessions.list` (`SessionInfo.project_root`). Default is
    /// a no-op so backends that do not persist identity metadata (file backend,
    /// test stubs) compile unchanged.
    async fn set_project_root(
        &self,
        key: &SessionKey,
        project_root: Option<&str>,
    ) -> Result<(), SessionStoreError> {
        let _ = (key, project_root);
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
    /// Cumulative `total_tokens` for a session (input + output over its
    /// lifetime), or `None` if the session row does not exist yet. Used by the
    /// autonomous-goal continuation hook to enforce a goal's token budget
    /// against the live session counter. Default `Ok(None)` — backends without
    /// token accounting leave the budget unenforced (iteration and deadline caps
    /// still apply), so only the SQLite-backed store needs to override this.
    async fn get_total_tokens(&self, key: &SessionKey) -> Result<Option<u64>, SessionStoreError> {
        let _ = key;
        Ok(None)
    }
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

    /// Hard-delete `task`-type sessions of the given `task_type` whose last
    /// activity predates `cutoff_secs` (UNIX seconds). Returns the number of
    /// session directories removed.
    ///
    /// Default: no-op (`Ok(0)`). Only the file backend materializes a
    /// directory per `SessionTarget::Isolated` run, so it is the only backend
    /// that accumulates these. The cron session reaper drives this on the same
    /// retention horizon as `cron_job_runs`, which is otherwise the only cron
    /// state that gets trimmed — the per-run transcript directories had no
    /// reaper at all.
    async fn reap_task_sessions(
        &self,
        task_type: &str,
        cutoff_secs: i64,
    ) -> Result<usize, SessionStoreError> {
        let _ = (task_type, cutoff_secs);
        Ok(0)
    }

    /// Stamp `run_id` + context-window occupancy onto the most recent
    /// assistant message row for this session. Called by the projector when it
    /// processes a [`crate::session::events::SessionEvent::AssistantRunMeta`]
    /// event, so the Panel can restore the occupancy gauge on session reload
    /// without a live `run_complete` event.
    ///
    /// Default: no-op (`Ok(())`). Only the SQLite-backed store overrides this;
    /// file-backend and test stubs compile unchanged.
    async fn stamp_last_assistant_metadata(
        &self,
        _key: &SessionKey,
        _metadata: &serde_json::Value,
    ) -> Result<(), SessionStoreError> {
        Ok(())
    }

    async fn patch_session(
        &self,
        key: &SessionKey,
        patch: &SessionPatch,
    ) -> Result<bool, SessionStoreError>;
    /// Accumulate one run's usage onto the session row. `cost_usd` is that
    /// run's priced cost (0.0 when it could not be priced). Called by
    /// `session_projector` on `AssistantRunMeta`.
    async fn update_session_usage(
        &self,
        key: &SessionKey,
        input_tokens: i64,
        output_tokens: i64,
        cost_usd: f64,
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

/// The shape check behind [`SessionStore::rescope_attribution`]: "only a
/// conversation may be rescoped" is a property of the verb, not a rule each
/// caller — or each backend — has to remember.
///
/// Factored out so it has exactly one author. Both shipped backends
/// (`file_backend`, `sqlite_backend`) call this rather than re-deriving the
/// `matches!(key, SessionKey::Group { .. })` check inline; a future third
/// implementor of `rescope_attribution` should too, rather than writing its
/// own copy that can drift from this one.
pub(crate) fn require_conversation_key(key: &SessionKey) -> Result<(), SessionStoreError> {
    if matches!(key, SessionKey::Group { .. }) {
        Ok(())
    } else {
        Err(SessionStoreError::Unsupported)
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
            tool_call_id: None,
            tool_name: None,
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

/// `delete_messages_from_seq` — the projection half of `chat.rewind`. Both
/// production backends must behave identically, so every case runs twice.
#[cfg(test)]
mod delete_from_seq_tests {
    use super::*;
    use crate::gateway::session_manager::{SessionManager, SessionManagerConfig};
    use crate::gateway::session_store::file_backend::{FileSessionStore, FileSessionStoreConfig};
    use crate::session::projection::row_id;
    use crate::sync_primitives::Arc;
    use tempfile::TempDir;

    fn key() -> SessionKey {
        SessionKey::Main {
            agent_id: "main".into(),
            main_key: "main".into(),
            epoch: 0,
        }
    }

    /// A row the projector wrote for source event `seq`.
    fn projected(seq: u64, role: &str, content: &str) -> MessageRecord {
        MessageRecord {
            id: row_id(&key().to_key_string(), seq),
            role: role.into(),
            content: content.into(),
            timestamp: seq as i64,
            metadata: None,
            input_tokens: 0,
            output_tokens: 0,
            tool_call_id: None,
            tool_name: None,
        }
    }

    /// A row nobody projected — e.g. the boot-time orphan notice. This is what
    /// makes `messages` NOT a 1:1 image of the live event log.
    fn foreign(id: &str, ts: i64, content: &str) -> MessageRecord {
        MessageRecord {
            id: id.into(),
            role: "system".into(),
            content: content.into(),
            timestamp: ts,
            metadata: None,
            input_tokens: 0,
            output_tokens: 0,
            tool_call_id: None,
            tool_name: None,
        }
    }

    fn file_store(temp: &TempDir) -> Arc<dyn SessionStore> {
        Arc::new(
            FileSessionStore::new(FileSessionStoreConfig {
                base_dir: temp.path().to_path_buf(),
                ..Default::default()
            })
            .unwrap(),
        )
    }

    fn sqlite_store(temp: &TempDir) -> Arc<dyn SessionStore> {
        Arc::new(
            SessionManager::new(SessionManagerConfig {
                db_path: temp.path().join("sessions.db"),
                ..Default::default()
            })
            .unwrap(),
        )
    }

    /// Seed the exact shape that breaks count-based truncation: a non-event-
    /// sourced row wedged between two turns, so row ordinals and event seqs
    /// disagree from that point on.
    ///
    /// rows: [u1(seq 1), a1(seq 2), orphan(no seq), u2(seq 3), a2(seq 4)]
    async fn seed(store: &Arc<dyn SessionStore>) {
        let k = key();
        store.get_or_create(&k).await.unwrap();
        for msg in [
            projected(1, "user", "u1"),
            projected(2, "assistant", "a1"),
            foreign("orphan-1", 3, "interrupted by restart"),
            projected(3, "user", "u2"),
            projected(4, "assistant", "a2"),
        ] {
            store.append_message(&k, msg).await.unwrap();
        }
    }

    async fn contents(store: &Arc<dyn SessionStore>) -> Vec<String> {
        store
            .get_history(&key(), None)
            .await
            .unwrap()
            .into_iter()
            .map(|m| m.content)
            .collect()
    }

    /// Rewinding at the LAST event must drop exactly that one row. Count-based
    /// truncation keeps `live_event_count == 3` rows — [u1, a1, orphan] — and
    /// so deletes `u2`, a still-live message. This is that off-by-N.
    async fn rewind_last_event_drops_only_its_row(store: Arc<dyn SessionStore>) {
        seed(&store).await;
        let removed = store.delete_messages_from_seq(&key(), 4).await.unwrap();
        assert_eq!(removed, 1, "only a2 is projected from seq >= 4");
        assert_eq!(
            contents(&store).await,
            vec!["u1", "a1", "interrupted by restart", "u2"],
            "u2 is live and must survive; the orphan row is not event-sourced"
        );
    }

    /// Mid-log rewind: everything projected from `seq >= 3` goes, everything
    /// before stays, and the non-event-sourced row is untouched either way.
    async fn rewind_mid_log_drops_the_suffix(store: Arc<dyn SessionStore>) {
        seed(&store).await;
        let removed = store.delete_messages_from_seq(&key(), 3).await.unwrap();
        assert_eq!(removed, 2, "u2 + a2");
        assert_eq!(
            contents(&store).await,
            vec!["u1", "a1", "interrupted by restart"]
        );
    }

    /// A seq past the head deletes nothing (idempotent re-rewind).
    async fn rewind_past_head_is_a_noop(store: Arc<dyn SessionStore>) {
        seed(&store).await;
        assert_eq!(store.delete_messages_from_seq(&key(), 99).await.unwrap(), 0);
        assert_eq!(contents(&store).await.len(), 5);
    }

    #[tokio::test]
    async fn file_backend_rewind_matches_source_seq() {
        let temp = TempDir::new().unwrap();
        rewind_last_event_drops_only_its_row(file_store(&temp)).await;
        let temp = TempDir::new().unwrap();
        rewind_mid_log_drops_the_suffix(file_store(&temp)).await;
        let temp = TempDir::new().unwrap();
        rewind_past_head_is_a_noop(file_store(&temp)).await;
    }

    #[tokio::test]
    async fn sqlite_backend_rewind_matches_source_seq() {
        let temp = TempDir::new().unwrap();
        rewind_last_event_drops_only_its_row(sqlite_store(&temp)).await;
        let temp = TempDir::new().unwrap();
        rewind_mid_log_drops_the_suffix(sqlite_store(&temp)).await;
        let temp = TempDir::new().unwrap();
        rewind_past_head_is_a_noop(sqlite_store(&temp)).await;
    }
}

/// P1 data isolation: session `owner_user_id`/`scope_id` stamping at
/// creation + `SessionFilter::owner_visible_to` filtering. Both production
/// backends must behave identically, so every case runs twice — same
/// discipline as `delete_from_seq_tests`.
#[cfg(test)]
mod owner_scope_tests {
    use super::*;
    use crate::gateway::session_manager::{SessionManager, SessionManagerConfig};
    use crate::gateway::session_store::file_backend::{FileSessionStore, FileSessionStoreConfig};
    use crate::scope::ScopeAttribution;
    use crate::sync_primitives::Arc;
    use tempfile::TempDir;

    fn key_for(agent: &str) -> SessionKey {
        SessionKey::main(agent)
    }

    fn file_store(temp: &TempDir) -> Arc<dyn SessionStore> {
        Arc::new(
            FileSessionStore::new(FileSessionStoreConfig {
                base_dir: temp.path().to_path_buf(),
                ..Default::default()
            })
            .unwrap(),
        )
    }

    fn sqlite_store(temp: &TempDir) -> Arc<dyn SessionStore> {
        Arc::new(
            SessionManager::new(SessionManagerConfig {
                db_path: temp.path().join("sessions.db"),
                ..Default::default()
            })
            .unwrap(),
        )
    }

    async fn create_under_scope_stamps_owner_and_scope(store: Arc<dyn SessionStore>) {
        let key = key_for("alice-1");
        let meta = crate::scope::with_scope(
            Some(ScopeAttribution::personal("u-alice")),
            store.get_or_create(&key),
        )
        .await
        .unwrap();
        assert_eq!(meta.owner_user_id.as_deref(), Some("u-alice"));
        assert_eq!(meta.scope_id.as_deref(), Some("personal:u-alice"));

        // Round-trip: read back from disk, both fields survive.
        let read = store.get_metadata(&key).await.unwrap().unwrap();
        assert_eq!(read.owner_user_id.as_deref(), Some("u-alice"));
        assert_eq!(read.scope_id.as_deref(), Some("personal:u-alice"));
    }

    async fn create_without_scope_leaves_fields_none_and_serializes_without_them(
        store: Arc<dyn SessionStore>,
    ) {
        let key = key_for("bob-1");
        let meta = store.get_or_create(&key).await.unwrap();
        assert!(meta.owner_user_id.is_none() && meta.scope_id.is_none());

        // Migration invariant: a None-owner metadata record contains neither
        // key (`skip_serializing_if`) — a pre-P1 `metadata.json` stays
        // byte-identical.
        let json = serde_json::to_string(&meta).unwrap();
        assert!(!json.contains("owner_user_id") && !json.contains("scope_id"));
    }

    async fn get_or_create_on_existing_session_never_restamps(store: Arc<dyn SessionStore>) {
        // A created it; B's later get_or_create must not steal ownership.
        let key = key_for("carol-1");
        let first = crate::scope::with_scope(
            Some(ScopeAttribution::personal("u-alice")),
            store.get_or_create(&key),
        )
        .await
        .unwrap();
        assert_eq!(first.owner_user_id.as_deref(), Some("u-alice"));

        let second = crate::scope::with_scope(
            Some(ScopeAttribution::personal("u-bob")),
            store.get_or_create(&key),
        )
        .await
        .unwrap();
        assert_eq!(
            second.owner_user_id.as_deref(),
            Some("u-alice"),
            "stamping is create-only"
        );
    }

    async fn list_sessions_filters_by_owner_visible_to(store: Arc<dyn SessionStore>) {
        // alice creates 2, bob creates 1, one legacy (no scope) row.
        crate::scope::with_scope(
            Some(ScopeAttribution::personal("u-alice")),
            store.get_or_create(&key_for("alice-1")),
        )
        .await
        .unwrap();
        crate::scope::with_scope(
            Some(ScopeAttribution::personal("u-alice")),
            store.get_or_create(&key_for("alice-2")),
        )
        .await
        .unwrap();
        crate::scope::with_scope(
            Some(ScopeAttribution::personal("u-bob")),
            store.get_or_create(&key_for("bob-1")),
        )
        .await
        .unwrap();
        store.get_or_create(&key_for("legacy-1")).await.unwrap();

        let alice_sessions = store
            .list_sessions(SessionFilter {
                owner_visible_to: Some("u-alice".to_string()),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(alice_sessions.len(), 2, "alice's 2 sessions only");
        assert!(alice_sessions
            .iter()
            .all(|s| s.owner_user_id.as_deref() == Some("u-alice")));

        let owner_sessions = store
            .list_sessions(SessionFilter {
                owner_visible_to: Some(crate::gateway::security::store::OWNER_USER_ID.to_string()),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(
            owner_sessions.len(),
            1,
            "legacy row reads as owned by OWNER_USER_ID"
        );
        assert!(owner_sessions[0].owner_user_id.is_none());

        let all_sessions = store.list_sessions(SessionFilter::default()).await.unwrap();
        assert_eq!(all_sessions.len(), 4, "no filter = every owner");
    }

    /// The list path's half of project rooms (P2).
    ///
    /// A room's sessions are owned by whoever created them, so an
    /// owner-equality filter hides them from every other member — the room
    /// works when addressed directly but never appears in the sidebar, with no
    /// error anywhere. This is the regression that pins both backends onto
    /// `visibility::session_visible_to`.
    async fn list_sessions_shows_a_member_a_room_they_did_not_create(store: Arc<dyn SessionStore>) {
        let _guard = crate::projects::roster::TEST_GUARD
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let projects =
            crate::projects::ProjectStore::new(rusqlite::Connection::open_in_memory().unwrap());
        projects.create_schema().unwrap();
        let room = projects.create("room", Some("u-alice"), None).unwrap();
        projects.add_member(&room.id, "u-bob").unwrap();

        // alice opens the room session and also has one of her own.
        crate::scope::with_scope(
            Some(ScopeAttribution {
                owner_user_id: "u-alice".to_string(),
                scope: crate::scope::ScopeId::Project(room.id.clone()),
            }),
            store.get_or_create(&key_for("room-1")),
        )
        .await
        .unwrap();
        crate::scope::with_scope(
            Some(ScopeAttribution::personal("u-alice")),
            store.get_or_create(&key_for("alice-private")),
        )
        .await
        .unwrap();

        let bob = store
            .list_sessions(SessionFilter {
                owner_visible_to: Some("u-bob".to_string()),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(
            bob.len(),
            1,
            "bob sees the room he is a member of and nothing else"
        );
        assert_eq!(
            bob[0].scope_id.as_deref(),
            Some(crate::scope::ScopeId::Project(room.id.clone()).render()).as_deref()
        );

        projects.remove_member(&room.id, "u-bob").unwrap();
        let bob_after = store
            .list_sessions(SessionFilter {
                owner_visible_to: Some("u-bob".to_string()),
                ..Default::default()
            })
            .await
            .unwrap();
        assert!(
            bob_after.is_empty(),
            "removal from the roster takes the room out of the list immediately"
        );
    }

    #[tokio::test]
    async fn file_backend_owner_scope_stamping() {
        let temp = TempDir::new().unwrap();
        create_under_scope_stamps_owner_and_scope(file_store(&temp)).await;
        let temp = TempDir::new().unwrap();
        create_without_scope_leaves_fields_none_and_serializes_without_them(file_store(&temp))
            .await;
        let temp = TempDir::new().unwrap();
        get_or_create_on_existing_session_never_restamps(file_store(&temp)).await;
        let temp = TempDir::new().unwrap();
        list_sessions_filters_by_owner_visible_to(file_store(&temp)).await;
        let temp = TempDir::new().unwrap();
        list_sessions_shows_a_member_a_room_they_did_not_create(file_store(&temp)).await;
    }

    #[tokio::test]
    async fn sqlite_backend_owner_scope_stamping() {
        let temp = TempDir::new().unwrap();
        create_under_scope_stamps_owner_and_scope(sqlite_store(&temp)).await;
        let temp = TempDir::new().unwrap();
        create_without_scope_leaves_fields_none_and_serializes_without_them(sqlite_store(&temp))
            .await;
        let temp = TempDir::new().unwrap();
        get_or_create_on_existing_session_never_restamps(sqlite_store(&temp)).await;
        let temp = TempDir::new().unwrap();
        list_sessions_filters_by_owner_visible_to(sqlite_store(&temp)).await;
        let temp = TempDir::new().unwrap();
        list_sessions_shows_a_member_a_room_they_did_not_create(sqlite_store(&temp)).await;
    }
}

#[cfg(test)]
mod caller_census {
    //! The narrow-writer family: session-attribution verbs whose safety
    //! argument is "there is exactly one caller".
    //!
    //! Each entry's doc names its sole caller. A doc comment has no test, so
    //! this is where that sentence becomes enforceable: a second production
    //! call site must fail by name rather than quietly widen a verb whose
    //! whole justification was that it is unreachable from anywhere else.

    use crate::utils::source_scan::{code_text, production_prefix, rust_sources_under};

    /// (verb, the one file allowed to call it).
    const SOLE_CALLERS: &[(&str, &str)] = &[(
        "backfill_attribution",
        "src/projects/attribution_backfill.rs",
    )];

    /// Every production `.rs` under `src/`, reduced to the code a compiler
    /// would see: comments AND string-literal payloads removed.
    ///
    /// NOT for the reason self-scanning might suggest: `production_prefix`
    /// runs first and excises everything from THIS file's own `#[cfg(test)]`
    /// boundary (line 394) onward, so `mod caller_census` — this module,
    /// including `SOLE_CALLERS` and this very comment — never reaches the
    /// corpus at all, `code_text` or not. What `code_text` actually guards
    /// against is a DIFFERENT production file's comment or string literal (a
    /// log message, a doc example, a stale TODO) containing the text
    /// `.backfill_attribution(` without being a real call site;
    /// comment-stripping alone would leave a string literal like that intact
    /// and miscount it as a caller.
    fn production_sources() -> Vec<(String, String)> {
        rust_sources_under(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"))
            .into_iter()
            .map(|(path, raw)| (path, code_text(&production_prefix(&raw))))
            .collect()
    }

    #[test]
    fn each_narrow_writer_still_has_exactly_one_caller() {
        let sources = production_sources();
        // Measured, not chosen: 2,455 `.rs` files under src/ on 2026-08-29
        // (`find src -name '*.rs' | wc -l`). 2,000 leaves ~18% margin for a
        // genuine future consolidation while still catching a walk that came
        // back with only a fraction of the tree — the same margin
        // `shutdown_forensics.rs`'s census uses against its own measured
        // count. The prior `> 500` floor would pass on a corpus that had
        // silently lost 80% of its files, which is the exact failure this
        // self-check exists to catch.
        assert!(
            sources.len() > 2_000,
            "the scanner found only {} files — it is not reading the tree it \
             thinks it is, and a shrinking sample reports 'all clear'",
            sources.len()
        );
        for (verb, owner) in SOLE_CALLERS {
            let call_needle = format!(".{verb}(");
            // A file that itself DECLARES the verb (the trait signature, or a
            // backend impl — possibly delegating to a same-named inherent
            // method, which reads as a call on the same line) is an
            // implementor, not a consumer. Exclude by that fact, derived per
            // run, rather than by a hand-written path list: a path allowlist
            // rots into a licence for whatever is on it, and would not notice
            // a third backend landing tomorrow. Same shape as the repo's
            // existing `admin_refusal.rs::receiver_holds_a_string`.
            let decl_needle = format!("fn {verb}(");
            let callers: Vec<&str> = sources
                .iter()
                .filter(|(_, body)| body.contains(&call_needle) && !body.contains(&decl_needle))
                .map(|(path, _)| path.as_str())
                .collect();
            assert_eq!(
                callers,
                vec![*owner],
                "`{verb}` documents `{owner}` as its ONLY caller. Found: {callers:?}. \
                 A file that declares `fn {verb}` (the trait signature or a backend \
                 impl) is excluded as an implementor, not a consumer. A new caller \
                 means the verb's safety argument changed — update the doc and this \
                 census together."
            );
        }
    }
}
