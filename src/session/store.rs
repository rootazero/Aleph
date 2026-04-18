//! SessionEventStore trait and SQLite schema for the session event log.
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

use std::sync::Arc;

use async_trait::async_trait;
use rusqlite::{params, Connection, OptionalExtension};
use tokio::sync::Mutex;

use crate::error::AlephError;
use crate::session::events::{EventSeq, SessionEvent, SessionEventRecord};
use crate::session::service::{SessionError, SessionId};

#[async_trait]
pub trait SessionEventStore: Send + Sync + 'static {
    /// Append a single event at the given seq. Fails if (session_id, seq) already exists.
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

    /// Load events with seq in [from..=to]. Either bound may be None.
    async fn load_events_range(
        &self,
        session_id: &SessionId,
        from: Option<EventSeq>,
        to: Option<EventSeq>,
    ) -> Result<Vec<SessionEventRecord>, SessionError>;

    /// Return the highest seq stored for this session, or 0 if none.
    async fn load_head_seq(&self, session_id: &SessionId) -> Result<EventSeq, SessionError>;
}

/// Create the `session_events` table and its indexes if missing.
///
/// Idempotent — safe to call on every DB open. Uses a savepoint so partial
/// failure leaves the database untouched. Mirrors the pattern used by the
/// other `migrate_add_*` functions in `src/resilience/database/migration.rs`.
pub fn migrate_add_session_events(conn: &Connection) -> Result<(), AlephError> {
    conn.execute_batch("SAVEPOINT migration_session_events")
        .map_err(|e| {
            AlephError::config(format!("Failed to begin session_events migration: {}", e))
        })?;

    let result = conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS session_events (
            session_id   TEXT    NOT NULL,
            seq          INTEGER NOT NULL,
            turn_id      TEXT,
            event_type   TEXT    NOT NULL,
            payload_json TEXT    NOT NULL,
            created_at   INTEGER NOT NULL,
            PRIMARY KEY (session_id, seq)
        );

        CREATE INDEX IF NOT EXISTS idx_session_events_session_turn
            ON session_events(session_id, turn_id);

        CREATE INDEX IF NOT EXISTS idx_session_events_session_type
            ON session_events(session_id, event_type);
        "#,
    );

    if let Err(e) = result {
        let _ = conn.execute_batch("ROLLBACK TO migration_session_events");
        return Err(AlephError::config(format!(
            "Failed to create session_events table: {}",
            e
        )));
    }

    conn.execute_batch("RELEASE migration_session_events").map_err(|e| {
        AlephError::config(format!(
            "Failed to commit session_events migration: {}",
            e
        ))
    })?;

    Ok(())
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
    /// [`migrate_add_session_events`] on the connection before use.
    pub fn new(conn: Connection) -> Self {
        Self {
            conn: Arc::new(Mutex::new(conn)),
        }
    }

    /// Wrap an existing shared connection — for composing with other stores
    /// that already share a `Connection`.
    pub fn with_shared(conn: Arc<Mutex<Connection>>) -> Self {
        Self { conn }
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
        let session_key = session_id_to_string(session_id);
        let turn_id = extract_turn_id(event).map(|u| u.to_string());
        let event_type = event_type_tag(event);
        let seq_i64 = seq as i64;

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
        let session_key = session_id_to_string(session_id);
        let from_val = from.unwrap_or(0) as i64;
        let to_val = to
            .map(|v| v as i64)
            .unwrap_or(i64::MAX);

        let conn = self.conn.lock().await;
        let mut stmt = conn
            .prepare(
                "SELECT seq, payload_json, created_at
                 FROM session_events
                 WHERE session_id = ?1 AND seq >= ?2 AND seq <= ?3
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
            out.push(SessionEventRecord {
                seq: seq as EventSeq,
                event,
                created_at_ms: created_at,
            });
        }
        Ok(out)
    }

    async fn load_head_seq(&self, session_id: &SessionId) -> Result<EventSeq, SessionError> {
        let session_key = session_id_to_string(session_id);

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

        Ok(max_seq.map(|v| v as EventSeq).unwrap_or(0))
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
fn session_id_to_string(id: &SessionId) -> String {
    serde_json::to_string(id).unwrap_or_default()
}

/// Extract the `turn_id` from any `SessionEvent` variant that carries one,
/// so it can be indexed for per-turn replay/trim.
fn extract_turn_id(event: &SessionEvent) -> Option<uuid::Uuid> {
    match event {
        SessionEvent::TurnStarted { turn_id, .. }
        | SessionEvent::TurnEnded { turn_id, .. }
        | SessionEvent::UserMessage { turn_id, .. }
        | SessionEvent::AssistantMessage { turn_id, .. }
        | SessionEvent::SystemMessage { turn_id, .. }
        | SessionEvent::LlmCallStarted { turn_id, .. }
        | SessionEvent::LlmCallEnded { turn_id, .. }
        | SessionEvent::ToolCallRequested { turn_id, .. }
        | SessionEvent::ToolCallApproved { turn_id, .. }
        | SessionEvent::ToolCallDenied { turn_id, .. }
        | SessionEvent::ToolResult { turn_id, .. }
        | SessionEvent::ToolError { turn_id, .. }
        | SessionEvent::SubagentSpawned { turn_id, .. }
        | SessionEvent::SubagentReturned { turn_id, .. }
        | SessionEvent::BudgetUpdated { turn_id, .. } => Some(*turn_id),
        SessionEvent::Error { turn_id, .. } => *turn_id,
        SessionEvent::SessionCreated { .. }
        | SessionEvent::SessionWoken { .. }
        | SessionEvent::SessionDetached { .. }
        | SessionEvent::CompactionPerformed { .. } => None,
    }
}

/// Static discriminant string for the `event_type` column.
///
/// Kept as a `&'static str` to avoid per-append allocation and to give the
/// storage layer a stable taxonomy independent of serde rename decisions.
fn event_type_tag(event: &SessionEvent) -> &'static str {
    match event {
        SessionEvent::SessionCreated { .. } => "session_created",
        SessionEvent::SessionWoken { .. } => "session_woken",
        SessionEvent::SessionDetached { .. } => "session_detached",
        SessionEvent::TurnStarted { .. } => "turn_started",
        SessionEvent::TurnEnded { .. } => "turn_ended",
        SessionEvent::UserMessage { .. } => "user_message",
        SessionEvent::AssistantMessage { .. } => "assistant_message",
        SessionEvent::SystemMessage { .. } => "system_message",
        SessionEvent::LlmCallStarted { .. } => "llm_call_started",
        SessionEvent::LlmCallEnded { .. } => "llm_call_ended",
        SessionEvent::ToolCallRequested { .. } => "tool_call_requested",
        SessionEvent::ToolCallApproved { .. } => "tool_call_approved",
        SessionEvent::ToolCallDenied { .. } => "tool_call_denied",
        SessionEvent::ToolResult { .. } => "tool_result",
        SessionEvent::ToolError { .. } => "tool_error",
        SessionEvent::SubagentSpawned { .. } => "subagent_spawned",
        SessionEvent::SubagentReturned { .. } => "subagent_returned",
        SessionEvent::BudgetUpdated { .. } => "budget_updated",
        SessionEvent::CompactionPerformed { .. } => "compaction_performed",
        SessionEvent::Error { .. } => "error",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrate_creates_session_events_table_and_indexes() {
        let conn = Connection::open_in_memory().unwrap();
        migrate_add_session_events(&conn).unwrap();

        // Table exists with expected columns in the expected order.
        let columns: Vec<String> = {
            let mut stmt = conn
                .prepare(
                    "SELECT name FROM pragma_table_info('session_events') ORDER BY cid ASC",
                )
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
            },
            at: at + 1,
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
}
