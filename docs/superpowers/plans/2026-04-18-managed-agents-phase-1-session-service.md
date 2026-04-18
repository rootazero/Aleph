# Managed-Agents Phase 1 — Session Service (Actor) — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Introduce `src/session/` with an event-sourced `SessionService` trait + `InProcessActorSessionService` (tokio actor per session, SQLite-backed append-only log), wire `agent_loop/**` onto it, and keep Gateway `session.*` RPC unchanged (still on `SessionManager`).

**Architecture:** One tokio task per session, synchronous SQLite writes for durability, full event replay on `wake(session_id)`. New `session_events` table alongside existing `messages` column; dual-write shim keeps both in sync during migration. Strangler-fig pattern: every task is independently shippable.

**Tech Stack:** Rust 2024, tokio (mpsc + broadcast + oneshot channels + select), sqlx or rusqlite (follow existing project convention), async_trait, serde, tracing. Uses existing `SessionKey` from `src/routing/session_key.rs` and existing SQLite backend from `src/gateway/session_store/`.

**Source spec:** `docs/superpowers/specs/2026-04-18-session-service-actor-design.md` §10 steps 5.1–5.7.

---

## Pre-flight

- [ ] **Pre-1: Create Phase 1 worktree**

Run (from the main repo root, NOT from inside another worktree):
```bash
cd /Volumes/TBU4/Workspace/Aleph
git worktree add -b feat/managed-agents-phase-1 ../Aleph-phase-1 main
cd ../Aleph-phase-1
git status
```
Expected: `On branch feat/managed-agents-phase-1`, clean tree. All subsequent work happens in `../Aleph-phase-1`.

- [ ] **Pre-2: Baseline snapshot**

Run:
```bash
echo "=== Phase 1 baseline ===" > /tmp/phase1-baseline.txt
grep -rcE '\bSessionManager\b' src/ | grep -v ':0$' >> /tmp/phase1-baseline.txt
echo "=== Current agent_loop SessionManager imports ===" >> /tmp/phase1-baseline.txt
grep -rn 'use.*SessionManager' src/agent_loop/ >> /tmp/phase1-baseline.txt
cat /tmp/phase1-baseline.txt
```
Records pre-migration counts; Task 12 diffs against this.

- [ ] **Pre-3: Baseline build green**

Run: `cargo check -p alephcore 2>&1 | tail -5`
Expected: `Finished dev ...` with no errors.

Run: `cargo test -p alephcore --lib 2>&1 | tail -5`
Expected: `test result:` line — record the "passed / failed" numbers. Phase 0 merge inherited 2 pre-existing failures (`telegram::config::tests::parse_v2_config_directly`, `memory::notes::ingest::prompts::tests::base_prompt_snapshot`). Phase 1 must not introduce new failures beyond these 2.

---

## Task 1: Module scaffold + `SessionEvent` schema

**Files:**
- Create: `src/session/mod.rs`
- Create: `src/session/events.rs`
- Modify: `src/lib.rs` (add `pub mod session;`)

**Context:** This task creates the module skeleton and the `SessionEvent` enum + its support types. No behavior yet — just types. Cargo check must pass.

- [ ] **Step 1.1: Create `src/session/mod.rs`**

```rust
//! Session Service — append-only event log per session with in-process actor.
//!
//! Phase 1 of the managed-agents refactor. Consumers (primarily `agent_loop`)
//! interact with sessions exclusively through the `SessionService` trait;
//! the underlying `InProcessActorSessionService` spawns one tokio task per
//! session and persists events synchronously to SQLite.
//!
//! See `docs/superpowers/specs/2026-04-18-session-service-actor-design.md`.

pub mod events;
pub mod service;
pub mod state;
pub mod store;
pub mod actor;
pub mod in_process;
pub mod shim;

pub use events::{
    ApprovalSource, ErrorKind, EventSeq, MessageContent, SessionEvent,
    SessionEventRecord, Timestamp, ToolOutput, TurnId, TurnOutcome, TurnTrigger,
};
pub use service::{SessionError, SessionHandle, SessionId, SessionService};
pub use state::SessionState;
pub use store::SessionEventStore;
pub use actor::{ActorCommand, SessionActor};
pub use in_process::InProcessActorSessionService;
```

- [ ] **Step 1.2: Create `src/session/events.rs`**

```rust
//! Event types for the session log.

use crate::gateway::session_manager::SessionIdentityMeta;
use serde::{Deserialize, Serialize};

pub type Timestamp = i64; // unix milliseconds
pub type EventSeq = u64;
pub type TurnId = uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TurnTrigger {
    UserMessage,
    SubagentRequest,
    Scheduled,
    Wake,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TurnOutcome {
    Completed,
    Cancelled,
    Errored { kind: ErrorKind },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalSource {
    User,
    Trusted,
    Autoconfirm,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ErrorKind {
    Llm,
    Tool,
    Sandbox,
    Harness,
    Serialization,
    Other,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MessageContent {
    /// Free-form text body (UI-displayable).
    pub text: String,
    /// Optional rich blocks (images, tool_use). Uses JSON to avoid pulling in
    /// provider-specific types at this layer.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub blocks: Vec<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolOutput {
    pub value: serde_json::Value,
    #[serde(default)]
    pub metadata: ToolOutputMetadata,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ToolOutputMetadata {
    #[serde(default)]
    pub latency_ms: u64,
    #[serde(default)]
    pub truncated: bool,
    #[serde(default)]
    pub cost_cents: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
#[non_exhaustive]
pub enum SessionEvent {
    SessionCreated { identity: SessionIdentityMeta, at: Timestamp },
    SessionWoken { at: Timestamp, prior_head: EventSeq },
    SessionDetached { at: Timestamp },

    TurnStarted { turn_id: TurnId, trigger: TurnTrigger, at: Timestamp },
    TurnEnded { turn_id: TurnId, outcome: TurnOutcome, at: Timestamp },

    UserMessage { turn_id: TurnId, content: MessageContent, at: Timestamp },
    AssistantMessage { turn_id: TurnId, content: MessageContent, at: Timestamp },
    SystemMessage { turn_id: TurnId, content: String, at: Timestamp },

    LlmCallStarted { turn_id: TurnId, provider: String, model: String, at: Timestamp },
    LlmCallEnded {
        turn_id: TurnId,
        tokens_in: u32,
        tokens_out: u32,
        finish_reason: String,
        at: Timestamp,
    },

    ToolCallRequested {
        turn_id: TurnId,
        call_id: String,
        name: String,
        input: serde_json::Value,
        at: Timestamp,
    },
    ToolCallApproved { turn_id: TurnId, call_id: String, by: ApprovalSource, at: Timestamp },
    ToolCallDenied { turn_id: TurnId, call_id: String, reason: String, at: Timestamp },
    ToolResult { turn_id: TurnId, call_id: String, output: ToolOutput, at: Timestamp },
    ToolError { turn_id: TurnId, call_id: String, error: String, at: Timestamp },

    SubagentSpawned {
        turn_id: TurnId,
        child_id: crate::routing::session_key::SessionKey,
        flow: String,
        at: Timestamp,
    },
    SubagentReturned {
        turn_id: TurnId,
        child_id: crate::routing::session_key::SessionKey,
        summary: String,
        at: Timestamp,
    },

    BudgetUpdated {
        turn_id: TurnId,
        tokens_used: u32,
        tokens_budget: u32,
        at: Timestamp,
    },
    CompactionPerformed {
        from_seq: EventSeq,
        to_seq: EventSeq,
        summary_ref: String,
        at: Timestamp,
    },

    Error {
        turn_id: Option<TurnId>,
        kind: ErrorKind,
        message: String,
        recoverable: bool,
        at: Timestamp,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SessionEventRecord {
    pub seq: EventSeq,
    pub event: SessionEvent,
    pub created_at_ms: Timestamp,
}

/// Current wall-clock in unix ms.
pub fn now_ms() -> Timestamp {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
```

- [ ] **Step 1.3: Create empty stub files for remaining modules**

Each of `src/session/{service,state,store,actor,in_process,shim}.rs` gets a minimal stub so `mod.rs` compiles:

```rust
// src/session/service.rs
//! SessionService trait — public facade over the session event log.
```

```rust
// src/session/state.rs
//! SessionState — in-memory reducer over SessionEvent.
```

```rust
// src/session/store.rs
//! SessionEventStore trait — persistence seam (SQLite in Phase 1).
```

```rust
// src/session/actor.rs
//! SessionActor — one tokio task per session.
```

```rust
// src/session/in_process.rs
//! InProcessActorSessionService — the default SessionService implementation.
```

```rust
// src/session/shim.rs
//! Dual-write helper during Phase 1 migration.
```

- [ ] **Step 1.4: Register the module in `src/lib.rs`**

Find the existing `pub mod` declarations and add (alphabetical order if the file uses it, otherwise at the end of the grouping):

```rust
pub mod session;
```

- [ ] **Step 1.5: Build**

Run: `cargo check -p alephcore 2>&1 | tail -5`
Expected: `Finished dev ...` with no errors. Unused-import warnings on the new stubs are acceptable at this stage.

- [ ] **Step 1.6: Commit**

```bash
git add src/session/ src/lib.rs
git commit -m "session: add module scaffold with SessionEvent schema

Phase 1 Task 1: types only, no runtime wiring. Defines SessionEvent
enum (#[non_exhaustive]), support types, and module skeleton. Stubs
for service/state/store/actor/in_process/shim filled in later tasks."
```

---

## Task 2: SQLite migration for `session_events` table

**Files:**
- Create: `migrations/<next-N>_create_session_events.sql` (follow the existing migration naming convention — `ls migrations/` first to see format)
- Modify: whatever boot path runs migrations (search for `MIGRATOR` or `include_str!.*sql`)

**Context:** Add the append-only log table. Schema from spec §7.

- [ ] **Step 2.1: Inspect existing migration convention**

Run: `ls migrations/ 2>/dev/null || find src -name '*.sql' -path '*migrations*'`
Note: the numbering pattern (e.g. `0001_foo.sql`, `20240101000001_foo.sql`) and whether `sqlx` or `rusqlite_migration` runs them.

- [ ] **Step 2.2: Create the migration file**

Path: `migrations/<next-N>_create_session_events.sql`

```sql
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
```

- [ ] **Step 2.3: Register the migration**

If migrations are embedded via `sqlx::migrate!()`, the file in the `migrations/` dir is auto-picked up. If they're in a handwritten list (`const MIGRATIONS: &[...]`), add the new file to that list. Grep for existing migrations:

```bash
grep -rn 'create_messages\|migrations' src/gateway/session_store/ 2>/dev/null | head
```

Match the existing style.

- [ ] **Step 2.4: Verify migration applies cleanly**

Run:
```bash
rm -rf /tmp/phase1-migration-test.db
cargo test -p alephcore --lib session_store 2>&1 | tail -10
```
(Or whichever test exists that exercises the migration path. If no such test is present, add a minimal smoke test: open a fresh SQLite DB, run migrations, assert `session_events` exists.)

- [ ] **Step 2.5: Commit**

```bash
git add migrations/ src/gateway/session_store/
git commit -m "session: add session_events table migration

Phase 1 Task 2: append-only log backing table. (session_id, seq) PK
enforces monotonic ordering per session. Indexes on turn_id and
event_type support common replay/inspection queries."
```

---

## Task 3: `SessionService` trait + `SessionError` + `SessionEventStore` trait

**Files:**
- Modify: `src/session/service.rs`
- Modify: `src/session/store.rs`

**Context:** Public trait surface + persistence seam. No impl yet.

- [ ] **Step 3.1: Write `src/session/service.rs`**

```rust
//! SessionService trait — public facade over the session event log.

use std::result::Result;

use async_trait::async_trait;
use tokio::sync::broadcast;

use crate::session::events::{EventSeq, SessionEvent, SessionEventRecord};

pub type SessionId = crate::routing::session_key::SessionKey;

#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    #[error("session not found: {0:?}")]
    NotFound(SessionId),
    #[error("actor shutdown")]
    ActorShutdown,
    #[error("storage error: {0}")]
    Storage(String),
    #[error("serialization: {0}")]
    Serialization(#[from] serde_json::Error),
    #[error("{0}")]
    Other(String),
}

#[derive(Debug, Clone)]
pub struct SessionHandle {
    pub id: SessionId,
    pub head_seq: EventSeq,
}

#[async_trait]
pub trait SessionService: Send + Sync + 'static {
    /// Idempotent attach — ensures an actor exists for this session.
    async fn attach(&self, id: SessionId) -> Result<SessionHandle, SessionError>;

    /// Read events in sequence range. `from`/`to` None = unbounded on that side.
    async fn get_events(
        &self,
        id: &SessionId,
        from: Option<EventSeq>,
        to: Option<EventSeq>,
    ) -> Result<Vec<SessionEventRecord>, SessionError>;

    /// Append an event. Synchronously persists before returning the assigned seq.
    async fn emit_event(
        &self,
        id: &SessionId,
        event: SessionEvent,
    ) -> Result<EventSeq, SessionError>;

    /// Subscribe to future events on this session. Late subscribers miss history;
    /// combine with `get_events` for backfill.
    async fn subscribe(
        &self,
        id: &SessionId,
    ) -> Result<broadcast::Receiver<SessionEventRecord>, SessionError>;

    /// Force-replace the actor (shutdown old, spawn new, replay). Writes a
    /// `SessionWoken { prior_head }` marker into the log.
    async fn wake(&self, id: &SessionId) -> Result<SessionHandle, SessionError>;

    /// Stop the actor (if any). Events remain persisted; session can be woken again.
    async fn detach(&self, id: &SessionId) -> Result<(), SessionError>;
}
```

- [ ] **Step 3.2: Write `src/session/store.rs`**

```rust
//! SessionEventStore trait — persistence seam.

use async_trait::async_trait;

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
```

- [ ] **Step 3.3: Build**

Run: `cargo check -p alephcore 2>&1 | tail -5`
Expected: `Finished dev ...` with no errors. (The `use` of new types in `mod.rs` re-exports must compile.)

- [ ] **Step 3.4: Commit**

```bash
git add src/session/service.rs src/session/store.rs
git commit -m "session: define SessionService + SessionEventStore traits

Phase 1 Task 3: trait surface for consumers and persistence backends.
SessionError variants cover not-found, actor-shutdown, storage, and
serialization failures. No implementations yet."
```

---

## Task 4: SQLite-backed `SessionEventStore` impl + unit tests

**Files:**
- Modify: `src/session/store.rs`
- Create: `src/session/store/sqlite.rs` (or keep in `store.rs` if project style prefers flat files — check existing conventions first)

**Context:** Concrete persistence. Use the same SQLite connection pool pattern as `src/gateway/session_store/sqlite_backend/` — read that file first to copy the pool-acquisition pattern.

- [ ] **Step 4.1: Inspect existing SQLite pool pattern**

Run: `head -60 src/gateway/session_store/sqlite_backend/mod.rs` (or wherever the pool is)
Note: pool type (`sqlx::SqlitePool` likely), how connections are acquired, and whether queries are compile-time checked.

- [ ] **Step 4.2: Add `SqliteEventStore` struct**

Append to `src/session/store.rs` (or put in a submodule — match project style):

```rust
use sqlx::SqlitePool;

pub struct SqliteEventStore {
    pool: SqlitePool,
}

impl SqliteEventStore {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
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

        sqlx::query(
            "INSERT INTO session_events
             (session_id, seq, turn_id, event_type, payload_json, created_at)
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(&session_key)
        .bind(seq as i64)
        .bind(turn_id)
        .bind(event_type)
        .bind(&payload)
        .bind(created_at_ms)
        .execute(&self.pool)
        .await
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
        let to_val = to.unwrap_or(u64::MAX) as i64;

        let rows: Vec<(i64, String, i64)> = sqlx::query_as(
            "SELECT seq, payload_json, created_at
             FROM session_events
             WHERE session_id = ? AND seq >= ? AND seq <= ?
             ORDER BY seq ASC",
        )
        .bind(&session_key)
        .bind(from_val)
        .bind(to_val)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| SessionError::Storage(e.to_string()))?;

        rows.into_iter()
            .map(|(seq, payload, ts)| -> Result<SessionEventRecord, SessionError> {
                let event: SessionEvent = serde_json::from_str(&payload)?;
                Ok(SessionEventRecord {
                    seq: seq as u64,
                    event,
                    created_at_ms: ts,
                })
            })
            .collect()
    }

    async fn load_head_seq(&self, session_id: &SessionId) -> Result<EventSeq, SessionError> {
        let session_key = session_id_to_string(session_id);
        let row: Option<(i64,)> = sqlx::query_as(
            "SELECT MAX(seq) FROM session_events WHERE session_id = ?",
        )
        .bind(&session_key)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| SessionError::Storage(e.to_string()))?;

        Ok(row.and_then(|(v,)| (v >= 0).then_some(v as u64)).unwrap_or(0))
    }
}

fn session_id_to_string(id: &SessionId) -> String {
    // SessionKey already implements Display / serialize; use the canonical form.
    serde_json::to_string(id).unwrap_or_default()
}

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
        _ => None,
    }
}

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
```

- [ ] **Step 4.3: Add unit tests at the end of `src/session/store.rs`**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::events::{now_ms, MessageContent, TurnTrigger};

    async fn memory_pool() -> SqlitePool {
        let pool = SqlitePool::connect(":memory:").await.unwrap();
        sqlx::query(include_str!("../../migrations/XXXX_create_session_events.sql"))
            .execute(&pool)
            .await
            .unwrap();
        pool
    }

    fn sample_session_id() -> SessionId {
        // Construct via SessionKey::Ephemeral("test") or whichever variant exists;
        // check src/routing/session_key.rs for the right constructor.
        crate::routing::session_key::SessionKey::Ephemeral("test-session".to_string())
    }

    #[tokio::test]
    async fn append_and_load_preserves_order() {
        let store = SqliteEventStore::new(memory_pool().await);
        let sid = sample_session_id();
        let tid = uuid::Uuid::new_v4();
        let at = now_ms();

        let e1 = SessionEvent::TurnStarted {
            turn_id: tid,
            trigger: TurnTrigger::UserMessage,
            at,
        };
        let e2 = SessionEvent::UserMessage {
            turn_id: tid,
            content: MessageContent { text: "hi".into(), blocks: vec![] },
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
    }

    #[tokio::test]
    async fn duplicate_seq_rejected() {
        let store = SqliteEventStore::new(memory_pool().await);
        let sid = sample_session_id();
        let tid = uuid::Uuid::new_v4();
        let at = now_ms();

        let e = SessionEvent::TurnStarted {
            turn_id: tid,
            trigger: TurnTrigger::UserMessage,
            at,
        };
        store.append(&sid, 1, &e, at).await.unwrap();
        let err = store.append(&sid, 1, &e, at).await.unwrap_err();
        assert!(matches!(err, SessionError::Storage(_)));
    }

    #[tokio::test]
    async fn head_seq_empty_is_zero() {
        let store = SqliteEventStore::new(memory_pool().await);
        let sid = sample_session_id();
        assert_eq!(store.load_head_seq(&sid).await.unwrap(), 0);
    }

    #[tokio::test]
    async fn head_seq_returns_max() {
        let store = SqliteEventStore::new(memory_pool().await);
        let sid = sample_session_id();
        let tid = uuid::Uuid::new_v4();
        let at = now_ms();
        let e = SessionEvent::TurnStarted {
            turn_id: tid,
            trigger: TurnTrigger::UserMessage,
            at,
        };
        store.append(&sid, 1, &e, at).await.unwrap();
        store.append(&sid, 2, &e, at).await.unwrap();
        store.append(&sid, 5, &e, at).await.unwrap();
        assert_eq!(store.load_head_seq(&sid).await.unwrap(), 5);
    }
}
```

Note: replace `../../migrations/XXXX_...sql` with the actual migration filename from Task 2.

- [ ] **Step 4.4: Run the tests**

Run: `cargo test -p alephcore --lib session::store 2>&1 | tail -20`
Expected: 4 tests passed. If `SessionKey::Ephemeral` has a different constructor, adjust `sample_session_id()` to match.

- [ ] **Step 4.5: Commit**

```bash
git add src/session/store.rs
git commit -m "session: SQLite implementation of SessionEventStore

Phase 1 Task 4: append, load_all_events, load_events_range, load_head_seq.
Serializes events as JSON; indexed by (session_id, seq) for ordered replay
and by (session_id, turn_id)/(session_id, event_type) for inspection."
```

---

## Task 5: `SessionState` reducer + unit tests

**Files:** Modify: `src/session/state.rs`

**Context:** Pure reducer function mapping events to in-memory state. Used by the actor during replay and after each `emit_event`. Consumers never see `SessionState` directly — it's internal to the actor.

- [ ] **Step 5.1: Write `src/session/state.rs`**

```rust
//! SessionState — in-memory reducer over SessionEvent.
//!
//! Pure function of the event stream. Used by SessionActor during replay
//! and after each emitted event. Never persisted; always rebuilt from the
//! event log.

use std::collections::HashMap;

use crate::gateway::session_manager::SessionIdentityMeta;
use crate::session::events::{ApprovalSource, SessionEvent, TurnId, TurnOutcome};

#[derive(Debug, Default, Clone)]
pub struct SessionState {
    pub identity: Option<SessionIdentityMeta>,
    pub current_turn: Option<TurnState>,
    pub completed_turns: usize,
    pub tokens_used: u32,
    pub tokens_budget: u32,
    pub wake_count: u32,
}

#[derive(Debug, Clone)]
pub struct TurnState {
    pub id: TurnId,
    pub pending_tool_calls: HashMap<String, PendingToolCall>,
}

#[derive(Debug, Clone)]
pub struct PendingToolCall {
    pub name: String,
    pub approved: Option<ApprovalSource>,
}

impl SessionState {
    pub fn apply(&mut self, event: &SessionEvent) {
        match event {
            SessionEvent::SessionCreated { identity, .. } => {
                self.identity = Some(identity.clone());
            }
            SessionEvent::SessionWoken { .. } => {
                self.wake_count += 1;
            }
            SessionEvent::SessionDetached { .. } => {}

            SessionEvent::TurnStarted { turn_id, .. } => {
                self.current_turn = Some(TurnState {
                    id: *turn_id,
                    pending_tool_calls: HashMap::new(),
                });
            }
            SessionEvent::TurnEnded { outcome, .. } => {
                if matches!(outcome, TurnOutcome::Completed) {
                    self.completed_turns += 1;
                }
                self.current_turn = None;
            }

            SessionEvent::UserMessage { .. }
            | SessionEvent::AssistantMessage { .. }
            | SessionEvent::SystemMessage { .. } => {
                // Messages don't mutate state directly — they're preserved in the event log
                // and materialized for UI via the projection layer.
            }

            SessionEvent::LlmCallStarted { .. } | SessionEvent::LlmCallEnded { .. } => {
                // LLM call events are observational; budget tracking happens via BudgetUpdated.
            }

            SessionEvent::ToolCallRequested { call_id, name, .. } => {
                if let Some(turn) = self.current_turn.as_mut() {
                    turn.pending_tool_calls.insert(
                        call_id.clone(),
                        PendingToolCall { name: name.clone(), approved: None },
                    );
                }
            }
            SessionEvent::ToolCallApproved { call_id, by, .. } => {
                if let Some(turn) = self.current_turn.as_mut() {
                    if let Some(pc) = turn.pending_tool_calls.get_mut(call_id) {
                        pc.approved = Some(by.clone());
                    }
                }
            }
            SessionEvent::ToolCallDenied { call_id, .. }
            | SessionEvent::ToolResult { call_id, .. }
            | SessionEvent::ToolError { call_id, .. } => {
                if let Some(turn) = self.current_turn.as_mut() {
                    turn.pending_tool_calls.remove(call_id);
                }
            }

            SessionEvent::SubagentSpawned { .. } | SessionEvent::SubagentReturned { .. } => {
                // Tracked via events; no parent-state mutation needed in Phase 1.
            }

            SessionEvent::BudgetUpdated { tokens_used, tokens_budget, .. } => {
                self.tokens_used = *tokens_used;
                self.tokens_budget = *tokens_budget;
            }
            SessionEvent::CompactionPerformed { .. } => {
                // Compaction's effect on state is encoded in the summary; state itself
                // is not truncated because replay is still from event seq 0.
            }

            SessionEvent::Error { .. } => {
                // Errors are observational; recovery logic is at the Harness layer.
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::events::{now_ms, MessageContent, ToolOutput, TurnTrigger};

    fn turn_started(tid: TurnId) -> SessionEvent {
        SessionEvent::TurnStarted { turn_id: tid, trigger: TurnTrigger::UserMessage, at: now_ms() }
    }

    fn turn_ended_completed(tid: TurnId) -> SessionEvent {
        SessionEvent::TurnEnded { turn_id: tid, outcome: TurnOutcome::Completed, at: now_ms() }
    }

    #[test]
    fn fresh_state_has_no_turn() {
        let s = SessionState::default();
        assert!(s.current_turn.is_none());
        assert_eq!(s.completed_turns, 0);
    }

    #[test]
    fn turn_started_sets_current_turn() {
        let mut s = SessionState::default();
        let tid = uuid::Uuid::new_v4();
        s.apply(&turn_started(tid));
        assert_eq!(s.current_turn.as_ref().unwrap().id, tid);
    }

    #[test]
    fn turn_ended_completed_increments_counter_and_clears_current() {
        let mut s = SessionState::default();
        let tid = uuid::Uuid::new_v4();
        s.apply(&turn_started(tid));
        s.apply(&turn_ended_completed(tid));
        assert!(s.current_turn.is_none());
        assert_eq!(s.completed_turns, 1);
    }

    #[test]
    fn tool_call_lifecycle_tracks_pending() {
        let mut s = SessionState::default();
        let tid = uuid::Uuid::new_v4();
        s.apply(&turn_started(tid));
        s.apply(&SessionEvent::ToolCallRequested {
            turn_id: tid,
            call_id: "c1".into(),
            name: "bash_exec".into(),
            input: serde_json::json!({}),
            at: now_ms(),
        });
        assert_eq!(s.current_turn.as_ref().unwrap().pending_tool_calls.len(), 1);

        s.apply(&SessionEvent::ToolResult {
            turn_id: tid,
            call_id: "c1".into(),
            output: ToolOutput {
                value: serde_json::json!("ok"),
                metadata: Default::default(),
            },
            at: now_ms(),
        });
        assert_eq!(s.current_turn.as_ref().unwrap().pending_tool_calls.len(), 0);
    }

    #[test]
    fn replay_is_deterministic() {
        let mut s1 = SessionState::default();
        let mut s2 = SessionState::default();
        let tid = uuid::Uuid::new_v4();
        let events = vec![
            turn_started(tid),
            SessionEvent::UserMessage {
                turn_id: tid,
                content: MessageContent { text: "hi".into(), blocks: vec![] },
                at: now_ms(),
            },
            turn_ended_completed(tid),
        ];
        for ev in &events {
            s1.apply(ev);
        }
        for ev in &events {
            s2.apply(ev);
        }
        assert_eq!(s1.completed_turns, s2.completed_turns);
        assert_eq!(s1.current_turn.is_none(), s2.current_turn.is_none());
    }

    #[test]
    fn wake_count_increments() {
        let mut s = SessionState::default();
        s.apply(&SessionEvent::SessionWoken { at: now_ms(), prior_head: 10 });
        s.apply(&SessionEvent::SessionWoken { at: now_ms(), prior_head: 20 });
        assert_eq!(s.wake_count, 2);
    }

    #[test]
    fn budget_updated_is_absolute() {
        let mut s = SessionState::default();
        let tid = uuid::Uuid::new_v4();
        s.apply(&SessionEvent::BudgetUpdated { turn_id: tid, tokens_used: 100, tokens_budget: 4000, at: now_ms() });
        assert_eq!(s.tokens_used, 100);
        s.apply(&SessionEvent::BudgetUpdated { turn_id: tid, tokens_used: 250, tokens_budget: 4000, at: now_ms() });
        assert_eq!(s.tokens_used, 250);
    }
}
```

- [ ] **Step 5.2: Run tests**

Run: `cargo test -p alephcore --lib session::state 2>&1 | tail -15`
Expected: 6 tests passed.

- [ ] **Step 5.3: Commit**

```bash
git add src/session/state.rs
git commit -m "session: SessionState reducer with apply() over events

Phase 1 Task 5: pure function mapping event stream to in-memory state.
Tracks current turn, pending tool calls, budget, wake count, completed
turns. Used by SessionActor during replay and post-emit."
```

---

## Task 6: `SessionActor` + command loop

**Files:** Modify: `src/session/actor.rs`

**Context:** The actor is one tokio task per session. It replays events on startup, then serves commands until its inbox closes or idle timeout fires.

- [ ] **Step 6.1: Write `src/session/actor.rs`**

```rust
//! SessionActor — one tokio task per session.

use std::sync::Arc;

use tokio::sync::{broadcast, mpsc, oneshot};
use tokio::time::{Duration, Instant};

use crate::session::events::{now_ms, EventSeq, SessionEvent, SessionEventRecord};
use crate::session::service::{SessionError, SessionId};
use crate::session::state::SessionState;
use crate::session::store::SessionEventStore;

/// How long an idle actor survives before self-terminating.
pub const DEFAULT_IDLE_TIMEOUT: Duration = Duration::from_secs(30 * 60);

pub enum ActorCommand {
    EmitEvent {
        event: SessionEvent,
        reply: oneshot::Sender<Result<EventSeq, SessionError>>,
    },
    GetEvents {
        from: Option<EventSeq>,
        to: Option<EventSeq>,
        reply: oneshot::Sender<Result<Vec<SessionEventRecord>, SessionError>>,
    },
    Subscribe {
        reply: oneshot::Sender<broadcast::Receiver<SessionEventRecord>>,
    },
    Shutdown {
        reply: oneshot::Sender<()>,
    },
}

pub struct SessionActor {
    pub id: SessionId,
    pub store: Arc<dyn SessionEventStore>,
    state: SessionState,
    head_seq: EventSeq,
    inbox: mpsc::Receiver<ActorCommand>,
    broadcaster: broadcast::Sender<SessionEventRecord>,
    idle_timeout: Duration,
}

impl SessionActor {
    pub fn new(
        id: SessionId,
        store: Arc<dyn SessionEventStore>,
        inbox: mpsc::Receiver<ActorCommand>,
        broadcaster: broadcast::Sender<SessionEventRecord>,
        idle_timeout: Duration,
    ) -> Self {
        Self {
            id,
            store,
            state: SessionState::default(),
            head_seq: 0,
            inbox,
            broadcaster,
            idle_timeout,
        }
    }

    /// Replays all persisted events and rebuilds `state` + `head_seq`.
    async fn replay(&mut self) -> Result<(), SessionError> {
        let records = self.store.load_all_events(&self.id).await?;
        for record in &records {
            self.state.apply(&record.event);
            self.head_seq = record.seq;
        }
        Ok(())
    }

    pub async fn run(mut self) {
        if let Err(e) = self.replay().await {
            tracing::error!(?e, "SessionActor replay failed; actor terminating");
            return;
        }

        let mut idle_deadline = Instant::now() + self.idle_timeout;
        loop {
            tokio::select! {
                biased;
                cmd = self.inbox.recv() => match cmd {
                    Some(ActorCommand::EmitEvent { event, reply }) => {
                        let seq = self.head_seq + 1;
                        let at = now_ms();
                        match self.store.append(&self.id, seq, &event, at).await {
                            Ok(()) => {
                                let record = SessionEventRecord { seq, event, created_at_ms: at };
                                self.state.apply(&record.event);
                                self.head_seq = seq;
                                let _ = self.broadcaster.send(record);
                                let _ = reply.send(Ok(seq));
                                idle_deadline = Instant::now() + self.idle_timeout;
                            }
                            Err(e) => {
                                let _ = reply.send(Err(e));
                            }
                        }
                    }
                    Some(ActorCommand::GetEvents { from, to, reply }) => {
                        let result = self.store.load_events_range(&self.id, from, to).await;
                        let _ = reply.send(result);
                        idle_deadline = Instant::now() + self.idle_timeout;
                    }
                    Some(ActorCommand::Subscribe { reply }) => {
                        let _ = reply.send(self.broadcaster.subscribe());
                        idle_deadline = Instant::now() + self.idle_timeout;
                    }
                    Some(ActorCommand::Shutdown { reply }) => {
                        let _ = reply.send(());
                        return;
                    }
                    None => {
                        // All senders dropped; exit cleanly.
                        return;
                    }
                },
                _ = tokio::time::sleep_until(idle_deadline) => {
                    tracing::debug!(id = ?self.id, "SessionActor idle timeout — detaching");
                    return;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::events::{MessageContent, TurnTrigger};
    use crate::session::store::SqliteEventStore;
    use sqlx::SqlitePool;

    async fn test_store() -> Arc<dyn SessionEventStore> {
        let pool = SqlitePool::connect(":memory:").await.unwrap();
        sqlx::query(include_str!("../../migrations/XXXX_create_session_events.sql"))
            .execute(&pool)
            .await
            .unwrap();
        Arc::new(SqliteEventStore::new(pool))
    }

    fn sample_id() -> SessionId {
        crate::routing::session_key::SessionKey::Ephemeral("actor-test".to_string())
    }

    #[tokio::test]
    async fn emit_then_get_returns_same_event() {
        let store = test_store().await;
        let id = sample_id();
        let (tx, rx) = mpsc::channel(8);
        let (bcast, _) = broadcast::channel(16);
        let actor = SessionActor::new(id.clone(), store, rx, bcast, DEFAULT_IDLE_TIMEOUT);
        let handle = tokio::spawn(actor.run());

        let (rtx, rrx) = oneshot::channel();
        tx.send(ActorCommand::EmitEvent {
            event: SessionEvent::TurnStarted {
                turn_id: uuid::Uuid::new_v4(),
                trigger: TurnTrigger::UserMessage,
                at: now_ms(),
            },
            reply: rtx,
        })
        .await
        .unwrap();
        let seq = rrx.await.unwrap().unwrap();
        assert_eq!(seq, 1);

        let (gtx, grx) = oneshot::channel();
        tx.send(ActorCommand::GetEvents { from: None, to: None, reply: gtx })
            .await
            .unwrap();
        let events = grx.await.unwrap().unwrap();
        assert_eq!(events.len(), 1);

        let (stx, srx) = oneshot::channel();
        tx.send(ActorCommand::Shutdown { reply: stx }).await.unwrap();
        srx.await.unwrap();
        handle.await.unwrap();
    }

    #[tokio::test]
    async fn subscribe_receives_subsequent_events() {
        let store = test_store().await;
        let id = sample_id();
        let (tx, rx) = mpsc::channel(8);
        let (bcast, _) = broadcast::channel(16);
        let actor = SessionActor::new(id.clone(), store, rx, bcast, DEFAULT_IDLE_TIMEOUT);
        tokio::spawn(actor.run());

        let (stx, srx) = oneshot::channel();
        tx.send(ActorCommand::Subscribe { reply: stx }).await.unwrap();
        let mut sub = srx.await.unwrap();

        let (rtx, _rrx) = oneshot::channel();
        tx.send(ActorCommand::EmitEvent {
            event: SessionEvent::UserMessage {
                turn_id: uuid::Uuid::new_v4(),
                content: MessageContent { text: "hi".into(), blocks: vec![] },
                at: now_ms(),
            },
            reply: rtx,
        })
        .await
        .unwrap();

        let record = sub.recv().await.unwrap();
        assert!(matches!(record.event, SessionEvent::UserMessage { .. }));
    }

    #[tokio::test]
    async fn replay_rebuilds_head_seq() {
        let store = test_store().await;
        let id = sample_id();
        let at = now_ms();
        // Seed 3 events directly in the store.
        for seq in 1..=3 {
            store
                .append(
                    &id,
                    seq,
                    &SessionEvent::TurnStarted {
                        turn_id: uuid::Uuid::new_v4(),
                        trigger: TurnTrigger::UserMessage,
                        at,
                    },
                    at,
                )
                .await
                .unwrap();
        }

        let (tx, rx) = mpsc::channel(8);
        let (bcast, _) = broadcast::channel(16);
        let actor = SessionActor::new(id.clone(), store, rx, bcast, DEFAULT_IDLE_TIMEOUT);
        tokio::spawn(actor.run());

        // Emit one more event; it should land at seq=4
        let (rtx, rrx) = oneshot::channel();
        tx.send(ActorCommand::EmitEvent {
            event: SessionEvent::TurnStarted {
                turn_id: uuid::Uuid::new_v4(),
                trigger: TurnTrigger::UserMessage,
                at,
            },
            reply: rtx,
        })
        .await
        .unwrap();
        let seq = rrx.await.unwrap().unwrap();
        assert_eq!(seq, 4);
    }
}
```

- [ ] **Step 6.2: Run tests**

Run: `cargo test -p alephcore --lib session::actor 2>&1 | tail -15`
Expected: 3 tests passed.

- [ ] **Step 6.3: Commit**

```bash
git add src/session/actor.rs
git commit -m "session: SessionActor with replay + command loop

Phase 1 Task 6: one tokio task per session. Replays from SQLite on
start, then serves EmitEvent/GetEvents/Subscribe/Shutdown. Idle 30m
auto-detach. Broadcast channel for fan-out to subscribers."
```

---

## Task 7: `InProcessActorSessionService` impl

**Files:** Modify: `src/session/in_process.rs`

**Context:** The service holds a router `HashMap<SessionId, mpsc::Sender<ActorCommand>>` + `HashMap<SessionId, broadcast::Sender<SessionEventRecord>>`. `attach`/`wake` manage the actor lifecycle; `emit_event`/`get_events`/`subscribe` route commands to the right actor; `detach` shuts an actor down.

- [ ] **Step 7.1: Write `src/session/in_process.rs`**

```rust
//! InProcessActorSessionService — the default SessionService implementation.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::{broadcast, mpsc, oneshot, RwLock};
use tokio::time::{timeout, Duration};

use crate::session::actor::{ActorCommand, SessionActor, DEFAULT_IDLE_TIMEOUT};
use crate::session::events::{now_ms, EventSeq, SessionEvent, SessionEventRecord};
use crate::session::service::{SessionError, SessionHandle, SessionId, SessionService};
use crate::session::store::SessionEventStore;

const COMMAND_BUFFER: usize = 64;
const BROADCAST_BUFFER: usize = 256;
const SHUTDOWN_GRACE: Duration = Duration::from_secs(5);

pub struct InProcessActorSessionService {
    store: Arc<dyn SessionEventStore>,
    senders: RwLock<HashMap<SessionId, mpsc::Sender<ActorCommand>>>,
    broadcasters: RwLock<HashMap<SessionId, broadcast::Sender<SessionEventRecord>>>,
    idle_timeout: Duration,
}

impl InProcessActorSessionService {
    pub fn new(store: Arc<dyn SessionEventStore>) -> Self {
        Self {
            store,
            senders: RwLock::new(HashMap::new()),
            broadcasters: RwLock::new(HashMap::new()),
            idle_timeout: DEFAULT_IDLE_TIMEOUT,
        }
    }

    pub fn with_idle_timeout(mut self, t: Duration) -> Self {
        self.idle_timeout = t;
        self
    }

    async fn sender_for(&self, id: &SessionId) -> Option<mpsc::Sender<ActorCommand>> {
        self.senders.read().await.get(id).cloned()
    }

    async fn spawn_actor(&self, id: &SessionId) -> Result<mpsc::Sender<ActorCommand>, SessionError> {
        let (tx, rx) = mpsc::channel(COMMAND_BUFFER);
        let (bcast_tx, _) = broadcast::channel(BROADCAST_BUFFER);
        let actor = SessionActor::new(
            id.clone(),
            self.store.clone(),
            rx,
            bcast_tx.clone(),
            self.idle_timeout,
        );
        tokio::spawn(actor.run());

        self.senders.write().await.insert(id.clone(), tx.clone());
        self.broadcasters.write().await.insert(id.clone(), bcast_tx);
        Ok(tx)
    }
}

#[async_trait]
impl SessionService for InProcessActorSessionService {
    async fn attach(&self, id: SessionId) -> Result<SessionHandle, SessionError> {
        if self.sender_for(&id).await.is_none() {
            self.spawn_actor(&id).await?;
        }
        let head = self.store.load_head_seq(&id).await?;
        Ok(SessionHandle { id, head_seq: head })
    }

    async fn get_events(
        &self,
        id: &SessionId,
        from: Option<EventSeq>,
        to: Option<EventSeq>,
    ) -> Result<Vec<SessionEventRecord>, SessionError> {
        let sender = match self.sender_for(id).await {
            Some(s) => s,
            None => {
                // Allow reads without a live actor — go direct to store.
                return self.store.load_events_range(id, from, to).await;
            }
        };
        let (tx, rx) = oneshot::channel();
        sender
            .send(ActorCommand::GetEvents { from, to, reply: tx })
            .await
            .map_err(|_| SessionError::ActorShutdown)?;
        rx.await.map_err(|_| SessionError::ActorShutdown)?
    }

    async fn emit_event(
        &self,
        id: &SessionId,
        event: SessionEvent,
    ) -> Result<EventSeq, SessionError> {
        let sender = match self.sender_for(id).await {
            Some(s) => s,
            None => self.spawn_actor(id).await?,
        };
        let (tx, rx) = oneshot::channel();
        sender
            .send(ActorCommand::EmitEvent { event, reply: tx })
            .await
            .map_err(|_| SessionError::ActorShutdown)?;
        rx.await.map_err(|_| SessionError::ActorShutdown)?
    }

    async fn subscribe(
        &self,
        id: &SessionId,
    ) -> Result<broadcast::Receiver<SessionEventRecord>, SessionError> {
        if self.sender_for(id).await.is_none() {
            self.spawn_actor(id).await?;
        }
        let bcast = {
            self.broadcasters
                .read()
                .await
                .get(id)
                .cloned()
                .ok_or_else(|| SessionError::Other("broadcaster missing".into()))?
        };
        Ok(bcast.subscribe())
    }

    async fn wake(&self, id: &SessionId) -> Result<SessionHandle, SessionError> {
        // 1. Shutdown old actor if present.
        if let Some(sender) = self.senders.write().await.remove(id) {
            let (tx, rx) = oneshot::channel();
            let _ = sender.send(ActorCommand::Shutdown { reply: tx }).await;
            let _ = timeout(SHUTDOWN_GRACE, rx).await;
        }
        self.broadcasters.write().await.remove(id);

        // 2. Spawn fresh actor — it replays from SQLite.
        let sender = self.spawn_actor(id).await?;

        // 3. Emit SessionWoken marker with prior_head = pre-wake head_seq.
        let prior_head = self.store.load_head_seq(id).await?;
        let (tx, rx) = oneshot::channel();
        sender
            .send(ActorCommand::EmitEvent {
                event: SessionEvent::SessionWoken { at: now_ms(), prior_head },
                reply: tx,
            })
            .await
            .map_err(|_| SessionError::ActorShutdown)?;
        let new_head = rx.await.map_err(|_| SessionError::ActorShutdown)??;

        Ok(SessionHandle { id: id.clone(), head_seq: new_head })
    }

    async fn detach(&self, id: &SessionId) -> Result<(), SessionError> {
        if let Some(sender) = self.senders.write().await.remove(id) {
            let (tx, rx) = oneshot::channel();
            let _ = sender.send(ActorCommand::Shutdown { reply: tx }).await;
            let _ = timeout(SHUTDOWN_GRACE, rx).await;
        }
        self.broadcasters.write().await.remove(id);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::events::{MessageContent, TurnTrigger};
    use crate::session::store::SqliteEventStore;
    use sqlx::SqlitePool;

    async fn fresh_service() -> InProcessActorSessionService {
        let pool = SqlitePool::connect(":memory:").await.unwrap();
        sqlx::query(include_str!("../../migrations/XXXX_create_session_events.sql"))
            .execute(&pool)
            .await
            .unwrap();
        let store = Arc::new(SqliteEventStore::new(pool));
        InProcessActorSessionService::new(store)
    }

    fn sample_id(label: &str) -> SessionId {
        crate::routing::session_key::SessionKey::Ephemeral(label.to_string())
    }

    #[tokio::test]
    async fn attach_is_idempotent() {
        let svc = fresh_service().await;
        let id = sample_id("idempo");
        let h1 = svc.attach(id.clone()).await.unwrap();
        let h2 = svc.attach(id.clone()).await.unwrap();
        assert_eq!(h1.id, h2.id);
    }

    #[tokio::test]
    async fn emit_then_get_roundtrip() {
        let svc = fresh_service().await;
        let id = sample_id("rt");
        let tid = uuid::Uuid::new_v4();
        let seq = svc
            .emit_event(&id, SessionEvent::TurnStarted {
                turn_id: tid,
                trigger: TurnTrigger::UserMessage,
                at: now_ms(),
            })
            .await
            .unwrap();
        assert_eq!(seq, 1);
        let events = svc.get_events(&id, None, None).await.unwrap();
        assert_eq!(events.len(), 1);
    }

    #[tokio::test]
    async fn wake_writes_session_woken_with_prior_head() {
        let svc = fresh_service().await;
        let id = sample_id("wake");
        let tid = uuid::Uuid::new_v4();
        for _ in 0..3 {
            svc.emit_event(&id, SessionEvent::TurnStarted {
                turn_id: tid,
                trigger: TurnTrigger::UserMessage,
                at: now_ms(),
            })
            .await
            .unwrap();
        }
        let handle = svc.wake(&id).await.unwrap();
        assert_eq!(handle.head_seq, 4, "SessionWoken lands at seq 4 after 3 events");

        let events = svc.get_events(&id, None, None).await.unwrap();
        let woken = events.iter().find(|r| matches!(r.event, SessionEvent::SessionWoken { .. }));
        assert!(woken.is_some());
        if let Some(r) = woken {
            if let SessionEvent::SessionWoken { prior_head, .. } = &r.event {
                assert_eq!(*prior_head, 3);
            }
        }
    }

    #[tokio::test]
    async fn subscribe_delivers_post_subscribe_events() {
        let svc = fresh_service().await;
        let id = sample_id("sub");
        svc.attach(id.clone()).await.unwrap();
        let mut rx = svc.subscribe(&id).await.unwrap();

        svc.emit_event(&id, SessionEvent::UserMessage {
            turn_id: uuid::Uuid::new_v4(),
            content: MessageContent { text: "hi".into(), blocks: vec![] },
            at: now_ms(),
        })
        .await
        .unwrap();

        let record = tokio::time::timeout(Duration::from_secs(1), rx.recv()).await.unwrap().unwrap();
        assert!(matches!(record.event, SessionEvent::UserMessage { .. }));
    }

    #[tokio::test]
    async fn detach_stops_actor_but_keeps_events() {
        let svc = fresh_service().await;
        let id = sample_id("det");
        let tid = uuid::Uuid::new_v4();
        svc.emit_event(&id, SessionEvent::TurnStarted {
            turn_id: tid,
            trigger: TurnTrigger::UserMessage,
            at: now_ms(),
        })
        .await
        .unwrap();
        svc.detach(&id).await.unwrap();

        // Events remain in the store
        let events = svc.get_events(&id, None, None).await.unwrap();
        assert_eq!(events.len(), 1);

        // Re-attach works
        let handle = svc.attach(id.clone()).await.unwrap();
        assert_eq!(handle.head_seq, 1);
    }
}
```

- [ ] **Step 7.2: Run tests**

Run: `cargo test -p alephcore --lib session::in_process 2>&1 | tail -20`
Expected: 5 tests passed.

- [ ] **Step 7.3: Commit**

```bash
git add src/session/in_process.rs
git commit -m "session: InProcessActorSessionService implementation

Phase 1 Task 7: the default SessionService impl. Routes commands to
per-session actors via mpsc; fans out events via broadcast; attach is
idempotent; wake force-replaces the actor and writes SessionWoken marker."
```

---

## Task 8: Crash-recovery integration test

**Files:** Create: `tests/session_service_crash_recovery.rs`

**Context:** End-to-end test that simulates a Harness crash mid-turn by aborting the actor task, then verifies `wake()` recovers and the next `emit_event` lands at the correct `seq`.

- [ ] **Step 8.1: Write the integration test**

```rust
//! Integration test: simulate actor/Harness crash, verify wake() recovers state.

use std::sync::Arc;
use std::time::Duration;

use alephcore::routing::session_key::SessionKey;
use alephcore::session::{
    events::{now_ms, MessageContent, SessionEvent, TurnTrigger},
    service::SessionService,
    store::{SessionEventStore, SqliteEventStore},
    InProcessActorSessionService,
};
use sqlx::SqlitePool;

async fn fresh_service() -> InProcessActorSessionService {
    let pool = SqlitePool::connect(":memory:").await.unwrap();
    sqlx::query(include_str!(
        "../migrations/XXXX_create_session_events.sql"
    ))
    .execute(&pool)
    .await
    .unwrap();
    let store: Arc<dyn SessionEventStore> = Arc::new(SqliteEventStore::new(pool));
    InProcessActorSessionService::new(store)
}

#[tokio::test]
async fn crash_then_wake_then_continue() {
    let svc = fresh_service().await;
    let id = SessionKey::Ephemeral("crash-recover".into());
    let tid = uuid::Uuid::new_v4();

    // 1. Run a few events normally
    svc.emit_event(&id, SessionEvent::TurnStarted {
        turn_id: tid,
        trigger: TurnTrigger::UserMessage,
        at: now_ms(),
    })
    .await
    .unwrap();
    svc.emit_event(&id, SessionEvent::UserMessage {
        turn_id: tid,
        content: MessageContent { text: "hi".into(), blocks: vec![] },
        at: now_ms(),
    })
    .await
    .unwrap();

    // 2. Simulate crash: detach forcefully (mimics Harness dropping its sender)
    svc.detach(&id).await.unwrap();

    // 3. Wake should bring the session back — new actor, replay from SQLite
    let handle = svc.wake(&id).await.unwrap();
    // Two emitted events + SessionWoken marker
    assert_eq!(handle.head_seq, 3);

    // 4. Post-wake emission continues with correct seq
    let seq = svc
        .emit_event(&id, SessionEvent::AssistantMessage {
            turn_id: tid,
            content: MessageContent { text: "hello back".into(), blocks: vec![] },
            at: now_ms(),
        })
        .await
        .unwrap();
    assert_eq!(seq, 4);

    // 5. get_events returns all 4 in order
    let events = svc.get_events(&id, None, None).await.unwrap();
    assert_eq!(events.len(), 4);
    let types: Vec<_> = events
        .iter()
        .map(|r| match &r.event {
            SessionEvent::TurnStarted { .. } => "turn_started",
            SessionEvent::UserMessage { .. } => "user_message",
            SessionEvent::SessionWoken { .. } => "session_woken",
            SessionEvent::AssistantMessage { .. } => "assistant_message",
            _ => "other",
        })
        .collect();
    assert_eq!(
        types,
        vec!["turn_started", "user_message", "session_woken", "assistant_message"]
    );
}

#[tokio::test]
async fn two_sessions_are_independent() {
    let svc = fresh_service().await;
    let s1 = SessionKey::Ephemeral("s1".into());
    let s2 = SessionKey::Ephemeral("s2".into());
    let tid = uuid::Uuid::new_v4();

    let e = |at: i64| SessionEvent::TurnStarted {
        turn_id: tid,
        trigger: TurnTrigger::UserMessage,
        at,
    };

    svc.emit_event(&s1, e(now_ms())).await.unwrap();
    svc.emit_event(&s1, e(now_ms())).await.unwrap();
    svc.emit_event(&s2, e(now_ms())).await.unwrap();

    let h1 = svc.attach(s1.clone()).await.unwrap();
    let h2 = svc.attach(s2.clone()).await.unwrap();
    assert_eq!(h1.head_seq, 2);
    assert_eq!(h2.head_seq, 1);
}
```

- [ ] **Step 8.2: Run the integration tests**

Run: `cargo test -p alephcore --test session_service_crash_recovery 2>&1 | tail -15`
Expected: 2 tests passed.

- [ ] **Step 8.3: Commit**

```bash
git add tests/session_service_crash_recovery.rs
git commit -m "session: crash-recovery integration test

Phase 1 Task 8: end-to-end wake() recovery. Emit events, detach
(simulating Harness death), wake, verify head_seq correct and new
emissions continue at the right seq. Cross-session independence test
included."
```

---

## Task 9: Dual-write shim between `SessionManager` and `SessionService`

**Files:**
- Modify: `src/session/shim.rs`
- Modify: the `SessionManager` append paths (grep `SessionManager::append\|append_message\|persist_message`)

**Context:** During migration, every `SessionManager` write is mirrored into `SessionService` so the new `session_events` table stays populated even for Gateway-originated messages. Read paths are unchanged. This keeps both stores consistent; Phase 6 removes the shim.

- [ ] **Step 9.1: Identify SessionManager append points**

Run:
```bash
grep -rn 'append_message\|append_user\|append_assistant\|append_tool\|persist_message\|record_message' src/gateway/session_manager/ src/gateway/session_store/ | head -30
```
Note: record every call site and its method signature.

- [ ] **Step 9.2: Write the shim helper**

```rust
//! Dual-write helper during Phase 1 migration.
//!
//! Each write on SessionManager mirrors to SessionService. Once the agent_loop
//! is fully migrated (end of Phase 1) the SessionManager side is still the only
//! source of truth for Gateway RPC reads; the shim keeps session_events
//! populated so Phase 4+ consumers can rely on it. Removed in Phase 6 when
//! Gateway RPC also migrates.

use std::sync::Arc;

use tracing::warn;

use crate::session::events::{now_ms, MessageContent, SessionEvent, TurnId, TurnTrigger};
use crate::session::service::{SessionId, SessionService};

/// Mirrors a user message into the session event log.
pub async fn mirror_user_message(
    svc: &Arc<dyn SessionService>,
    id: &SessionId,
    turn_id: TurnId,
    text: String,
) {
    if let Err(e) = svc
        .emit_event(
            id,
            SessionEvent::UserMessage {
                turn_id,
                content: MessageContent { text, blocks: vec![] },
                at: now_ms(),
            },
        )
        .await
    {
        warn!(session_id = ?id, error = ?e, "dual-write mirror_user_message failed");
    }
}

/// Mirrors an assistant message into the session event log.
pub async fn mirror_assistant_message(
    svc: &Arc<dyn SessionService>,
    id: &SessionId,
    turn_id: TurnId,
    text: String,
) {
    if let Err(e) = svc
        .emit_event(
            id,
            SessionEvent::AssistantMessage {
                turn_id,
                content: MessageContent { text, blocks: vec![] },
                at: now_ms(),
            },
        )
        .await
    {
        warn!(session_id = ?id, error = ?e, "dual-write mirror_assistant_message failed");
    }
}

/// Mirrors a turn-start. Call before the first message in a turn arrives.
pub async fn mirror_turn_started(
    svc: &Arc<dyn SessionService>,
    id: &SessionId,
    turn_id: TurnId,
    trigger: TurnTrigger,
) {
    if let Err(e) = svc
        .emit_event(
            id,
            SessionEvent::TurnStarted { turn_id, trigger, at: now_ms() },
        )
        .await
    {
        warn!(session_id = ?id, error = ?e, "dual-write mirror_turn_started failed");
    }
}

// Add more helpers as append paths are discovered. Keep each helper single-
// purpose; the goal is that the call site has one obvious mirror function
// to call.
```

- [ ] **Step 9.3: Wire the shim at each append site**

For each call site identified in Step 9.1, add the matching `mirror_*` call immediately after the SessionManager append. Example pattern:

```rust
// Before:
self.session_manager.append_user_message(&session_id, text.clone()).await?;

// After:
self.session_manager.append_user_message(&session_id, text.clone()).await?;
crate::session::shim::mirror_user_message(
    &self.session_service,
    &session_id,
    current_turn_id,
    text.clone(),
).await;
```

Plumbing note: wherever `SessionManager` is held, the struct needs a parallel `Arc<dyn SessionService>` field. Thread it through the boot path in `src/bin/aleph-server/commands/start/`. Follow the existing DI pattern (`AppContext` builder or equivalent — grep for how `SessionManager` is constructed to find it).

- [ ] **Step 9.4: Write a dual-write consistency test**

Create `tests/session_service_dual_write.rs`:

```rust
//! Dual-write consistency: SessionManager writes produce matching session_events.
//!
//! Exact test setup depends on SessionManager's constructor. Use whatever test
//! harness already exists for SessionManager (grep for `fn test_.*session_manager`
//! in src/gateway/session_manager/*).

// NOTE: This test file starts as a skeleton. Fill in once Step 9.3 is done and
// you have a working way to construct a SessionManager + SessionService pair
// in tests. The assertion shape is:
//
// 1. construct both services
// 2. append 5 messages via SessionManager
// 3. query session_events for the same session_id
// 4. assert 5 records, same order, matching content

#[tokio::test]
#[ignore = "fill in once dual-write wiring lands"]
async fn five_sessionmanager_messages_produce_five_events() {
    // TODO: initial skeleton. Implement after Step 9.3 is merged on its own
    // commit; the implementer for this step should replace this body and
    // remove #[ignore].
}
```

Then replace the `#[ignore]` body with the real test using whatever constructors/fixtures exist in the SessionManager test suite.

- [ ] **Step 9.5: Build + run full test suite**

Run: `cargo test -p alephcore 2>&1 | tail -15`
Expected: all tests green except the 2 pre-existing failures. The new consistency test must pass.

- [ ] **Step 9.6: Commit**

```bash
git add src/session/shim.rs tests/session_service_dual_write.rs src/gateway/ src/bin/aleph-server/
git commit -m "session: dual-write shim between SessionManager and SessionService

Phase 1 Task 9: mirror each SessionManager append into SessionService
so session_events populates in parallel. Read paths unchanged. Phase 6
removes the shim when Gateway RPC also migrates."
```

---

## Task 10: Migrate `agent_loop` **reads** to `SessionService`

**Files:** Every `agent_loop/**` file that currently reads from `SessionManager`. The call sites are listed by Pre-2's baseline snapshot.

**Context:** Replace each `SessionManager::get_*` / `SessionManager::history` read with the equivalent `SessionService::get_events` + a local projection helper. Read migration is strictly additive — no behavior change visible to consumers.

- [ ] **Step 10.1: Add a projection helper**

Create `src/session/projection.rs`:

```rust
//! Projects a session event stream into message-shaped views for agent_loop.
//!
//! Phase 1 bridge: agent_loop used to read UnifiedMessage arrays from
//! SessionManager; during the migration it reads events from SessionService
//! and projects them into the same shape here.

use crate::session::events::{SessionEvent, SessionEventRecord};

#[derive(Debug, Clone)]
pub struct ProjectedMessage {
    pub role: MessageRole,
    pub text: String,
    pub at_ms: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub enum MessageRole {
    User,
    Assistant,
    System,
}

/// Turn a raw event stream into the message-view agent_loop expects.
pub fn project_messages(events: &[SessionEventRecord]) -> Vec<ProjectedMessage> {
    events
        .iter()
        .filter_map(|record| match &record.event {
            SessionEvent::UserMessage { content, .. } => Some(ProjectedMessage {
                role: MessageRole::User,
                text: content.text.clone(),
                at_ms: record.created_at_ms,
            }),
            SessionEvent::AssistantMessage { content, .. } => Some(ProjectedMessage {
                role: MessageRole::Assistant,
                text: content.text.clone(),
                at_ms: record.created_at_ms,
            }),
            SessionEvent::SystemMessage { content, .. } => Some(ProjectedMessage {
                role: MessageRole::System,
                text: content.clone(),
                at_ms: record.created_at_ms,
            }),
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::events::{now_ms, MessageContent};

    fn rec(seq: u64, ev: SessionEvent) -> SessionEventRecord {
        SessionEventRecord { seq, event: ev, created_at_ms: now_ms() }
    }

    #[test]
    fn projects_only_message_variants() {
        let tid = uuid::Uuid::new_v4();
        let events = vec![
            rec(1, SessionEvent::TurnStarted { turn_id: tid, trigger: crate::session::events::TurnTrigger::UserMessage, at: now_ms() }),
            rec(2, SessionEvent::UserMessage { turn_id: tid, content: MessageContent { text: "hi".into(), blocks: vec![] }, at: now_ms() }),
            rec(3, SessionEvent::AssistantMessage { turn_id: tid, content: MessageContent { text: "hello".into(), blocks: vec![] }, at: now_ms() }),
        ];
        let msgs = project_messages(&events);
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].role, MessageRole::User);
        assert_eq!(msgs[1].role, MessageRole::Assistant);
    }
}
```

Register in `src/session/mod.rs`:
```rust
pub mod projection;
pub use projection::{project_messages, MessageRole, ProjectedMessage};
```

- [ ] **Step 10.2: Migrate read call sites one at a time**

For each agent_loop read (typically `history()`, `get_messages()`, `get_last_message()`, etc.):

1. Add an `Arc<dyn SessionService>` field to the enclosing struct if not already present
2. Replace the read call:
   ```rust
   // Before
   let msgs = self.session_manager.history(&session_id).await?;

   // After
   let events = self.session_service.get_events(&session_id, None, None).await?;
   let msgs = crate::session::project_messages(&events);
   ```
3. Run `cargo check -p alephcore`
4. Run `cargo test -p alephcore --lib <nearest module>`
5. Commit with a message like `agent_loop: migrate read X to SessionService`

Do this **one site at a time**, one commit each — the engineer should NOT batch more than one read migration per commit. If there are 15 read sites, this task produces 15 commits. That granularity makes bisect-debuggable if something breaks.

- [ ] **Step 10.3: Verify read migration complete**

Run:
```bash
grep -rn 'use.*session_manager::SessionManager\|self\.session_manager\.' src/agent_loop/ | head -20
```
Expected: only lines related to *writes* remaining (reads should all be gone).

- [ ] **Step 10.4: Final commit for the read migration batch**

Run the full test suite:
```bash
cargo test -p alephcore 2>&1 | tail -10
```
Expected: same pass count as Pre-3's baseline (plus the new session-module tests), same 2 pre-existing failures, no new failures.

If any new failures — do NOT proceed. Roll back the problematic commit (`git revert <sha>`) and investigate.

---

## Task 11: Migrate `agent_loop` **writes** to `SessionService`

**Files:** Every `agent_loop/**` file that currently writes to `SessionManager`.

**Context:** Replace each `SessionManager::append_*` write with `SessionService::emit_event`. The dual-write shim from Task 9 keeps the SessionManager side in sync (so Gateway RPC reads still see the message). After Phase 1, SessionManager is "write-through" for Gateway but agent_loop only writes to SessionService.

- [ ] **Step 11.1: Identify write call sites**

Run:
```bash
grep -rn 'session_manager\..*append\|session_manager\..*record' src/agent_loop/ | head -20
```

- [ ] **Step 11.2: Migrate write call sites one at a time**

For each write:

```rust
// Before
self.session_manager.append_user_message(&session_id, text.clone()).await?;

// After
let turn_id = self.current_turn_id; // TurnId from the current agent_loop turn
self.session_service
    .emit_event(
        &session_id,
        crate::session::events::SessionEvent::UserMessage {
            turn_id,
            content: crate::session::events::MessageContent { text: text.clone(), blocks: vec![] },
            at: crate::session::events::now_ms(),
        },
    )
    .await?;
```

One write site = one commit.

Where agent_loop doesn't yet have a `TurnId`, introduce one: generate via `uuid::Uuid::new_v4()` at the top of each turn and thread it through. This is a required piece of Phase 1 scope (TurnStarted/TurnEnded wrap every turn).

- [ ] **Step 11.3: Add `TurnStarted`/`TurnEnded` bracketing**

At the start of each agent turn in agent_loop:
```rust
let turn_id = uuid::Uuid::new_v4();
self.session_service.emit_event(&session_id, SessionEvent::TurnStarted {
    turn_id,
    trigger: match trigger_source { /* map to TurnTrigger */ },
    at: now_ms(),
}).await?;
```

At the end:
```rust
self.session_service.emit_event(&session_id, SessionEvent::TurnEnded {
    turn_id,
    outcome: if errored { TurnOutcome::Errored { kind: ErrorKind::Harness } } else { TurnOutcome::Completed },
    at: now_ms(),
}).await?;
```

- [ ] **Step 11.4: Verify write migration complete**

Run:
```bash
grep -rn 'session_manager\..*append\|session_manager\..*record' src/agent_loop/
```
Expected: zero output.

Run: `cargo test -p alephcore 2>&1 | tail -10`
Expected: no new failures vs Pre-3 baseline.

- [ ] **Step 11.5: Full agent_loop smoke test**

Start aleph-server (killing stale processes first per CLAUDE.md):
```bash
pkill -f "target/release/aleph-server" 2>/dev/null
pkill -f "target/debug/aleph-server" 2>/dev/null
sleep 2
cargo run --bin aleph-server -- start &
SERVER_PID=$!
sleep 5
```

Then send a trivial chat request through whichever interface is easiest (CLI, WebChat, or a direct WebSocket call to `agent.run`). After completion:

```bash
# Read the session_events table directly to confirm the turn was recorded
sqlite3 ~/.aleph/data/*.db "SELECT seq, event_type FROM session_events ORDER BY session_id, seq LIMIT 20;"
```

Expected: TurnStarted → UserMessage → LlmCallStarted → AssistantMessage → LlmCallEnded → TurnEnded (or equivalent sequence depending on whether tools were called).

Kill the server: `kill $SERVER_PID`.

- [ ] **Step 11.6: Final commit for write migration**

```bash
git add src/agent_loop/
git commit -m "agent_loop: migrate writes to SessionService (Phase 1 final)

All agent_loop writes now go through SessionService::emit_event.
SessionManager stays in the Gateway RPC path via the dual-write shim.
Agent turns are bracketed by TurnStarted/TurnEnded events."
```

---

## Task 12: Documentation + final verification + release gate

**Files:**
- Create: `docs/reference/SESSION_SERVICE.md`
- Modify: `docs/reference/ARCHITECTURE.md` (add SessionService to system diagram)
- Modify: `docs/reference/GLOSSARY.md` (point "Session" entry to this spec)
- Modify: `src/gateway/session_manager/mod.rs` (docstring marking "Phase 1 compatibility layer")
- Modify: `CHANGELOG.md`

- [ ] **Step 12.1: Write the SessionService reference doc**

```markdown
# SessionService

> Append-only event log per session, with an in-process tokio actor.
> Phase 1 of the [managed-agents refactor](../superpowers/specs/2026-04-18-managed-agents-refactor-roadmap.md).

## Public surface

`src/session/service.rs::SessionService` — async trait with:
- `attach(id) → SessionHandle` — ensure actor is running
- `emit_event(id, event) → EventSeq` — append + sync persist
- `get_events(id, from, to) → Vec<SessionEventRecord>` — read range
- `subscribe(id) → broadcast::Receiver<SessionEventRecord>` — live fan-out
- `wake(id) → SessionHandle` — force-replace actor (crash recovery)
- `detach(id) → ()` — stop actor, keep events

## Implementation

`src/session/in_process.rs::InProcessActorSessionService` spawns one tokio task per session. Each task (`SessionActor`) replays events from SQLite on start, then serves commands until its inbox closes or idle timeout fires (default 30 min).

## Storage

SQLite table `session_events`:
```sql
CREATE TABLE session_events (
    session_id   TEXT    NOT NULL,
    seq          INTEGER NOT NULL,
    turn_id      TEXT,
    event_type   TEXT    NOT NULL,
    payload_json TEXT    NOT NULL,
    created_at   INTEGER NOT NULL,
    PRIMARY KEY (session_id, seq)
);
```

Synchronous writes; SQLite WAL mode; `(session_id, seq)` PK enforces monotonic ordering.

## Event schema

See `src/session/events.rs::SessionEvent` (`#[non_exhaustive]` enum). Variants cover session lifecycle, turn boundaries, messages, LLM interaction, tool calls, subagent delegation, budget/compaction, and errors.

## Gateway RPC relationship

Phase 1 does NOT migrate Gateway `session.*` RPC methods. `SessionManager` remains the public face for those methods; a dual-write shim (`src/session/shim.rs`) mirrors each SessionManager append into SessionService so `session_events` stays populated. Phase 6 removes the shim.

## `wake(session_id)` semantics

1. Shutdown old actor (if any); grace period 5s
2. Spawn fresh actor; it replays all persisted events
3. Write `SessionWoken { prior_head }` event into the log
4. Return new `SessionHandle`

A Harness that crashes mid-turn surfaces as a `TurnStarted` with no matching `TurnEnded` — the replacement Harness decides whether to retry, abandon, or close with an `Error` event.

## Consumer migration status

| Consumer | Status |
|----------|--------|
| `agent_loop` | Reads + writes via SessionService (Phase 1) |
| Gateway `session.*` RPC | Still on `SessionManager` (Phase 6 migrates) |
| Memory / Dream subsystems | Read-only access via `SessionService::get_events` |
```

- [ ] **Step 12.2: Cross-link from ARCHITECTURE.md**

Under the "Agent execution" section (or equivalent), add:

```markdown
### Session Service

Agent execution reads and writes session state exclusively through
[`SessionService`](./SESSION_SERVICE.md). The underlying
`InProcessActorSessionService` spawns one tokio task per session and
persists each event synchronously to the `session_events` table.
Gateway RPC methods continue to use `SessionManager` until Phase 6.
```

- [ ] **Step 12.3: Update GLOSSARY.md**

Find the Session entry and update the "Aleph today" line:
```markdown
**Aleph today:** `SessionService` trait (`src/session/`), backed by an
in-process tokio actor with SQLite persistence. Trait shape permits
cross-process backends later. See [SESSION_SERVICE.md](./SESSION_SERVICE.md).
```

- [ ] **Step 12.4: Mark SessionManager as compatibility layer**

In `src/gateway/session_manager/mod.rs`, add a module-level docstring at the top:

```rust
//! SessionManager — Phase 1 compatibility layer for Gateway `session.*` RPC.
//!
//! Agent execution (`agent_loop`) no longer reads or writes SessionManager
//! directly; it uses `crate::session::SessionService`. Every SessionManager
//! append is mirrored into SessionService via `src/session/shim.rs` so
//! `session_events` remains the authoritative log.
//!
//! Phase 6 migrates Gateway RPC to SessionService directly and removes this
//! layer.
```

- [ ] **Step 12.5: CHANGELOG entry**

Append under `## [Unreleased]`:

```markdown
### Added
- **Session Service:** `src/session/` introduces `SessionService` + `InProcessActorSessionService` — append-only event log per session with one tokio actor per session, SQLite-backed, crash-recoverable via `wake(session_id)`. Phase 1 of the managed-agents refactor. Agent execution now reads/writes sessions through this service; Gateway `session.*` RPC remains on the `SessionManager` compatibility layer until Phase 6.
- **Docs:** `docs/reference/SESSION_SERVICE.md` reference documentation.

### Changed
- `src/agent_loop/**` no longer imports `SessionManager` directly; reads go through `SessionService::get_events` + a local projection helper, writes through `SessionService::emit_event`. Turn boundaries are now explicit `TurnStarted`/`TurnEnded` events.
```

- [ ] **Step 12.6: Final verification gate**

Run:
```bash
echo "=== no SessionManager imports in agent_loop ==="
grep -rn 'use.*session_manager::SessionManager' src/agent_loop/
echo ""
echo "=== no SessionManager calls in agent_loop ==="
grep -rn 'session_manager\.' src/agent_loop/
echo ""
echo "=== session_events table exists ==="
ls migrations/*session_events*
echo ""
echo "=== new module structure ==="
ls src/session/
echo ""
echo "=== test suite ==="
cargo test -p alephcore 2>&1 | tail -10
```

Expected:
- Zero lines from the first two greps
- `session_events` migration file present
- `src/session/` contains `mod.rs events.rs service.rs state.rs store.rs actor.rs in_process.rs shim.rs projection.rs`
- Test result: same pass count as Pre-3 baseline (+ new session-module + integration tests), same 2 pre-existing failures (`telegram::config::tests::parse_v2_config_directly`, `memory::notes::ingest::prompts::tests::base_prompt_snapshot`), no new failures

If any verification fails — do NOT declare Phase 1 done. Fix or revert.

- [ ] **Step 12.7: Clippy**

Run: `cargo clippy -p alephcore -- -D warnings 2>&1 | tail -30`
Expected: no warnings in the new `src/session/` files. Pre-existing warnings elsewhere are not this task's concern.

- [ ] **Step 12.8: Final commits for docs + changelog**

```bash
git add docs/reference/SESSION_SERVICE.md docs/reference/ARCHITECTURE.md docs/reference/GLOSSARY.md
git commit -m "docs: Session Service reference + architecture cross-link"

git add src/gateway/session_manager/mod.rs
git commit -m "session_manager: mark as Phase 1 compatibility layer in docstring"

git add CHANGELOG.md
git commit -m "changelog: note Phase 1 Session Service"
```

- [ ] **Step 12.9: Release gate — STOP**

Phase 1 is code-complete. Do NOT auto-release. Present the option to the user:

> "Phase 1 implementation complete on branch `feat/managed-agents-phase-1`. All commits green, no new test failures beyond the 2 pre-existing on main. Ready to:
>
> 1. **Merge to main** — `git -C /Volumes/TBU4/Workspace/Aleph merge feat/managed-agents-phase-1 --no-ff`
> 2. **Release** — `just release $(date +%Y.%m.%d)` (check CHANGELOG entry first)
> 3. **Both**
> 4. **Start Phase 2 brainstorm** (Tools unified execute() interface)
>
> Which?"

Only proceed on the user's explicit choice.

---

## Non-Goals (Explicitly Out of Scope — repeated from spec)

- Migrating Gateway `session.*` RPC methods to SessionService (Phase 6)
- Cross-process Session daemon (roadmap §12)
- Deleting the `messages` column (it remains the Gateway-read materialized view)
- Snapshot-based wake() optimization (full replay is adequate in v1)
- Changing existing `SessionKey` variants or routing semantics

## Rollback Strategy

If any task's gate fails and the cause isn't obvious within 15 minutes:

```bash
# Find the commit sha that introduced the breakage
git log --oneline main..HEAD

# Revert that single commit (non-destructive; preserves history)
git revert <sha>
```

Never `git reset --hard` on this branch without explicit user consent (per user CLAUDE.md).

## Done-ness Signals

Phase 1 is done when:
1. All 12 tasks checked off
2. `grep -rn 'SessionManager' src/agent_loop/` returns zero hits
3. `wake(session_id)` crash-recovery integration test green
4. All Gateway `session.*` RPC regression tests green
5. `cargo test -p alephcore` shows no new failures vs Pre-3 baseline
6. CHANGELOG entry committed
7. User has made a release/merge/hold decision at Step 12.9

Proceed to **Phase 2 brainstorming** only after all signals are green.
