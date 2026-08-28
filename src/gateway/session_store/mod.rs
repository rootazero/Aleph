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
    stamp_millis, CheckpointSummary, DeleteResult, HistoryPage, MessageRecord, SearchHit,
    SessionFilter, SessionMetadata, SessionPatch, SessionPreview, TruncateResult,
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

    /// A window of this session's transcript AND how long the whole transcript
    /// is, from one read.
    ///
    /// `before` is a cursor: only messages *strictly older* than that instant
    /// are admitted, and with a `limit` the most recent `limit` of those,
    /// oldest-first. `None` admits everything, making the window identical to
    /// [`get_history`].
    ///
    /// The cursor is an INSTANT, not a number. It used to be an `i64`
    /// documented as unix seconds, and the store it is compared against holds
    /// [`MessageRecord::timestamp`] in a mixed unit (see that field's doc):
    /// seconds in some rows, milliseconds in others, sometimes both inside one
    /// transcript. A millisecond row is ~1000x any seconds cursor, so it never
    /// satisfied `timestamp < before` — a cursor page silently omitted the
    /// majority of a real install's rows and returned nothing at all for its
    /// largest sessions, which a client can only read as "you have reached the
    /// beginning". A type the caller cannot spell in the wrong unit is the fix;
    /// the ranking itself goes through [`stamp_millis`].
    ///
    /// **There is deliberately no separate `history_len`.** There was, and its
    /// only caller fetched the count and the window in two calls against a
    /// store a live run appends to — so the two answers described two different
    /// sessions and the client's `total - count` was wrong by whatever landed
    /// in between. That was managed with a comment about which order made the
    /// skew fall the safer way and a source-level guard pinning the order,
    /// which a later edit could satisfy lexically while breaking semantically
    /// (move the count into a helper, call the helper after the window). One
    /// method has no order to get wrong, and a caller holding an
    /// `Arc<dyn SessionStore>` has no second call to reach for. If you ever need
    /// "how many are older than T", that is a different question and deserves
    /// its own method rather than a parameter here.
    ///
    /// **The cursor has no in-repo client, and that is a reviewed decision
    /// rather than an oversight.** `chat.history?before=…` has been on the wire
    /// since v2026.04.02 and no Panel or CLI surface calls it: both answer "show
    /// me earlier messages" by WIDENING the window instead — the Panel re-runs
    /// its one hydration path (prepending a page would mis-key its `<For>` rows,
    /// see `HISTORY_PAGE`), and the CLI footer says "raise --limit, or omit it".
    /// An earlier round justified keeping the cursor with "this question has no
    /// second answer in the repo"; that is false — those two ARE the second and
    /// third answers — so do not lean on it when reopening the CUT. The leg that
    /// does hold is the shape of removing it: `HistoryParams` does not deny
    /// unknown fields, so a third-party caller still sending `before` would
    /// silently receive the NEWEST window instead of an older page. Making that
    /// fail loudly costs more code than deleting the cursor removes, and the
    /// cursor is now correct and covered. Reviewed and kept, 2026-08-28.
    ///
    /// The default impl reads the transcript ONCE and answers both from it —
    /// correct for every store, exactly consistent, and for the file backend
    /// half the work it used to do (the count parsed the whole transcript, then
    /// the window parsed it again). SQL-backed stores override it to push both
    /// down, and must take their connection ONCE so the pair stays atomic.
    ///
    /// [`get_history`]: SessionStore::get_history
    /// [`MessageRecord::timestamp`]: crate::gateway::session_store::types::MessageRecord::timestamp
    /// [`stamp_millis`]: crate::gateway::session_store::types::stamp_millis
    async fn history_page(
        &self,
        key: &SessionKey,
        limit: Option<usize>,
        before: Option<chrono::DateTime<chrono::Utc>>,
    ) -> Result<HistoryPage, SessionStoreError> {
        let all = self.get_history(key, None).await?;
        // Taken before the window consumes `all`; both describe the same read,
        // which is the whole point of this method.
        let total = all.len();
        Ok(HistoryPage {
            rows: paginate_before(all, limit, before),
            total: Some(total),
        })
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

/// Pure pagination core shared by [`SessionStore::history_page`]: given a
/// chronological (oldest-first) message list, keep only messages strictly older
/// than `before` (when set), then return the most recent `limit` of those still
/// in oldest-first order.
///
/// Factored out so the predicate/window logic is unit-testable without a store
/// or database, and so every default-impl backend shares one definition.
pub(crate) fn paginate_before(
    all: Vec<MessageRecord>,
    limit: Option<usize>,
    before: Option<chrono::DateTime<chrono::Utc>>,
) -> Vec<MessageRecord> {
    let mut filtered: Vec<MessageRecord> = match before {
        // Both sides through `stamp_millis`, never the raw field: the column
        // holds two units and the raw comparison ranks every millisecond row
        // as newer than every seconds row. `stamp_millis` is total, so no row
        // can be dropped for being unreadable — the alternative, comparing
        // `instant()`, would silently exclude a stamp no calendar can
        // represent instead of just placing it.
        Some(cut) => {
            let cut = cut.timestamp_millis();
            all.into_iter()
                .filter(|m| stamp_millis(m.timestamp) < cut)
                .collect()
        }
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

    /// A cursor at `secs` past the epoch. The cursor is an instant now, so a
    /// caller cannot hand it a number in the wrong unit — which is the whole
    /// repair; these helpers just keep the existing cases readable.
    fn at(secs: i64) -> chrono::DateTime<chrono::Utc> {
        chrono::DateTime::from_timestamp(secs, 0).expect("representable")
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
        let out = paginate_before(sample(), None, Some(at(30)));
        let ids: Vec<&str> = out.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, ["a", "b"]);
    }

    #[test]
    fn before_with_limit_returns_most_recent_of_the_older_set() {
        // Older-than-40 set is [a,b,c]; the trailing 2 are [b,c], oldest-first.
        let out = paginate_before(sample(), Some(2), Some(at(40)));
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
        assert!(paginate_before(sample(), Some(10), Some(at(5))).is_empty());
    }

    /// The case this function silently got wrong for its whole life.
    ///
    /// Values here are REAL epoch stamps, not the small round numbers the
    /// cases above use: the seconds/milliseconds boundary is `1e11`, so a
    /// toy value like `20_000` is unambiguously seconds no matter what it was
    /// meant to represent, and a test written with toy "milliseconds" passes
    /// or fails for reasons that have nothing to do with the property. (That
    /// is not a weakness of the boundary — it is the constant's stated design:
    /// no conversation Aleph could have recorded falls in the ambiguous gap.)
    #[test]
    fn a_millisecond_row_is_older_than_a_cursor_after_it() {
        const BASE: i64 = 1_785_062_232; // 2026-07-26T10:37:12Z
        let ms = vec![rec("a", (BASE + 10) * 1000), rec("b", (BASE + 20) * 1000)];
        let out = paginate_before(ms, None, Some(at(BASE + 30)));
        let ids: Vec<&str> = out.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(
            ids,
            ["a", "b"],
            "both rows are older than the cursor; the raw comparison kept none"
        );
    }

    /// Two spellings inside ONE transcript — not hypothetical: two sessions on
    /// the measured install carried both. The page must be ordered by real
    /// time, not by which unit each row happens to use.
    #[test]
    fn a_transcript_with_both_units_pages_by_real_time() {
        const BASE: i64 = 1_785_062_232;
        let mixed = vec![
            rec("t10s", BASE + 10),
            rec("t20ms", (BASE + 20) * 1000),
            rec("t30s", BASE + 30),
            rec("t40ms", (BASE + 40) * 1000),
        ];
        let out = paginate_before(mixed, None, Some(at(BASE + 35)));
        let ids: Vec<&str> = out.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(
            ids,
            ["t10s", "t20ms", "t30s"],
            "the millisecond rows must interleave with the seconds ones by \
             instant; raw, `t20ms` and `t40ms` both rank above any cursor"
        );
    }

    /// A stamp no calendar can represent still gets a position rather than
    /// disappearing. `stamp_millis` is total for exactly this reason: comparing
    /// `instant()` would have made an unreadable row invisible to every cursor
    /// page, which is a second way to hide content while reporting success.
    ///
    /// `i64::MIN` specifically, because it is the value with no positive
    /// counterpart — writing this case is what surfaced an `abs()` that
    /// panicked in debug and wrapped in release.
    #[test]
    fn an_unrepresentable_stamp_is_placed_not_dropped() {
        const BASE: i64 = 1_785_062_232;
        let rows = vec![rec("min", i64::MIN), rec("a", BASE + 10)];
        let out = paginate_before(rows, None, Some(at(BASE + 30)));
        let ids: Vec<&str> = out.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, ["min", "a"], "i64::MIN ranks oldest, and is kept");
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

/// The `before` cursor against a transcript that mixes timestamp units.
///
/// FILE BACKEND ONLY, and that is a property of the stores rather than a gap in
/// the coverage: `FileSessionStore::append_transcript` serializes the record it
/// is given, so the unit is whichever PRODUCER wrote it — `MessageProjector`
/// stamps `created_at_ms` (milliseconds) while direct writers such as
/// `agent_instance` stamp `timestamp()` (seconds) — whereas the SQLite backend
/// never stores a caller's stamp at all (`add_message_full` overwrites it with
/// its own `now().timestamp()`), so its column cannot be made mixed and this
/// case is unreachable there.
///
/// Measured on a real install, 2026-08-28: 1745 of 3030 rows were
/// millisecond-stamped, the three largest sessions were entirely so, and two
/// transcripts carried both spellings. Against the raw column a seconds cursor
/// admits no millisecond row at all, so `chat.history?before=…` answered
/// `{count: 0, messages: []}` — a success a client can only read as "you have
/// reached the beginning of the conversation".
#[cfg(test)]
mod mixed_unit_cursor_tests {
    use super::*;
    use crate::gateway::session_store::file_backend::{FileSessionStore, FileSessionStoreConfig};
    use crate::sync_primitives::Arc;
    use tempfile::TempDir;

    /// 2026-07-26T10:37:12Z, the base both spellings are offset from.
    const BASE: i64 = 1_785_062_232;

    fn row(id: &str, ts: i64) -> MessageRecord {
        MessageRecord {
            id: id.to_string(),
            role: "user".into(),
            content: id.to_string(),
            timestamp: ts,
            metadata: None,
            input_tokens: 0,
            output_tokens: 0,
            tool_call_id: None,
            tool_name: None,
        }
    }

    async fn seeded(temp: &TempDir) -> (Arc<dyn SessionStore>, SessionKey) {
        let store: Arc<dyn SessionStore> = Arc::new(
            FileSessionStore::new(FileSessionStoreConfig {
                base_dir: temp.path().to_path_buf(),
                ..Default::default()
            })
            .unwrap(),
        );
        let key = SessionKey::main("mixed-units");
        store.get_or_create(&key).await.unwrap();
        // Chronological, alternating units — the shape a session gets when the
        // projector and a direct writer both append to it.
        for (id, ts) in [
            ("t10s", BASE + 10),
            ("t20ms", (BASE + 20) * 1000),
            ("t30ms", (BASE + 30) * 1000),
            ("t40s", BASE + 40),
        ] {
            store.append_message(&key, row(id, ts)).await.unwrap();
        }
        (store, key)
    }

    fn ids(rows: &[MessageRecord]) -> Vec<&str> {
        rows.iter().map(|m| m.content.as_str()).collect()
    }

    fn at(secs: i64) -> chrono::DateTime<chrono::Utc> {
        chrono::DateTime::from_timestamp(secs, 0).expect("representable")
    }

    /// The regression itself: a cursor placed between `t30ms` and `t40s` must
    /// admit the two millisecond rows. Raw, it admitted only `t10s`.
    #[tokio::test]
    async fn a_cursor_admits_millisecond_rows_older_than_it() {
        let temp = TempDir::new().unwrap();
        let (store, key) = seeded(&temp).await;

        let page = store
            .history_page(&key, None, Some(at(BASE + 35)))
            .await
            .unwrap();
        assert_eq!(
            ids(&page.rows),
            ["t10s", "t20ms", "t30ms"],
            "a millisecond row is ~1000x any cursor when compared raw, so this \
             page used to be just [t10s] — a short page reads as the start of \
             the transcript"
        );
    }

    /// And the window is still the TRAILING end of the cursor-filtered set, so
    /// paging backwards walks the conversation rather than restarting it.
    #[tokio::test]
    async fn a_limit_windows_the_cursor_filtered_set() {
        let temp = TempDir::new().unwrap();
        let (store, key) = seeded(&temp).await;

        let page = store
            .history_page(&key, Some(2), Some(at(BASE + 35)))
            .await
            .unwrap();
        assert_eq!(ids(&page.rows), ["t20ms", "t30ms"]);
    }

    /// A cursor older than everything still yields nothing — the repair widens
    /// what the cursor can see, it does not stop the cursor from excluding.
    #[tokio::test]
    async fn a_cursor_before_the_conversation_yields_nothing() {
        let temp = TempDir::new().unwrap();
        let (store, key) = seeded(&temp).await;

        let page = store
            .history_page(&key, None, Some(at(BASE - 1)))
            .await
            .unwrap();
        assert!(ids(&page.rows).is_empty());
    }
}

/// [`HistoryPage::total`] answers the one question the window cannot.
///
/// `get_history(limit)` serves the TRAILING rows, so its length says nothing
/// about whether anything was cut: a full page and a whole short transcript are
/// both exactly `limit` long. Everything here is about that indistinguishability
/// — run against BOTH backends, because the SQLite store answers with a
/// `COUNT(*)` under the same lock as the window while the file store falls
/// through to the trait's default impl and measures the read it just did, and a
/// divergence between them is precisely the bug this method invites.
#[cfg(test)]
mod history_page_total_tests {
    use super::*;
    use crate::gateway::session_manager::{SessionManager, SessionManagerConfig};
    use crate::gateway::session_store::file_backend::{FileSessionStore, FileSessionStoreConfig};
    use crate::sync_primitives::Arc;
    use tempfile::TempDir;

    fn key() -> SessionKey {
        SessionKey::main("history-len")
    }

    fn row(ts: i64) -> MessageRecord {
        MessageRecord {
            id: format!("m{ts}"),
            role: "user".into(),
            content: format!("message {ts}"),
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

    async fn seed(store: &Arc<dyn SessionStore>, n: i64) {
        let k = key();
        store.get_or_create(&k).await.unwrap();
        for ts in 1..=n {
            store.append_message(&k, row(ts)).await.unwrap();
        }
    }

    async fn counts_the_whole_session_not_the_window(store: Arc<dyn SessionStore>) {
        seed(&store, 5).await;

        let windowed = store.get_history(&key(), Some(2)).await.unwrap();
        assert_eq!(windowed.len(), 2, "the window is the trailing 2");
        assert_eq!(
            windowed.first().map(|m| m.content.as_str()),
            Some("message 4"),
            "and it is the trailing end — which is why the beginning needs \
             announcing at all"
        );
        assert_eq!(
            store
                .history_page(&key(), None, None)
                .await
                .unwrap()
                .total
                .unwrap(),
            5,
            "total is the whole session, unaffected by the caller's limit"
        );
    }

    /// The case a length-based guess gets wrong. With `limit == total` the
    /// window is full AND complete, so `got.len() >= limit` claims there is
    /// more above when there is nothing; only a separately-reported total can
    /// tell those apart.
    async fn a_full_window_that_is_also_the_whole_transcript(store: Arc<dyn SessionStore>) {
        seed(&store, 3).await;

        let windowed = store.get_history(&key(), Some(3)).await.unwrap();
        assert_eq!(windowed.len(), 3);
        assert_eq!(
            store
                .history_page(&key(), None, None)
                .await
                .unwrap()
                .total
                .unwrap(),
            windowed.len(),
            "equal means complete; the guess `len() >= limit` reads this exact \
             shape as truncated"
        );
    }

    async fn an_empty_session_counts_zero(store: Arc<dyn SessionStore>) {
        store.get_or_create(&key()).await.unwrap();
        assert_eq!(
            store
                .history_page(&key(), None, None)
                .await
                .unwrap()
                .total
                .unwrap(),
            0
        );
    }

    #[tokio::test]
    async fn file_backend_history_page_total() {
        let temp = TempDir::new().unwrap();
        counts_the_whole_session_not_the_window(file_store(&temp)).await;
        let temp = TempDir::new().unwrap();
        a_full_window_that_is_also_the_whole_transcript(file_store(&temp)).await;
        let temp = TempDir::new().unwrap();
        an_empty_session_counts_zero(file_store(&temp)).await;
    }

    #[tokio::test]
    async fn sqlite_backend_history_page_total() {
        let temp = TempDir::new().unwrap();
        counts_the_whole_session_not_the_window(sqlite_store(&temp)).await;
        let temp = TempDir::new().unwrap();
        a_full_window_that_is_also_the_whole_transcript(sqlite_store(&temp)).await;
        let temp = TempDir::new().unwrap();
        an_empty_session_counts_zero(sqlite_store(&temp)).await;
    }
}

/// What a stored message's timestamp MEANS — when the message happened, not
/// when its row was written.
///
/// Run against both backends because they used to answer differently and the
/// difference was invisible: the file store serializes the producer's record
/// verbatim, while `add_message_full` replaced the stamp with `now()`. On a
/// live turn those two instants are milliseconds apart, so the substitution
/// only showed up where an event is projected later than it occurred — a
/// reconciler, a backfill, an import — and there it showed up as the whole
/// conversation being dated by the moment it was re-read.
#[cfg(test)]
mod message_stamp_fidelity_tests {
    use super::*;
    use crate::gateway::session_manager::{SessionManager, SessionManagerConfig};
    use crate::gateway::session_store::file_backend::{FileSessionStore, FileSessionStoreConfig};
    use crate::sync_primitives::Arc;
    use tempfile::TempDir;

    /// 2020-06-01T00:00:00Z, in milliseconds — the unit `MessageProjector`
    /// stamps (`created_at_ms`). Far enough in the past that "was this replaced
    /// by the insert clock" is answerable without a tolerance.
    const HAPPENED_AT_MS: i64 = 1_590_969_600_000;

    fn key() -> SessionKey {
        SessionKey::main("stamp-fidelity")
    }

    fn row(timestamp: i64) -> MessageRecord {
        MessageRecord {
            id: "m1".into(),
            role: "user".into(),
            content: "hello".into(),
            timestamp,
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

    async fn write_then_read(store: &Arc<dyn SessionStore>, msg: MessageRecord) -> MessageRecord {
        store.get_or_create(&key()).await.unwrap();
        store.append_message(&key(), msg).await.unwrap();
        store
            .get_history(&key(), None)
            .await
            .unwrap()
            .pop()
            .expect("the row that was just appended")
    }

    async fn a_producers_stamp_survives_the_write(store: Arc<dyn SessionStore>) {
        let read = write_then_read(&store, row(HAPPENED_AT_MS)).await;
        let at = read.instant().expect("a representable stamp");
        assert_eq!(
            at.timestamp_millis(),
            HAPPENED_AT_MS,
            "the store re-dated a message its producer had already dated. \
             `append_message` is handed the instant the message occurred; an \
             insert that stamps `now()` over it makes this backend disagree \
             with the other one about the same conversation."
        );
    }

    #[tokio::test]
    async fn file_backend_keeps_the_producers_stamp() {
        let temp = TempDir::new().unwrap();
        a_producers_stamp_survives_the_write(file_store(&temp)).await;
    }

    #[tokio::test]
    async fn sqlite_backend_keeps_the_producers_stamp() {
        let temp = TempDir::new().unwrap();
        a_producers_stamp_survives_the_write(sqlite_store(&temp)).await;
    }

    /// The two backends agree about one record — the property the fix is
    /// actually about, and the one neither backend's own test can state.
    #[tokio::test]
    async fn both_backends_date_the_same_record_the_same_way() {
        let ftemp = TempDir::new().unwrap();
        let stemp = TempDir::new().unwrap();
        let from_file = write_then_read(&file_store(&ftemp), row(HAPPENED_AT_MS)).await;
        let from_sqlite = write_then_read(&sqlite_store(&stemp), row(HAPPENED_AT_MS)).await;
        assert_eq!(
            from_file.instant(),
            from_sqlite.instant(),
            "the same message, appended to the two backends, comes back with \
             two different instants — which is what a user sees when a session \
             moves between them"
        );
    }

    /// Rows whose producer said nothing still get the insert clock, exactly as
    /// every row did before. The fix only ever REMOVES rows from that set.
    ///
    /// SQLite only: the file backend serializes whatever it is handed, so a
    /// zero there stays a zero (1970). That divergence predates this change and
    /// is untouched by it — no production producer leaves the field unset (all
    /// four stamp it), and the value appears only in test fixtures.
    #[tokio::test]
    async fn sqlite_dates_an_unstamped_row_by_its_insert() {
        let before = chrono::Utc::now().timestamp_millis();
        let temp = TempDir::new().unwrap();
        let read = write_then_read(&sqlite_store(&temp), row(0)).await;
        let at = read
            .instant()
            .expect("a representable stamp")
            .timestamp_millis();
        assert!(
            at >= before,
            "a row nobody dated came back dated {at}, before this test started \
             ({before}) — the zero was stored verbatim and the row now sorts to \
             1970, which is the head of every DELETE-boundary query"
        );
    }

    /// A stamp no calendar can represent must not reach the column either:
    /// `truncate_messages` and `compact_session` pick the boundary of a DELETE
    /// by ranking it, and one absurd magnitude drags a row to an extreme of
    /// that ranking.
    #[tokio::test]
    async fn sqlite_refuses_to_store_an_unrepresentable_stamp() {
        let before = chrono::Utc::now().timestamp_millis();
        let temp = TempDir::new().unwrap();
        let read = write_then_read(&sqlite_store(&temp), row(i64::MIN)).await;
        let at = read
            .instant()
            .expect("fell back to a representable stamp")
            .timestamp_millis();
        assert!(at >= before, "stored an unrepresentable stamp: {at}");
    }
}
