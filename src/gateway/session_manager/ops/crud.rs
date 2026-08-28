use rusqlite::params;
use tracing::debug;

use super::{
    map_session_metadata, SessionManager, SessionManagerError, SessionMetadata, SessionState,
};
use crate::gateway::router::SessionKey;
use crate::gateway::session_store::types::{HistoryPage, MessageRecord};

/// One row for [`SessionManager::add_message_full`] to insert.
///
/// A struct rather than the nine positional parameters it replaces. Three pairs
/// of neighbours there had the same type and entirely different meanings —
/// `input_tokens`/`output_tokens`, `tool_call_id`/`tool_name`, and (once the
/// message's own instant joined them) `source_seq`/`occurred_at`, both
/// `Option<i64>`. Positionally those are silently swappable: a contract with no
/// compiler behind it, which is the shape §10 of CLAUDE.md names for `from_row`
/// and its `SELECT`s. Naming them also retired the `too_many_arguments` waiver.
///
/// Deliberately no `Default` and no `blank()` constructor. Both callers spell
/// every field, so adding one here is a compile error at each of them — which
/// is the same property `add_message_full`'s exhaustive destructure gives on
/// the reading side. A `..NewMessage::blank()` shorthand (this had one for an
/// hour) hands the new field a silent zero at every construction site instead,
/// which is exactly the failure the struct replaced positional arguments to
/// avoid.
pub(crate) struct NewMessage<'a> {
    pub role: &'a str,
    pub content: &'a str,
    pub metadata: Option<&'a str>,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub tool_call_id: Option<&'a str>,
    pub tool_name: Option<&'a str>,
    /// The `session_events` seq this row was projected from, so a rewind can
    /// delete exactly the rows whose source events it retired. `None` for rows
    /// that are not event-sourced.
    pub source_seq: Option<i64>,
    /// When the message HAPPENED, in the producer's own (ambiguous) unit — see
    /// [`MessageRecord::timestamp`]. `None` when the caller has no record to
    /// speak for; `add_message_full` then stamps the row with the insert, which
    /// is what every row got before this field existed.
    ///
    /// [`MessageRecord::timestamp`]: crate::gateway::session_store::types::MessageRecord::timestamp
    pub occurred_at: Option<i64>,
}

impl SessionManager {
    /// Get or create a session
    pub async fn get_or_create(
        &self,
        key: &SessionKey,
    ) -> Result<SessionMetadata, SessionManagerError> {
        let key_str = key.to_key_string();
        let agent_id = key.agent_id().to_string();
        let session_type = super::session_type_str(key);
        let now = chrono::Utc::now().timestamp();

        let conn = self
            .conn
            .lock()
            .map_err(|e| SessionManagerError::DatabaseError(format!("Lock error: {e}")))?;

        // Try to get existing session
        let existing: Option<SessionMetadata> = conn
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
            .ok();

        if let Some(mut meta) = existing {
            // **Audit fix**: explicitly handle the stopped/closed terminal
            // states. The previous code's fallback ("state was not 'created' or
            // 'idle'; just bump last_active_at") ran a bare UPDATE on any
            // non-Active session, including a row that an operator had put
            // into `state='stopped'`. The DB kept `state='stopped'` while the
            // caller saw `meta.state` as `Some(SessionState::Active)` (set by the
            // earlier successful transition OR inferred from last_active_at),
            // and inbound routing continued to treat the session as live. The
            // contract for a stopped session must be explicit: refuse a silent
            // resume; require an explicit reopen.
            match meta.state {
                Some(SessionState::Stopped) => {
                    return Err(SessionManagerError::SessionStopped(meta.key.clone()));
                }
                _ => {}
            }

            // Transition Created or Idle -> Active
            let state_update = conn.execute(
                "UPDATE sessions SET last_active_at = ?, state = 'active' WHERE key = ? AND state IN ('created', 'idle')",
                params![now, &key_str],
            );
            if state_update.is_ok_and(|n| n == 0) {
                // State was not 'created' or 'idle' (e.g., already 'active' or 'running')
                // Just update last_active_at
                conn.execute(
                    "UPDATE sessions SET last_active_at = ? WHERE key = ?",
                    params![now, &key_str],
                )
                .ok();
            } else {
                // State was updated to 'active' - reflect this in returned metadata
                meta.state = Some(SessionState::Active);
            }

            return Ok(meta);
        }

        // Create new session
        let mut meta = SessionMetadata {
            key: key_str,
            agent_id,
            session_type,
            created_at: now,
            last_active_at: now,
            message_count: 0,
            total_tokens: 0,
            auto_reset_at: None,
            state: Some(SessionState::Created),
            topic: None,
            status: None,
            identity_meta: None,
            label: None,
            input_tokens: 0,
            output_tokens: 0,
            model: None,
            model_provider: None,
            parent_session_key: None,
            compaction_count: 0,
            ..Default::default()
        };
        // P1 data isolation: stamp owner/scope from the ambient dispatch
        // scope before persisting. No-op (leaves both `None`) outside any
        // `scope::with_scope` context — cron/internal/A2A creators.
        meta.stamp_attribution();

        conn.execute(
            "INSERT INTO sessions (key, agent_id, session_type, created_at, last_active_at, state, owner_user_id, scope_id)
             VALUES (?, ?, ?, ?, ?, 'created', ?, ?)",
            params![
                &meta.key,
                &meta.agent_id,
                &meta.session_type,
                now,
                now,
                &meta.owner_user_id,
                &meta.scope_id
            ],
        )
        .map_err(|e| SessionManagerError::DatabaseError(format!("Insert failed: {e}")))?;

        debug!("Created session: {}", meta.key);

        Ok(meta)
    }

    /// Add a message to a session.
    ///
    /// (There was an `add_message_with_meta` between this and `add_message_full`.
    /// It was `#[deprecated]`, its only caller was this function, and that caller
    /// passed `None` for every one of its five extra parameters — including the
    /// `model` / `model_provider` pair that `add_message_full` no longer takes.)
    pub async fn add_message(
        &self,
        key: &SessionKey,
        role: &str,
        content: &str,
    ) -> Result<i64, SessionManagerError> {
        self.add_message_full(
            key,
            NewMessage {
                role,
                content,
                metadata: None,
                input_tokens: 0,
                output_tokens: 0,
                tool_call_id: None,
                tool_name: None,
                source_seq: None,
                // No record behind this call, so nothing to say about when the
                // message happened; `add_message_full` stamps it with the
                // insert.
                occurred_at: None,
            },
        )
        .await
    }

    /// Full insert — includes the two tool-tracking columns added in Task 1 and
    /// the `source_seq` back-reference to the `session_events` event this row
    /// was projected from (`None` for rows that are not event-sourced). The
    /// sqlite `append_message` trait impl forwards the real values from the
    /// `MessageRecord` so tool cards survive a Panel reload and `chat.rewind`
    /// can delete exactly the rows whose source events it retired.
    pub(crate) async fn add_message_full(
        &self,
        key: &SessionKey,
        msg: NewMessage<'_>,
    ) -> Result<i64, SessionManagerError> {
        // Exhaustive destructuring: a field added to `NewMessage` is a compile
        // error here rather than a value that silently never reaches the INSERT.
        let NewMessage {
            role,
            content,
            metadata,
            input_tokens,
            output_tokens,
            tool_call_id,
            tool_name,
            source_seq,
            occurred_at,
        } = msg;

        let key_str = key.to_key_string();
        // `now` is the SESSION's activity clock (`sessions.last_active_at`, in
        // seconds). It is no longer also the message's timestamp — see `at_ms`.
        let now = chrono::Utc::now().timestamp();

        // When the message HAPPENED, in milliseconds.
        //
        // This column used to be `now` for EVERY row: `append_message` handed
        // over the producer's stamp and this insert dropped it on the floor, so
        // a SQLite install recorded when the row was WRITTEN rather than when
        // the message occurred — and the file backend, which preserves the
        // producer's stamp, disagreed with SQLite about the same conversation.
        // Live turns hid it (the two instants differ by milliseconds); any path
        // that projects an event later — a reconciler, a backfill, an import —
        // did not.
        //
        // The fallback is the OLD behaviour, narrowed, not a new guess. `None`
        // (the `add_message` convenience, which has no record to speak for),
        // `0` (a producer that left the field at its zero value) and a
        // magnitude no calendar can represent all mean "the producer did not
        // say when". For those, INSERT time is the one true statement available
        // about the row, and it is exactly what all of them got before. So this
        // change only ever REMOVES rows from the set that gets INSERT time; it
        // never adds one, and it never keeps a stamp whose absurd magnitude
        // would drag the row to one extreme of the two queries that rank this
        // column to choose the boundary of a DELETE (`truncate_messages`,
        // `compact_session`).
        //
        // Stored already normalized to milliseconds rather than verbatim: this
        // backend then keeps writing ONE unit going forward. Readers cope
        // either way (`stamp_millis` / `stamp_millis_sql`), but a column that
        // acquires a third mixture is a column every future query has to
        // remember about.
        let at_ms = occurred_at
            .filter(|raw| *raw != 0)
            .map(crate::gateway::session_store::types::stamp_millis)
            .filter(|ms| chrono::DateTime::from_timestamp_millis(*ms).is_some())
            .unwrap_or_else(|| chrono::Utc::now().timestamp_millis());

        // Use scope block to ensure lock is released before any await
        let (message_id, needs_compaction) = {
            // `mut` because the message insert + session counters run inside
            // `conn.transaction()` (see below).
            let mut conn = self
                .conn
                .lock()
                .map_err(|e| SessionManagerError::DatabaseError(format!("Lock error: {e}")))?;

            // **Audit fix**: the message INSERT, FTS sync and the
            // `message_count = message_count + 1` UPDATE previously ran as
            // separate statements with no transaction — a crash between them
            // left `message_count` one behind reality (the value
            // `sessions.list` returns to the Panel). The trio now commits or
            // rolls back together. The FTS insert stays best-effort INSIDE
            // the transaction: a failed FTS write is swallowed (`.ok()`) so
            // search degrades without rolling back the message row — the
            // same tolerance the pre-transaction code had.
            let tx = conn
                .transaction()
                .map_err(|e| SessionManagerError::DatabaseError(format!("Begin tx failed: {e}")))?;

            // Insert message
            tx.execute(
                "INSERT INTO messages (session_key, role, content, timestamp, metadata, \
                 input_tokens, output_tokens, tool_call_id, tool_name, source_seq) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                params![
                    &key_str,
                    role,
                    content,
                    at_ms,
                    metadata,
                    input_tokens,
                    output_tokens,
                    tool_call_id,
                    tool_name,
                    source_seq
                ],
            )
            .map_err(|e| {
                SessionManagerError::DatabaseError(format!("Insert message failed: {e}"))
            })?;

            let message_id = tx.last_insert_rowid();

            // Sync FTS5 index (non-fatal — search degrades gracefully)
            tx.execute(
                "INSERT INTO messages_fts(rowid, content) VALUES (?, ?)",
                params![message_id, content],
            )
            .ok();

            // Only transition to Running if current state allows it
            let valid_transition: bool = tx
                .query_row(
                    "SELECT state FROM sessions WHERE key = ?",
                    params![&key_str],
                    |row| {
                        let state_str: Option<String> = row.get(0)?;
                        Ok(state_str.is_none_or(|s| s != "stopped" && s != "error"))
                    },
                )
                .unwrap_or(true);

            let derived_title: Option<String> = if role == "user" {
                tx.query_row(
                    "SELECT derived_title FROM sessions WHERE key = ?",
                    params![&key_str],
                    |row| row.get::<_, Option<String>>(0),
                )
                .ok()
                .flatten()
                .or_else(|| {
                    let title = content.trim();
                    let title = if title.chars().count() > 60 {
                        title.chars().take(60).collect::<String>() + "..."
                    } else {
                        title.to_string()
                    };
                    if title.is_empty() {
                        None
                    } else {
                        Some(title)
                    }
                })
            } else {
                None
            };

            // Deliberately NOT accumulating this row's tokens onto
            // `sessions.input_tokens` / `output_tokens` / `total_tokens`. This
            // used to, and it was invisible only because every row's tokens were
            // 0 (the projector's feeder event was never emitted). Now that the
            // rows carry real per-call tokens, adding them here as well as in
            // `update_session_usage` — which the run's `AssistantRunMeta` calls
            // with the run's authoritative billed total — would bill the session
            // twice for the same tokens. One writer, and it is the run's report:
            // it also covers the calls a retry discarded before they could ever
            // become a message row.
            let mut session_update_sql = String::from(
                "UPDATE sessions SET last_active_at = ?, message_count = message_count + 1",
            );
            if valid_transition {
                session_update_sql.push_str(", state = 'running'");
            }
            if derived_title.is_some() {
                session_update_sql.push_str(", derived_title = ?");
            }
            session_update_sql.push_str(" WHERE key = ?");

            let mut params: Vec<&dyn rusqlite::ToSql> = vec![&now];
            if let Some(ref dt) = derived_title {
                params.push(dt);
            }
            params.push(&key_str);
            tx.execute(&session_update_sql, params.as_slice())
                .map_err(|e| {
                    SessionManagerError::DatabaseError(format!("Update session failed: {e}"))
                })?;

            tx.commit()
                .map_err(|e| SessionManagerError::DatabaseError(format!("Commit failed: {e}")))?;

            // Check if compaction needed
            let count: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM messages WHERE session_key = ?",
                    params![&key_str],
                    |row| row.get(0),
                )
                .unwrap_or(0);

            (message_id, count as usize > self.config.max_messages)
        }; // Lock released here

        if needs_compaction {
            self.compact_session(key).await?;
        }

        self.emit_session_updated(&key_str);

        Ok(message_id)
    }

    /// Get session history
    /// The SQL spelling of [`stamp_millis`] — `messages.timestamp` normalized to
    /// milliseconds, for ranking and for the `before` cursor.
    ///
    /// Built from [`SECONDS_MILLIS_BOUNDARY`] rather than repeating the literal:
    /// a second copy of that number is a second definition of the rule, which is
    /// the failure the constant's own doc was written to prevent. The Rust and
    /// SQL forms are held to each other by
    /// `stamp_millis_tests::the_sql_and_rust_spellings_agree`, which evaluates
    /// both over the same values in a real connection — the only check that can
    /// see them drift, since neither is expressible in terms of the other.
    ///
    /// There is no index on `messages(timestamp)` (only on `session_key`), so
    /// ordering by this expression costs nothing an index would otherwise have
    /// saved: SQLite was already sorting.
    ///
    /// [`stamp_millis`]: crate::gateway::session_store::types::stamp_millis
    /// [`SECONDS_MILLIS_BOUNDARY`]: crate::gateway::session_store::types::SECONDS_MILLIS_BOUNDARY
    pub(crate) fn stamp_millis_sql() -> String {
        let boundary = crate::gateway::session_store::types::SECONDS_MILLIS_BOUNDARY;
        format!("(CASE WHEN abs(timestamp) >= {boundary} THEN timestamp ELSE timestamp * 1000 END)")
    }

    /// The SELECT behind every windowed read of `messages`.
    ///
    /// One builder rather than a copy per entry point. `get_history` and the
    /// cursor path were near-identical and had already drifted: the cursor
    /// compared the RAW column while the plain path ranked through
    /// [`stamp_millis_sql`], which is why `chat.history?before=…` answered
    /// `{count: 0}` for the largest sessions on a real install.
    ///
    /// [`stamp_millis_sql`]: Self::stamp_millis_sql
    fn history_sql(limit: Option<usize>, cursored: bool) -> String {
        const COLS: &str = "id, role, content, timestamp, metadata, input_tokens, \
                            output_tokens, tool_call_id, tool_name";
        let stamp = Self::stamp_millis_sql();
        // The cursor is compared against the same normalized expression the
        // ordering uses, never the bare column.
        let cursor = if cursored {
            format!(" AND {stamp} < ?")
        } else {
            String::new()
        };
        match limit {
            // Take the most-recent `n` that satisfy the predicate (inner DESC),
            // then re-sort ASC for chronological display.
            Some(n) => format!(
                "SELECT {COLS} FROM ( \
                    SELECT {COLS} FROM messages \
                    WHERE session_key = ?{cursor} ORDER BY {stamp} DESC, id DESC LIMIT {n} \
                 ) ORDER BY {stamp} ASC, id ASC"
            ),
            None => format!(
                "SELECT {COLS} FROM messages \
                 WHERE session_key = ?{cursor} ORDER BY {stamp} ASC, id ASC"
            ),
        }
    }

    /// Positional decode of [`history_sql`]'s column list. The two live
    /// together so the indices and the `SELECT` cannot be edited apart — that
    /// pairing is a contract with no compiler behind it, and it had three
    /// hand-copied instances before this.
    ///
    /// [`history_sql`]: Self::history_sql
    fn map_message_row(row: &rusqlite::Row) -> rusqlite::Result<MessageRecord> {
        Ok(MessageRecord {
            id: row.get::<_, i64>(0)?.to_string(),
            role: row.get(1)?,
            content: row.get(2)?,
            timestamp: row.get(3)?,
            metadata: row
                .get::<_, Option<String>>(4)?
                .and_then(|s| serde_json::from_str(&s).ok()),
            input_tokens: row.get(5)?,
            output_tokens: row.get(6)?,
            tool_call_id: row.get(7)?,
            tool_name: row.get(8)?,
        })
    }

    /// Read a window from a connection the caller already holds, so a caller
    /// that needs two answers about one session can get them without letting go
    /// in between.
    fn read_history_locked(
        conn: &rusqlite::Connection,
        key_str: &str,
        limit: Option<usize>,
        before_ms: Option<i64>,
    ) -> Result<Vec<MessageRecord>, SessionManagerError> {
        let sql = Self::history_sql(limit, before_ms.is_some());
        let mut stmt = conn
            .prepare(&sql)
            .map_err(|e| SessionManagerError::DatabaseError(e.to_string()))?;
        let mut binds: Vec<&dyn rusqlite::ToSql> = vec![&key_str];
        if let Some(ms) = before_ms.as_ref() {
            binds.push(ms);
        }
        let rows = stmt
            .query_map(binds.as_slice(), Self::map_message_row)
            .map_err(|e| SessionManagerError::DatabaseError(e.to_string()))?
            .filter_map(Result::ok)
            .collect();
        Ok(rows)
    }

    /// `COUNT(*)` from a connection the caller already holds.
    ///
    /// Not `read_history_locked(..).len()`: the caller wants one integer, and
    /// materialising every row, every column and every metadata blob of a
    /// transcript to throw all of it away is what the trait's default impl does
    /// because it has no SQL to push down to.
    fn count_history_locked(
        conn: &rusqlite::Connection,
        key_str: &str,
    ) -> Result<usize, SessionManagerError> {
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM messages WHERE session_key = ?",
                params![key_str],
                |row| row.get(0),
            )
            .map_err(|e| SessionManagerError::DatabaseError(e.to_string()))?;
        // `COUNT(*)` is non-negative by construction; the cast is the i64 the
        // driver hands back meeting the usize the trait promises.
        Ok(count.max(0) as usize)
    }

    /// Get session history — the trailing `limit` rows, oldest-first.
    pub async fn get_history(
        &self,
        key: &SessionKey,
        limit: Option<usize>,
    ) -> Result<Vec<MessageRecord>, SessionManagerError> {
        let key_str = key.to_key_string();
        let conn = self
            .conn
            .lock()
            .map_err(|e| SessionManagerError::DatabaseError(format!("Lock error: {e}")))?;
        Self::read_history_locked(&conn, &key_str, limit, None)
    }

    /// The SQL-side [`SessionStore::history_page`]: a window of the transcript
    /// and the length of the whole transcript, under ONE lock.
    ///
    /// One acquisition rather than two, and that is correctness rather than
    /// economy: every writer on this database goes through the same
    /// `Mutex<Connection>`, so holding it across both statements makes the pair
    /// atomic and the two answers describe the same session. Letting go in
    /// between lets an append land there, which is exactly the skew the gateway
    /// used to manage with a comment about which statement should run first.
    ///
    /// A count that fails answers `None` rather than failing the call: the
    /// transcript already succeeded, and "we do not know how long the
    /// conversation is" is honest where reporting the window's own length would
    /// tell the client it is holding the whole thing.
    ///
    /// [`SessionStore::history_page`]: crate::gateway::session_store::SessionStore::history_page
    pub async fn history_page(
        &self,
        key: &SessionKey,
        limit: Option<usize>,
        before: Option<chrono::DateTime<chrono::Utc>>,
    ) -> Result<HistoryPage, SessionManagerError> {
        let key_str = key.to_key_string();
        let conn = self
            .conn
            .lock()
            .map_err(|e| SessionManagerError::DatabaseError(format!("Lock error: {e}")))?;
        let total = Self::count_history_locked(&conn, &key_str).ok();
        let rows = Self::read_history_locked(
            &conn,
            &key_str,
            limit,
            before.map(|b| b.timestamp_millis()),
        )?;
        Ok(HistoryPage { rows, total })
    }

    /// Reset (clear) a session
    pub async fn reset_session(&self, key: &SessionKey) -> Result<bool, SessionManagerError> {
        let key_str = key.to_key_string();
        let conn = self
            .conn
            .lock()
            .map_err(|e| SessionManagerError::DatabaseError(format!("Lock error: {e}")))?;

        // Sync FTS5: remove entries before deleting messages
        conn.execute(
            "DELETE FROM messages_fts WHERE rowid IN (SELECT id FROM messages WHERE session_key = ?)",
            params![&key_str],
        )
        .ok();

        let deleted = conn
            .execute(
                "DELETE FROM messages WHERE session_key = ?",
                params![&key_str],
            )
            .map_err(|e| SessionManagerError::DatabaseError(e.to_string()))?;

        conn.execute(
            "UPDATE sessions SET message_count = 0, last_active_at = ?, state = 'created' WHERE key = ?",
            params![chrono::Utc::now().timestamp(), &key_str],
        )
        .ok();

        debug!("Reset session {}: deleted {} messages", key_str, deleted);

        Ok(deleted > 0)
    }

    /// Delete a session entirely
    pub async fn delete_session(&self, key: &SessionKey) -> Result<bool, SessionManagerError> {
        let key_str = key.to_key_string();
        // Scoped so the connection guard is released before the `.await`
        // below — holding a std `MutexGuard` across a suspension point makes
        // the future `!Send`.
        let deleted = {
            let conn = self
                .conn
                .lock()
                .map_err(|e| SessionManagerError::DatabaseError(format!("Lock error: {e}")))?;

            // Sync FTS5: remove entries before deleting messages
            conn.execute(
                "DELETE FROM messages_fts WHERE rowid IN (SELECT id FROM messages WHERE session_key = ?)",
                params![&key_str],
            )
            .ok();

            // Delete messages first
            conn.execute(
                "DELETE FROM messages WHERE session_key = ?",
                params![&key_str],
            )
            .ok();

            // Delete session
            conn.execute("DELETE FROM sessions WHERE key = ?", params![&key_str])
                .map_err(|e| SessionManagerError::DatabaseError(e.to_string()))?
        };

        // The transcript is gone; the working memory it produced must go with
        // it. The scratchpad plan file sits beside the resume snapshot the
        // delete handler already purges: both are keyed by the *stable*
        // session key, so a session re-created under that key would otherwise
        // inherit the deleted conversation's execution list — and the
        // goal-loop would keep vetoing stop until the new session worked
        // through someone else's plan.
        crate::builtin_tools::scratchpad_registry::purge_session_scratchpad(&key_str).await;

        debug!("Deleted session: {}", key_str);

        Ok(deleted > 0)
    }
}
