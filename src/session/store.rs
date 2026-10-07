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
//! # Payload envelope
//!
//! `payload_json` is the event's own serde object plus two keys the store
//! adds: `v`, the [`SESSION_EVENT_SCHEMA_VERSION`] the row was written under,
//! and `ignorable: true` on the rows [`crate::session::events::ignorable`]
//! says an older build may skip unread. [`encode_row`] is the one writer of
//! that envelope and [`decode_row`] the one reader: a row this build cannot
//! turn into a [`SessionEvent`] is a [`DecodedRow::Undecodable`] value, so it
//! refuses the session that holds it and no other. The envelope goes through
//! `serde_json::Value` (no `preserve_order` in this workspace), so a row's
//! keys are written in alphabetical order — `"at", …, "type", "v"` — where
//! the pre-envelope rows led with `"type"`; decoding is order-independent
//! and no SQL reads `payload_json` textually, so the two shapes coexist.
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
    durability_of, Durability, EventSeq, Retire, SessionEvent, SessionEventRecord, Timestamp,
    TurnId,
};
use crate::session::service::{SessionError, SessionId};

#[async_trait]
pub trait SessionEventStore: Send + Sync + 'static {
    /// Append `events` at consecutive seqs starting from `first_seq`, in ONE
    /// transaction, optionally retiring a range in that same transaction.
    ///
    /// This is the only write path: a multi-step durable operation either
    /// lands whole or not at all — manual `/compact` is its summary row, its
    /// checkpoint row and `Retire::Through { through: cut, live }` in one
    /// call; `chat.rewind` / `session.truncate` are `Retire::From(seq)` plus
    /// the `RunFinished` closer the cut would otherwise leave owed. Fails —
    /// with nothing written and nothing retired — if any `(session_id, seq)`
    /// already exists, and with [`SessionError::RetireSpanChanged`] if a
    /// `Retire::Through` retires any count other than its `live`. **Every
    /// implementer must enforce that count inside the transaction.** One that
    /// ignores `live` still compiles and still passes every test that does not
    /// race a clear, and silently brings back the race it exists to close: a
    /// summary of turns the user erased, at the head of every future prompt.
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

    /// Load events with seq in [from..to) WITH their retirement state — the
    /// one read that looks past `retired_at` at full payloads
    /// ([`load_retired_run_anchors`](Self::load_retired_run_anchors) reads
    /// only two marker kinds, from the `event_type` column).
    ///
    /// Its one consumer is `session_decompress` (Context Fabric spec
    /// 2026-10-01 §3.1b): restoring a folded span means reading the rows
    /// `Retire::Through` soft-retired, and answering honestly about a span
    /// `Retire::From` erased means seeing those rows too — a fold whose own
    /// record was hard-retired must still resolve, or its error would read
    /// "no such fold" instead of "erased". The BM25-mirror flag is the
    /// on-disk witness that tells the two retirements apart: `Through` keeps
    /// the mirror (compacted turns stay recallable), `From` deletes it in
    /// the same transaction (erased turns must not come back).
    ///
    /// Decode policy mirrors the live read's `fold_strict`: ignorable rows
    /// drop, the first undecodable row refuses the slice — a restoration
    /// that silently skipped a row would present a gap as the whole span.
    ///
    /// Default `Ok(vec![])`, for the same reason as
    /// [`load_retired_run_anchors`](Self::load_retired_run_anchors): a store
    /// with no soft delete has nothing retired to restore. A store that DOES
    /// soft-delete must override it, or every decompress answers as if the
    /// fold's rows were gone.
    async fn load_events_with_retirement(
        &self,
        session_id: &SessionId,
        from: Option<EventSeq>,
        to: Option<EventSeq>,
    ) -> Result<Vec<RetiredEventRow>, SessionError> {
        let _ = (session_id, from, to);
        Ok(Vec::new())
    }

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
    /// (`load_all_events`, `load_events_range`, `load_rows`,
    /// `load_run_markers`, `search_events`) skip retired events, so the model
    /// stops replaying them.
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

    /// This session's RETIRED `RunStarted` / `AssistantRunMeta` rows —
    /// `seq` and kind only, in `seq` order.
    ///
    /// The one reader that looks past `retired_at`, and it exposes nothing
    /// the live conversation would replay: a retired row is a fact about the
    /// past, and the fact the whole-session heal needs is "did this run's
    /// meta ever land?". A `chat.rewind` / `session.truncate` / `/undo` whose
    /// cut falls inside a finished, already-billed run retires the run's meta
    /// and closes the run again with a `Cancelled` closer; read from live rows
    /// alone that run is finished-without-meta, and the heal would synthesize
    /// a stamp and bill the surviving tokens a second time
    /// (`session_projector::collect_run_spans` consumes this to see the meta
    /// where it landed). Kinds come from the `event_type` column, never the
    /// payload, so a retired row this build cannot decode (the doctor's
    /// `retire_record` exit) cannot refuse the read.
    ///
    /// Default `Ok(vec![])`, for the same reason as [`is_retired`](Self::is_retired):
    /// a store with no soft delete has retired nothing. A store that DOES
    /// soft-delete must override it, or its heal reads every rewound run as
    /// never billed.
    async fn load_retired_run_anchors(
        &self,
        session_id: &SessionId,
    ) -> Result<Vec<RetiredRunAnchor>, SessionError> {
        let _ = session_id;
        Ok(Vec::new())
    }

    /// Cross-session scan for resume detection. Returns, per session, that
    /// session's run-marker events — the `MARKER_EVENT_TYPES` set, which is
    /// pinned equal to the reducer's `is_marker` — in `seq` order.
    /// Sessions with no run markers are omitted. Served by the existing
    /// `(session_id, event_type)` index.
    ///
    /// Decoded per session: a marker row this build cannot read makes THAT
    /// session's slice `Err` ([`MarkerSlice`]) and leaves every other
    /// session's slice whole. The outer `Err` is the query itself failing.
    async fn load_run_markers(&self) -> Result<Vec<(SessionId, MarkerSlice)>, SessionError>;

    /// Every live row of one session, decoded one at a time, in `seq` order.
    ///
    /// The doctor's read: a row this build cannot decode is a
    /// [`DecodedRow::Undecodable`] VALUE here, not the read's failure, so the
    /// rows around it can still be named and the bad one retired
    /// ([`retire_record`](Self::retire_record)). The model-facing readers
    /// (`load_all_events`, `load_events_range`) fold the same rows strictly
    /// and refuse the session instead.
    ///
    /// Default is a refusal: a store that cannot expose its rows must say so
    /// rather than answer "no rows".
    async fn load_rows(&self, session_id: &SessionId) -> Result<Vec<DecodedRow>, SessionError> {
        let _ = session_id;
        Err(SessionError::Storage(
            "this event store cannot expose raw rows".into(),
        ))
    }

    /// Retire exactly the row at `seq` — the doctor's `fix=true` exit for an
    /// undecodable record. Soft delete like [`retire_from`](Self::retire_from):
    /// the row survives, seq allocation is unaffected, every live-conversation
    /// reader skips it from now on. The BM25 mirror row, if the writing build
    /// indexed one, is left where it is — the record is one this build cannot
    /// read, and its snippet is text that build rendered.
    ///
    /// `Ok(true)` when this call retired it; `Ok(false)` when it was already
    /// retired or no such row exists — idempotent, and the two cases are
    /// deliberately one answer: the caller has just read the row it names.
    ///
    /// Default is a refusal, for the same reason as [`load_rows`](Self::load_rows).
    async fn retire_record(
        &self,
        session_id: &SessionId,
        seq: EventSeq,
    ) -> Result<bool, SessionError> {
        let _ = (session_id, seq);
        Err(SessionError::Storage(
            "this event store cannot retire a single record".into(),
        ))
    }

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

    /// Atomically claim one still-unanswered Tool call for restricted replay.
    /// Implementations must compare the durable head, the session's retire
    /// generation, and the call's outcome state in the same transaction as
    /// the cursor increment. A held, unexpired lease must be refused
    /// ([`ReplayClaimResult::LeaseHeld`]) rather than overwritten, so the
    /// original claim's token stays committable.
    ///
    /// `lease_ms` is the caller's lease duration in unix ms (must be at
    /// least [`REPLAY_LEASE_TTL_MS`], i.e. 300_000ms) and is written to
    /// `lease_until` — the executor passes a value covering the tool's
    /// invoke deadline, not a store-wide default. A caller may pass a
    /// *longer* lease to cover a slower tool, but a value below the floor
    /// is refused ([`SessionError::Other`]) without touching the cursor:
    /// a lease shorter than the default Tool budget could otherwise
    /// fence-take-over a live handler mid-flight.
    async fn claim_replay_call(
        &self,
        session_id: &SessionId,
        expected_head: EventSeq,
        expected_generation: RetireGeneration,
        call_id: &str,
        max_attempts: u32,
        lease_ms: i64,
    ) -> Result<ReplayClaimResult, SessionError> {
        let _ = (
            session_id,
            expected_head,
            expected_generation,
            call_id,
            max_attempts,
            lease_ms,
        );
        Err(SessionError::Storage(
            "this event store cannot claim replay calls".into(),
        ))
    }

    /// Commit one real replay outcome for a previously claimed call. The
    /// implementation must verify, in the same transaction as the append,
    /// that the claim token is non-empty, that the cursor is still `active`,
    /// and that the cursor's stored retire generation matches the current
    /// one. Any mismatch — a stale token, an answered/rewound cursor, or a
    /// retire landing between claim and commit — fails closed as
    /// [`ReplayOutcomeResult::ClaimLost`] and appends nothing.
    async fn commit_replay_outcome(
        &self,
        session_id: &SessionId,
        call_id: &str,
        claim_token: &str,
        event: &SessionEvent,
        created_at_ms: i64,
    ) -> Result<ReplayOutcomeResult, SessionError> {
        let _ = (session_id, call_id, claim_token, event, created_at_ms);
        Err(SessionError::Storage(
            "this event store cannot commit replay outcomes".into(),
        ))
    }

    /// Read the session's retire generation — the counter bumped on every
    /// retire/rewind, which [`claim_replay_call`](Self::claim_replay_call)
    /// compares against its `expected_generation`. A store that cannot report
    /// it refuses (fail-closed): a claim must never proceed on a generation
    /// its caller did not read.
    async fn load_retire_generation(
        &self,
        session_id: &SessionId,
    ) -> Result<RetireGeneration, SessionError> {
        let _ = session_id;
        Err(SessionError::Storage(
            "this event store cannot report retire generation".into(),
        ))
    }

    /// Read back the durable post-guardrail input a call actually ran with
    /// (§4.3). Fail-closed: `None` when the call has zero markers (unknown —
    /// never written, so the call is not replay eligible) or two-or-more
    /// (ambiguous — it was dispatched and marked more than once, so its true
    /// input is unknowable). `Some` only for exactly one marker.
    async fn load_tool_call_effective_input(
        &self,
        session_id: &SessionId,
        call_id: &str,
    ) -> Result<Option<EffectiveInputRecord>, SessionError> {
        let _ = (session_id, call_id);
        Err(SessionError::Storage(
            "this event store cannot read effective inputs".into(),
        ))
    }

    /// Release a previously taken claim that will NOT start an effect — the
    /// inverse of [`claim_replay_call`](Self::claim_replay_call). The
    /// implementation must verify, in the same transaction as the cursor
    /// write, that the token is non-empty, that the cursor is still `active`,
    /// and that the stored token matches. A stale token must never release a
    /// live claim, so any mismatch fails closed as
    /// [`ReplayReleaseResult::ClaimLost`]. A successful release frees the
    /// lease and does NOT consume the attempt (it is decremented) because no
    /// effect started under it.
    async fn release_replay_claim(
        &self,
        session_id: &SessionId,
        call_id: &str,
        claim_token: &str,
    ) -> Result<ReplayReleaseResult, SessionError> {
        let _ = (session_id, call_id, claim_token);
        Err(SessionError::Storage(
            "this event store cannot release replay claims".into(),
        ))
    }
}

/// Monotonic, session-level counter bumped on every retire/rewind. A replay
/// claim carries the generation its caller read; a mismatch means a retire
/// landed between the read and the claim — including a high-seq retire that
/// `load_head_seq` cannot see, because soft delete keeps the retired row's
/// `MAX(seq)` in place.
pub type RetireGeneration = u64;

/// Default/test floor for a replay claim's lease, in unix ms. The claim API
/// takes the caller's `lease_ms`, so this constant is the fallback for tests
/// and the enforced lower bound for every caller: the store rejects any
/// `lease_ms` below it. It must never be shorter than the project's default
/// Tool budget (300s) or a normal long call could be fence-taken-over
/// mid-flight. A caller may pass a longer lease; a crashed worker's claim
/// still expires at the caller-supplied deadline rather than wedging the
/// call forever.
pub const REPLAY_LEASE_TTL_MS: i64 = 300_000;

/// `state` column values for `session_replay_cursors`.
const REPLAY_STATE_ACTIVE: &str = "active";
const REPLAY_STATE_ANSWERED: &str = "answered";
const REPLAY_STATE_RELEASED: &str = "released";

/// The durable post-guardrail input a call actually ran with (§4.3), read back
/// from exactly one [`SessionEvent::ToolCallEffectiveInput`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectiveInputRecord {
    pub turn_id: TurnId,
    pub input: serde_json::Value,
    pub at: Timestamp,
}

/// Result of the serialized per-call replay claim.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReplayClaimResult {
    Claimed {
        attempt: u32,
        claim_token: String,
    },
    HeadChanged {
        expected: EventSeq,
        found: EventSeq,
    },
    /// A retire/rewind landed after the caller read the head and generation.
    GenerationChanged {
        expected: RetireGeneration,
        found: RetireGeneration,
    },
    NotDangling,
    /// Another worker holds an unexpired lease on this call: the claim was
    /// refused and the holder's token stays committable.
    LeaseHeld {
        attempt: u32,
        lease_until_ms: i64,
    },
    BudgetExhausted {
        attempts: u32,
        max_attempts: u32,
    },
}

/// Result of the unanswered-call precondition and outcome append.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplayOutcomeResult {
    Committed { seq: EventSeq },
    AlreadyAnswered,
    ClaimLost,
}

/// Result of releasing a claim that will not start an effect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplayReleaseResult {
    /// The cursor was `active` with the matching token: released. Token and
    /// lease cleared, state moved to `released`, and the attempt is NOT
    /// consumed (decremented) because no effect started under it.
    Released,
    /// The cursor is already `answered`: nothing to release.
    AlreadyAnswered,
    /// No cursor row, the token did not match, or the cursor was not `active`:
    /// fail-closed — a stale token must never release a live claim.
    ClaimLost,
}

/// One row from [`SessionEventStore::load_events_with_retirement`]: the
/// decoded event plus the two retirement facts the fold-restoration path
/// needs.
#[derive(Debug, Clone)]
pub struct RetiredEventRow {
    /// The decoded event with its seq.
    pub record: SessionEventRecord,
    /// `retired_at` is stamped (either retirement kind).
    pub retired: bool,
    /// The BM25 mirror row for this seq survives. `Retire::Through`
    /// (compaction) keeps it; `Retire::From` (clear/rewind) deletes it in the
    /// same transaction — making this the on-disk witness that distinguishes
    /// a compacted span from an erased one. Rows with no content body (turn
    /// markers, checkpoints) never had a mirror row; only content-bearing
    /// rows make the flag meaningful.
    pub fts_mirror_present: bool,
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

        CREATE TABLE IF NOT EXISTS session_replay_cursors (
            session_id      TEXT    NOT NULL,
            call_id         TEXT    NOT NULL,
            attempts        INTEGER NOT NULL,
            state           TEXT    NOT NULL,
            claim_token     TEXT    NOT NULL,
            lease_until     INTEGER NOT NULL,
            claim_generation INTEGER NOT NULL,
            PRIMARY KEY (session_id, call_id)
        );

        CREATE TABLE IF NOT EXISTS session_retire_generations (
            session_id TEXT    NOT NULL,
            generation INTEGER NOT NULL,
            PRIMARY KEY (session_id)
        );
        "#,
        )
        .and_then(|()| add_retired_at_column(conn))
        .and_then(|()| add_replay_cursor_columns(conn));

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

/// Add the `state`, `lease_until`, and `claim_generation` columns to a
/// pre-existing `session_replay_cursors` table. Legacy rows (written before
/// the lease existed) read as an already-expired active claim: their
/// `claim_token` and `attempts` survive, but a new claim may take the cursor
/// over immediately — the safe recovery for a worker that crashed mid-replay
/// and left a claim with no way to expire it. `claim_generation` defaults to
/// `0`, so a legacy claim is only committable while the session is still at
/// generation 0 — a retire/rewind invalidates it (fail-closed).
///
/// `SQLite` has no `ADD COLUMN IF NOT EXISTS`, so probe `pragma_table_info`
/// first — a DB created by the current `CREATE TABLE` above already has them.
fn add_replay_cursor_columns(conn: &Connection) -> Result<(), rusqlite::Error> {
    for (name, ddl) in [
        (
            "state",
            "ALTER TABLE session_replay_cursors ADD COLUMN state TEXT NOT NULL DEFAULT 'active'",
        ),
        (
            "lease_until",
            "ALTER TABLE session_replay_cursors ADD COLUMN lease_until INTEGER NOT NULL DEFAULT 0",
        ),
        (
            "claim_generation",
            "ALTER TABLE session_replay_cursors ADD COLUMN claim_generation INTEGER NOT NULL DEFAULT 0",
        ),
    ] {
        let exists: bool = conn.query_row(
            "SELECT COUNT(*) > 0 FROM pragma_table_info('session_replay_cursors') WHERE name = ?1",
            [name],
            |row| row.get(0),
        )?;
        if !exists {
            conn.execute_batch(ddl)?;
        }
    }
    Ok(())
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

    /// Insert one `session_events` row exactly as another build would have
    /// written it — past [`encode_row`], which is the point: a reader must
    /// cope with what is on disk, not with what this build would have put
    /// there. The ONE fixture for every "a row this build cannot read" test,
    /// in-crate and in `tests/` (`test-helpers`), here because `conn` is
    /// private.
    #[cfg(any(test, feature = "test-helpers"))]
    pub async fn insert_raw_row_for_test(
        &self,
        sid: &SessionId,
        seq: i64,
        event_type: &str,
        json: &str,
    ) {
        let conn = self.conn.lock().await;
        conn.execute(
            "INSERT INTO session_events (session_id, seq, event_type, payload_json, created_at) \
             VALUES (?1, ?2, ?3, ?4, 1)",
            params![session_id_to_string(sid).unwrap(), seq, event_type, json],
        )
        .unwrap();
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

/// The schema version stamped on every row as `"v"`. Bumped when the payload
/// envelope itself changes shape — not when a variant is added, which the
/// `type` tag already names.
pub const SESSION_EVENT_SCHEMA_VERSION: u16 = 1;

/// The prefix of serde's rendered message for an unknown variant of ANY enum
/// at any depth — an unknown `type` tag and an unknown word inside a known
/// event's body render the same way. [`names_unknown_outer_variant`] appends
/// the row's own tag, which is what tells the two apart. Serde's wording,
/// not ours — pinned by
/// `tests::the_unknown_variant_guard_keys_on_serdes_own_wording`.
const UNKNOWN_VARIANT_PREFIX: &str = "unknown variant";

/// True iff serde's message says the row's `type` tag ITSELF is the unknown
/// variant: `unknown variant `<kind_tag>``. A known `type` whose body holds an
/// unknown inner word (`"outcome":"from_the_future"`) renders `unknown
/// variant `from_the_future`` and does not match — it is corruption to this
/// build, never a row a newer build could have marked skippable.
fn names_unknown_outer_variant(err: &str, kind_tag: &str) -> bool {
    err.starts_with(&format!("{UNKNOWN_VARIANT_PREFIX} `{kind_tag}`"))
}

/// A `session_events` row this build could not turn into a [`SessionEvent`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UndecodableRecord {
    pub seq: EventSeq,
    /// The row's `type` tag, when the payload was at least JSON.
    pub kind_tag: Option<String>,
    /// serde's rendered reason.
    pub error: String,
}

impl std::fmt::Display for UndecodableRecord {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "seq {} (type `{}`) could not be decoded by this build: {}",
            self.seq,
            self.kind_tag.as_deref().unwrap_or("?"),
            self.error
        )
    }
}

/// One row, as [`decode_row`] read it.
// Not boxed: `query_rows` / `load_run_markers` build one per row and `fold_strict`
// moves it out at once; nearly every row is `Event`, so `Box` = one alloc per event.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone)]
pub enum DecodedRow {
    Event(SessionEventRecord),
    /// A `type` this build does not know, on a row whose writer — a build that
    /// does know it — marked `ignorable: true`: skipped unread, by that
    /// writer's own policy.
    Skipped {
        seq: EventSeq,
        kind_tag: String,
    },
    Undecodable(UndecodableRecord),
}

/// One session's run markers as [`SessionEventStore::load_run_markers`]
/// hands them over: the decoded slice, or the first row of it this build
/// could not decode.
pub type MarkerSlice = Result<Vec<SessionEventRecord>, UndecodableRecord>;

/// Which of the two run anchors a retired row is — the two event kinds the
/// positional meta join reads (`session_projector::collect_run_spans`): an
/// opener moves the anchor, a meta marks the span the anchor names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetiredAnchorKind {
    RunStarted,
    RunMeta,
}

/// One retired `RunStarted` / `AssistantRunMeta` row as
/// [`SessionEventStore::load_retired_run_anchors`] hands it over: its
/// position and its kind, nothing of its payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetiredRunAnchor {
    pub seq: EventSeq,
    pub kind: RetiredAnchorKind,
}

/// The one encoder: `v` on every row, `ignorable` only when the policy table
/// says so (absent otherwise — never a literal `false`).
pub fn encode_row(event: &SessionEvent) -> Result<String, SessionError> {
    let mut v = serde_json::to_value(event)?;
    let serde_json::Value::Object(m) = &mut v else {
        return Err(SessionError::Storage(
            "event did not serialize to an object".into(),
        ));
    };
    m.insert("v".into(), SESSION_EVENT_SCHEMA_VERSION.into());
    if crate::session::events::ignorable(event) {
        m.insert("ignorable".into(), true.into());
    }
    Ok(v.to_string())
}

/// The one decoder. A `type` this build does not know AND `"ignorable": true`
/// on the row ⇒ [`DecodedRow::Skipped`]; a missing key reads as false; a known
/// `type` whose body will not parse is corruption, not skippable; anything
/// else ⇒ [`DecodedRow::Undecodable`].
pub fn decode_row(seq: EventSeq, created_at_ms: i64, json: &str) -> DecodedRow {
    let err = match serde_json::from_str::<SessionEvent>(json) {
        Ok(event) => {
            return DecodedRow::Event(SessionEventRecord {
                seq,
                event,
                created_at_ms,
            })
        }
        Err(e) => e,
    };
    let raw: Option<serde_json::Value> = serde_json::from_str(json).ok();
    let kind_tag = raw
        .as_ref()
        .and_then(|v| v["type"].as_str())
        .map(str::to_string);
    let error = err.to_string();
    let ignorable = raw
        .as_ref()
        .is_some_and(|v| v["ignorable"] == serde_json::Value::Bool(true));
    match kind_tag {
        Some(kind_tag) if ignorable && names_unknown_outer_variant(&error, &kind_tag) => {
            DecodedRow::Skipped { seq, kind_tag }
        }
        kind_tag => DecodedRow::Undecodable(UndecodableRecord {
            seq,
            kind_tag,
            error,
        }),
    }
}

/// The strict fold for the model-facing readers: `Skipped` rows are dropped
/// (debug-traced), and the first `Undecodable` refuses the whole slice.
pub fn fold_strict(rows: Vec<DecodedRow>) -> Result<Vec<SessionEventRecord>, UndecodableRecord> {
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        match row {
            DecodedRow::Event(r) => out.push(r),
            DecodedRow::Skipped { seq, kind_tag } => {
                tracing::debug!(seq, kind_tag, "session_events: ignorable row skipped");
            }
            DecodedRow::Undecodable(u) => return Err(u),
        }
    }
    Ok(out)
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
        Retire::Through {
            through: through_seq,
            ..
        } => {
            let through_val = i64::try_from(through_seq).unwrap_or(i64::MAX);
            tx.execute(
                "UPDATE session_events SET retired_at = ?3
                 WHERE session_id = ?1 AND seq <= ?2 AND retired_at IS NULL",
                params![session_key, through_val, at],
            )
        }
    }
}

fn replay_call_state(
    tx: &rusqlite::Transaction<'_>,
    session_key: &str,
    call_id: &str,
) -> Result<(usize, usize), SessionError> {
    let mut stmt = tx
        .prepare(
            "SELECT seq, payload_json, created_at FROM session_events
             WHERE session_id = ?1 AND retired_at IS NULL ORDER BY seq ASC",
        )
        .map_err(|e| SessionError::Storage(e.to_string()))?;
    let rows = stmt
        .query_map(params![session_key], |row| {
            let seq: i64 = row.get(0)?;
            let payload: String = row.get(1)?;
            let created_at: i64 = row.get(2)?;
            Ok((seq, payload, created_at))
        })
        .map_err(|e| SessionError::Storage(e.to_string()))?;

    let mut requested = 0usize;
    let mut outcomes = 0usize;
    for row in rows {
        let (seq, payload, created_at) = row.map_err(|e| SessionError::Storage(e.to_string()))?;
        let seq = u64::try_from(seq)
            .map_err(|_| SessionError::Storage(format!("stored seq {seq} is negative")))?;
        let event = match decode_row(seq, created_at, &payload) {
            DecodedRow::Event(record) => record.event,
            DecodedRow::Skipped { .. } => continue,
            DecodedRow::Undecodable(record) => {
                return Err(SessionError::UndecodableRecord(record));
            }
        };
        match event {
            SessionEvent::ToolCallRequested { call_id: id, .. } if id == call_id => {
                requested += 1;
            }
            SessionEvent::ToolResult { call_id: id, .. }
            | SessionEvent::ToolError { call_id: id, .. }
                if id == call_id =>
            {
                outcomes += 1;
            }
            _ => {}
        }
    }
    Ok((requested, outcomes))
}

/// Read back the durable effective-input markers for one call (§4.3).
/// Fail-closed: `None` when zero markers (unknown — never written, so not
/// replay eligible) or two-or-more (ambiguous — dispatched and marked more
/// than once, so the true input is unknowable). `Some` only for exactly one.
fn replay_effective_input(
    tx: &rusqlite::Transaction<'_>,
    session_key: &str,
    call_id: &str,
) -> Result<Option<EffectiveInputRecord>, SessionError> {
    let mut stmt = tx
        .prepare(
            "SELECT seq, payload_json, created_at FROM session_events
             WHERE session_id = ?1 AND event_type = ?2 AND retired_at IS NULL ORDER BY seq ASC",
        )
        .map_err(|e| SessionError::Storage(e.to_string()))?;
    let rows = stmt
        .query_map(params![session_key, "tool_call_effective_input"], |row| {
            let seq: i64 = row.get(0)?;
            let payload: String = row.get(1)?;
            let created_at: i64 = row.get(2)?;
            Ok((seq, payload, created_at))
        })
        .map_err(|e| SessionError::Storage(e.to_string()))?;

    let mut found: Option<EffectiveInputRecord> = None;
    for row in rows {
        let (seq, payload, created_at) = row.map_err(|e| SessionError::Storage(e.to_string()))?;
        let seq = u64::try_from(seq)
            .map_err(|_| SessionError::Storage(format!("stored seq {seq} is negative")))?;
        let event = match decode_row(seq, created_at, &payload) {
            DecodedRow::Event(record) => record.event,
            DecodedRow::Skipped { .. } => continue,
            DecodedRow::Undecodable(record) => {
                return Err(SessionError::UndecodableRecord(record));
            }
        };
        if let SessionEvent::ToolCallEffectiveInput {
            turn_id,
            call_id: id,
            input,
            at,
        } = event
        {
            if id == call_id {
                // A second marker makes the true input ambiguous: fail closed.
                if found.is_some() {
                    return Ok(None);
                }
                found = Some(EffectiveInputRecord { turn_id, input, at });
            }
        }
    }
    Ok(found)
}

fn replay_head(
    tx: &rusqlite::Transaction<'_>,
    session_key: &str,
) -> Result<EventSeq, SessionError> {
    let max_seq: Option<i64> = tx
        .query_row(
            "SELECT MAX(seq) FROM session_events WHERE session_id = ?1",
            params![session_key],
            |row| row.get(0),
        )
        .map_err(|e| SessionError::Storage(e.to_string()))?;
    match max_seq {
        Some(value) if value >= 0 => u64::try_from(value)
            .map_err(|_| SessionError::Storage(format!("stored seq {value} is invalid"))),
        Some(value) => Err(SessionError::Storage(format!(
            "stored seq {value} is negative"
        ))),
        None => Ok(0),
    }
}

/// Read the session's retire generation, or `0` for a session that has never
/// retired anything.
fn replay_generation(
    tx: &rusqlite::Transaction<'_>,
    session_key: &str,
) -> Result<RetireGeneration, SessionError> {
    let generation: Option<i64> = tx
        .query_row(
            "SELECT generation FROM session_retire_generations WHERE session_id = ?1",
            params![session_key],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e| SessionError::Storage(e.to_string()))?;
    match generation {
        Some(value) if value >= 0 => u64::try_from(value).map_err(|_| {
            SessionError::Storage(format!("stored retire generation {value} is invalid"))
        }),
        Some(value) => Err(SessionError::Storage(format!(
            "stored retire generation {value} is negative"
        ))),
        None => Ok(0),
    }
}

/// Read the replay cursor for one call: `(attempts, state, lease_until)`.
fn replay_cursor(
    tx: &rusqlite::Transaction<'_>,
    session_key: &str,
    call_id: &str,
) -> Result<Option<(u32, String, i64)>, SessionError> {
    let row: Option<(i64, String, i64)> = tx
        .query_row(
            "SELECT attempts, state, lease_until FROM session_replay_cursors
             WHERE session_id = ?1 AND call_id = ?2",
            params![session_key, call_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(|e| SessionError::Storage(e.to_string()))?;
    match row {
        Some((attempts, state, lease_until)) => {
            if attempts < 0 {
                return Err(SessionError::Storage(
                    "negative replay attempt cursor".into(),
                ));
            }
            let attempts = u32::try_from(attempts)
                .map_err(|_| SessionError::Storage("replay attempt cursor overflow".into()))?;
            Ok(Some((attempts, state, lease_until)))
        }
        None => Ok(None),
    }
}

/// Bump the session's retire generation inside `tx`, so a claim that read the
/// old generation fails its precondition. Called only when a retire actually
/// retired rows (see callers): an idempotent no-op retire must not invalidate
/// in-flight claims, because the conversation did not change.
fn bump_retire_generation(
    tx: &rusqlite::Transaction<'_>,
    session_key: &str,
) -> rusqlite::Result<()> {
    tx.execute(
        "INSERT INTO session_retire_generations (session_id, generation) VALUES (?1, 1)
         ON CONFLICT(session_id) DO UPDATE SET generation = generation + 1",
        params![session_key],
    )?;
    Ok(())
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
        let retired = retire_in_txn(&tx, session_key, r, at)
            .map_err(|e| SessionError::Storage(e.to_string()))?;
        // The head-side condition (see `Retire::Through`): checked inside the
        // transaction, so it and the commit cannot be separated by a
        // `retire_from` on another path. Returning drops `tx` uncommitted,
        // which rolls the retire back with nothing inserted.
        if let Retire::Through { live, .. } = r {
            if retired != live {
                return Err(SessionError::RetireSpanChanged {
                    expected: live,
                    found: retired,
                });
            }
        }
        if retired > 0 {
            bump_retire_generation(&tx, session_key)
                .map_err(|e| SessionError::Storage(e.to_string()))?;
        }
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
                payload: encode_row(event)?,
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

    async fn claim_replay_call(
        &self,
        session_id: &SessionId,
        expected_head: EventSeq,
        expected_generation: RetireGeneration,
        call_id: &str,
        max_attempts: u32,
        lease_ms: i64,
    ) -> Result<ReplayClaimResult, SessionError> {
        let session_key = session_id_to_string(session_id)?;
        if lease_ms < REPLAY_LEASE_TTL_MS {
            return Err(SessionError::Other(format!(
                "replay claim lease_ms {lease_ms} is below the {REPLAY_LEASE_TTL_MS}ms floor"
            )));
        }
        let mut conn = self.conn.lock().await;
        Ok(with_synchronous_full(
            &mut conn,
            |conn| -> Result<ReplayClaimResult, SessionError> {
                let tx = conn
                    .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
                    .map_err(|e| {
                        SessionError::Storage(format!("replay claim BEGIN failed: {e}"))
                    })?;
                let found_head = replay_head(&tx, &session_key)?;
                if found_head != expected_head {
                    return Ok(ReplayClaimResult::HeadChanged {
                        expected: expected_head,
                        found: found_head,
                    });
                }
                let found_generation = replay_generation(&tx, &session_key)?;
                if found_generation != expected_generation {
                    return Ok(ReplayClaimResult::GenerationChanged {
                        expected: expected_generation,
                        found: found_generation,
                    });
                }
                let (requested, outcomes) = replay_call_state(&tx, &session_key, call_id)?;
                if requested != 1 || outcomes != 0 {
                    return Ok(ReplayClaimResult::NotDangling);
                }
                let now = crate::session::events::now_ms();
                // A held, unexpired lease belongs to another worker: refuse
                // without touching the cursor, so the holder's token still
                // commits and its attempts survive.
                let attempts = match replay_cursor(&tx, &session_key, call_id)? {
                    Some((attempts, state, lease_until))
                        if state == REPLAY_STATE_ACTIVE && lease_until > now =>
                    {
                        return Ok(ReplayClaimResult::LeaseHeld {
                            attempt: attempts,
                            lease_until_ms: lease_until,
                        });
                    }
                    Some((attempts, ..)) => attempts,
                    None => 0,
                };
                if attempts >= max_attempts {
                    return Ok(ReplayClaimResult::BudgetExhausted {
                        attempts,
                        max_attempts,
                    });
                }
                let next_attempt = attempts + 1;
                let claim_token = uuid::Uuid::new_v4().to_string();
                let claim_generation = i64::try_from(found_generation).map_err(|_| {
                    SessionError::Storage(format!(
                        "retire generation {found_generation} exceeds i64::MAX"
                    ))
                })?;
                // Fail closed on a `lease_ms` large enough that `now + lease_ms`
                // overflows i64: refuse before the INSERT so neither the cursor
                // nor the attempt is written.
                let lease_until = now.checked_add(lease_ms).ok_or_else(|| {
                    SessionError::Storage(format!(
                        "replay claim lease_ms {lease_ms} overflows lease expiry from now {now}"
                    ))
                })?;
                tx.execute(
                    "INSERT INTO session_replay_cursors
                 (session_id, call_id, attempts, state, claim_token, lease_until, claim_generation)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                 ON CONFLICT(session_id, call_id) DO UPDATE SET
                   attempts = excluded.attempts,
                   state = excluded.state,
                   claim_token = excluded.claim_token,
                   lease_until = excluded.lease_until,
                   claim_generation = excluded.claim_generation",
                    params![
                        session_key,
                        call_id,
                        i64::from(next_attempt),
                        REPLAY_STATE_ACTIVE,
                        claim_token,
                        lease_until,
                        claim_generation,
                    ],
                )
                .map_err(|e| SessionError::Storage(e.to_string()))?;
                tx.commit().map_err(|e| {
                    SessionError::Storage(format!("replay claim COMMIT failed: {e}"))
                })?;
                Ok(ReplayClaimResult::Claimed {
                    attempt: next_attempt,
                    claim_token,
                })
            },
        )??)
    }

    async fn commit_replay_outcome(
        &self,
        session_id: &SessionId,
        call_id: &str,
        claim_token: &str,
        event: &SessionEvent,
        created_at_ms: i64,
    ) -> Result<ReplayOutcomeResult, SessionError> {
        let event_call_id = match event {
            SessionEvent::ToolResult { call_id, .. } | SessionEvent::ToolError { call_id, .. } => {
                call_id
            }
            _ => {
                return Err(SessionError::Other(
                    "replay outcome must be ToolResult or ToolError".into(),
                ));
            }
        };
        if event_call_id != call_id {
            return Err(SessionError::Other(
                "replay outcome call_id does not match claim".into(),
            ));
        }
        let row = EncodedRow {
            seq: 0,
            turn_id: extract_turn_id(event).map(|id| id.to_string()),
            event_type: event_type_tag(event),
            payload: encode_row(event)?,
            created_at: created_at_ms,
            fts_body: render_event_text(event),
        };
        let session_key = session_id_to_string(session_id)?;
        let mut conn = self.conn.lock().await;
        let result = with_synchronous_full(
            &mut conn,
            |conn| -> Result<ReplayOutcomeResult, SessionError> {
                let tx = conn
                    .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
                    .map_err(|e| {
                        SessionError::Storage(format!("replay outcome BEGIN failed: {e}"))
                    })?;
                // An empty token is never a valid claim: rejecting it here closes
                // the bypass where an answered cursor's cleared token ('') would
                // otherwise equal a caller's empty string. The cursor must still
                // be `active`, and its stored retire generation must match the
                // current one — a retire/rewind landed after the claim invalidates
                // the token even though the call may read dangling again.
                if claim_token.is_empty() {
                    return Ok(ReplayOutcomeResult::ClaimLost);
                }
                let stored: Option<(String, String, i64)> = tx
                    .query_row(
                        "SELECT claim_token, state, claim_generation
                     FROM session_replay_cursors
                     WHERE session_id = ?1 AND call_id = ?2",
                        params![session_key, call_id],
                        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                    )
                    .optional()
                    .map_err(|e| SessionError::Storage(e.to_string()))?;
                let Some((stored_token, state, claim_generation)) = stored else {
                    return Ok(ReplayOutcomeResult::ClaimLost);
                };
                if state != REPLAY_STATE_ACTIVE {
                    return Ok(ReplayOutcomeResult::ClaimLost);
                }
                if stored_token != claim_token {
                    return Ok(ReplayOutcomeResult::ClaimLost);
                }
                let current_generation = replay_generation(&tx, &session_key)?;
                let current_generation_i64 = i64::try_from(current_generation).map_err(|_| {
                    SessionError::Storage(format!(
                        "retire generation {current_generation} exceeds i64::MAX"
                    ))
                })?;
                if claim_generation != current_generation_i64 {
                    return Ok(ReplayOutcomeResult::ClaimLost);
                }
                let (requested, outcomes) = replay_call_state(&tx, &session_key, call_id)?;
                if requested != 1 || outcomes != 0 {
                    return Ok(ReplayOutcomeResult::AlreadyAnswered);
                }
                let seq = replay_head(&tx, &session_key)?
                    .checked_add(1)
                    .ok_or_else(|| SessionError::Storage("seq overflow".into()))?;
                let seq_i64 = i64::try_from(seq)
                    .map_err(|_| SessionError::Storage(format!("seq {seq} exceeds i64::MAX")))?;
                tx.execute(
                    "INSERT INTO session_events
                 (session_id, seq, turn_id, event_type, payload_json, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![
                        session_key,
                        seq_i64,
                        row.turn_id,
                        row.event_type,
                        row.payload,
                        row.created_at,
                    ],
                )
                .map_err(|e| SessionError::Storage(e.to_string()))?;
                // Answer the cursor, KEEPING `attempts`: the budget is a durable
                // count of replay tries for this call and must survive a rewind
                // that makes the call dangling again. Clearing the token makes a
                // repeat commit with the stale token read as ClaimLost.
                let updated = tx
                    .execute(
                        "UPDATE session_replay_cursors
                     SET state = ?3, claim_token = '', lease_until = 0
                     WHERE session_id = ?1 AND call_id = ?2 AND claim_token = ?4",
                        params![session_key, call_id, REPLAY_STATE_ANSWERED, claim_token],
                    )
                    .map_err(|e| SessionError::Storage(e.to_string()))?;
                if updated != 1 {
                    return Ok(ReplayOutcomeResult::ClaimLost);
                }
                tx.commit().map_err(|e| {
                    SessionError::Storage(format!("replay outcome COMMIT failed: {e}"))
                })?;
                Ok(ReplayOutcomeResult::Committed { seq })
            },
        )??;
        if let ReplayOutcomeResult::Committed { seq } = result {
            if let Some(body) = row.fts_body {
                if let Err(e) = conn.execute(
                    "INSERT INTO session_events_fts
                     (body, session_id, seq, event_type, created_at)
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![body, session_key, seq, row.event_type, row.created_at],
                ) {
                    tracing::debug!(error = %e, "replay outcome FTS index insert failed");
                }
            }
        }
        Ok(result)
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
        fold_strict(query_rows(&conn, &session_key, from_val, to_val)?)
            .map_err(SessionError::UndecodableRecord)
    }

    async fn load_events_with_retirement(
        &self,
        session_id: &SessionId,
        from: Option<EventSeq>,
        to: Option<EventSeq>,
    ) -> Result<Vec<RetiredEventRow>, SessionError> {
        let session_key = session_id_to_string(session_id)?;
        // Same saturating-bound rule as `load_events_range`.
        let from_val = i64::try_from(from.unwrap_or(0)).unwrap_or(i64::MAX);
        let to_val = to.and_then(|v| i64::try_from(v).ok()).unwrap_or(i64::MAX);

        let conn = self.conn.lock().await;
        let mut stmt = conn
            .prepare(
                "SELECT e.seq, e.payload_json, e.created_at, e.retired_at,
                        (SELECT COUNT(*) FROM session_events_fts f
                         WHERE f.session_id = e.session_id AND f.seq = e.seq)
                 FROM session_events e
                 WHERE e.session_id = ?1 AND e.seq >= ?2 AND e.seq < ?3
                 ORDER BY e.seq ASC",
            )
            .map_err(|e| SessionError::Storage(e.to_string()))?;
        let rows = stmt
            .query_map(params![session_key, from_val, to_val], |row| {
                let seq: i64 = row.get(0)?;
                let payload: String = row.get(1)?;
                let created_at: i64 = row.get(2)?;
                let retired_at: Option<i64> = row.get(3)?;
                let fts_rows: i64 = row.get(4)?;
                Ok((seq, payload, created_at, retired_at.is_some(), fts_rows > 0))
            })
            .map_err(|e| SessionError::Storage(e.to_string()))?;

        let mut out = Vec::new();
        for row in rows {
            let (seq, payload, created_at, retired, fts_mirror_present) =
                row.map_err(|e| SessionError::Storage(e.to_string()))?;
            let seq = u64::try_from(seq)
                .map_err(|_| SessionError::Storage(format!("stored seq {seq} is negative")))?;
            // The trait contract's decode policy: ignorable rows drop, the
            // first undecodable row refuses the slice (fold_strict's shape,
            // kept per-row here so the retirement flags stay aligned).
            match decode_row(seq, created_at, &payload) {
                DecodedRow::Event(record) => out.push(RetiredEventRow {
                    record,
                    retired,
                    fts_mirror_present,
                }),
                DecodedRow::Skipped { seq, kind_tag } => {
                    tracing::debug!(seq, kind_tag, "session_events: ignorable row skipped");
                }
                DecodedRow::Undecodable(u) => return Err(SessionError::UndecodableRecord(u)),
            }
        }
        Ok(out)
    }

    async fn load_rows(&self, session_id: &SessionId) -> Result<Vec<DecodedRow>, SessionError> {
        let session_key = session_id_to_string(session_id)?;
        let conn = self.conn.lock().await;
        query_rows(&conn, &session_key, 0, i64::MAX)
    }

    async fn retire_record(
        &self,
        session_id: &SessionId,
        seq: EventSeq,
    ) -> Result<bool, SessionError> {
        let session_key = session_id_to_string(session_id)?;
        let seq_i64 = i64::try_from(seq)
            .map_err(|_| SessionError::Storage(format!("seq {seq} exceeds i64::MAX")))?;
        let at = crate::session::events::now_ms();

        let mut conn = self.conn.lock().await;
        // `retired_at IS NULL` is what makes a second call answer `false`
        // instead of re-stamping the row. The retire and its generation bump
        // share one transaction: a claim that read the pre-fix generation
        // must fail, and a partial failure must not leave one without the
        // other.
        let tx = conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(|e| SessionError::Storage(format!("retire_record BEGIN failed: {e}")))?;
        let changed = tx
            .execute(
                "UPDATE session_events SET retired_at = ?1
                 WHERE session_id = ?2 AND seq = ?3 AND retired_at IS NULL",
                params![at, session_key, seq_i64],
            )
            .map_err(|e| SessionError::Storage(format!("retire_record failed: {e}")))?;
        if changed > 0 {
            bump_retire_generation(&tx, &session_key)
                .map_err(|e| SessionError::Storage(e.to_string()))?;
        }
        tx.commit()
            .map_err(|e| SessionError::Storage(format!("retire_record COMMIT failed: {e}")))?;
        Ok(changed == 1)
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

    async fn load_retire_generation(
        &self,
        session_id: &SessionId,
    ) -> Result<RetireGeneration, SessionError> {
        let session_key = session_id_to_string(session_id)?;
        let conn = self.conn.lock().await;
        let generation: Option<i64> = conn
            .query_row(
                "SELECT generation FROM session_retire_generations WHERE session_id = ?1",
                params![session_key],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| SessionError::Storage(e.to_string()))?;
        match generation {
            Some(v) if v >= 0 => u64::try_from(v).map_err(|_| {
                SessionError::Storage(format!("stored retire generation {v} is invalid"))
            }),
            Some(v) => Err(SessionError::Storage(format!(
                "stored retire generation {v} is negative"
            ))),
            None => Ok(0),
        }
    }

    async fn load_tool_call_effective_input(
        &self,
        session_id: &SessionId,
        call_id: &str,
    ) -> Result<Option<EffectiveInputRecord>, SessionError> {
        let session_key = session_id_to_string(session_id)?;
        let mut conn = self.conn.lock().await;
        let tx = conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(|e| SessionError::Storage(format!("effective input BEGIN failed: {e}")))?;
        let found = replay_effective_input(&tx, &session_key, call_id)?;
        tx.commit()
            .map_err(|e| SessionError::Storage(format!("effective input COMMIT failed: {e}")))?;
        Ok(found)
    }

    async fn release_replay_claim(
        &self,
        session_id: &SessionId,
        call_id: &str,
        claim_token: &str,
    ) -> Result<ReplayReleaseResult, SessionError> {
        let session_key = session_id_to_string(session_id)?;
        let mut conn = self.conn.lock().await;
        Ok(with_synchronous_full(
            &mut conn,
            |conn| -> Result<ReplayReleaseResult, SessionError> {
                let tx = conn
                    .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
                    .map_err(|e| {
                        SessionError::Storage(format!("replay release BEGIN failed: {e}"))
                    })?;
                // An empty token is never a valid claim (same bypass closed in
                // commit): rejecting it here means a cleared cursor cannot be
                // released by an empty-string caller.
                if claim_token.is_empty() {
                    return Ok(ReplayReleaseResult::ClaimLost);
                }
                let stored: Option<(String, String, i64)> = tx
                    .query_row(
                        "SELECT claim_token, state, attempts
                         FROM session_replay_cursors
                         WHERE session_id = ?1 AND call_id = ?2",
                        params![session_key, call_id],
                        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                    )
                    .optional()
                    .map_err(|e| SessionError::Storage(e.to_string()))?;
                let Some((stored_token, state, attempts)) = stored else {
                    return Ok(ReplayReleaseResult::ClaimLost);
                };
                if state == REPLAY_STATE_ANSWERED {
                    return Ok(ReplayReleaseResult::AlreadyAnswered);
                }
                if state != REPLAY_STATE_ACTIVE {
                    return Ok(ReplayReleaseResult::ClaimLost);
                }
                if stored_token != claim_token {
                    return Ok(ReplayReleaseResult::ClaimLost);
                }
                // Release WITHOUT consuming the attempt: no effect started
                // under this claim, so the budget is preserved for the next
                // real try. Floor at 0 in case the cursor was hand-seeded.
                let released_attempts = if attempts > 0 { attempts - 1 } else { 0 };
                let updated = tx
                    .execute(
                        "UPDATE session_replay_cursors
                         SET state = ?3, claim_token = '', lease_until = 0, attempts = ?4
                         WHERE session_id = ?1 AND call_id = ?2 AND claim_token = ?5",
                        params![
                            session_key,
                            call_id,
                            REPLAY_STATE_RELEASED,
                            released_attempts,
                            claim_token,
                        ],
                    )
                    .map_err(|e| SessionError::Storage(e.to_string()))?;
                if updated != 1 {
                    return Ok(ReplayReleaseResult::ClaimLost);
                }
                tx.commit().map_err(|e| {
                    SessionError::Storage(format!("replay release COMMIT failed: {e}"))
                })?;
                Ok(ReplayReleaseResult::Released)
            },
        )??)
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
        if n > 0 {
            bump_retire_generation(&tx, &session_key)
                .map_err(|e| SessionError::Storage(e.to_string()))?;
        }
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

    async fn load_retired_run_anchors(
        &self,
        session_id: &SessionId,
    ) -> Result<Vec<RetiredRunAnchor>, SessionError> {
        let session_key = session_id_to_string(session_id)?;
        // Same discipline as `load_run_markers`: the IN-list is rendered from
        // the constant the census pins, never spelled inline, and the column
        // is decoded through the same constant — one table, two directions.
        let in_list = RUN_ANCHOR_EVENT_TYPES
            .iter()
            .map(|(t, _)| format!("'{t}'"))
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "SELECT seq, event_type
             FROM session_events
             WHERE session_id = ?1 AND retired_at IS NOT NULL
               AND event_type IN ({in_list})
             ORDER BY seq ASC"
        );
        let conn = self.conn.lock().await;
        let mut stmt = conn
            .prepare(&sql)
            .map_err(|e| SessionError::Storage(e.to_string()))?;
        let rows = stmt
            .query_map(params![session_key], |row| {
                let seq: i64 = row.get(0)?;
                let event_type: String = row.get(1)?;
                Ok((seq, event_type))
            })
            .map_err(|e| SessionError::Storage(e.to_string()))?;
        let mut out = Vec::new();
        for row in rows {
            let (seq, event_type) = row.map_err(|e| SessionError::Storage(e.to_string()))?;
            let seq = u64::try_from(seq)
                .map_err(|_| SessionError::Storage(format!("stored seq {seq} is negative")))?;
            // The IN-list admits exactly these tags, so a miss here is the
            // constant disagreeing with itself — refuse rather than guess.
            let kind = RUN_ANCHOR_EVENT_TYPES
                .iter()
                .find(|(t, _)| *t == event_type)
                .map(|(_, k)| *k)
                .ok_or_else(|| {
                    SessionError::Storage(format!(
                        "retired anchor row {seq} has event_type {event_type:?}, outside the anchor set"
                    ))
                })?;
            out.push(RetiredRunAnchor { seq, kind });
        }
        Ok(out)
    }

    async fn load_run_markers(&self) -> Result<Vec<(SessionId, MarkerSlice)>, SessionError> {
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
        // contiguous, so a running group key is enough — no HashMap. Rows
        // are grouped DECODED-PER-ROW and folded per group, so a marker this
        // build cannot read refuses its own session's slice and no other's.
        let mut grouped: Vec<(SessionId, Vec<DecodedRow>)> = Vec::new();
        for row in rows {
            let (session_id_str, seq, payload, created_at) =
                row.map_err(|e| SessionError::Storage(e.to_string()))?;
            let session_id: SessionId = serde_json::from_str(&session_id_str)?;
            let seq = u64::try_from(seq)
                .map_err(|_| SessionError::Storage(format!("stored seq {seq} is negative")))?;
            let decoded = decode_row(seq, created_at, &payload);
            match grouped.last_mut() {
                Some((sid, records)) if *sid == session_id => {
                    records.push(decoded);
                }
                _ => grouped.push((session_id, vec![decoded])),
            }
        }
        Ok(grouped
            .into_iter()
            .map(|(sid, rows)| (sid, fold_strict(rows)))
            .collect())
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

/// One session's live rows with `seq` in `[from, to)`, each through
/// [`decode_row`]. The one SELECT behind `load_events_range`, `load_rows` and
/// (via `load_all_events`) every model-facing read; the callers differ only
/// in what they do with a row that did not decode.
fn query_rows(
    conn: &Connection,
    session_key: &str,
    from: i64,
    to: i64,
) -> Result<Vec<DecodedRow>, SessionError> {
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
        .query_map(params![session_key, from, to], |row| {
            let seq: i64 = row.get(0)?;
            let payload: String = row.get(1)?;
            let created_at: i64 = row.get(2)?;
            Ok((seq, payload, created_at))
        })
        .map_err(|e| SessionError::Storage(e.to_string()))?;

    let mut out = Vec::new();
    for row in rows {
        let (seq, payload, created_at) = row.map_err(|e| SessionError::Storage(e.to_string()))?;
        let seq = u64::try_from(seq)
            .map_err(|_| SessionError::Storage(format!("stored seq {seq} is negative")))?;
        out.push(decode_row(seq, created_at, &payload));
    }
    Ok(out)
}

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
        | SessionEvent::ToolCallEffectiveInput { turn_id, .. }
        | SessionEvent::ToolCallApproved { turn_id, .. }
        | SessionEvent::ToolCallDenied { turn_id, .. }
        | SessionEvent::ToolCallParked { turn_id, .. }
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
        | SessionEvent::CompactionPerformed { .. }
        | SessionEvent::FoldRecorded { .. } => None,
    }
}

/// The `event_type_tag` of every variant `reduction::is_marker` accepts —
/// the one list `load_run_markers` selects by. Pinned equal by test
/// (`tests::marker_event_types_are_exactly_the_reducers_marker_set`).
pub(crate) const MARKER_EVENT_TYPES: [&str; 3] =
    ["run_started", "run_finished", "resume_attempted"];

/// The `event_type_tag` of each [`RetiredAnchorKind`], in the enum's order —
/// the list `load_retired_run_anchors` selects by and the map it decodes the
/// column with. Pinned against `event_type_tag` over the real variants by
/// `tests::retired_anchor_event_types_are_the_two_anchor_kinds_tags`.
pub(crate) const RUN_ANCHOR_EVENT_TYPES: [(&str, RetiredAnchorKind); 2] = [
    ("run_started", RetiredAnchorKind::RunStarted),
    ("assistant_run_meta", RetiredAnchorKind::RunMeta),
];

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
        SessionEvent::ToolCallEffectiveInput { .. } => "tool_call_effective_input",
        SessionEvent::ToolCallApproved { .. } => "tool_call_approved",
        SessionEvent::ToolCallDenied { .. } => "tool_call_denied",
        SessionEvent::ToolCallParked { .. } => "tool_call_parked",
        SessionEvent::ToolResult { .. } => "tool_result",
        SessionEvent::ToolError { .. } => "tool_error",
        SessionEvent::SubagentSpawned { .. } => "subagent_spawned",
        SessionEvent::SubagentReturned { .. } => "subagent_returned",
        SessionEvent::CompactionPerformed { .. } => "compaction_performed",
        SessionEvent::FoldRecorded { .. } => "fold_recorded",
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
/// The content an event carries, as `(role label, raw body)` — raw meaning
/// untrimmed and uncapped, the verbatim text `session_decompress` restores.
/// `None` for events with no content body. This is the ONE place that decides
/// which variants are content-bearing: the FTS mirror (`render_event_text`
/// trims and caps the same body) and the fold-restoration path both derive
/// from it, so indexing and restoration cannot drift apart.
pub(crate) fn event_content_text(event: &SessionEvent) -> Option<(&'static str, Cow<'_, str>)> {
    let labelled: (&'static str, Cow<'_, str>) = match event {
        SessionEvent::UserMessage { content, .. } => ("user", Cow::Borrowed(&content.text)),
        SessionEvent::AssistantMessage { content, .. } => {
            ("assistant", Cow::Borrowed(&content.text))
        }
        SessionEvent::SystemMessage { content, .. } => ("system", Cow::Borrowed(content)),
        SessionEvent::ToolCallRequested { name, input, .. } => {
            ("tool_call", Cow::Owned(format!("{name} {input}")))
        }
        SessionEvent::ToolResult { output, .. } => ("tool_result", render_json(&output.value)),
        SessionEvent::ToolError { error, .. } => ("tool_error", Cow::Borrowed(error)),
        SessionEvent::ToolCallDenied { reason, .. } => ("tool_denied", Cow::Borrowed(reason)),
        SessionEvent::SubagentReturned { summary, .. } => ("subagent", Cow::Borrowed(summary)),
        SessionEvent::Error { message, .. } => ("error", Cow::Borrowed(message)),
        _ => return None,
    };
    Some(labelled)
}

fn render_event_text(event: &SessionEvent) -> Option<String> {
    let (_, raw) = event_content_text(event)?;
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

/// Every production reader of [`global_session_event_store`], and what an
/// uninstalled handle reads as THERE — one row per file, the second column
/// taken from the code at that site.
///
/// Not a hand-list: the table this replaces named four readers while the
/// tree held more than twice that many files that do, this one included.
/// The first column is pinned to the tree by
/// `the_event_store_reader_census_matches_the_tree`, through the same walk
/// `session::service::SESSION_SERVICE_READERS` uses — its scope, and which
/// spellings of a read it sees, are stated on
/// [`crate::utils::source_scan::files_whose_production_code_reads`]. The
/// second column is prose and is NOT pinned; whoever changes a site's `None`
/// arm owes this table the new sentence.
///
/// The census's first version saw only the CALL spelling, and the first
/// version of this doc restated its verdict as a fact — "`session_projector.rs`
/// never read the handle at all". It does: `resolve_events` falls back to it
/// as a function POINTER (`.or_else(global_session_event_store)`), the
/// spelling that census could not see. The row is below; the sentence was
/// the instrument's blind spot written down as the tree's shape (判据 §3).
///
/// `#[cfg(test)]` because the census is its only reader: the table is
/// documentation the tree can contradict, not runtime data.
#[cfg(test)]
pub(crate) const SESSION_EVENT_STORE_READERS: &[(&str, &str)] = &[
    (
        "src/builtin_tools/recall_events.rs",
        "`Ok(empty)` plus a note to the model (\"not available in this deployment\")",
    ),
    (
        "src/builtin_tools/decompress.rs",
        "`Err` (\"session event log not available\") — without the log there are no folds to \
         restore, and an empty answer would read as \"nothing was ever compacted\"",
    ),
    (
        "src/diagnostics/mod.rs",
        "`core/projection-holes` and `core/session-log` are registered with `None` and report \
         UNKNOWN (two builder sites)",
    ),
    (
        "src/gateway/execution_engine/run_loop/inner.rs",
        "the legacy `messages`→`session_events` backfill is skipped in silence (the read is \
         paired with the service handle; no `else` arm)",
    ),
    (
        "src/gateway/handlers/chat.rs",
        "`chat.history`'s snapshot carries `last_run: None` — \"we did not find out\", never a \
         clean answer",
    ),
    (
        "src/gateway/handlers/session/db_handlers/query.rs",
        "`sessions.list` leaves every row's `last_run` at `None` — \"we did not find out\"",
    ),
    (
        "src/gateway/handlers/tool_output.rs",
        "`trace.tool_output` answers `SERVICE_UNAVAILABLE` (\"session event log not available\")",
    ),
    (
        "src/gateway/handlers/trace_replay.rs",
        "`trace.by_runs` replays with an empty presentation map — holes show nothing, never \
         \"no diff\"",
    ),
    (
        "src/gateway/session_projector.rs",
        "`resolve_events` falls back to the handle as a fn pointer when no store is pinned, at \
         two sites: the drain treats every seq as live (`event_retired` → `Ok(false)`, so a \
         retired row still projects); a heal pass puts its claim back and reports `errored`",
    ),
    (
        "src/session/store.rs",
        "`retire_live_events` answers `Ok(0)` — \"retired nothing\", indistinguishable from \
         nothing to retire",
    ),
    (
        "src/teams/dispatcher/schedule/reclaim.rs",
        "the crashed attempt's member-session repair is skipped with no log line; the reclaim \
         to `Pending` itself still happens",
    ),
];

/// `ConsumerDecides`, and this handle is the sharper case of the pair in this
/// batch: the production reads produce *different* answers, some of which
/// are reported to the caller as success — see `SESSION_EVENT_STORE_READERS`
/// (above; `#[cfg(test)]`, because the tree is its reader) for each one's
/// reading, and for how that list is kept honest.
///
/// Each arm is individually defensible (all doc-comment their reasoning),
/// which is exactly why no `IndistinguishableDefault { reads_as }` sentence
/// could be written for this slot: there is no single thing a missing handle
/// reads as. This variant records that there is more than one of them.
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
/// conditional, and without this the readers named in
/// `SESSION_EVENT_STORE_READERS` cannot tell "this deployment has no
/// session-event log" from "boot died before it reached
/// `build_sqlite_session_service`'s call site in `start_server`". Named rather
/// than numbered on purpose — a line coordinate in this file would be a
/// carried number for a file that has no reason to track `start/mod.rs`.
/// `because` is quoted verbatim to an operator.
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

        async fn load_retired_run_anchors(
            &self,
            session_id: &SessionId,
        ) -> Result<Vec<RetiredRunAnchor>, SessionError> {
            self.inner.load_retired_run_anchors(session_id).await
        }

        async fn load_run_markers(&self) -> Result<Vec<(SessionId, MarkerSlice)>, SessionError> {
            self.inner.load_run_markers().await
        }

        async fn load_rows(&self, session_id: &SessionId) -> Result<Vec<DecodedRow>, SessionError> {
            self.inner.load_rows(session_id).await
        }

        async fn retire_record(
            &self,
            session_id: &SessionId,
            seq: EventSeq,
        ) -> Result<bool, SessionError> {
            self.inner.retire_record(session_id, seq).await
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
        let cursor_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master
                 WHERE type='table' AND name='session_replay_cursors'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(cursor_count, 1);
        let gen_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master
                 WHERE type='table' AND name='session_retire_generations'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(gen_count, 1);
    }

    // -----------------------------------------------------------------------
    // SqliteEventStore tests
    use crate::routing::session_key::SessionKey;
    use crate::session::events::{now_ms, MessageContent, ToolOutput, TurnTrigger};

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
    fn tool_requested(tid: uuid::Uuid, call_id: &str, at: i64) -> SessionEvent {
        SessionEvent::ToolCallRequested {
            turn_id: tid,
            call_id: call_id.into(),
            name: "safe_tool".into(),
            input: serde_json::json!({"value": 1}),
            identity: Some(crate::tools::descriptor::ToolCallIdentity {
                schema_version: crate::tools::descriptor::SCHEMA_VERSION,
                revision: 1,
                replay_policy: crate::tools::descriptor::ReplayPolicy::Safe,
                replay_contract_fingerprint: None,
            }),
            at,
        }
    }

    #[tokio::test]
    async fn replay_claim_is_expected_head_and_budget_bounded() {
        let store = make_store();
        let sid = sample_session_id();
        let tid = uuid::Uuid::new_v4();
        let at = now_ms();
        store
            .append(&sid, 1, &tool_requested(tid, "call-1", at), at)
            .await
            .unwrap();
        let head = store.load_head_seq(&sid).await.unwrap();

        let first = store
            .claim_replay_call(&sid, head, 0, "call-1", 2, REPLAY_LEASE_TTL_MS)
            .await
            .unwrap();
        let ReplayClaimResult::Claimed {
            attempt,
            claim_token,
        } = first
        else {
            panic!("expected first claim");
        };
        assert_eq!(attempt, 1);

        let stale = store
            .claim_replay_call(&sid, head - 1, 0, "call-1", 2, REPLAY_LEASE_TTL_MS)
            .await
            .unwrap();
        assert!(matches!(stale, ReplayClaimResult::HeadChanged { .. }));

        // A second claim while the first lease is held must NOT overwrite the
        // token: the lease is refused and the original token stays committable.
        let second = store
            .claim_replay_call(&sid, head, 0, "call-1", 2, REPLAY_LEASE_TTL_MS)
            .await
            .unwrap();
        assert!(matches!(
            second,
            ReplayClaimResult::LeaseHeld { attempt: 1, .. }
        ));

        let result = SessionEvent::ToolResult {
            turn_id: tid,
            call_id: "call-1".into(),
            output: ToolOutput {
                value: serde_json::json!({"ok": true}),
                metadata: Default::default(),
            },
            at: at + 1,
        };
        assert!(matches!(
            store
                .commit_replay_outcome(&sid, "call-1", "wrong-token", &result, at + 1)
                .await
                .unwrap(),
            ReplayOutcomeResult::ClaimLost
        ));
        assert!(matches!(
            store
                .commit_replay_outcome(&sid, "call-1", &claim_token, &result, at + 1)
                .await
                .unwrap(),
            ReplayOutcomeResult::Committed { seq: 2 }
        ));
    }

    #[tokio::test]
    async fn replay_outcome_requires_claim_and_answers_once() {
        let store = make_store();
        let sid = sample_session_id();
        let tid = uuid::Uuid::new_v4();
        let at = now_ms();
        store
            .append(&sid, 1, &tool_requested(tid, "call-2", at), at)
            .await
            .unwrap();
        let claim = store
            .claim_replay_call(&sid, 1, 0, "call-2", 1, REPLAY_LEASE_TTL_MS)
            .await
            .unwrap();
        let ReplayClaimResult::Claimed { claim_token, .. } = claim else {
            panic!("expected claim")
        };
        let result = SessionEvent::ToolError {
            turn_id: tid,
            call_id: "call-2".into(),
            error: "replay failed".into(),
            at: at + 1,
        };
        assert!(matches!(
            store
                .commit_replay_outcome(&sid, "call-2", "wrong", &result, at + 1)
                .await
                .unwrap(),
            ReplayOutcomeResult::ClaimLost
        ));
        assert!(matches!(
            store
                .commit_replay_outcome(&sid, "call-2", &claim_token, &result, at + 1)
                .await
                .unwrap(),
            ReplayOutcomeResult::Committed { seq: 2 }
        ));
        assert!(matches!(
            store
                .commit_replay_outcome(&sid, "call-2", &claim_token, &result, at + 2)
                .await
                .unwrap(),
            ReplayOutcomeResult::ClaimLost
        ));
        assert_eq!(store.load_all_events(&sid).await.unwrap().len(), 2);
    }

    /// Test-only: force a claim's lease to expire so a later claim may take
    /// the cursor over without waiting out `REPLAY_LEASE_TTL_MS`.
    async fn expire_lease(store: &SqliteEventStore, call_id: &str) {
        let conn = store.conn.lock().await;
        conn.execute(
            "UPDATE session_replay_cursors SET lease_until = 0 WHERE call_id = ?1",
            params![call_id],
        )
        .unwrap();
    }

    #[tokio::test]
    async fn replay_claim_expired_lease_takes_over_and_hits_budget() {
        let store = make_store();
        let sid = sample_session_id();
        let tid = uuid::Uuid::new_v4();
        let at = now_ms();
        store
            .append(&sid, 1, &tool_requested(tid, "call-1", at), at)
            .await
            .unwrap();
        let head = store.load_head_seq(&sid).await.unwrap();

        let first = store
            .claim_replay_call(&sid, head, 0, "call-1", 2, REPLAY_LEASE_TTL_MS)
            .await
            .unwrap();
        let ReplayClaimResult::Claimed { attempt: a1, .. } = first else {
            panic!("expected first claim");
        };
        assert_eq!(a1, 1);

        // A held, unexpired lease refuses a second claim.
        let held = store
            .claim_replay_call(&sid, head, 0, "call-1", 2, REPLAY_LEASE_TTL_MS)
            .await
            .unwrap();
        assert!(matches!(
            held,
            ReplayClaimResult::LeaseHeld { attempt: 1, .. }
        ));

        // Expire the lease; the next claim may take over with attempt 2.
        expire_lease(&store, "call-1").await;
        let second = store
            .claim_replay_call(&sid, head, 0, "call-1", 2, REPLAY_LEASE_TTL_MS)
            .await
            .unwrap();
        assert!(matches!(
            second,
            ReplayClaimResult::Claimed { attempt: 2, .. }
        ));

        // The budget is durable: after attempt 2 the cursor is exhausted.
        expire_lease(&store, "call-1").await;
        let exhausted = store
            .claim_replay_call(&sid, head, 0, "call-1", 2, REPLAY_LEASE_TTL_MS)
            .await
            .unwrap();
        assert!(matches!(
            exhausted,
            ReplayClaimResult::BudgetExhausted {
                attempts: 2,
                max_attempts: 2
            }
        ));
    }

    /// Regression: a lease below the enforced 300_000ms floor is refused
    /// before any cursor write, so it neither creates nor bumps the attempt
    /// cursor. A later valid (floor) claim still starts at attempt 1.
    #[tokio::test]
    async fn replay_claim_below_floor_lease_is_refused_without_cursor_write() {
        let store = make_store();
        let sid = sample_session_id();
        let tid = uuid::Uuid::new_v4();
        let at = now_ms();
        store
            .append(&sid, 1, &tool_requested(tid, "call-1", at), at)
            .await
            .unwrap();
        let head = store.load_head_seq(&sid).await.unwrap();

        // Any lease below the floor — including a merely-positive one — must
        // fail closed with the existing SessionError and touch nothing.
        for below_floor in [1_i64, REPLAY_LEASE_TTL_MS - 1] {
            let err = store
                .claim_replay_call(&sid, head, 0, "call-1", 2, below_floor)
                .await
                .expect_err("below-floor lease must be refused");
            assert!(
                matches!(err, SessionError::Other(_)),
                "unexpected error for lease_ms {below_floor}: {err:?}"
            );
        }

        // No cursor row was written by the refused claims. The session key is
        // stored as JSON (see `session_id_to_string`), so match that encoding.
        {
            let session_key = serde_json::to_string(&sid).unwrap();
            let conn = store.conn.lock().await;
            let rows: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM session_replay_cursors WHERE session_id = ?1",
                    params![session_key],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(rows, 0, "a refused claim must not write a cursor row");
        }

        // A valid floor lease still claims attempt 1: no budget was consumed.
        let valid = store
            .claim_replay_call(&sid, head, 0, "call-1", 2, REPLAY_LEASE_TTL_MS)
            .await
            .unwrap();
        assert!(matches!(
            valid,
            ReplayClaimResult::Claimed { attempt: 1, .. }
        ));
    }

    fn effective_input_marker(
        tid: uuid::Uuid,
        call_id: &str,
        input: serde_json::Value,
        at: i64,
    ) -> SessionEvent {
        SessionEvent::ToolCallEffectiveInput {
            turn_id: tid,
            call_id: call_id.into(),
            input,
            at,
        }
    }

    /// §4.3: the durable post-guardrail input is read back exactly as written,
    /// and a call with no marker reads `None` (unknown, not replay eligible).
    #[tokio::test]
    async fn effective_input_is_durably_readable_and_absent_is_none() {
        let store = make_store();
        let sid = sample_session_id();
        let tid = uuid::Uuid::new_v4();
        let at = now_ms();
        store
            .append(&sid, 1, &tool_requested(tid, "call-1", at), at)
            .await
            .unwrap();
        store
            .append(
                &sid,
                2,
                &effective_input_marker(tid, "call-1", serde_json::json!({ "cmd": "ls" }), at + 1),
                at + 1,
            )
            .await
            .unwrap();

        let got = store
            .load_tool_call_effective_input(&sid, "call-1")
            .await
            .unwrap()
            .expect("exactly one marker is present");
        assert_eq!(got.turn_id, tid);
        assert_eq!(got.input, serde_json::json!({ "cmd": "ls" }));
        assert_eq!(got.at, at + 1);

        // A different call_id has no marker: unknown.
        assert!(store
            .load_tool_call_effective_input(&sid, "call-absent")
            .await
            .unwrap()
            .is_none());
    }

    /// Two markers for one call make the true input ambiguous: fail closed to
    /// `None` — the call must never be replayed from either input.
    #[tokio::test]
    async fn effective_input_fails_closed_on_multiple_markers() {
        let store = make_store();
        let sid = sample_session_id();
        let tid = uuid::Uuid::new_v4();
        let at = now_ms();
        store
            .append(&sid, 1, &tool_requested(tid, "call-1", at), at)
            .await
            .unwrap();
        store
            .append(
                &sid,
                2,
                &effective_input_marker(tid, "call-1", serde_json::json!("a"), at + 1),
                at + 1,
            )
            .await
            .unwrap();
        store
            .append(
                &sid,
                3,
                &effective_input_marker(tid, "call-1", serde_json::json!("b"), at + 2),
                at + 2,
            )
            .await
            .unwrap();

        assert!(store
            .load_tool_call_effective_input(&sid, "call-1")
            .await
            .unwrap()
            .is_none());
    }

    /// The marker survives a process restart: written under one connection and
    /// read back under a fresh one over the same file.
    #[tokio::test]
    async fn effective_input_survives_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sessions.db");
        let sid = sample_session_id();
        let tid = uuid::Uuid::new_v4();
        let at = now_ms();

        {
            let conn = rusqlite::Connection::open(&path).unwrap();
            migrate_add_session_events(&conn).unwrap();
            let store = SqliteEventStore::new(conn);
            store
                .append(&sid, 1, &tool_requested(tid, "call-1", at), at)
                .await
                .unwrap();
            store
                .append(
                    &sid,
                    2,
                    &effective_input_marker(tid, "call-1", serde_json::json!({ "v": 7 }), at + 1),
                    at + 1,
                )
                .await
                .unwrap();
        }

        // New connection over the same file = a fresh store instance.
        let conn = rusqlite::Connection::open(&path).unwrap();
        migrate_add_session_events(&conn).unwrap();
        let reopened = SqliteEventStore::new(conn);
        let got = reopened
            .load_tool_call_effective_input(&sid, "call-1")
            .await
            .unwrap()
            .expect("marker must survive a restart");
        assert_eq!(got.input, serde_json::json!({ "v": 7 }));
    }

    /// A claim that will not start an effect can be released: the lease is
    /// freed and the attempt is NOT consumed, so a later claim starts again at
    /// attempt 1 (no budget burned).
    #[tokio::test]
    async fn release_replay_claim_frees_lease_and_preserves_budget() {
        let store = make_store();
        let sid = sample_session_id();
        let tid = uuid::Uuid::new_v4();
        let at = now_ms();
        store
            .append(&sid, 1, &tool_requested(tid, "call-1", at), at)
            .await
            .unwrap();
        let head = store.load_head_seq(&sid).await.unwrap();

        let first = store
            .claim_replay_call(&sid, head, 0, "call-1", 2, REPLAY_LEASE_TTL_MS)
            .await
            .unwrap();
        let ReplayClaimResult::Claimed {
            attempt,
            claim_token,
        } = first
        else {
            panic!("expected first claim");
        };
        assert_eq!(attempt, 1);

        // While held, the lease refuses a second claim.
        let held = store
            .claim_replay_call(&sid, head, 0, "call-1", 2, REPLAY_LEASE_TTL_MS)
            .await
            .unwrap();
        assert!(matches!(
            held,
            ReplayClaimResult::LeaseHeld { attempt: 1, .. }
        ));

        // Release the claim; the next claim is immediately available at attempt 1.
        assert!(matches!(
            store
                .release_replay_claim(&sid, "call-1", &claim_token)
                .await
                .unwrap(),
            ReplayReleaseResult::Released
        ));
        let again = store
            .claim_replay_call(&sid, head, 0, "call-1", 2, REPLAY_LEASE_TTL_MS)
            .await
            .unwrap();
        assert!(matches!(
            again,
            ReplayClaimResult::Claimed { attempt: 1, .. }
        ));
    }

    /// A stale or empty token never releases a live claim; the cursor stays
    /// active with its lease intact, and the real token stays committable.
    #[tokio::test]
    async fn release_replay_claim_fails_closed_on_token_mismatch() {
        let store = make_store();
        let sid = sample_session_id();
        let tid = uuid::Uuid::new_v4();
        let at = now_ms();
        store
            .append(&sid, 1, &tool_requested(tid, "call-1", at), at)
            .await
            .unwrap();
        let head = store.load_head_seq(&sid).await.unwrap();
        let first = store
            .claim_replay_call(&sid, head, 0, "call-1", 2, REPLAY_LEASE_TTL_MS)
            .await
            .unwrap();
        let ReplayClaimResult::Claimed { claim_token, .. } = first else {
            panic!("expected first claim");
        };

        assert!(matches!(
            store
                .release_replay_claim(&sid, "call-1", "wrong-token")
                .await
                .unwrap(),
            ReplayReleaseResult::ClaimLost
        ));
        assert!(matches!(
            store
                .release_replay_claim(&sid, "call-1", "")
                .await
                .unwrap(),
            ReplayReleaseResult::ClaimLost
        ));

        // The live claim survives the failed releases: still held by the real
        // token, which still commits.
        let held = store
            .claim_replay_call(&sid, head, 0, "call-1", 2, REPLAY_LEASE_TTL_MS)
            .await
            .unwrap();
        assert!(matches!(
            held,
            ReplayClaimResult::LeaseHeld { attempt: 1, .. }
        ));
        let result = SessionEvent::ToolResult {
            turn_id: tid,
            call_id: "call-1".into(),
            output: ToolOutput {
                value: serde_json::json!({ "ok": true }),
                metadata: Default::default(),
            },
            at: at + 1,
        };
        assert!(matches!(
            store
                .commit_replay_outcome(&sid, "call-1", &claim_token, &result, at + 1)
                .await
                .unwrap(),
            ReplayOutcomeResult::Committed { seq: 2 }
        ));
    }

    /// Releasing a released or answered cursor is fail-closed, and a released
    /// claim can never be committed with its (now cleared) token.
    #[tokio::test]
    async fn release_replay_claim_is_once_only_and_cannot_be_committed() {
        let store = make_store();
        let sid = sample_session_id();
        let tid = uuid::Uuid::new_v4();
        let at = now_ms();
        store
            .append(&sid, 1, &tool_requested(tid, "call-1", at), at)
            .await
            .unwrap();
        let head = store.load_head_seq(&sid).await.unwrap();
        let first = store
            .claim_replay_call(&sid, head, 0, "call-1", 2, REPLAY_LEASE_TTL_MS)
            .await
            .unwrap();
        let ReplayClaimResult::Claimed { claim_token, .. } = first else {
            panic!("expected first claim");
        };

        assert!(matches!(
            store
                .release_replay_claim(&sid, "call-1", &claim_token)
                .await
                .unwrap(),
            ReplayReleaseResult::Released
        ));
        // The token was cleared: a second release with the same token is lost.
        assert!(matches!(
            store
                .release_replay_claim(&sid, "call-1", &claim_token)
                .await
                .unwrap(),
            ReplayReleaseResult::ClaimLost
        ));
        // A released claim cannot be committed.
        let result = SessionEvent::ToolResult {
            turn_id: tid,
            call_id: "call-1".into(),
            output: ToolOutput {
                value: serde_json::json!({ "ok": true }),
                metadata: Default::default(),
            },
            at: at + 1,
        };
        assert!(matches!(
            store
                .commit_replay_outcome(&sid, "call-1", &claim_token, &result, at + 1)
                .await
                .unwrap(),
            ReplayOutcomeResult::ClaimLost
        ));
        assert_eq!(store.load_all_events(&sid).await.unwrap().len(), 1);
    }

    /// Releasing a cursor that has no row (never claimed) is fail-closed.
    #[tokio::test]
    async fn release_replay_claim_without_a_cursor_is_claim_lost() {
        let store = make_store();
        let sid = sample_session_id();
        assert!(matches!(
            store
                .release_replay_claim(&sid, "call-1", "token")
                .await
                .unwrap(),
            ReplayReleaseResult::ClaimLost
        ));
    }

    /// Regression: a lease at or above the floor whose `now + lease_ms`
    /// would overflow i64 must fail closed with an existing SessionError and
    /// leave no cursor/attempt row behind — extreme i64 input must not panic
    /// or wrap into a bogus expiry.
    #[tokio::test]
    async fn replay_claim_overflowing_lease_is_refused_without_cursor_write() {
        let store = make_store();
        let sid = sample_session_id();
        let tid = uuid::Uuid::new_v4();
        let at = now_ms();
        store
            .append(&sid, 1, &tool_requested(tid, "call-1", at), at)
            .await
            .unwrap();
        let head = store.load_head_seq(&sid).await.unwrap();

        let err = store
            .claim_replay_call(&sid, head, 0, "call-1", 2, i64::MAX)
            .await
            .expect_err("overflowing lease must be refused");
        assert!(
            matches!(err, SessionError::Storage(_)),
            "unexpected error for overflowing lease: {err:?}"
        );

        // The refused claim wrote no cursor row, so no attempt was consumed.
        {
            let session_key = serde_json::to_string(&sid).unwrap();
            let conn = store.conn.lock().await;
            let rows: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM session_replay_cursors WHERE session_id = ?1",
                    params![session_key],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(rows, 0, "a refused claim must not write a cursor row");
        }

        // A valid floor lease still claims attempt 1: no budget was consumed.
        let valid = store
            .claim_replay_call(&sid, head, 0, "call-1", 2, REPLAY_LEASE_TTL_MS)
            .await
            .unwrap();
        assert!(matches!(
            valid,
            ReplayClaimResult::Claimed { attempt: 1, .. }
        ));
    }

    #[tokio::test]
    async fn replay_commit_preserves_budget_across_rewind() {
        let store = make_store();
        let sid = sample_session_id();
        let tid = uuid::Uuid::new_v4();
        let at = now_ms();
        store
            .append(&sid, 1, &tool_requested(tid, "call-1", at), at)
            .await
            .unwrap();
        let result = SessionEvent::ToolResult {
            turn_id: tid,
            call_id: "call-1".into(),
            output: ToolOutput {
                value: serde_json::json!({"ok": true}),
                metadata: Default::default(),
            },
            at: at + 1,
        };

        // First claim + answer consumes attempt 1.
        let head = store.load_head_seq(&sid).await.unwrap();
        let claim = store
            .claim_replay_call(&sid, head, 0, "call-1", 2, REPLAY_LEASE_TTL_MS)
            .await
            .unwrap();
        let ReplayClaimResult::Claimed {
            claim_token,
            attempt,
        } = claim
        else {
            panic!("expected claim");
        };
        assert_eq!(attempt, 1);
        assert!(matches!(
            store
                .commit_replay_outcome(&sid, "call-1", &claim_token, &result, at + 1)
                .await
                .unwrap(),
            ReplayOutcomeResult::Committed { seq: 2 }
        ));

        // Rewind the answer: the call is dangling again and the generation
        // bumps, but the head (MAX(seq)) is unchanged — the exact case the
        // generation precondition exists to catch.
        assert_eq!(store.retire_from(&sid, 2).await.unwrap(), 1);
        assert_eq!(store.load_retire_generation(&sid).await.unwrap(), 1);

        // Re-claim with the new generation: attempts resume at 2, not 1.
        let head = store.load_head_seq(&sid).await.unwrap();
        let reclaim = store
            .claim_replay_call(&sid, head, 1, "call-1", 2, REPLAY_LEASE_TTL_MS)
            .await
            .unwrap();
        let ReplayClaimResult::Claimed {
            claim_token: token2,
            attempt: attempt2,
        } = reclaim
        else {
            panic!("expected reclaim");
        };
        assert_eq!(attempt2, 2);
        assert!(matches!(
            store
                .commit_replay_outcome(&sid, "call-1", &token2, &result, at + 2)
                .await
                .unwrap(),
            ReplayOutcomeResult::Committed { seq: 3 }
        ));

        // Rewind again and prove the budget survived both answers.
        assert_eq!(store.retire_from(&sid, 3).await.unwrap(), 1);
        assert_eq!(store.load_retire_generation(&sid).await.unwrap(), 2);
        let head = store.load_head_seq(&sid).await.unwrap();
        let exhausted = store
            .claim_replay_call(&sid, head, 2, "call-1", 2, REPLAY_LEASE_TTL_MS)
            .await
            .unwrap();
        assert!(matches!(
            exhausted,
            ReplayClaimResult::BudgetExhausted {
                attempts: 2,
                max_attempts: 2
            }
        ));
    }

    #[tokio::test]
    async fn replay_claim_detects_high_seq_retire_via_generation() {
        let store = make_store();
        let sid = sample_session_id();
        let tid = uuid::Uuid::new_v4();
        let at = now_ms();
        store
            .append(&sid, 1, &tool_requested(tid, "call-1", at), at)
            .await
            .unwrap();

        let head = store.load_head_seq(&sid).await.unwrap();
        let claim = store
            .claim_replay_call(&sid, head, 0, "call-1", 2, REPLAY_LEASE_TTL_MS)
            .await
            .unwrap();
        assert!(matches!(claim, ReplayClaimResult::Claimed { .. }));

        // Retire the dangling call itself. `MAX(seq)` still sees the row, so a
        // head-only claim would wrongly proceed; the generation catches it.
        assert_eq!(store.retire_from(&sid, 1).await.unwrap(), 1);
        assert_eq!(
            store.load_head_seq(&sid).await.unwrap(),
            1,
            "soft delete keeps MAX(seq)"
        );
        assert_eq!(store.load_retire_generation(&sid).await.unwrap(), 1);

        let stale_gen = store
            .claim_replay_call(&sid, head, 0, "call-1", 2, REPLAY_LEASE_TTL_MS)
            .await
            .unwrap();
        assert!(matches!(
            stale_gen,
            ReplayClaimResult::GenerationChanged {
                expected: 0,
                found: 1
            }
        ));
    }

    /// Regression: after a commit answers a claim and the answer is then
    /// retired (making the call dangling again), the cursor is left in the
    /// `answered` state with a cleared (empty) token. A later commit must NOT
    /// sneak through on an empty token — which equals the cleared stored
    /// token — nor on the stale real token (the cursor is `answered`, not
    /// `active`). Both fail closed and append nothing.
    #[tokio::test]
    async fn replay_commit_rejects_empty_token_and_answered_cursor_after_retire() {
        let store = make_store();
        let sid = sample_session_id();
        let tid = uuid::Uuid::new_v4();
        let at = now_ms();
        store
            .append(&sid, 1, &tool_requested(tid, "call-1", at), at)
            .await
            .unwrap();
        let result = SessionEvent::ToolResult {
            turn_id: tid,
            call_id: "call-1".into(),
            output: ToolOutput {
                value: serde_json::json!({"ok": true}),
                metadata: Default::default(),
            },
            at: at + 1,
        };

        let head = store.load_head_seq(&sid).await.unwrap();
        let claim = store
            .claim_replay_call(&sid, head, 0, "call-1", 2, REPLAY_LEASE_TTL_MS)
            .await
            .unwrap();
        let ReplayClaimResult::Claimed { claim_token, .. } = claim else {
            panic!("expected claim")
        };
        assert!(matches!(
            store
                .commit_replay_outcome(&sid, "call-1", &claim_token, &result, at + 1)
                .await
                .unwrap(),
            ReplayOutcomeResult::Committed { seq: 2 }
        ));

        // Retire the answer: the call reads dangling again (request live,
        // outcome retired), but the cursor stays `answered` with a cleared token.
        assert_eq!(store.retire_from(&sid, 2).await.unwrap(), 1);

        // Empty token must not bypass the claim even though the stored token
        // is also empty and the call reads dangling.
        assert!(matches!(
            store
                .commit_replay_outcome(&sid, "call-1", "", &result, at + 2)
                .await
                .unwrap(),
            ReplayOutcomeResult::ClaimLost
        ));

        // A stale real token on an `answered` cursor also fails closed.
        assert!(matches!(
            store
                .commit_replay_outcome(&sid, "call-1", &claim_token, &result, at + 3)
                .await
                .unwrap(),
            ReplayOutcomeResult::ClaimLost
        ));

        // Nothing was appended: only the request survives; the answer stays retired.
        assert_eq!(store.load_all_events(&sid).await.unwrap().len(), 1);
    }

    /// The commit side of the generation precondition: a claim taken under
    /// generation N must not commit once a retire bumps the generation, even
    /// though the call still reads dangling and the cursor is still `active`.
    #[tokio::test]
    async fn replay_commit_rejects_stale_generation_token() {
        let store = make_store();
        let sid = sample_session_id();
        let tid = uuid::Uuid::new_v4();
        let at = now_ms();
        store
            .append(&sid, 1, &tool_requested(tid, "call-1", at), at)
            .await
            .unwrap();
        store
            .append(&sid, 2, &tool_requested(tid, "call-2", at), at)
            .await
            .unwrap();
        let result = SessionEvent::ToolResult {
            turn_id: tid,
            call_id: "call-2".into(),
            output: ToolOutput {
                value: serde_json::json!({"ok": true}),
                metadata: Default::default(),
            },
            at: at + 1,
        };

        let head = store.load_head_seq(&sid).await.unwrap();
        let claim = store
            .claim_replay_call(&sid, head, 0, "call-2", 2, REPLAY_LEASE_TTL_MS)
            .await
            .unwrap();
        let ReplayClaimResult::Claimed { claim_token, .. } = claim else {
            panic!("expected claim")
        };

        // Retire only call-1 (seq 1): call-2 stays live and dangling, but the
        // generation bumps, invalidating the in-flight claim's generation.
        assert!(store.retire_record(&sid, 1).await.unwrap());
        assert_eq!(store.load_retire_generation(&sid).await.unwrap(), 1);

        // The cursor is still active with a matching token, but its stored
        // generation is stale: the commit must fail closed and append nothing.
        assert!(matches!(
            store
                .commit_replay_outcome(&sid, "call-2", &claim_token, &result, at + 1)
                .await
                .unwrap(),
            ReplayOutcomeResult::ClaimLost
        ));

        // No outcome appended: call-2's request remains the last live event.
        assert_eq!(store.load_head_seq(&sid).await.unwrap(), 2);
        assert_eq!(store.load_all_events(&sid).await.unwrap().len(), 1);
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
        let (got_sid, slice) = &markers[0];
        assert_eq!(*got_sid, sid);
        let records = slice.as_ref().unwrap();
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
        let markers = groups[0].1.as_ref().unwrap();
        let seqs: Vec<u64> = markers.iter().map(|r| r.seq).collect();
        assert_eq!(seqs, vec![1, 2]);
        assert!(matches!(
            markers[1].event,
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

    /// The anchor IN-list is `event_type_tag` over the two variants the
    /// positional join reads — derived through the one sampler, so a renamed
    /// tag cannot leave the retired read selecting nothing.
    #[test]
    fn retired_anchor_event_types_are_the_two_anchor_kinds_tags() {
        let all: Vec<SessionEvent> = crate::session::events::fixtures::sample_of_every_kind()
            .into_iter()
            .map(|(_, e)| e)
            .collect();
        let tag_of = |pick: fn(&SessionEvent) -> bool| -> &'static str {
            let tags: std::collections::BTreeSet<&str> = all
                .iter()
                .filter(|e| pick(e))
                .map(|e| event_type_tag(e))
                .collect();
            assert_eq!(tags.len(), 1, "one variant, one tag: {tags:?}");
            tags.into_iter().next().unwrap()
        };
        let derived = [
            (
                tag_of(|e| matches!(e, SessionEvent::RunStarted { .. })),
                RetiredAnchorKind::RunStarted,
            ),
            (
                tag_of(|e| matches!(e, SessionEvent::AssistantRunMeta { .. })),
                RetiredAnchorKind::RunMeta,
            ),
        ];
        assert_eq!(derived, RUN_ANCHOR_EVENT_TYPES);
    }

    /// The retired read sees ONLY retired rows, only the two anchor kinds, by
    /// kind and position — and a live meta stays invisible to it. This is the
    /// store half of the rewind re-bill pin
    /// (`session_projector::tests::a_rewind_inside_a_billed_run_does_not_bill_it_again`).
    #[tokio::test]
    async fn retired_run_anchors_are_the_retired_openers_and_metas_only() {
        let store = make_store();
        let sid = sample_session_id();
        let tid = uuid::Uuid::new_v4();
        let at = now_ms();
        let meta = |run: &str| SessionEvent::AssistantRunMeta {
            turn_id: tid,
            run_id: run.into(),
            context_tokens: None,
            context_window: None,
            total_tokens: None,
            cost_usd: None,
            model: None,
            model_provider: None,
            at,
        };
        let log = [
            run_started("a", at),
            turn_started(tid, at),
            run_finished("a", at),
            meta("a"),
            run_started("b", at),
            run_finished("b", at),
            meta("b"),
        ];
        for (i, e) in log.iter().enumerate() {
            store.append(&sid, i as EventSeq + 1, e, at).await.unwrap();
        }
        assert!(
            store
                .load_retired_run_anchors(&sid)
                .await
                .unwrap()
                .is_empty(),
            "nothing retired, nothing to see — a live meta is not an anchor here"
        );
        // A rewind that cuts inside run b: rows 6.. retired, the closer
        // appended in the same batch.
        store
            .append_batch(
                &sid,
                8,
                &[(run_finished("b", at), at)],
                Some(Retire::From(6)),
                Durability::Normal,
            )
            .await
            .unwrap();
        assert_eq!(
            store.load_retired_run_anchors(&sid).await.unwrap(),
            vec![RetiredRunAnchor {
                seq: 7,
                kind: RetiredAnchorKind::RunMeta,
            }],
            "run b's retired meta, not its live opener, not the retired closer"
        );
        store.retire_from(&sid, 1).await.unwrap();
        let kinds: Vec<(EventSeq, RetiredAnchorKind)> = store
            .load_retired_run_anchors(&sid)
            .await
            .unwrap()
            .into_iter()
            .map(|a| (a.seq, a.kind))
            .collect();
        assert_eq!(
            kinds,
            vec![
                (1, RetiredAnchorKind::RunStarted),
                (4, RetiredAnchorKind::RunMeta),
                (5, RetiredAnchorKind::RunStarted),
                (7, RetiredAnchorKind::RunMeta),
            ],
            "everything retired: both openers and both metas, in seq order, no turn / finish rows"
        );
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
    /// head-side bound is driven here. `live` is the count the caller expects
    /// to retire; a mismatch is the rollback pinned below, so every caller of
    /// this helper states the count it read.
    async fn retire_through_batch(
        store: &SqliteEventStore,
        sid: &SessionId,
        through: EventSeq,
        live: usize,
    ) {
        let next = store.load_head_seq(sid).await.unwrap() + 1;
        store
            .append_batch(
                sid,
                next,
                &[],
                Some(Retire::Through { through, live }),
                Durability::Normal,
            )
            .await
            .unwrap();
    }

    /// F25: a head-side retire that finds fewer live rows than its caller
    /// read — here because a tail-side clear landed in between — rolls the
    /// WHOLE batch back. The row it would have appended (the summary, in
    /// production) is not in the log, and the rows the clear left live are
    /// still live. Asserted as effects on the log (criterion #4).
    #[tokio::test]
    async fn retire_through_rolls_back_when_the_span_shrank_under_it() {
        let store = make_store();
        let sid = sample_session_id();
        let tid = uuid::Uuid::new_v4();
        let at = now_ms();
        for (seq, text) in [(1u64, "one"), (2, "two"), (3, "three")] {
            store
                .append(&sid, seq, &user_message(tid, text, at), at)
                .await
                .unwrap();
        }
        // The caller read seqs 1..=2 as the span; then a clear erased from 2.
        store.retire_from(&sid, 2).await.unwrap();

        let err = store
            .append_batch(
                &sid,
                4,
                &[(user_message(tid, "summary of one and two", at), at)],
                Some(Retire::Through {
                    through: 2,
                    live: 2,
                }),
                Durability::Normal,
            )
            .await
            .expect_err("a span that shrank must not be retired");
        assert!(
            matches!(
                err,
                SessionError::RetireSpanChanged {
                    expected: 2,
                    found: 1
                }
            ),
            "{err:?}"
        );

        let live = store.load_all_events(&sid).await.unwrap();
        assert_eq!(
            live.iter().map(|r| r.seq).collect::<Vec<_>>(),
            vec![1],
            "nothing appended, and seq 1 was not retired by the rolled-back batch"
        );
        assert_eq!(store.load_head_seq(&sid).await.unwrap(), 3);
    }

    /// The control for the rollback above: a tail-side clear PAST the span
    /// leaves the count intact, so the head-side retire commits.
    #[tokio::test]
    async fn retire_through_commits_when_only_the_tail_past_it_changed() {
        let store = make_store();
        let sid = sample_session_id();
        let tid = uuid::Uuid::new_v4();
        let at = now_ms();
        for (seq, text) in [(1u64, "one"), (2, "two"), (3, "three")] {
            store
                .append(&sid, seq, &user_message(tid, text, at), at)
                .await
                .unwrap();
        }
        store.retire_from(&sid, 3).await.unwrap();

        store
            .append_batch(
                &sid,
                4,
                &[(user_message(tid, "summary", at), at)],
                Some(Retire::Through {
                    through: 2,
                    live: 2,
                }),
                Durability::Normal,
            )
            .await
            .expect("the summarized span is intact");
        let live = store.load_all_events(&sid).await.unwrap();
        assert_eq!(live.iter().map(|r| r.seq).collect::<Vec<_>>(), vec![4]);
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

        retire_through_batch(&store, &sid, 2, 2).await;

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
        // Zero live rows remain at or below the bound, so zero is the count.
        retire_through_batch(&store, &sid, 2, 0).await;
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

        retire_through_batch(&store, &sid, 1, 1).await;
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

        retire_through_batch(&store, &sid_b, 1, 1).await;

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

    /// A `session_replay_cursors` table written before `state`/`lease_until`
    /// existed must migrate in place: legacy rows read as an already-expired
    /// active claim, so a new claim takes them over (attempt+1) rather than
    /// wedging the call forever.
    #[test]
    fn migrates_pre_existing_replay_cursor_table() {
        let conn = Connection::open_in_memory().unwrap();
        // The pre-lease cursor schema, verbatim.
        conn.execute_batch(
            "CREATE TABLE session_replay_cursors (
                 session_id   TEXT NOT NULL,
                 call_id      TEXT NOT NULL,
                 attempts     INTEGER NOT NULL,
                 claim_token  TEXT NOT NULL,
                 PRIMARY KEY (session_id, call_id)
             );
             INSERT INTO session_replay_cursors
                 (session_id, call_id, attempts, claim_token)
                 VALUES ('s1', 'call-1', 1, 'stale-token');",
        )
        .unwrap();

        migrate_add_session_events(&conn).unwrap();
        // Idempotent on an already-migrated DB.
        migrate_add_session_events(&conn).unwrap();

        // The new columns exist with the recoverable defaults.
        let state: String = conn
            .query_row(
                "SELECT state FROM session_replay_cursors WHERE call_id = 'call-1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(state, "active");
        let lease: i64 = conn
            .query_row(
                "SELECT lease_until FROM session_replay_cursors WHERE call_id = 'call-1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(lease, 0, "legacy lease reads as already expired");
        let claim_gen: i64 = conn
            .query_row(
                "SELECT claim_generation FROM session_replay_cursors WHERE call_id = 'call-1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(claim_gen, 0, "legacy claim_generation defaults to 0");
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
            payload: encode_row(&run_started("r", at)).unwrap(),
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

    // -----------------------------------------------------------------------
    // Per-row decode: the envelope, the three row verdicts, and the isolation
    // of one bad row to the session that holds it
    // -----------------------------------------------------------------------

    /// A row exactly as another build would have written it — see
    /// [`SqliteEventStore::insert_raw_row_for_test`].
    async fn raw_insert(
        store: &SqliteEventStore,
        sid: &SessionId,
        seq: i64,
        event_type: &str,
        json: &str,
    ) {
        store
            .insert_raw_row_for_test(sid, seq, event_type, json)
            .await;
    }

    #[test]
    fn every_row_carries_the_schema_version_and_omits_ignorable_when_false() {
        let json = encode_row(&user_message(uuid::Uuid::new_v4(), "hi", 1)).unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["v"], SESSION_EVENT_SCHEMA_VERSION);
        assert!(
            v.get("ignorable").is_none(),
            "absent, never `false`: {json}"
        );
        assert!(matches!(decode_row(1, 1, &json), DecodedRow::Event(_)));
    }

    #[test]
    fn tool_call_requested_identity_survives_encode_decode_row() {
        use crate::tools::descriptor::{ReplayPolicy, ToolCallIdentity};

        let event = SessionEvent::ToolCallRequested {
            turn_id: uuid::Uuid::new_v4(),
            call_id: "c".into(),
            name: "bash".into(),
            input: serde_json::json!({ "cmd": "ls" }),
            identity: Some(ToolCallIdentity {
                schema_version: 1,
                revision: 3,
                replay_policy: ReplayPolicy::Safe,
                replay_contract_fingerprint: None,
            }),
            at: 1,
        };
        let json = encode_row(&event).unwrap();
        let DecodedRow::Event(record) = decode_row(1, 1, &json) else {
            panic!("identity-bearing row must decode: {json}");
        };
        let SessionEvent::ToolCallRequested { identity, .. } = record.event else {
            panic!("expected ToolCallRequested");
        };
        assert_eq!(
            identity,
            Some(ToolCallIdentity {
                schema_version: 1,
                revision: 3,
                replay_policy: ReplayPolicy::Safe,
                replay_contract_fingerprint: None,
            })
        );

        let legacy = r#"{"type":"tool_call_requested","turn_id":"00000000-0000-0000-0000-000000000000","call_id":"c","name":"bash","input":{},"at":1,"v":1}"#;
        let DecodedRow::Event(record) = decode_row(2, 1, legacy) else {
            panic!("legacy row must decode");
        };
        let SessionEvent::ToolCallRequested { identity, .. } = record.event else {
            panic!("expected ToolCallRequested");
        };
        assert_eq!(identity, None);
    }

    #[test]
    fn an_unknown_variant_is_undecodable_unless_the_row_says_ignorable() {
        assert!(matches!(
            decode_row(7, 1, r#"{"type":"from_the_future","v":9}"#),
            DecodedRow::Undecodable(UndecodableRecord { seq: 7, kind_tag: Some(t), .. }) if t == "from_the_future"
        ));
        assert!(matches!(
            decode_row(8, 1, r#"{"type":"from_the_future","ignorable":true}"#),
            DecodedRow::Skipped { seq: 8, kind_tag } if kind_tag == "from_the_future"
        ));
        // A missing key reads as false; a KNOWN variant that will not parse is
        // corruption, not skippable.
        assert!(matches!(
            decode_row(9, 1, r#"{"type":"from_the_future","ignorable":"yes"}"#),
            DecodedRow::Undecodable(_)
        ));
        assert!(matches!(
            decode_row(10, 1, r#"{"type":"tool_call_requested","ignorable":true}"#),
            DecodedRow::Undecodable(_)
        ));
        assert!(matches!(
            decode_row(11, 1, "{not json"),
            DecodedRow::Undecodable(UndecodableRecord { kind_tag: None, .. })
        ));
        // A KNOWN type whose body holds an unknown word of an INNER enum:
        // serde says `unknown variant` for that too, but the row's own tag is
        // not the unknown one — corruption, never skippable.
        assert!(matches!(
            decode_row(
                12,
                1,
                r#"{"type":"run_finished","run_id":"r","outcome":"from_the_future","at":1,"ignorable":true}"#
            ),
            DecodedRow::Undecodable(UndecodableRecord { seq: 12, kind_tag: Some(t), .. }) if t == "run_finished"
        ));
    }

    /// `decode_row` tells "a `type` this build does not know" from "a `type` it
    /// knows whose body will not parse" by serde's rendered wording. That
    /// wording is serde's, not ours, so the shape the guard keys on —
    /// `unknown variant `<the row's own tag>`` — is pinned against REAL
    /// errors: a serde release that rewords it turns this red, instead of
    /// silently turning every row a newer build marked ignorable into
    /// corruption. The inner-enum case is the one the prefix alone cannot
    /// tell apart: it carries the same prefix with a different name after it.
    #[test]
    fn the_unknown_variant_guard_keys_on_serdes_own_wording() {
        let unknown =
            serde_json::from_str::<SessionEvent>(r#"{"type":"from_the_future"}"#).unwrap_err();
        assert!(
            names_unknown_outer_variant(&unknown.to_string(), "from_the_future"),
            "serde no longer says `{UNKNOWN_VARIANT_PREFIX} `<tag>``: {unknown}"
        );
        let known =
            serde_json::from_str::<SessionEvent>(r#"{"type":"tool_call_requested"}"#).unwrap_err();
        assert!(
            !known.to_string().starts_with(UNKNOWN_VARIANT_PREFIX),
            "a known variant with a broken body must not read as unknown: {known}"
        );
        let inner = serde_json::from_str::<SessionEvent>(
            r#"{"type":"run_finished","run_id":"r","outcome":"from_the_future","at":1}"#,
        )
        .unwrap_err();
        assert!(
            inner.to_string().starts_with(UNKNOWN_VARIANT_PREFIX),
            "the premise: an inner unknown word renders the same prefix: {inner}"
        );
        assert!(
            !names_unknown_outer_variant(&inner.to_string(), "run_finished"),
            "the guard must not read the inner word as the row's own tag: {inner}"
        );
    }

    #[tokio::test]
    async fn one_bad_row_refuses_only_its_own_session() {
        let store = make_store();
        let (a, b) = (SessionKey::main("a"), SessionKey::main("b"));
        for sid in [&a, &b] {
            store.append(sid, 1, &run_started("r", 1), 1).await.unwrap();
        }
        raw_insert(
            &store,
            &a,
            2,
            "from_the_future",
            r#"{"type":"from_the_future"}"#,
        )
        .await;
        let err = store.load_all_events(&a).await.unwrap_err();
        assert!(
            matches!(
                err,
                SessionError::UndecodableRecord(UndecodableRecord { seq: 2, .. })
            ),
            "{err}"
        );
        assert_eq!(store.load_all_events(&b).await.unwrap().len(), 1);
        // The marker scan: a bad MARKER row refuses only that session's slice.
        raw_insert(
            &store,
            &a,
            3,
            "run_finished",
            r#"{"type":"run_finished","outcome":"???"}"#,
        )
        .await;
        let groups = store.load_run_markers().await.unwrap();
        assert!(matches!(
            &groups.iter().find(|(s, _)| *s == a).unwrap().1,
            Err(u) if u.seq == 3
        ));
        assert!(matches!(
            &groups.iter().find(|(s, _)| *s == b).unwrap().1,
            Ok(m) if m.len() == 1
        ));
        // `retire_record` is the single-record exit doctor --fix takes.
        assert!(store.retire_record(&a, 2).await.unwrap());
        assert!(!store.retire_record(&a, 2).await.unwrap(), "idempotent");
    }

    #[tokio::test]
    async fn an_ignorable_row_is_skipped_by_readers_and_counted_by_load_rows() {
        let store = make_store();
        let sid = sample_session_id();
        store
            .append(&sid, 1, &run_started("r", 1), 1)
            .await
            .unwrap();
        raw_insert(
            &store,
            &sid,
            2,
            "from_the_future",
            r#"{"type":"from_the_future","ignorable":true}"#,
        )
        .await;
        assert_eq!(store.load_all_events(&sid).await.unwrap().len(), 1);
        let rows = store.load_rows(&sid).await.unwrap();
        assert_eq!(
            rows.iter()
                .filter(|r| matches!(r, DecodedRow::Skipped { .. }))
                .count(),
            1
        );
    }

    /// The schema evolves by adding columns and never by gating on a version
    /// number: a gate that refuses to open an older or newer file is fail-dead
    /// for every session at once, while an added column reads as NULL on the
    /// rows written before it (spec 7.4).
    #[test]
    fn the_event_table_migration_only_ever_adds_columns() {
        let src = crate::utils::source_scan::production_code_lines(include_str!("store.rs"));
        let alters: Vec<&str> = src
            .lines()
            .filter(|l| l.contains("ALTER TABLE session_events"))
            .collect();
        assert!(
            !alters.is_empty() && alters.iter().all(|l| l.contains("ADD COLUMN")),
            "{alters:?}"
        );
        assert!(
            !src.contains("user_version"),
            "a schema gate is fail-dead (spec 7.4)"
        );
    }

    /// [`SESSION_EVENT_STORE_READERS`]'s first column equals the set of files
    /// whose production code reads the handle — equality, both directions,
    /// derived from the tree by the walk the table's doc names. This file is
    /// in the set on its own merits (`retire_live_events`), not by exclusion.
    /// Mutation (T17): comment out one row ⇒ red naming that file.
    #[test]
    fn the_event_store_reader_census_matches_the_tree() {
        use crate::utils::source_scan::files_whose_production_code_reads;
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let found = files_whose_production_code_reads(&root, "global_session_event_store");
        let listed: std::collections::BTreeSet<String> = SESSION_EVENT_STORE_READERS
            .iter()
            .map(|(file, _)| (*file).to_string())
            .collect();
        assert_eq!(
            SESSION_EVENT_STORE_READERS.len(),
            listed.len(),
            "a file is listed twice in SESSION_EVENT_STORE_READERS"
        );
        assert!(
            !found.is_empty(),
            "self-protection: the walk found no reader at all — blind, not clean"
        );
        assert_eq!(
            found, listed,
            "a reader of the session-event-store handle appeared or vanished; update \
             SESSION_EVENT_STORE_READERS with what a missing handle reads as there"
        );
    }
}
