//! `SessionEventStore` trait and `SQLite` schema for the session event log.
//!
//! Phase 1 Task 2 adds the backing table. Task 3 introduces the
//! `SessionEventStore` trait defined below. Task 4 adds `SqliteEventStore`,
//! the concrete rusqlite-backed implementation.
//!
//! # Schema
//!
//! Append-only `session_events` log; one row per event. Monotonic ordering per
//! session is enforced by the `(session_id, seq)` primary key. Secondary
//! indexes support the two main inspection queries:
//!
//! - replay/trim by turn: `(session_id, turn_id)`
//! - type-filtered scans (e.g. tool calls only): `(session_id, event_type)`
//!
//! See `docs/superpowers/specs/2026-04-18-session-service-actor-design.md` §7.
//!
//! # Async model
//!
//! Consistent with sibling stores in `src/teams/`, `src/gateway/`, etc. the
//! concrete `SqliteEventStore` wraps a `rusqlite::Connection` in
//! `Arc<tokio::sync::Mutex<_>>` rather than using `spawn_blocking`. All
//! `session_events` queries are short and use prepared statements, so the
//! mutex-hold time is bounded.

use std::borrow::Cow;
use std::sync::Arc;

use crate::capability::{CapabilitySlot, MissingSemantics, SlotStatus};
use async_trait::async_trait;
use rusqlite::{params, Connection, OptionalExtension};
use tokio::sync::Mutex;

use crate::error::AlephError;
use crate::session::events::{
    durability_of, Durability, EventSeq, Retire, SessionEvent, SessionEventRecord,
};
use crate::session::service::{SessionError, SessionId};

#[async_trait]
pub trait SessionEventStore: Send + Sync + 'static {
    /// Append `events` at consecutive seqs starting from `first_seq`, in ONE
    /// transaction, optionally retiring a range in that same transaction.
    ///
    /// This is the only write path: a multi-step durable operation either
    /// lands whole or not at all — manual `/compact` is its summary row, its
    /// checkpoint row and `Retire::Through(cut)` in one call; `chat.rewind` /
    /// `session.truncate` are `Retire::From(seq)` plus the `RunFinished`
    /// closer the cut would otherwise leave owed. Fails — with nothing written
    /// and nothing retired — if any `(session_id, seq)` already exists.
    /// `retire` runs BEFORE the inserts so the batch's own rows stay live.
    /// `durability` decides whether the commit fsyncs the WAL
    /// ([`Durability::Barrier`]) or rides the store's resting level.
    ///
    /// An empty `events` with no `retire` is refused: a "batch" that could do
    /// nothing would report success for nothing. An empty `events` WITH a
    /// `retire` is a retire-only batch and is legal.
    async fn append_batch(
        &self,
        session_id: &SessionId,
        first_seq: EventSeq,
        events: &[(SessionEvent, i64)],
        retire: Option<Retire>,
        durability: Durability,
    ) -> Result<(), SessionError>;

    /// Single append = a batch of one. Kept for direct-store writers and tests.
    ///
    /// Fails if (`session_id`, seq) already exists.
    async fn append(
        &self,
        session_id: &SessionId,
        seq: EventSeq,
        event: &SessionEvent,
        created_at_ms: i64,
    ) -> Result<(), SessionError> {
        let one = [(event.clone(), created_at_ms)];
        self.append_batch(session_id, seq, &one, None, durability_of(event))
            .await
    }

    /// Load all events for a session, ordered by seq ascending.
    async fn load_all_events(
        &self,
        session_id: &SessionId,
    ) -> Result<Vec<SessionEventRecord>, SessionError>;

    /// Load events with seq in [from..to). Either bound may be None.
    async fn load_events_range(
        &self,
        session_id: &SessionId,
        from: Option<EventSeq>,
        to: Option<EventSeq>,
    ) -> Result<Vec<SessionEventRecord>, SessionError>;

    /// Return the highest seq stored for this session, or 0 if none.
    ///
    /// Counts retired events too: seq is an allocation counter, and reusing a
    /// retired seq would collide with its still-present row on the
    /// `(session_id, seq)` primary key.
    async fn load_head_seq(&self, session_id: &SessionId) -> Result<EventSeq, SessionError>;

    /// Retire every event with `seq >= from_seq`, removing it from the live
    /// conversation, as a transaction of its own. Returns how many events this
    /// call newly retired.
    ///
    /// The standalone retire: `chat.clear` / `sessions.reset` /
    /// `sessions.delete` (`retire_live_events`) come through here because they
    /// retire from 1 and append nothing. A retire that must land WITH new rows
    /// (`chat.rewind` / `session.truncate` and their `RunFinished` closer,
    /// manual `/compact` and its summary) is not a second call after
    /// [`append_batch`] — it is the `retire` argument OF that batch
    /// ([`Retire::From`] / [`Retire::Through`]); the head-side `Through` has no
    /// standalone method at all. Same SQL either way (`retire_in_txn`).
    ///
    /// Soft delete: the rows survive, so the append-only log stays intact and
    /// seq allocation is unaffected. All readers of the live conversation
    /// (`load_all_events`, `load_events_range`, `load_run_markers`,
    /// `search_events`) skip retired events, so the model stops replaying them.
    /// The BM25 mirror rows for the range are deleted too — see
    /// [`Retire::From`] for why the two sides differ.
    ///
    /// Idempotent: already-retired events keep their original retirement
    /// timestamp and are not counted again.
    ///
    /// [`append_batch`]: SessionEventStore::append_batch
    /// [`Retire::From`]: crate::session::events::Retire::From
    /// [`Retire::Through`]: crate::session::events::Retire::Through
    async fn retire_from(
        &self,
        session_id: &SessionId,
        from_seq: EventSeq,
    ) -> Result<usize, SessionError>;

    /// True when the event at `seq` exists and has been retired.
    ///
    /// The `messages` projection is drained asynchronously, so an event can be
    /// retired while it still sits in the projector's queue. The projector
    /// re-checks here at WRITE time; without it a `clear` silently un-clears
    /// itself in the transcript milliseconds later.
    ///
    /// Default `Ok(false)` — a store with no soft delete has nothing to hide.
    async fn is_retired(
        &self,
        session_id: &SessionId,
        seq: EventSeq,
    ) -> Result<bool, SessionError> {
        let _ = (session_id, seq);
        Ok(false)
    }

    /// Cross-session scan for resume detection. Returns, per session, that
    /// session's run-marker events — the `MARKER_EVENT_TYPES` set, which is
    /// pinned equal to the reducer's `is_marker` — in `seq` order.
    /// Sessions with no run markers are omitted. Served by the existing
    /// `(session_id, event_type)` index.
    async fn load_run_markers(
        &self,
    ) -> Result<Vec<(SessionId, Vec<SessionEventRecord>)>, SessionError>;

    /// BM25 search over this session's content-bearing events (messages, tool
    /// calls / results / errors). Returns up to `limit` hits, most relevant
    /// first.
    ///
    /// This is the session-continuity counterpart to `ctx_search`: after
    /// compaction evicts old turns from the context window, the events survive
    /// on disk and stay queryable here, so the model can recover "where it
    /// left off" by retrieving only the relevant slices instead of
    /// re-importing the whole history.
    ///
    /// The default returns no hits, so mock / alternative stores remain valid
    /// without implementing search.
    async fn search_events(
        &self,
        session_id: &SessionId,
        query: &str,
        limit: usize,
    ) -> Result<Vec<SessionEventHit>, SessionError> {
        let _ = (session_id, query, limit);
        Ok(Vec::new())
    }
}

/// One BM25 hit from [`SessionEventStore::search_events`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionEventHit {
    /// Sequence number of the matching event within its session.
    pub seq: EventSeq,
    /// Stable event-type tag (e.g. `tool_result`, `user_message`).
    pub event_type: String,
    /// Wall-clock the event was recorded (unix ms).
    pub created_at_ms: i64,
    /// Excerpt of the event body around the match.
    pub snippet: String,
}

/// Create the `session_events` table and its indexes if missing.
///
/// Idempotent — safe to call on every DB open. Uses a savepoint so partial
/// failure leaves the database untouched. Mirrors the pattern used by the
/// other `migrate_add_*` functions in `src/resilience/database/migration.rs`.
pub fn migrate_add_session_events(conn: &Connection) -> Result<(), AlephError> {
    conn.execute_batch("SAVEPOINT migration_session_events")
        .map_err(|e| {
            AlephError::config(format!("Failed to begin session_events migration: {e}"))
        })?;

    let result = conn
        .execute_batch(
            r#"
        CREATE TABLE IF NOT EXISTS session_events (
            session_id   TEXT    NOT NULL,
            seq          INTEGER NOT NULL,
            turn_id      TEXT,
            event_type   TEXT    NOT NULL,
            payload_json TEXT    NOT NULL,
            created_at   INTEGER NOT NULL,
            retired_at   INTEGER,
            PRIMARY KEY (session_id, seq)
        );

        CREATE INDEX IF NOT EXISTS idx_session_events_session_turn
            ON session_events(session_id, turn_id);

        CREATE INDEX IF NOT EXISTS idx_session_events_session_type
            ON session_events(session_id, event_type);
        "#,
        )
        .and_then(|()| add_retired_at_column(conn));

    if let Err(e) = result {
        let _ = conn.execute_batch("ROLLBACK TO migration_session_events");
        return Err(AlephError::config(format!(
            "Failed to create session_events table: {e}"
        )));
    }

    conn.execute_batch("RELEASE migration_session_events")
        .map_err(|e| {
            AlephError::config(format!("Failed to commit session_events migration: {e}"))
        })?;

    Ok(())
}

/// Add the `retired_at` soft-delete column to a pre-existing `session_events`
/// table. `NULL` = live; a unix-ms stamp = retired (see
/// [`SessionEventStore::retire_from`]).
///
/// `SQLite` has no `ADD COLUMN IF NOT EXISTS`, so probe `pragma_table_info`
/// first — a DB created by the current `CREATE TABLE` above already has it.
fn add_retired_at_column(conn: &Connection) -> Result<(), rusqlite::Error> {
    let exists: bool = conn.query_row(
        "SELECT COUNT(*) > 0 FROM pragma_table_info('session_events') WHERE name = 'retired_at'",
        [],
        |row| row.get(0),
    )?;
    if exists {
        return Ok(());
    }
    conn.execute_batch("ALTER TABLE session_events ADD COLUMN retired_at INTEGER")
}

/// Create the `session_events_fts` FTS5 mirror table if missing.
///
/// This is the BM25-searchable companion to `session_events`: every
/// content-bearing event is mirrored here on append (see
/// [`SessionEventStore::append_batch`]) so that, after compaction evicts old turns
/// from the context window, the model can retrieve the relevant slices via the
/// `session_search` tool instead of re-importing the whole history.
///
/// Idempotent. Requires `SQLite` built with FTS5, which `rusqlite`'s `bundled`
/// feature provides — the same prerequisite already relied on by
/// [`crate::context::retrieval::ContentIndex`], so no new dependency. The body
/// is the only indexed column; the rest are `UNINDEXED` storage used for
/// session filtering and result shaping.
pub fn migrate_add_session_events_fts(conn: &Connection) -> Result<(), AlephError> {
    conn.execute_batch(
        "CREATE VIRTUAL TABLE IF NOT EXISTS session_events_fts USING fts5(
             body,
             session_id UNINDEXED,
             seq UNINDEXED,
             event_type UNINDEXED,
             created_at UNINDEXED,
             tokenize = 'porter unicode61'
         );",
    )
    .map_err(|e| AlephError::config(format!("Failed to create session_events_fts table: {e}")))
}

// ---------------------------------------------------------------------------
// SqliteEventStore — rusqlite-backed `SessionEventStore`
// ---------------------------------------------------------------------------

/// rusqlite-backed `SessionEventStore`.
///
/// Holds a single `Connection` under `Arc<tokio::sync::Mutex<_>>`; this matches
/// the async-sqlite pattern used elsewhere in the codebase (`teams::sessions`,
/// `gateway::session_manager`, `resilience::database::state_database`).
pub struct SqliteEventStore {
    conn: Arc<Mutex<Connection>>,
}

impl SqliteEventStore {
    /// Wrap an already-migrated `Connection`. Callers must invoke
    /// [`migrate_add_session_events`] on the connection before use; the
    /// `session_events_fts` BM25 mirror is ensured here automatically so every
    /// construction site (prod + tests) gets searchable events without extra
    /// wiring. Best-effort — if FTS5 is somehow unavailable, indexing simply
    /// degrades and `search_events` returns no hits.
    pub fn new(conn: Connection) -> Self {
        let _ = migrate_add_session_events_fts(&conn);
        Self {
            conn: Arc::new(Mutex::new(conn)),
        }
    }
}

/// One `session_events` row, shaped and encoded before the connection lock is
/// taken so the transaction holds the lock only for the SQL itself.
struct EncodedRow {
    seq: i64,
    turn_id: Option<String>,
    event_type: &'static str,
    payload: String,
    created_at: i64,
    fts_body: Option<String>,
}

/// The ONE payload encoder for `payload_json`. Private seam: the envelope
/// writer replaces it with a `pub encode_row` and deletes this.
fn encode_payload(event: &SessionEvent) -> Result<String, SessionError> {
    Ok(serde_json::to_string(event)?)
}

/// Retire a range inside an already-open transaction. Returns how many rows
/// this newly retired.
///
/// Takes the `Transaction` itself, not a `Connection`: "inside the
/// transaction" is then a type invariant — a caller that wants to retire in
/// autocommit mode (and so let a failed batch keep its retirement) has no
/// value of this type to hand over.
///
/// `retired_at IS NULL` makes both arms idempotent: a second retire of the
/// same range matches nothing and reports 0. An out-of-range seq saturates to
/// `i64::MAX`, which is exact in both arms: `From` then matches no row (it
/// must not widen downward and retire events the caller never named), and
/// `Through` matches every row, which IS "everything at or below a bound no
/// stored seq can exceed".
///
/// `From` also drops the retired rows from the BM25 mirror, or `recall_events`
/// would hand the model the very content `chat.clear` / `chat.rewind` just
/// erased. The FTS table is a derived index, not the log, so a physical
/// delete there does not break the append-only guarantee. `Through` keeps
/// the mirror: compaction evicts turns from the prompt but they must stay
/// recallable, so `recall_events` can still surface a detail the summary
/// abstracted away — deleting the FTS rows would make the "compaction is not
/// a net loss" contract false. `Through` is reached only as the `retire`
/// argument of [`SessionEventStore::append_batch`]; `From` also has the
/// standalone [`SessionEventStore::retire_from`].
fn retire_in_txn(
    tx: &rusqlite::Transaction<'_>,
    session_key: &str,
    retire: Retire,
    at: i64,
) -> rusqlite::Result<usize> {
    match retire {
        Retire::From(from_seq) => {
            let from_val = i64::try_from(from_seq).unwrap_or(i64::MAX);
            let n = tx.execute(
                "UPDATE session_events SET retired_at = ?3
                 WHERE session_id = ?1 AND seq >= ?2 AND retired_at IS NULL",
                params![session_key, from_val, at],
            )?;
            tx.execute(
                "DELETE FROM session_events_fts WHERE session_id = ?1 AND seq >= ?2",
                params![session_key, from_val],
            )?;
            Ok(n)
        }
        Retire::Through(through_seq) => {
            let through_val = i64::try_from(through_seq).unwrap_or(i64::MAX);
            tx.execute(
                "UPDATE session_events SET retired_at = ?3
                 WHERE session_id = ?1 AND seq <= ?2 AND retired_at IS NULL",
                params![session_key, through_val, at],
            )
        }
    }
}

/// Run `f` with `PRAGMA synchronous=FULL` in force, then put the connection
/// back at NORMAL — on every exit path. `f` returns a plain value, so no `?`
/// inside it can skip the restore; the only early return here is a raise that
/// never took effect. A failed restore is logged, not propagated: stuck at
/// FULL is slower, never less durable.
///
/// This is what [`Durability::Barrier`] means at the SQLite level: the
/// commit inside `f` fsyncs the WAL, the commits after it ride NORMAL again.
fn with_synchronous_full<T>(
    conn: &mut Connection,
    f: impl FnOnce(&mut Connection) -> T,
) -> Result<T, SessionError> {
    conn.execute_batch("PRAGMA synchronous=FULL")
        .map_err(|e| SessionError::Storage(format!("PRAGMA synchronous=FULL: {e}")))?;
    let out = f(conn);
    if let Err(e) = conn.execute_batch("PRAGMA synchronous=NORMAL") {
        tracing::warn!(
            error = %e,
            "append_batch: could not restore PRAGMA synchronous=NORMAL"
        );
    }
    Ok(out)
}

/// The transaction itself: `BEGIN IMMEDIATE`, retire, insert every row,
/// `COMMIT`. Any error returns before `commit`, and dropping the
/// `Transaction` uncommitted rolls it back (rusqlite's default
/// `DropBehavior::Rollback`) — so a batch whose third row collides leaves rows
/// one and two behind with it, and leaves the retired range live.
fn write_batch(
    conn: &mut Connection,
    session_key: &str,
    rows: &[EncodedRow],
    retire: Option<Retire>,
    at: i64,
) -> Result<(), SessionError> {
    let tx = conn
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(|e| SessionError::Storage(format!("append_batch BEGIN IMMEDIATE failed: {e}")))?;
    // Retire FIRST so the batch's own rows, appended after, stay live.
    if let Some(r) = retire {
        retire_in_txn(&tx, session_key, r, at).map_err(|e| SessionError::Storage(e.to_string()))?;
    }
    for row in rows {
        tx.execute(
            "INSERT INTO session_events
             (session_id, seq, turn_id, event_type, payload_json, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                session_key,
                row.seq,
                row.turn_id,
                row.event_type,
                row.payload,
                row.created_at,
            ],
        )
        .map_err(|e| SessionError::Storage(e.to_string()))?;
    }
    tx.commit()
        .map_err(|e| SessionError::Storage(format!("append_batch COMMIT failed: {e}")))
}

#[async_trait]
impl SessionEventStore for SqliteEventStore {
    async fn append_batch(
        &self,
        session_id: &SessionId,
        first_seq: EventSeq,
        events: &[(SessionEvent, i64)],
        retire: Option<Retire>,
        durability: Durability,
    ) -> Result<(), SessionError> {
        if events.is_empty() && retire.is_none() {
            return Err(SessionError::Other(
                "append_batch: empty batch with nothing to retire".into(),
            ));
        }
        let session_key = session_id_to_string(session_id)?;
        let mut rows = Vec::with_capacity(events.len());
        for (i, (event, at)) in events.iter().enumerate() {
            let seq = first_seq
                .checked_add(i as u64)
                .ok_or_else(|| SessionError::Storage("seq overflow".into()))?;
            let seq = i64::try_from(seq)
                .map_err(|_| SessionError::Storage(format!("seq {seq} exceeds i64::MAX")))?;
            rows.push(EncodedRow {
                seq,
                turn_id: extract_turn_id(event).map(|u| u.to_string()),
                event_type: event_type_tag(event),
                payload: encode_payload(event)?,
                created_at: *at,
                fts_body: render_event_text(event),
            });
        }
        let at = crate::session::events::now_ms();

        let mut conn = self.conn.lock().await;
        // A Barrier fsyncs the WAL at THIS commit only; a Normal batch never
        // touches the pragma, so whatever level the connection was opened at
        // is exactly what it commits under.
        match durability {
            Durability::Barrier => with_synchronous_full(&mut conn, |c| {
                write_batch(c, &session_key, &rows, retire, at)
            })??,
            Durability::Normal => write_batch(&mut conn, &session_key, &rows, retire, at)?,
        }

        // Mirror content-bearing events into the FTS index so prior turns stay
        // BM25-searchable after compaction evicts them from context. Strictly
        // best-effort and outside the transaction: an indexing failure must
        // never block the authoritative append above (continuity of the log
        // outranks searchability).
        for row in rows.iter().filter(|r| r.fts_body.is_some()) {
            if let Err(e) = conn.execute(
                "INSERT INTO session_events_fts
                 (body, session_id, seq, event_type, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    row.fts_body,
                    session_key,
                    row.seq,
                    row.event_type,
                    row.created_at
                ],
            ) {
                tracing::debug!(
                    error = %e,
                    "session_events_fts index insert failed; session_search degraded"
                );
            }
        }

        Ok(())
    }

    async fn load_all_events(
        &self,
        session_id: &SessionId,
    ) -> Result<Vec<SessionEventRecord>, SessionError> {
        self.load_events_range(session_id, None, None).await
    }

    async fn load_events_range(
        &self,
        session_id: &SessionId,
        from: Option<EventSeq>,
        to: Option<EventSeq>,
    ) -> Result<Vec<SessionEventRecord>, SessionError> {
        let session_key = session_id_to_string(session_id)?;
        // An out-of-range `from` (> i64::MAX) must not silently fall back to 0,
        // which would widen the lower bound and return events earlier than requested.
        // Saturate to i64::MAX so an overflowing lower bound matches no rows instead.
        let from_val = i64::try_from(from.unwrap_or(0)).unwrap_or(i64::MAX);
        let to_val = to.and_then(|v| i64::try_from(v).ok()).unwrap_or(i64::MAX);

        let conn = self.conn.lock().await;
        let mut stmt = conn
            .prepare(
                "SELECT seq, payload_json, created_at
                 FROM session_events
                 WHERE session_id = ?1 AND seq >= ?2 AND seq < ?3
                   AND retired_at IS NULL
                 ORDER BY seq ASC",
            )
            .map_err(|e| SessionError::Storage(e.to_string()))?;

        let rows = stmt
            .query_map(params![session_key, from_val, to_val], |row| {
                let seq: i64 = row.get(0)?;
                let payload: String = row.get(1)?;
                let created_at: i64 = row.get(2)?;
                Ok((seq, payload, created_at))
            })
            .map_err(|e| SessionError::Storage(e.to_string()))?;

        let mut out = Vec::new();
        for row in rows {
            let (seq, payload, created_at) =
                row.map_err(|e| SessionError::Storage(e.to_string()))?;
            let event: SessionEvent = serde_json::from_str(&payload)?;
            let seq = u64::try_from(seq)
                .map_err(|_| SessionError::Storage(format!("stored seq {seq} is negative")))?;
            out.push(SessionEventRecord {
                seq,
                event,
                created_at_ms: created_at,
            });
        }
        Ok(out)
    }

    async fn load_head_seq(&self, session_id: &SessionId) -> Result<EventSeq, SessionError> {
        let session_key = session_id_to_string(session_id)?;

        let conn = self.conn.lock().await;
        let max_seq: Option<i64> = conn
            .query_row(
                "SELECT MAX(seq) FROM session_events WHERE session_id = ?1",
                params![session_key],
                |row| row.get::<_, Option<i64>>(0),
            )
            .optional()
            .map_err(|e| SessionError::Storage(e.to_string()))?
            .flatten();

        let head = match max_seq {
            Some(v) if v >= 0 => v as u64,
            Some(v) => return Err(SessionError::Storage(format!("stored seq {v} is negative"))),
            None => 0,
        };
        Ok(head)
    }

    async fn retire_from(
        &self,
        session_id: &SessionId,
        from_seq: EventSeq,
    ) -> Result<usize, SessionError> {
        let session_key = session_id_to_string(session_id)?;
        let at = crate::session::events::now_ms();

        let mut conn = self.conn.lock().await;
        // The UPDATE and the FTS DELETE in `retire_in_txn` must share one
        // transaction or a partial failure (e.g. disk-full mid-statement)
        // leaves rows marked retired while their content stays in the BM25
        // mirror — exactly the leak this method exists to prevent. Dropping
        // the `Transaction` on the error path rolls back.
        let tx = conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(|e| SessionError::Storage(format!("retire_from BEGIN failed: {e}")))?;
        let n = retire_in_txn(&tx, &session_key, Retire::From(from_seq), at)
            .map_err(|e| SessionError::Storage(e.to_string()))?;
        tx.commit()
            .map_err(|e| SessionError::Storage(format!("retire_from COMMIT failed: {e}")))?;
        Ok(n)
    }

    async fn is_retired(
        &self,
        session_id: &SessionId,
        seq: EventSeq,
    ) -> Result<bool, SessionError> {
        let session_key = session_id_to_string(session_id)?;
        let seq_i64 = i64::try_from(seq)
            .map_err(|_| SessionError::Storage(format!("seq {seq} exceeds i64::MAX")))?;

        let conn = self.conn.lock().await;
        let retired: Option<bool> = conn
            .query_row(
                "SELECT retired_at IS NOT NULL FROM session_events
                 WHERE session_id = ?1 AND seq = ?2",
                params![session_key, seq_i64],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| SessionError::Storage(e.to_string()))?;
        // An unknown seq is not retired: the projector's queue can only carry
        // events that were appended, so this is a store the event never
        // reached (tests, alternative store) — nothing to withhold.
        Ok(retired.unwrap_or(false))
    }

    async fn load_run_markers(
        &self,
    ) -> Result<Vec<(SessionId, Vec<SessionEventRecord>)>, SessionError> {
        let conn = self.conn.lock().await;
        // The IN-list is rendered from `MARKER_EVENT_TYPES`, never spelled
        // inline: the tags are `event_type_tag` literals this module owns (no
        // user input reaches this string), and the constant is what the
        // equality census against `reduction::is_marker` reads.
        let in_list = MARKER_EVENT_TYPES
            .iter()
            .map(|t| format!("'{t}'"))
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "SELECT session_id, seq, payload_json, created_at
             FROM session_events
             WHERE event_type IN ({in_list})
               AND retired_at IS NULL
             ORDER BY session_id, seq ASC"
        );
        let mut stmt = conn
            .prepare(&sql)
            .map_err(|e| SessionError::Storage(e.to_string()))?;

        let rows = stmt
            .query_map([], |row| {
                let session_id: String = row.get(0)?;
                let seq: i64 = row.get(1)?;
                let payload: String = row.get(2)?;
                let created_at: i64 = row.get(3)?;
                Ok((session_id, seq, payload, created_at))
            })
            .map_err(|e| SessionError::Storage(e.to_string()))?;

        // Group consecutive rows by session_id. The SQL `ORDER BY
        // session_id, seq` guarantees all of one session's markers are
        // contiguous, so a running group key is enough — no HashMap.
        let mut grouped: Vec<(SessionId, Vec<SessionEventRecord>)> = Vec::new();
        for row in rows {
            let (session_id_str, seq, payload, created_at) =
                row.map_err(|e| SessionError::Storage(e.to_string()))?;
            let session_id: SessionId = serde_json::from_str(&session_id_str)?;
            let event: SessionEvent = serde_json::from_str(&payload)?;
            let seq = u64::try_from(seq)
                .map_err(|_| SessionError::Storage(format!("stored seq {seq} is negative")))?;
            let record = SessionEventRecord {
                seq,
                event,
                created_at_ms: created_at,
            };
            match grouped.last_mut() {
                Some((sid, records)) if *sid == session_id => {
                    records.push(record);
                }
                _ => grouped.push((session_id, vec![record])),
            }
        }
        Ok(grouped)
    }

    async fn search_events(
        &self,
        session_id: &SessionId,
        query: &str,
        limit: usize,
    ) -> Result<Vec<SessionEventHit>, SessionError> {
        // Reuse the same FTS5 query hardening as the offloaded-output index.
        let Some(match_expr) = crate::context::retrieval::sanitize_fts_query(query) else {
            return Ok(Vec::new());
        };
        let session_key = session_id_to_string(session_id)?;
        let limit_i64 = i64::try_from(limit).unwrap_or(i64::MAX);

        let conn = self.conn.lock().await;
        let mut stmt = conn
            .prepare(
                "SELECT seq, event_type, created_at,
                        snippet(session_events_fts, 0, '', '', ' … ', 14) AS snip
                 FROM session_events_fts
                 WHERE session_events_fts MATCH ?1 AND session_id = ?2
                 ORDER BY bm25(session_events_fts)
                 LIMIT ?3",
            )
            .map_err(|e| SessionError::Storage(e.to_string()))?;

        let rows = stmt
            .query_map(params![match_expr, session_key, limit_i64], |row| {
                let seq: i64 = row.get(0)?;
                let event_type: String = row.get(1)?;
                let created_at: i64 = row.get(2)?;
                let snippet: String = row.get(3)?;
                Ok((seq, event_type, created_at, snippet))
            })
            .map_err(|e| SessionError::Storage(e.to_string()))?;

        let mut hits = Vec::new();
        for row in rows {
            let (seq, event_type, created_at, snippet) =
                row.map_err(|e| SessionError::Storage(e.to_string()))?;
            let seq = u64::try_from(seq)
                .map_err(|_| SessionError::Storage(format!("stored seq {seq} is negative")))?;
            hits.push(SessionEventHit {
                seq,
                event_type,
                created_at_ms: created_at,
                snippet,
            });
        }
        Ok(hits)
    }
}

// ---------------------------------------------------------------------------
// Row-shaping helpers
// ---------------------------------------------------------------------------

/// Canonical string form of a `SessionId` for the `session_id` column.
///
/// Uses `serde_json::to_string` so the persisted form round-trips losslessly
/// through `serde` and remains stable against any future `Display`
/// refactors on `SessionKey`.
fn session_id_to_string(id: &SessionId) -> Result<String, SessionError> {
    serde_json::to_string(id)
        .map_err(|e| SessionError::Storage(format!("failed to serialize session_id: {e}")))
}

/// Extract the `turn_id` from any `SessionEvent` variant that carries one,
/// so it can be indexed for per-turn replay/trim.
const fn extract_turn_id(event: &SessionEvent) -> Option<uuid::Uuid> {
    match event {
        SessionEvent::TurnStarted { turn_id, .. }
        | SessionEvent::UserMessage { turn_id, .. }
        | SessionEvent::AssistantMessage { turn_id, .. }
        | SessionEvent::SystemMessage { turn_id, .. }
        | SessionEvent::ToolCallRequested { turn_id, .. }
        | SessionEvent::ToolCallApproved { turn_id, .. }
        | SessionEvent::ToolCallDenied { turn_id, .. }
        | SessionEvent::ToolResult { turn_id, .. }
        | SessionEvent::ToolError { turn_id, .. }
        | SessionEvent::SubagentSpawned { turn_id, .. }
        | SessionEvent::SubagentReturned { turn_id, .. }
        | SessionEvent::AssistantRunMeta { turn_id, .. } => Some(*turn_id),
        SessionEvent::Error { turn_id, .. } => *turn_id,
        SessionEvent::SessionWoken { .. }
        | SessionEvent::SessionForked { .. }
        | SessionEvent::RunStarted { .. }
        | SessionEvent::RunFinished { .. }
        | SessionEvent::ResumeAttempted { .. }
        | SessionEvent::CompactionPerformed { .. } => None,
    }
}

/// The `event_type_tag` of every variant `reduction::is_marker` accepts —
/// the one list `load_run_markers` selects by. Pinned equal by test
/// (`tests::marker_event_types_are_exactly_the_reducers_marker_set`).
pub(crate) const MARKER_EVENT_TYPES: [&str; 3] =
    ["run_started", "run_finished", "resume_attempted"];

/// Static discriminant string for the `event_type` column.
///
/// Kept as a `&'static str` to avoid per-append allocation and to give the
/// storage layer a stable taxonomy independent of serde rename decisions.
// rust-doctor-disable-next-line high-cyclomatic-complexity
pub(crate) const fn event_type_tag(event: &SessionEvent) -> &'static str {
    match event {
        SessionEvent::SessionWoken { .. } => "session_woken",
        SessionEvent::RunStarted { .. } => "run_started",
        SessionEvent::RunFinished { .. } => "run_finished",
        SessionEvent::ResumeAttempted { .. } => "resume_attempted",
        SessionEvent::TurnStarted { .. } => "turn_started",
        SessionEvent::UserMessage { .. } => "user_message",
        SessionEvent::AssistantMessage { .. } => "assistant_message",
        SessionEvent::AssistantRunMeta { .. } => "assistant_run_meta",
        SessionEvent::SystemMessage { .. } => "system_message",
        SessionEvent::ToolCallRequested { .. } => "tool_call_requested",
        SessionEvent::ToolCallApproved { .. } => "tool_call_approved",
        SessionEvent::ToolCallDenied { .. } => "tool_call_denied",
        SessionEvent::ToolResult { .. } => "tool_result",
        SessionEvent::ToolError { .. } => "tool_error",
        SessionEvent::SubagentSpawned { .. } => "subagent_spawned",
        SessionEvent::SubagentReturned { .. } => "subagent_returned",
        SessionEvent::CompactionPerformed { .. } => "compaction_performed",
        SessionEvent::SessionForked { .. } => "session_forked",
        SessionEvent::Error { .. } => "error",
    }
}

// ---------------------------------------------------------------------------
// FTS body extraction
// ---------------------------------------------------------------------------

/// Max characters mirrored into the FTS body for a single event. Tool results
/// can be large; capping keeps the index lean while preserving enough text for
/// a meaningful BM25 match and snippet.
const MAX_FTS_BODY_CHARS: usize = 8_000;

/// Extract the searchable text for an event, or `None` for pure control events
/// (turn / run / session / llm markers, approvals, budget ticks) that carry no
/// content worth indexing.
///
/// This is mechanical field extraction — not semantic classification — so it
/// stays on the right side of R7 (LLM sovereignty): the model decides what is
/// relevant via its query; we only surface the raw text it can match against.
fn render_event_text(event: &SessionEvent) -> Option<String> {
    let raw: Cow<'_, str> = match event {
        SessionEvent::UserMessage { content, .. } => Cow::Borrowed(&content.text),
        SessionEvent::AssistantMessage { content, .. } => Cow::Borrowed(&content.text),
        SessionEvent::SystemMessage { content, .. } => Cow::Borrowed(content),
        SessionEvent::ToolCallRequested { name, input, .. } => {
            Cow::Owned(format!("{name} {input}"))
        }
        SessionEvent::ToolResult { output, .. } => render_json(&output.value),
        SessionEvent::ToolError { error, .. } => Cow::Borrowed(error),
        SessionEvent::ToolCallDenied { reason, .. } => Cow::Borrowed(reason),
        SessionEvent::SubagentReturned { summary, .. } => Cow::Borrowed(summary),
        SessionEvent::Error { message, .. } => Cow::Borrowed(message),
        _ => return None,
    };
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(cap_chars(trimmed, MAX_FTS_BODY_CHARS))
    }
}

/// Render a tool-output JSON value to searchable plain text: bare strings pass
/// through unquoted (the common case — most tool outputs are strings); other
/// shapes fall back to compact JSON so their tokens are still matchable.
fn render_json(value: &serde_json::Value) -> Cow<'_, str> {
    match value {
        serde_json::Value::String(s) => Cow::Borrowed(s),
        other => Cow::Owned(other.to_string()),
    }
}

/// UTF-8-safe truncation to at most `max` characters (project rule P7).
fn cap_chars(s: &str, max: usize) -> String {
    match s.char_indices().nth(max) {
        Some((byte_idx, _)) => s[..byte_idx].to_string(),
        None => s.to_string(),
    }
}

// ---------------------------------------------------------------------------
// Process-wide accessor
// ---------------------------------------------------------------------------

/// `ConsumerDecides`, and this handle is the sharper case of the pair in this
/// batch: the production reads produce *different* answers, some of which
/// are reported to the caller as success.
///
/// | reader | an uninstalled read becomes |
/// |---|---|
/// | `builtin_tools/recall_events.rs` | `Ok(empty)` plus a note to the model |
/// | [`retire_live_events`] | `Ok(0)` — "retired nothing", indistinguishable from "there was nothing to retire" |
/// | `gateway/session_projector.rs` | reads `is_retired` on its own store handle (no process-wide accessor needed) |
/// | `gateway/execution_engine/run_loop/inner.rs` | the legacy backfill is skipped in silence |
///
/// (`builtin_tools/sessions/compact_tool.rs` left this table on 2026-09-12:
/// `/compact` now writes through the session service's `emit_batch` and reads
/// no store handle of its own.)
///
/// Each arm is individually defensible (all doc-comment their reasoning),
/// which is exactly why no `IndistinguishableDefault { reads_as }` sentence
/// could be written for this slot: there is no single thing a missing handle
/// reads as. Task 15 adjudicates the arms; this variant records that there is
/// more than one of them — recount rather than inherit a number.
static GLOBAL_EVENT_STORE: CapabilitySlot<Arc<dyn SessionEventStore>> =
    CapabilitySlot::new("session/event-store", MissingSemantics::ConsumerDecides);

/// Install the process-wide session event store. Called once at daemon boot
/// (`aleph-server start`) so the `session_search` builtin tool can reach the
/// event log without threading dependencies through the `AlephTool` trait.
/// Mirrors [`crate::tools::result_store::set_global_tool_result_store`].
/// Idempotent: a second call is ignored.
#[inline]
pub fn set_global_session_event_store(store: Arc<dyn SessionEventStore>) {
    let _ = GLOBAL_EVENT_STORE.install(store);
}

/// Record that boot reached this slot and had nothing to install.
///
/// The `else` half of [`set_global_session_event_store`]: boot's install is
/// conditional, and without this the five consumers named on the static above
/// cannot tell "this deployment has no session-event log" from "boot died
/// before it reached `build_sqlite_session_service`'s call site in
/// `start_server`". Named rather than numbered on purpose — a line coordinate
/// in this file would be a carried number for a file that has no reason to
/// track `start/mod.rs`. `because` is quoted verbatim to an operator.
#[inline]
pub fn decline_global_session_event_store(because: &'static str) {
    GLOBAL_EVENT_STORE.decline(because);
}

/// Fetch the process-wide session event store, if one has been installed.
///
/// ⚠️ `None` says nothing about whether boot reached this slot. Ask
/// [`global_session_event_store_slot`]`().outcome()` for that.
#[inline]
pub fn global_session_event_store() -> Option<Arc<dyn SessionEventStore>> {
    GLOBAL_EVENT_STORE.get().cloned()
}

/// The handle above, type-erased for the roster — see
/// [`crate::spend::global_ledger_slot`] for why this shape.
pub(crate) const fn global_session_event_store_slot() -> &'static dyn SlotStatus {
    &GLOBAL_EVENT_STORE
}

/// Retire every live event at or after `from_seq` in the process-wide event log
/// (`from_seq = 1` clears the session outright). Returns how many events this
/// call newly retired.
///
/// The gateway's `messages` table is only the Panel's read projection: clearing
/// it while the event log survives leaves the model replaying everything the
/// user thought they had deleted. Callers that clear or rewind a conversation
/// must come through here first.
///
/// `Ok(0)` when no store is installed (CLI one-shot, tests) — there is no event
/// log, hence nothing that could still be replayed.
pub async fn retire_live_events(
    session_id: &SessionId,
    from_seq: EventSeq,
) -> Result<usize, SessionError> {
    match global_session_event_store() {
        Some(store) => store.retire_from(session_id, from_seq).await,
        None => Ok(0),
    }
}

/// The process-wide event store used by tests that need the real
/// `retire_live_events` path (the handlers reach the store through the
/// process-wide slot above, so they cannot be handed one).
///
/// A single shared in-memory store: `set_global_session_event_store` only ever
/// honours the first call, so every test must install the SAME instance or the
/// losers would silently observe a store they never wrote to. Tests keep to
/// their own session keys.
#[cfg(test)]
pub(crate) fn install_test_event_store() -> Arc<SqliteEventStore> {
    static TEST_STORE: std::sync::OnceLock<Arc<SqliteEventStore>> = std::sync::OnceLock::new();
    let store = TEST_STORE
        .get_or_init(|| {
            let conn = Connection::open_in_memory().expect("in-memory sqlite");
            migrate_add_session_events(&conn).expect("migrate session_events");
            Arc::new(SqliteEventStore::new(conn))
        })
        .clone();
    set_global_session_event_store(store.clone());
    store
}

// Note (severed-wire-2026-09-05-modules2 session I-3):
// the process-wide accessor `is_event_retired` was deleted — the projector
// reaches the trait method directly on its own `Arc<dyn SessionEventStore>`
// handle (gateway/session_projector.rs:650), so the slot indirection had no
// caller and the doc-cited "consulted by the projector before it writes a
// row" contract was unwired. If a future caller needs the global accessor,
// re-introduce it together with a real call site — the half-wired slot was
// worse than either end state.

/// Test instrument for "this operation is ONE store transaction".
#[cfg(test)]
pub(crate) mod test_support {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    /// A real SQLite store that counts its write entry points, so a test can
    /// assert "one transaction" as a NUMBER instead of trusting the caller.
    ///
    /// The two counted methods are the two ways a caller can change the live
    /// log (`append_batch`, which carries the batch's retire, and the
    /// standalone `retire_from`). `append` is deliberately NOT overridden: the
    /// trait default routes it through `self.append_batch`, so a single-row
    /// write is counted too — every write that reaches this store is a batch
    /// it counted. "One transaction" therefore reads as
    /// `append_batches == 1 && retire_froms == 0`.
    pub(crate) struct CountingStore {
        pub inner: Arc<SqliteEventStore>,
        pub append_batches: AtomicUsize,
        pub retire_froms: AtomicUsize,
    }

    impl CountingStore {
        /// A fresh in-memory store, migrated, with every counter at zero.
        pub(crate) fn in_memory() -> Arc<Self> {
            let conn = Connection::open_in_memory().expect("in-memory sqlite");
            migrate_add_session_events(&conn).expect("migrate session_events");
            Arc::new(Self {
                inner: Arc::new(SqliteEventStore::new(conn)),
                append_batches: AtomicUsize::new(0),
                retire_froms: AtomicUsize::new(0),
            })
        }
    }

    #[async_trait]
    impl SessionEventStore for CountingStore {
        async fn append_batch(
            &self,
            session_id: &SessionId,
            first_seq: EventSeq,
            events: &[(SessionEvent, i64)],
            retire: Option<Retire>,
            durability: Durability,
        ) -> Result<(), SessionError> {
            self.append_batches
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            self.inner
                .append_batch(session_id, first_seq, events, retire, durability)
                .await
        }

        async fn load_all_events(
            &self,
            session_id: &SessionId,
        ) -> Result<Vec<SessionEventRecord>, SessionError> {
            self.inner.load_all_events(session_id).await
        }

        async fn load_events_range(
            &self,
            session_id: &SessionId,
            from: Option<EventSeq>,
            to: Option<EventSeq>,
        ) -> Result<Vec<SessionEventRecord>, SessionError> {
            self.inner.load_events_range(session_id, from, to).await
        }

        async fn load_head_seq(&self, session_id: &SessionId) -> Result<EventSeq, SessionError> {
            self.inner.load_head_seq(session_id).await
        }

        async fn retire_from(
            &self,
            session_id: &SessionId,
            from_seq: EventSeq,
        ) -> Result<usize, SessionError> {
            self.retire_froms
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            self.inner.retire_from(session_id, from_seq).await
        }

        async fn is_retired(
            &self,
            session_id: &SessionId,
            seq: EventSeq,
        ) -> Result<bool, SessionError> {
            self.inner.is_retired(session_id, seq).await
        }

        async fn load_run_markers(
            &self,
        ) -> Result<Vec<(SessionId, Vec<SessionEventRecord>)>, SessionError> {
            self.inner.load_run_markers().await
        }

        async fn search_events(
            &self,
            session_id: &SessionId,
            query: &str,
            limit: usize,
        ) -> Result<Vec<SessionEventHit>, SessionError> {
            self.inner.search_events(session_id, query, limit).await
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ========================================================================
    // The process-global handle, as a capability slot
    // ========================================================================

    /// See `session::service::tests::the_accessor_exposes_this_handle_to_the_roster`
    /// for why this asserts through the accessor rather than the static.
    #[test]
    fn the_accessor_exposes_this_handle_to_the_roster() {
        let slot = global_session_event_store_slot();
        assert_eq!(slot.id(), "session/event-store");
        assert!(matches!(slot.missing(), MissingSemantics::ConsumerDecides));
    }

    #[test]
    fn migrate_creates_session_events_table_and_indexes() {
        let conn = Connection::open_in_memory().unwrap();
        migrate_add_session_events(&conn).unwrap();

        // Table exists with expected columns in the expected order.
        let columns: Vec<String> = {
            let mut stmt = conn
                .prepare("SELECT name FROM pragma_table_info('session_events') ORDER BY cid ASC")
                .unwrap();
            stmt.query_map([], |row| row.get::<_, String>(0))
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap()
        };
        assert_eq!(
            columns,
            vec![
                "session_id",
                "seq",
                "turn_id",
                "event_type",
                "payload_json",
                "created_at",
                "retired_at",
            ]
        );

        // Primary key is (session_id, seq).
        let pk_cols: Vec<String> = {
            let mut stmt = conn
                .prepare(
                    "SELECT name FROM pragma_table_info('session_events') \
                     WHERE pk > 0 ORDER BY pk ASC",
                )
                .unwrap();
            stmt.query_map([], |row| row.get::<_, String>(0))
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap()
        };
        assert_eq!(pk_cols, vec!["session_id", "seq"]);

        // Both secondary indexes exist.
        for idx in [
            "idx_session_events_session_turn",
            "idx_session_events_session_type",
        ] {
            let found: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name=?1",
                    [idx],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(found, 1, "expected index {} to exist", idx);
        }
    }

    #[test]
    fn migrate_is_idempotent() {
        let conn = Connection::open_in_memory().unwrap();
        migrate_add_session_events(&conn).unwrap();
        migrate_add_session_events(&conn).unwrap();
        migrate_add_session_events(&conn).unwrap();

        let table_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master \
                 WHERE type='table' AND name='session_events'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(table_count, 1);
    }

    // -----------------------------------------------------------------------
    // SqliteEventStore tests
    // -----------------------------------------------------------------------

    use crate::routing::session_key::SessionKey;
    use crate::session::events::{now_ms, MessageContent, TurnTrigger};

    fn make_store() -> SqliteEventStore {
        let conn = Connection::open_in_memory().unwrap();
        migrate_add_session_events(&conn).unwrap();
        SqliteEventStore::new(conn)
    }

    fn sample_session_id() -> SessionId {
        SessionKey::ephemeral("test")
    }

    fn turn_started(tid: uuid::Uuid, at: i64) -> SessionEvent {
        SessionEvent::TurnStarted {
            turn_id: tid,
            trigger: TurnTrigger::UserMessage,
            at,
        }
    }

    fn run_started(run_id: &str, at: i64) -> SessionEvent {
        SessionEvent::RunStarted {
            run_id: run_id.to_string(),
            at,
            project_root: None,
            envelope: None,
        }
    }

    fn run_finished(run_id: &str, at: i64) -> SessionEvent {
        SessionEvent::RunFinished {
            run_id: run_id.to_string(),
            outcome: crate::session::events::RunOutcome::Completed,
            at,
        }
    }

    #[tokio::test]
    async fn append_and_load_preserves_order() {
        let store = make_store();
        let sid = sample_session_id();
        let tid = uuid::Uuid::new_v4();
        let at = now_ms();

        let e1 = turn_started(tid, at);
        let e2 = SessionEvent::UserMessage {
            turn_id: tid,
            content: MessageContent {
                text: "hi".into(),
                blocks: vec![],
                thinking: None,
                thinking_signature: None,
            },
            at: at + 1,
            synthetic: false,
            author_user_id: None,
        };

        store.append(&sid, 1, &e1, at).await.unwrap();
        store.append(&sid, 2, &e2, at + 1).await.unwrap();

        let loaded = store.load_all_events(&sid).await.unwrap();
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0].seq, 1);
        assert_eq!(loaded[1].seq, 2);
        assert!(matches!(loaded[0].event, SessionEvent::TurnStarted { .. }));
        assert!(matches!(loaded[1].event, SessionEvent::UserMessage { .. }));
        assert_eq!(loaded[0].created_at_ms, at);
        assert_eq!(loaded[1].created_at_ms, at + 1);
    }

    #[tokio::test]
    async fn load_range_uses_half_open_upper_bound() {
        let store = make_store();
        let sid = sample_session_id();
        let tid = uuid::Uuid::new_v4();
        let at = now_ms();
        for seq in 1..=4u64 {
            store
                .append(&sid, seq, &turn_started(tid, at), at)
                .await
                .unwrap();
        }

        let events = store
            .load_events_range(&sid, Some(2), Some(4))
            .await
            .unwrap();
        assert_eq!(
            events.iter().map(|event| event.seq).collect::<Vec<_>>(),
            vec![2, 3]
        );
    }

    #[tokio::test]
    async fn duplicate_seq_rejected() {
        let store = make_store();
        let sid = sample_session_id();
        let tid = uuid::Uuid::new_v4();
        let at = now_ms();
        let e = turn_started(tid, at);

        store.append(&sid, 1, &e, at).await.unwrap();
        let err = store.append(&sid, 1, &e, at).await.unwrap_err();
        assert!(
            matches!(err, SessionError::Storage(_)),
            "expected Storage error on duplicate seq, got {err:?}"
        );
    }

    #[tokio::test]
    async fn head_seq_empty_is_zero() {
        let store = make_store();
        let sid = sample_session_id();
        assert_eq!(store.load_head_seq(&sid).await.unwrap(), 0);
    }

    #[tokio::test]
    async fn head_seq_returns_max() {
        let store = make_store();
        let sid = sample_session_id();
        let tid = uuid::Uuid::new_v4();
        let at = now_ms();
        let e = turn_started(tid, at);

        store.append(&sid, 1, &e, at).await.unwrap();
        store.append(&sid, 2, &e, at).await.unwrap();
        store.append(&sid, 5, &e, at).await.unwrap();

        assert_eq!(store.load_head_seq(&sid).await.unwrap(), 5);
    }

    #[tokio::test]
    async fn load_run_markers_empty_when_no_markers() {
        let store = make_store();
        let sid = sample_session_id();
        let tid = uuid::Uuid::new_v4();
        let at = now_ms();
        store
            .append(&sid, 1, &turn_started(tid, at), at)
            .await
            .unwrap();
        let markers = store.load_run_markers().await.unwrap();
        assert!(markers.is_empty());
    }

    #[tokio::test]
    async fn load_run_markers_groups_by_session_in_seq_order() {
        let store = make_store();
        let sid = sample_session_id();
        let tid = uuid::Uuid::new_v4();
        let at = now_ms();
        // Interleave a non-marker event between two markers.
        store
            .append(&sid, 1, &run_started("r1", at), at)
            .await
            .unwrap();
        store
            .append(&sid, 2, &turn_started(tid, at), at)
            .await
            .unwrap();
        store
            .append(&sid, 3, &run_finished("r1", at + 5), at + 5)
            .await
            .unwrap();
        store
            .append(&sid, 4, &run_started("r2", at + 10), at + 10)
            .await
            .unwrap();

        let markers = store.load_run_markers().await.unwrap();
        assert_eq!(markers.len(), 1, "exactly one session has markers");
        let (got_sid, records) = &markers[0];
        assert_eq!(*got_sid, sid);
        assert_eq!(records.len(), 3, "3 markers, non-marker excluded");
        assert_eq!(records[0].seq, 1);
        assert_eq!(records[1].seq, 3);
        assert_eq!(records[2].seq, 4);
        assert!(matches!(records[0].event, SessionEvent::RunStarted { .. }));
        assert!(matches!(records[1].event, SessionEvent::RunFinished { .. }));
        assert!(matches!(records[2].event, SessionEvent::RunStarted { .. }));
    }

    #[tokio::test]
    async fn load_run_markers_separates_distinct_sessions() {
        let store = make_store();
        let sid_a = SessionKey::ephemeral("sess-a");
        let sid_b = SessionKey::ephemeral("sess-b");
        let at = now_ms();
        store
            .append(&sid_a, 1, &run_started("ra", at), at)
            .await
            .unwrap();
        store
            .append(&sid_b, 1, &run_started("rb", at), at)
            .await
            .unwrap();
        let markers = store.load_run_markers().await.unwrap();
        assert_eq!(markers.len(), 2);
    }

    /// §5.1: the intent stamp is a marker — the resume ratchet is counted off
    /// the same query the boot scan classifies from, so a stamp the query did
    /// not return would be a stamp that never capped anything.
    #[tokio::test]
    async fn load_run_markers_returns_resume_attempted_as_a_marker() {
        let store = make_store();
        let sid = SessionKey::main("m");
        let at = 1_700_000_000_000;
        store
            .append(&sid, 1, &run_started("r1", at), at)
            .await
            .unwrap();
        store
            .append(
                &sid,
                2,
                &SessionEvent::ResumeAttempted {
                    target: 1,
                    attempt: 1,
                },
                at + 1,
            )
            .await
            .unwrap();
        store
            .append(
                &sid,
                3,
                &SessionEvent::SystemMessage {
                    turn_id: uuid::Uuid::new_v4(),
                    content: "x".into(),
                    at,
                },
                at + 2,
            )
            .await
            .unwrap();
        let groups = store.load_run_markers().await.unwrap();
        let seqs: Vec<u64> = groups[0].1.iter().map(|r| r.seq).collect();
        assert_eq!(seqs, vec![1, 2]);
        assert!(matches!(
            groups[0].1[1].event,
            SessionEvent::ResumeAttempted {
                target: 1,
                attempt: 1
            }
        ));
    }

    /// The SQL IN-list and the reducer's `is_marker` are two spellings of one set.
    #[test]
    fn marker_event_types_are_exactly_the_reducers_marker_set() {
        // ONE sampler for the whole enum: T1's `events::fixtures::sample_of_every_kind()`,
        // whose completeness is pinned against the enum source there. A second
        // sampler here would be the same list twice (criterion #1).
        let all: Vec<SessionEvent> = crate::session::events::fixtures::sample_of_every_kind()
            .into_iter()
            .map(|(_, e)| e)
            .collect();
        let derived: std::collections::BTreeSet<&str> = all
            .iter()
            .filter(|e| crate::session::reduction::is_marker(e))
            .map(|e| event_type_tag(e))
            .collect();
        let declared: std::collections::BTreeSet<&str> =
            MARKER_EVENT_TYPES.iter().copied().collect();
        assert_eq!(derived, declared);
    }

    // -----------------------------------------------------------------------
    // FTS5 event search (search_events)
    // -----------------------------------------------------------------------

    fn user_message(tid: uuid::Uuid, text: &str, at: i64) -> SessionEvent {
        SessionEvent::UserMessage {
            turn_id: tid,
            content: MessageContent {
                text: text.to_string(),
                blocks: vec![],
                thinking: None,
                thinking_signature: None,
            },
            at,
            synthetic: false,
            author_user_id: None,
        }
    }

    #[tokio::test]
    async fn indexes_and_searches_user_message() {
        let store = make_store();
        let sid = sample_session_id();
        let tid = uuid::Uuid::new_v4();
        let at = now_ms();
        store
            .append(
                &sid,
                1,
                &user_message(tid, "please refactor the payment refund handler", at),
                at,
            )
            .await
            .unwrap();

        let hits = store
            .search_events(&sid, "payment refund", 5)
            .await
            .unwrap();
        assert_eq!(hits.len(), 1, "should find the user message");
        assert_eq!(hits[0].seq, 1);
        assert_eq!(hits[0].event_type, "user_message");
        assert!(hits[0].snippet.to_lowercase().contains("payment"));
    }

    #[tokio::test]
    async fn indexes_tool_result_and_error() {
        let store = make_store();
        let sid = sample_session_id();
        let tid = uuid::Uuid::new_v4();
        let at = now_ms();
        store
            .append(
                &sid,
                1,
                &SessionEvent::ToolResult {
                    turn_id: tid,
                    call_id: "c1".into(),
                    output: crate::session::events::ToolOutput {
                        value: serde_json::json!("compiled crate alephcore successfully"),
                        metadata: Default::default(),
                    },
                    at,
                },
                at,
            )
            .await
            .unwrap();
        store
            .append(
                &sid,
                2,
                &SessionEvent::ToolError {
                    turn_id: tid,
                    call_id: "c2".into(),
                    error: "linker failed: undefined symbol".into(),
                    at,
                },
                at,
            )
            .await
            .unwrap();

        let err_hits = store
            .search_events(&sid, "linker undefined symbol", 5)
            .await
            .unwrap();
        assert!(
            err_hits.iter().any(|h| h.event_type == "tool_error"),
            "tool error should be searchable, got {err_hits:?}"
        );

        let ok_hits = store
            .search_events(&sid, "compiled successfully", 5)
            .await
            .unwrap();
        assert!(
            ok_hits.iter().any(|h| h.event_type == "tool_result"),
            "tool result body should be searchable, got {ok_hits:?}"
        );
    }

    #[tokio::test]
    async fn control_events_are_not_indexed() {
        let store = make_store();
        let sid = sample_session_id();
        let tid = uuid::Uuid::new_v4();
        let at = now_ms();
        // A turn-started marker carries no content worth indexing.
        store
            .append(&sid, 1, &turn_started(tid, at), at)
            .await
            .unwrap();
        let hits = store
            .search_events(&sid, "started trigger turn user message", 5)
            .await
            .unwrap();
        assert!(
            hits.is_empty(),
            "control events must not be indexed, got {hits:?}"
        );
    }

    #[tokio::test]
    async fn search_is_scoped_to_session() {
        let store = make_store();
        let sid_a = SessionKey::ephemeral("scope-a");
        let sid_b = SessionKey::ephemeral("scope-b");
        let tid = uuid::Uuid::new_v4();
        let at = now_ms();
        store
            .append(&sid_a, 1, &user_message(tid, "alpha kangaroo note", at), at)
            .await
            .unwrap();
        store
            .append(&sid_b, 1, &user_message(tid, "beta kangaroo note", at), at)
            .await
            .unwrap();

        let hits = store.search_events(&sid_a, "kangaroo", 5).await.unwrap();
        assert_eq!(hits.len(), 1, "must only see session A's event");
        assert!(hits[0].snippet.to_lowercase().contains("alpha"));
    }

    // -----------------------------------------------------------------------
    // Soft delete (retire_from)
    // -----------------------------------------------------------------------

    /// Clearing a conversation must make the model forget it: the replay path
    /// (`load_all_events`) has to come back empty even though the append-only
    /// rows are still on disk.
    #[tokio::test]
    async fn retire_from_start_empties_the_replay_but_keeps_the_log() {
        let store = make_store();
        let sid = sample_session_id();
        let tid = uuid::Uuid::new_v4();
        let at = now_ms();
        store
            .append(
                &sid,
                1,
                &user_message(tid, "remember the passphrase", at),
                at,
            )
            .await
            .unwrap();
        store
            .append(&sid, 2, &turn_started(tid, at), at)
            .await
            .unwrap();

        assert_eq!(store.retire_from(&sid, 1).await.unwrap(), 2);

        assert!(
            store.load_all_events(&sid).await.unwrap().is_empty(),
            "replay must see nothing after a full retire"
        );
        assert!(
            store
                .search_events(&sid, "passphrase", 5)
                .await
                .unwrap()
                .is_empty(),
            "retired content must not stay recallable via BM25 search"
        );

        // The append-only log itself survives (constitution A3).
        let rows: i64 = {
            let conn = store.conn.lock().await;
            conn.query_row("SELECT COUNT(*) FROM session_events", [], |r| r.get(0))
                .unwrap()
        };
        assert_eq!(rows, 2, "soft delete must not drop rows");
    }

    #[tokio::test]
    async fn retire_from_keeps_earlier_events() {
        let store = make_store();
        let sid = sample_session_id();
        let tid = uuid::Uuid::new_v4();
        let at = now_ms();
        for seq in 1..=4u64 {
            store
                .append(&sid, seq, &user_message(tid, &format!("msg {seq}"), at), at)
                .await
                .unwrap();
        }

        assert_eq!(store.retire_from(&sid, 3).await.unwrap(), 2);

        let live = store.load_all_events(&sid).await.unwrap();
        assert_eq!(live.len(), 2);
        assert_eq!(live[0].seq, 1);
        assert_eq!(live[1].seq, 2);
    }

    #[tokio::test]
    async fn retire_from_is_idempotent() {
        let store = make_store();
        let sid = sample_session_id();
        let tid = uuid::Uuid::new_v4();
        let at = now_ms();
        store
            .append(&sid, 1, &user_message(tid, "hello", at), at)
            .await
            .unwrap();

        assert_eq!(store.retire_from(&sid, 1).await.unwrap(), 1);
        assert_eq!(
            store.retire_from(&sid, 1).await.unwrap(),
            0,
            "retiring the same range twice must be a no-op"
        );
        assert!(store.load_all_events(&sid).await.unwrap().is_empty());
    }

    /// Retired rows keep their seq, so the next append must land *past* them —
    /// reusing a retired seq would collide on the `(session_id, seq)` PK.
    #[tokio::test]
    async fn head_seq_ignores_retirement_so_appends_do_not_collide() {
        let store = make_store();
        let sid = sample_session_id();
        let tid = uuid::Uuid::new_v4();
        let at = now_ms();
        store
            .append(&sid, 1, &user_message(tid, "before clear", at), at)
            .await
            .unwrap();
        store.retire_from(&sid, 1).await.unwrap();

        assert_eq!(store.load_head_seq(&sid).await.unwrap(), 1);

        store
            .append(&sid, 2, &user_message(tid, "after clear", at), at)
            .await
            .unwrap();
        let live = store.load_all_events(&sid).await.unwrap();
        assert_eq!(live.len(), 1, "only the post-clear event is live");
        assert_eq!(live[0].seq, 2);
    }

    /// `Retire::Through` has no standalone method: it is reached only as the
    /// `retire` argument of `append_batch`, so a retire-only batch is how the
    /// head-side bound is driven here.
    async fn retire_through_batch(store: &SqliteEventStore, sid: &SessionId, through: EventSeq) {
        let next = store.load_head_seq(sid).await.unwrap() + 1;
        store
            .append_batch(
                sid,
                next,
                &[],
                Some(Retire::Through(through)),
                Durability::Normal,
            )
            .await
            .unwrap();
    }

    /// `retired_at` of one row, read off the private connection.
    async fn retired_at(store: &SqliteEventStore, sid: &SessionId, seq: EventSeq) -> Option<i64> {
        let conn = store.conn.lock().await;
        conn.query_row(
            "SELECT retired_at FROM session_events WHERE session_id = ?1 AND seq = ?2",
            params![session_id_to_string(sid).unwrap(), seq as i64],
            |row| row.get::<_, Option<i64>>(0),
        )
        .unwrap()
    }

    #[tokio::test]
    async fn retire_through_drops_the_prefix_and_keeps_the_tail() {
        let store = make_store();
        let sid = sample_session_id();
        let tid = uuid::Uuid::new_v4();
        let at = now_ms();
        for (seq, text) in [(1u64, "oldest"), (2, "middle"), (3, "newest")] {
            store
                .append(&sid, seq, &user_message(tid, text, at), at)
                .await
                .unwrap();
        }

        retire_through_batch(&store, &sid, 2).await;

        let live = store.load_all_events(&sid).await.unwrap();
        assert_eq!(live.len(), 1);
        assert_eq!(live[0].seq, 3, "only the tail survives the head retirement");
        assert!(retired_at(&store, &sid, 1).await.is_some());
        assert!(
            retired_at(&store, &sid, 2).await.is_some(),
            "the bound is inclusive"
        );
        assert!(retired_at(&store, &sid, 3).await.is_none());

        // Idempotent, exactly like its `retire_from` mirror: an already-retired
        // row keeps its ORIGINAL retirement timestamp. Pinned with a sentinel
        // rather than by counting, since a batch reports no count — and a
        // sentinel is not vacuous when the two calls share a millisecond.
        {
            let conn = store.conn.lock().await;
            conn.execute(
                "UPDATE session_events SET retired_at = 4242 WHERE session_id = ?1 AND seq = 1",
                params![session_id_to_string(&sid).unwrap()],
            )
            .unwrap();
        }
        retire_through_batch(&store, &sid, 2).await;
        assert_eq!(
            retired_at(&store, &sid, 1).await,
            Some(4242),
            "retiring the same range twice must not re-stamp the rows"
        );
        assert_eq!(store.load_all_events(&sid).await.unwrap().len(), 1);
        // Seq allocation is unaffected — the rows are still there.
        assert_eq!(store.load_head_seq(&sid).await.unwrap(), 3);
    }

    #[tokio::test]
    async fn retire_through_keeps_the_search_index_unlike_clear() {
        // The one deliberate asymmetry between the two `Retire` arms:
        // `chat.clear` must erase content from the BM25 mirror, compaction
        // must NOT — the turns leave the live prompt but stay recallable.
        // Losing this makes the "compaction is not a net loss" contract false.
        let store = make_store();
        let sid = sample_session_id();
        let tid = uuid::Uuid::new_v4();
        let at = now_ms();
        store
            .append(&sid, 1, &user_message(tid, "peregrine falcon", at), at)
            .await
            .unwrap();
        store
            .append(&sid, 2, &user_message(tid, "kept turn", at), at)
            .await
            .unwrap();

        retire_through_batch(&store, &sid, 1).await;
        assert_eq!(store.load_all_events(&sid).await.unwrap().len(), 1);
        assert!(
            !store
                .search_events(&sid, "peregrine", 5)
                .await
                .unwrap()
                .is_empty(),
            "compacted content must remain searchable"
        );

        // Contrast: `retire_from` (clear/rewind) DOES purge the mirror.
        store.retire_from(&sid, 1).await.unwrap();
        assert!(
            store
                .search_events(&sid, "peregrine", 5)
                .await
                .unwrap()
                .is_empty(),
            "cleared content must not remain searchable"
        );
    }

    #[tokio::test]
    async fn retire_through_does_not_touch_other_sessions() {
        let store = make_store();
        let sid_a = SessionKey::ephemeral("keep-a");
        let sid_b = SessionKey::ephemeral("compact-b");
        let tid = uuid::Uuid::new_v4();
        let at = now_ms();
        store
            .append(&sid_a, 1, &user_message(tid, "a", at), at)
            .await
            .unwrap();
        store
            .append(&sid_b, 1, &user_message(tid, "b", at), at)
            .await
            .unwrap();

        retire_through_batch(&store, &sid_b, 1).await;

        assert_eq!(store.load_all_events(&sid_a).await.unwrap().len(), 1);
        assert!(store.load_all_events(&sid_b).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn retire_does_not_touch_other_sessions() {
        let store = make_store();
        let sid_a = SessionKey::ephemeral("keep-a");
        let sid_b = SessionKey::ephemeral("clear-b");
        let tid = uuid::Uuid::new_v4();
        let at = now_ms();
        store
            .append(&sid_a, 1, &user_message(tid, "a", at), at)
            .await
            .unwrap();
        store
            .append(&sid_b, 1, &user_message(tid, "b", at), at)
            .await
            .unwrap();

        store.retire_from(&sid_b, 1).await.unwrap();

        assert_eq!(store.load_all_events(&sid_a).await.unwrap().len(), 1);
        assert!(store.load_all_events(&sid_b).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn retired_run_markers_are_not_resumed() {
        let store = make_store();
        let sid = sample_session_id();
        let at = now_ms();
        store
            .append(&sid, 1, &run_started("r1", at), at)
            .await
            .unwrap();

        store.retire_from(&sid, 1).await.unwrap();

        assert!(
            store.load_run_markers().await.unwrap().is_empty(),
            "a cleared session must not look like an interrupted run"
        );
    }

    /// A database written before `retired_at` existed must migrate in place and
    /// keep serving its rows (legacy rows read as live).
    #[tokio::test]
    async fn migrates_pre_existing_db_without_retired_at_column() {
        let conn = Connection::open_in_memory().unwrap();
        // The pre-soft-delete schema, verbatim.
        conn.execute_batch(
            "CREATE TABLE session_events (
                 session_id   TEXT    NOT NULL,
                 seq          INTEGER NOT NULL,
                 turn_id      TEXT,
                 event_type   TEXT    NOT NULL,
                 payload_json TEXT    NOT NULL,
                 created_at   INTEGER NOT NULL,
                 PRIMARY KEY (session_id, seq)
             );",
        )
        .unwrap();

        let sid = sample_session_id();
        let tid = uuid::Uuid::new_v4();
        let at = now_ms();
        let legacy = user_message(tid, "written before the migration", at);
        conn.execute(
            "INSERT INTO session_events
             (session_id, seq, turn_id, event_type, payload_json, created_at)
             VALUES (?1, 1, ?2, 'user_message', ?3, ?4)",
            params![
                session_id_to_string(&sid).unwrap(),
                tid.to_string(),
                serde_json::to_string(&legacy).unwrap(),
                at,
            ],
        )
        .unwrap();

        migrate_add_session_events(&conn).unwrap();
        // Idempotent on an already-migrated DB.
        migrate_add_session_events(&conn).unwrap();

        let store = SqliteEventStore::new(conn);
        let live = store.load_all_events(&sid).await.unwrap();
        assert_eq!(live.len(), 1, "legacy rows must read back as live");
        assert_eq!(live[0].seq, 1);

        assert_eq!(store.retire_from(&sid, 1).await.unwrap(), 1);
        assert!(store.load_all_events(&sid).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn punctuation_only_query_is_empty_not_error() {
        let store = make_store();
        let sid = sample_session_id();
        let tid = uuid::Uuid::new_v4();
        let at = now_ms();
        store
            .append(&sid, 1, &user_message(tid, "hello world", at), at)
            .await
            .unwrap();
        let hits = store.search_events(&sid, "()[]{}!!!", 5).await.unwrap();
        assert!(hits.is_empty());
    }

    // -----------------------------------------------------------------------
    // append_batch — one transaction per batch, retire inside it, Barrier
    // durability restored either way
    // -----------------------------------------------------------------------

    /// Atomicity without an injected failure: pre-seed seq 3, then batch
    /// [1, 2, 3]. Row 3 collides on the primary key, and rows 1 and 2 —
    /// already INSERTed inside the same transaction — must roll back with it.
    #[tokio::test]
    async fn a_batch_whose_third_row_collides_leaves_nothing_behind() {
        let store = make_store();
        let sid = sample_session_id();
        let tid = uuid::Uuid::new_v4();
        let at = now_ms();
        store
            .append(&sid, 3, &turn_started(tid, at), at)
            .await
            .unwrap();
        let batch = vec![
            (turn_started(tid, at), at),
            (user_message(tid, "x", at), at),
            (turn_started(tid, at), at),
        ];
        let err = store
            .append_batch(&sid, 1, &batch, None, Durability::Normal)
            .await
            .unwrap_err();
        assert!(matches!(err, SessionError::Storage(_)), "{err:?}");
        let live: Vec<_> = store
            .load_all_events(&sid)
            .await
            .unwrap()
            .into_iter()
            .map(|r| r.seq)
            .collect();
        assert_eq!(
            live,
            vec![3],
            "rows 1 and 2 must have rolled back with row 3"
        );
    }

    #[tokio::test]
    async fn retire_and_insert_are_one_transaction() {
        let store = make_store();
        let sid = sample_session_id();
        let tid = uuid::Uuid::new_v4();
        let at = now_ms();
        for seq in 1..=3 {
            store
                .append(&sid, seq, &user_message(tid, "old", at), at)
                .await
                .unwrap();
        }
        // Makes seq 5 collide below.
        store
            .append(&sid, 5, &turn_started(tid, at), at)
            .await
            .unwrap();
        let batch = vec![(run_finished("r", at), at), (turn_started(tid, at), at)];
        store
            .append_batch(&sid, 4, &batch, Some(Retire::From(2)), Durability::Normal)
            .await
            .unwrap_err();
        let live: Vec<_> = store
            .load_all_events(&sid)
            .await
            .unwrap()
            .into_iter()
            .map(|r| r.seq)
            .collect();
        assert_eq!(
            live,
            vec![1, 2, 3, 5],
            "a failed batch must not have retired anything either"
        );
        // And the successful shape: the batch's own rows are live, the retired
        // range is not.
        let batch = vec![(run_finished("r", at), at)];
        store
            .append_batch(&sid, 6, &batch, Some(Retire::From(2)), Durability::Normal)
            .await
            .unwrap();
        let live: Vec<_> = store
            .load_all_events(&sid)
            .await
            .unwrap()
            .into_iter()
            .map(|r| r.seq)
            .collect();
        assert_eq!(live, vec![1, 6]);
        // Seq 1 is live and still says "old", so it MUST still hit; seqs 2 and
        // 3 were retired and must be gone from the mirror as well.
        let mut hits: Vec<_> = store
            .search_events(&sid, "old", 10)
            .await
            .unwrap()
            .into_iter()
            .map(|h| h.seq)
            .collect();
        hits.sort_unstable();
        assert_eq!(
            hits,
            vec![1],
            "From deletes the BM25 mirror for the retired range like retire_from"
        );
    }

    /// The connection's CURRENT `PRAGMA synchronous` (OFF = 0, NORMAL = 1,
    /// FULL = 2), read off the same connection the batch wrote through.
    fn pragma_synchronous(conn: &Connection) -> i64 {
        conn.query_row("PRAGMA synchronous", [], |r| r.get::<_, i64>(0))
            .unwrap()
    }

    /// Put the connection at production's resting level (`open_sqlite_safe`
    /// sets NORMAL; a bare in-memory connection defaults to FULL, which would
    /// make "raised to FULL" indistinguishable from "never raised").
    fn rest_at_normal(conn: &Connection) {
        conn.execute_batch("PRAGMA synchronous=NORMAL").unwrap();
        assert_eq!(pragma_synchronous(conn), 1);
    }

    #[tokio::test]
    async fn barrier_restores_normal_after_success_and_after_failure() {
        async fn sync(s: &SqliteEventStore) -> i64 {
            let conn = s.conn.lock().await;
            pragma_synchronous(&conn)
        }
        let store = make_store();
        let sid = sample_session_id();
        let at = now_ms();
        store
            .append_batch(
                &sid,
                1,
                &[(run_started("r", at), at)],
                None,
                Durability::Barrier,
            )
            .await
            .unwrap();
        assert_eq!(
            sync(&store).await,
            1,
            "NORMAL restored after a Barrier commit"
        );
        store
            .append_batch(
                &sid,
                1,
                &[(run_started("r", at), at)],
                None,
                Durability::Barrier,
            )
            .await
            .unwrap_err();
        assert_eq!(
            sync(&store).await,
            1,
            "NORMAL restored after a Barrier rollback"
        );
    }

    /// The raise itself, observed from INSIDE the window: while the closure
    /// runs — and it runs a real `write_batch` — `PRAGMA synchronous` reads
    /// FULL, and after it returns the connection is back at NORMAL. Starting
    /// from NORMAL is what makes this a guard: on a fresh in-memory connection
    /// (default FULL) a deleted raise would still read 2.
    #[tokio::test]
    async fn barrier_raises_full_for_the_transaction_and_only_for_it() {
        let store = make_store();
        let sid = sample_session_id();
        let session_key = session_id_to_string(&sid).unwrap();
        let at = now_ms();
        let rows = vec![EncodedRow {
            seq: 1,
            turn_id: None,
            event_type: event_type_tag(&run_started("r", at)),
            payload: encode_payload(&run_started("r", at)).unwrap(),
            created_at: at,
            fts_body: None,
        }];
        let mut conn = store.conn.lock().await;
        rest_at_normal(&conn);
        let (inside, written) = with_synchronous_full(&mut conn, |c| {
            let inside = pragma_synchronous(c);
            (inside, write_batch(c, &session_key, &rows, None, at))
        })
        .unwrap();
        written.unwrap();
        assert_eq!(inside, 2, "the transaction ran under FULL");
        assert_eq!(pragma_synchronous(&conn), 1, "back to NORMAL after it");
        let live: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM session_events WHERE session_id = ?1",
                params![session_key],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(live, 1, "the row written inside the window is committed");
    }

    /// A `Normal` batch does not touch the pragma at all — whatever level the
    /// connection was opened at survives it — while a `Barrier` batch lands
    /// on NORMAL (the store's resting level), not on "whatever it was before".
    #[tokio::test]
    async fn a_normal_batch_leaves_the_pragma_alone() {
        async fn sync(s: &SqliteEventStore) -> i64 {
            let conn = s.conn.lock().await;
            pragma_synchronous(&conn)
        }
        let store = make_store();
        let sid = sample_session_id();
        let at = now_ms();
        store
            .conn
            .lock()
            .await
            .execute_batch("PRAGMA synchronous=OFF")
            .unwrap();
        store
            .append_batch(
                &sid,
                1,
                &[(run_started("r", at), at)],
                None,
                Durability::Normal,
            )
            .await
            .unwrap();
        assert_eq!(
            sync(&store).await,
            0,
            "a Normal batch must not raise or restore anything"
        );
        store
            .append_batch(
                &sid,
                2,
                &[(run_started("r", at), at)],
                None,
                Durability::Barrier,
            )
            .await
            .unwrap();
        assert_eq!(
            sync(&store).await,
            1,
            "a Barrier batch restores NORMAL, not the previous OFF"
        );
    }

    #[tokio::test]
    async fn an_empty_batch_with_nothing_to_retire_is_refused() {
        let store = make_store();
        let err = store
            .append_batch(&sample_session_id(), 1, &[], None, Durability::Normal)
            .await
            .unwrap_err();
        assert!(matches!(err, SessionError::Other(_)));
    }
}
