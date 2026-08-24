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
use crate::session::events::{EventSeq, SessionEvent, SessionEventRecord};
use crate::session::service::{SessionError, SessionId};

#[async_trait]
pub trait SessionEventStore: Send + Sync + 'static {
    /// Append a single event at the given seq. Fails if (`session_id`, seq) already exists.
    async fn append(
        &self,
        session_id: &SessionId,
        seq: EventSeq,
        event: &SessionEvent,
        created_at_ms: i64,
    ) -> Result<(), SessionError>;

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
    /// conversation. Returns how many events this call newly retired.
    ///
    /// Soft delete: the rows survive, so the append-only log stays intact and
    /// seq allocation is unaffected. All readers of the live conversation
    /// (`load_all_events`, `load_events_range`, `load_run_markers`,
    /// `search_events`) skip retired events, so the model stops replaying them.
    ///
    /// Idempotent: already-retired events keep their original retirement
    /// timestamp and are not counted again.
    async fn retire_from(
        &self,
        session_id: &SessionId,
        from_seq: EventSeq,
    ) -> Result<usize, SessionError>;

    /// Retire every event with `seq <= through_seq` — the head-side mirror of
    /// [`retire_from`], and the primitive behind manual `/compact`.
    ///
    /// Two deliberate differences from [`retire_from`]:
    ///
    /// 1. **The BM25 mirror is kept.** `retire_from` backs `chat.clear` /
    ///    `chat.rewind`, where leaving the content searchable would hand the
    ///    model the very turns the user just erased. Compaction is the
    ///    opposite intent: the turns leave the *live prompt* but must stay
    ///    recallable, so `recall_events` can still surface a detail the
    ///    summary abstracted away. Deleting the FTS rows here would make the
    ///    "compaction is not a net loss" contract false.
    /// 2. **No default `Ok(0)`.** A store that cannot retire must say so
    ///    rather than silently report a compaction that did not happen — the
    ///    caller appends its summary first and treats this error as
    ///    "summary recorded, context unchanged".
    ///
    /// Idempotent, like its mirror: already-retired events keep their original
    /// timestamp and are not counted again.
    async fn retire_through(
        &self,
        session_id: &SessionId,
        through_seq: EventSeq,
    ) -> Result<usize, SessionError> {
        let _ = (session_id, through_seq);
        Err(SessionError::Storage(
            "this event store does not support head-side retirement (manual compaction)".into(),
        ))
    }

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
    /// session's `RunStarted` / `RunFinished` events in `seq` order.
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
/// [`SqliteEventStore::append`]) so that, after compaction evicts old turns
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

#[async_trait]
impl SessionEventStore for SqliteEventStore {
    async fn append(
        &self,
        session_id: &SessionId,
        seq: EventSeq,
        event: &SessionEvent,
        created_at_ms: i64,
    ) -> Result<(), SessionError> {
        let payload = serde_json::to_string(event)?;
        let session_key = session_id_to_string(session_id)?;
        let turn_id = extract_turn_id(event).map(|u| u.to_string());
        let event_type = event_type_tag(event);
        let seq_i64 = i64::try_from(seq)
            .map_err(|_| SessionError::Storage(format!("seq {seq} exceeds i64::MAX")))?;

        let conn = self.conn.lock().await;
        conn.execute(
            "INSERT INTO session_events
             (session_id, seq, turn_id, event_type, payload_json, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                session_key,
                seq_i64,
                turn_id,
                event_type,
                payload,
                created_at_ms,
            ],
        )
        .map_err(|e| SessionError::Storage(e.to_string()))?;

        // Mirror content-bearing events into the FTS index so prior turns stay
        // BM25-searchable after compaction evicts them from context. Strictly
        // best-effort: an indexing failure must never block the authoritative
        // append above (continuity of the log outranks searchability).
        if let Some(body) = render_event_text(event) {
            if let Err(e) = conn.execute(
                "INSERT INTO session_events_fts
                 (body, session_id, seq, event_type, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![body, session_key, seq_i64, event_type, created_at_ms],
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
        // An out-of-range `from_seq` must not widen the range and retire events
        // the caller never asked for — saturate high so it matches no rows.
        let from_val = i64::try_from(from_seq).unwrap_or(i64::MAX);
        let at = crate::session::events::now_ms();

        let conn = self.conn.lock().await;
        // `retired_at IS NULL` makes this idempotent: a second retire of the
        // same range matches nothing and reports 0 newly-retired events.
        //
        // Both the UPDATE and the FTS DELETE must run in the same transaction
        // or a partial failure (e.g. disk-full mid-statement) leaves the rows
        // marked retired while their content stays in the BM25 mirror — exactly
        // the leak this method exists to prevent.
        conn.execute_batch("BEGIN IMMEDIATE")
            .map_err(|e| SessionError::Storage(format!("retire_from BEGIN failed: {e}")))?;

        let retired = match conn.execute(
            "UPDATE session_events SET retired_at = ?3
                 WHERE session_id = ?1 AND seq >= ?2 AND retired_at IS NULL",
            params![session_key, from_val, at],
        ) {
            Ok(n) => n,
            Err(e) => {
                let _ = conn.execute_batch("ROLLBACK");
                return Err(SessionError::Storage(e.to_string()));
            }
        };

        // Drop the retired events from the BM25 mirror as well, or `recall_events`
        // would hand the model the very content the user just cleared. The FTS
        // table is a derived index, not the log, so a physical delete here does
        // not violate the append-only guarantee. Unlike the best-effort insert in
        // `append`, this failure is propagated: a half-retire that leaves cleared
        // content searchable is exactly the leak this method exists to prevent.
        if let Err(e) = conn.execute(
            "DELETE FROM session_events_fts WHERE session_id = ?1 AND seq >= ?2",
            params![session_key, from_val],
        ) {
            let _ = conn.execute_batch("ROLLBACK");
            return Err(SessionError::Storage(e.to_string()));
        }

        conn.execute_batch("COMMIT")
            .map_err(|e| SessionError::Storage(format!("retire_from COMMIT failed: {e}")))?;
        Ok(retired)
    }

    async fn retire_through(
        &self,
        session_id: &SessionId,
        through_seq: EventSeq,
    ) -> Result<usize, SessionError> {
        let session_key = session_id_to_string(session_id)?;
        // An out-of-range `through_seq` must not widen the range past what the
        // caller asked for. Unlike `retire_from`'s lower bound, saturating an
        // upper bound HIGH is the widening direction, so clamp to i64::MAX only
        // because no stored seq can exceed it — the range is still exactly
        // "everything at or below the requested boundary".
        let through_val = i64::try_from(through_seq).unwrap_or(i64::MAX);
        let at = crate::session::events::now_ms();

        let conn = self.conn.lock().await;
        // Single statement, so no explicit transaction: unlike `retire_from`
        // there is no paired FTS delete to keep atomic — the BM25 mirror is
        // deliberately preserved (see the trait doc).
        //
        // `retired_at IS NULL` makes this idempotent: re-compacting the same
        // prefix matches nothing and reports 0 newly-retired events.
        conn.execute(
            "UPDATE session_events SET retired_at = ?3
                 WHERE session_id = ?1 AND seq <= ?2 AND retired_at IS NULL",
            params![session_key, through_val, at],
        )
        .map_err(|e| SessionError::Storage(e.to_string()))
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
        let mut stmt = conn
            .prepare(
                "SELECT session_id, seq, payload_json, created_at
                 FROM session_events
                 WHERE event_type IN ('run_started', 'run_finished')
                   AND retired_at IS NULL
                 ORDER BY session_id, seq ASC",
            )
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
        | SessionEvent::CompactionPerformed { .. } => None,
    }
}

/// Static discriminant string for the `event_type` column.
///
/// Kept as a `&'static str` to avoid per-append allocation and to give the
/// storage layer a stable taxonomy independent of serde rename decisions.
// rust-doctor-disable-next-line high-cyclomatic-complexity
const fn event_type_tag(event: &SessionEvent) -> &'static str {
    match event {
        SessionEvent::SessionWoken { .. } => "session_woken",
        SessionEvent::RunStarted { .. } => "run_started",
        SessionEvent::RunFinished { .. } => "run_finished",
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
/// batch: five production reads produce four *different* answers, two of which
/// are reported to the caller as success.
///
/// | reader | an uninstalled read becomes |
/// |---|---|
/// | `builtin_tools/recall_events.rs` | `Ok(empty)` plus a note to the model |
/// | `builtin_tools/sessions/compact_tool.rs` | an `AlephError` |
/// | [`retire_live_events`] | `Ok(0)` — "retired nothing", indistinguishable from "there was nothing to retire" |
/// | [`is_event_retired`] | `Ok(false)` — "not retired", the fail-open direction |
/// | `gateway/execution_engine/run_loop/inner.rs` | the legacy backfill is skipped in silence |
///
/// Each arm is individually defensible (all four doc-comment their reasoning),
/// which is exactly why no `IndistinguishableDefault { reads_as }` sentence
/// could be written for this slot: there is no single thing a missing handle
/// reads as. Task 15 adjudicates the arms; this variant records that there are
/// four of them.
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

/// Fetch the process-wide session event store, if one has been installed.
///
/// ⚠️ `None` says nothing about whether boot reached this slot. Ask
/// [`global_session_event_store_slot`]`().outcome()` for that.
#[inline]
pub fn global_session_event_store() -> Option<Arc<dyn SessionEventStore>> {
    GLOBAL_EVENT_STORE.get().cloned()
}

/// The handle above, type-erased for the roster — see
/// [`crate::spend::global_ledger_slot`] for why this shape, and why the
/// `#[allow(dead_code)]` expires with Task 11 rather than outliving it.
#[allow(dead_code)]
pub(crate) fn global_session_event_store_slot() -> &'static dyn SlotStatus {
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
/// `retire_live_events` / `is_event_retired` path (the handlers reach the store
/// through the process-wide slot above, so they cannot be handed one).
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

/// True when the event at `seq` has been retired in the process-wide event log.
///
/// Consulted by the projector before it writes a row: the drain is async, so a
/// `clear` / `rewind` can retire an event that is still queued. `false` when no
/// store is installed (CLI one-shot, tests) — there is no soft-delete state to
/// respect.
pub async fn is_event_retired(session_id: &SessionId, seq: EventSeq) -> Result<bool, SessionError> {
    match global_session_event_store() {
        Some(store) => store.is_retired(session_id, seq).await,
        None => Ok(false),
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

        let retired = store.retire_through(&sid, 2).await.unwrap();
        assert_eq!(retired, 2);

        let live = store.load_all_events(&sid).await.unwrap();
        assert_eq!(live.len(), 1);
        assert_eq!(live[0].seq, 3, "only the tail survives the head retirement");
        // Idempotent, exactly like its `retire_from` mirror.
        assert_eq!(store.retire_through(&sid, 2).await.unwrap(), 0);
        // Seq allocation is unaffected — the rows are still there.
        assert_eq!(store.load_head_seq(&sid).await.unwrap(), 3);
    }

    #[tokio::test]
    async fn retire_through_keeps_the_search_index_unlike_clear() {
        // The one deliberate asymmetry with `retire_from`: `chat.clear` must
        // erase content from the BM25 mirror, compaction must NOT — the turns
        // leave the live prompt but stay recallable. Losing this makes the
        // "compaction is not a net loss" contract false.
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

        store.retire_through(&sid, 1).await.unwrap();
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

        store.retire_through(&sid_b, 1).await.unwrap();

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
}
