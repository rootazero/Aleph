# Session Service (Actor) — Phase 1 Design

**Date**: 2026-04-18
**Status**: Design approved — ready for implementation plan
**Parent**: [managed-agents-refactor-roadmap](./2026-04-18-managed-agents-refactor-roadmap.md) §7 Phase 1
**Scope**: Aleph Core — `src/session/` new module, `agent_loop/**` migration

---

## 1. Goal

Introduce an append-only, event-sourced `SessionService` with an in-process actor per session, so Aleph's agent execution achieves Anthropic's **stateless harness + crash-recoverable session** pattern. The trait shape must permit a cross-process daemon backend in a later phase without changing consumer code.

## 2. Non-Goals

- Not replacing `SessionManager` at the Gateway RPC boundary (Phase 6)
- Not introducing a cross-process daemon (roadmap §12 "Not Now")
- Not deleting the `messages` SQLite column (stays as events' materialized view)
- Not optimizing replay performance until a real bottleneck is measured (YAGNI)
- Not changing Gateway `session.*` RPC method semantics

## 3. Decisions Locked (from brainstorming §1–§6)

| Axis | Choice | Rationale |
|------|--------|-----------|
| Event storage | **New `session_events` table alongside existing `messages`** | True append-only; zero UI/RPC churn; strangler-compatible |
| Actor granularity | **One tokio task per session** | Isolation enables wake() semantics; tokio tasks are cheap; aligns with future multi-brain |
| wake() replay | **Full event replay** (no snapshots) | Simplest + deterministic; Aleph session sizes don't warrant snapshot complexity in v1 |
| Persistence timing | **Synchronous: SQLite write before `emit_event` returns** | Durability over throughput; WAL mode keeps it fast |
| Event schema versioning | `#[non_exhaustive]` enum + `#[serde(default)]` on new fields | Additive-only evolution; no user-data migrations |
| Idle timeout | **30 minutes** before auto-detach | Balance between resource use and wake-on-demand latency |
| Unfinished turn recovery | **Harness decides** — SessionService stays neutral | Event log is source of truth; auto-healing would hide bugs |
| Gateway RPC | **Unchanged in Phase 1** — keep SessionManager for `session.*` | Scope discipline; Phase 6 migrates |

## 4. Architecture

```
┌──────────────────────────────────────────────────┐
│  Gateway RPC (session.*)                          │
│  ↓ keeps going to SessionManager (unchanged)      │
└──────────────────────────────────────────────────┘
                      ↕ (Phase 1 leaves this seam intact)

┌──────────────────────────────────────────────────┐
│  agent_loop / Harness consumers                   │
│  ↓ use SessionService trait exclusively           │
└──────────────────────────────────────────────────┘
                      ↓
              ┌─────────────────┐
              │ SessionService  │  trait; async; Result<_, SessionError>
              └────────┬────────┘
                       │
              ┌────────▼────────────────────────────┐
              │ InProcessActorSessionService        │
              │                                     │
              │  HashMap<SessionId, mpsc::Sender>   │  — actor router
              │                                     │
              │  ┌───────────┐  ┌───────────┐       │
              │  │ Actor #1  │  │ Actor #2  │  ...  │  — one per session
              │  └─────┬─────┘  └─────┬─────┘       │
              │        │              │              │
              │        ↓ sync write   ↓              │
              │    session_events (new SQLite table) │
              │                                      │
              │    messages (kept; written via       │
              │    dual-write shim during migration) │
              └──────────────────────────────────────┘
```

**Invariants**:
- `session_events` is the single source of truth; `messages` is a projection
- Every mutation of a session goes through its actor → guarantees event ordering + monotonic `seq`
- Each `emit_event` call fsyncs to SQLite before returning; actor-local state updates after the write
- `wake(session_id)` = shutdown old actor (if any) → spawn new actor → actor replays all events from SQLite → actor emits `SessionWoken { prior_head: old_head }` marker → ready
- Cross-session operations are fully parallel; within-session operations are serialized by the owning actor

## 5. Public API — `SessionService` trait

```rust
#[async_trait::async_trait]
pub trait SessionService: Send + Sync + 'static {
    async fn attach(&self, id: SessionId) -> Result<SessionHandle, SessionError>;

    async fn get_events(
        &self,
        id: &SessionId,
        from: Option<EventSeq>,
        to: Option<EventSeq>,
    ) -> Result<Vec<SessionEventRecord>, SessionError>;

    async fn emit_event(
        &self,
        id: &SessionId,
        event: SessionEvent,
    ) -> Result<EventSeq, SessionError>;

    async fn subscribe(
        &self,
        id: &SessionId,
    ) -> Result<tokio::sync::broadcast::Receiver<SessionEventRecord>, SessionError>;

    async fn wake(&self, id: &SessionId) -> Result<SessionHandle, SessionError>;

    async fn detach(&self, id: &SessionId) -> Result<(), SessionError>;
}

pub struct SessionHandle {
    pub id: SessionId,
    pub head_seq: EventSeq,
}

pub struct SessionEventRecord {
    pub seq: EventSeq,
    pub event: SessionEvent,
    pub created_at_ms: i64,
}

pub type EventSeq = u64;
pub type SessionId = crate::routing::session_key::SessionKey; // reuse existing
pub type TurnId = uuid::Uuid;
```

**Design notes**:
- `attach` is idempotent. Repeated calls return the same logical handle and guarantee an actor is alive.
- `emit_event` returns the assigned `seq` so callers can log/trace without re-reading.
- `subscribe` uses `tokio::sync::broadcast` (multi-consumer). Late subscribers miss historical events; use `get_events` to backfill.
- `wake` is the "something went wrong, force replacement" escape hatch. Normal flow uses `attach`.
- `detach` stops the actor if idle. Does NOT delete events. Session can be woken again later.
- No `update_state` or any direct state-mutation method — **state is only changeable by appending events.**

## 6. `SessionEvent` schema

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[non_exhaustive]
pub enum SessionEvent {
    // Lifecycle
    SessionCreated    { identity: SessionIdentityMeta, at: Timestamp },
    SessionWoken      { at: Timestamp, prior_head: EventSeq },
    SessionDetached   { at: Timestamp },

    // Turn boundaries (one Think→Act iteration)
    TurnStarted       { turn_id: TurnId, trigger: TurnTrigger, at: Timestamp },
    TurnEnded         { turn_id: TurnId, outcome: TurnOutcome, at: Timestamp },

    // Messages (what UI displays)
    UserMessage       { turn_id: TurnId, content: MessageContent, at: Timestamp },
    AssistantMessage  { turn_id: TurnId, content: MessageContent, at: Timestamp },
    SystemMessage     { turn_id: TurnId, content: String, at: Timestamp },

    // LLM interaction
    LlmCallStarted    { turn_id: TurnId, provider: String, model: String, at: Timestamp },
    LlmCallEnded      { turn_id: TurnId, tokens_in: u32, tokens_out: u32, finish_reason: String, at: Timestamp },

    // Tool calls
    ToolCallRequested { turn_id: TurnId, call_id: String, name: String, input: serde_json::Value, at: Timestamp },
    ToolCallApproved  { turn_id: TurnId, call_id: String, by: ApprovalSource, at: Timestamp },
    ToolCallDenied    { turn_id: TurnId, call_id: String, reason: String, at: Timestamp },
    ToolResult        { turn_id: TurnId, call_id: String, output: ToolOutput, at: Timestamp },
    ToolError         { turn_id: TurnId, call_id: String, error: String, at: Timestamp },

    // Subagent / delegation
    SubagentSpawned   { turn_id: TurnId, child_id: SessionId, flow: String, at: Timestamp },
    SubagentReturned  { turn_id: TurnId, child_id: SessionId, summary: String, at: Timestamp },

    // Context / budget / compaction
    BudgetUpdated        { turn_id: TurnId, tokens_used: u32, tokens_budget: u32, at: Timestamp },
    CompactionPerformed  { from_seq: EventSeq, to_seq: EventSeq, summary_ref: String, at: Timestamp },

    // Errors (surfaced to LLM as tool-error events per Anthropic pattern)
    Error { turn_id: Option<TurnId>, kind: ErrorKind, message: String, recoverable: bool, at: Timestamp },
}
```

**Support types** (defined in `src/session/events.rs`):
- `Timestamp` = `i64` unix ms
- `TurnTrigger` = `UserMessage | SubagentRequest | Scheduled | Wake`
- `TurnOutcome` = `Completed | Cancelled | Errored { kind }`
- `MessageContent` = reuse existing `UnifiedMessage` content structure (text + images + tool_use blocks)
- `ApprovalSource` = `User | Trusted | Autoconfirm`
- `ToolOutput` = struct with `value: serde_json::Value`, `metadata: { cost, latency_ms, truncated }`
- `ErrorKind` = `Llm | Tool | Sandbox | Harness | Serialization | Other`

## 7. Storage — SQLite schema

```sql
CREATE TABLE session_events (
    session_id   TEXT    NOT NULL,
    seq          INTEGER NOT NULL,
    turn_id      TEXT,                       -- nullable
    event_type   TEXT    NOT NULL,
    payload_json TEXT    NOT NULL,           -- serde_json::to_string(&event)
    created_at   INTEGER NOT NULL,           -- unix ms
    PRIMARY KEY (session_id, seq)
);

CREATE INDEX idx_events_session_turn ON session_events(session_id, turn_id);
CREATE INDEX idx_events_session_type ON session_events(session_id, event_type);
```

- `seq` is assigned by the owning actor; monotonically increasing per session
- Duplicate `(session_id, seq)` violates PK → unrecoverable inconsistency → panic
- Schema migration ships with Phase 1's first SQLite migration file

## 8. Actor internals

```rust
struct SessionActor {
    id: SessionId,
    store: Arc<dyn SessionEventStore>,
    state: SessionState,
    head_seq: EventSeq,
    inbox: mpsc::Receiver<ActorCommand>,
    broadcaster: broadcast::Sender<SessionEventRecord>,
    idle_deadline: tokio::time::Instant,
}

enum ActorCommand {
    EmitEvent { event: SessionEvent, reply: oneshot::Sender<Result<EventSeq>> },
    GetEvents { from: Option<EventSeq>, to: Option<EventSeq>, reply: oneshot::Sender<Result<Vec<SessionEventRecord>>> },
    Subscribe { reply: oneshot::Sender<broadcast::Receiver<SessionEventRecord>> },
    Shutdown  { reply: oneshot::Sender<()> },
}
```

**Run loop** (pseudocode):
```rust
async fn run(mut self) -> Result<()> {
    // Phase 1: REPLAY
    let past = self.store.load_all_events(&self.id).await?;
    for record in &past {
        self.state.apply(&record.event);
        self.head_seq = record.seq;
    }

    // Phase 2: SERVE
    loop {
        tokio::select! {
            Some(cmd) = self.inbox.recv() => match cmd {
                EmitEvent { event, reply } => {
                    let seq = self.head_seq + 1;
                    let record = SessionEventRecord { seq, event, created_at_ms: now_ms() };
                    if let Err(e) = self.store.append(&self.id, &record).await {
                        let _ = reply.send(Err(e));
                        continue;
                    }
                    self.state.apply(&record.event);
                    self.head_seq = seq;
                    let _ = self.broadcaster.send(record.clone());
                    let _ = reply.send(Ok(seq));
                    self.idle_deadline = Instant::now() + IDLE_TIMEOUT;
                }
                // ... other commands
                Shutdown { reply } => {
                    let _ = reply.send(());
                    return Ok(());
                }
            },
            _ = tokio::time::sleep_until(self.idle_deadline) => {
                // auto-detach
                return Ok(());
            },
        }
    }
}
```

**`SessionState`** (in `src/session/state.rs`):
- Redux-style reducer: `state.apply(event: &SessionEvent)` mutates in place
- Holds: current turn (if any), pending tool calls map, running budget, identity, head summary
- Pure function of the event stream — never side effects during apply
- Not exposed externally; consumers only see events

## 9. `wake(session_id)` protocol

```rust
async fn wake(&self, id: &SessionId) -> Result<SessionHandle> {
    // 1. If actor alive, shut it down cleanly
    if let Some(sender) = self.actors.write().await.remove(id) {
        let (tx, rx) = oneshot::channel();
        let _ = sender.send(ActorCommand::Shutdown { reply: tx });
        let _ = timeout(Duration::from_secs(5), rx).await;
    }

    // 2. Spawn a fresh actor (which will replay)
    let (inbox_tx, inbox_rx) = mpsc::channel(64);
    let (bcast_tx, _) = broadcast::channel(256);
    let actor = SessionActor::new(id.clone(), self.store.clone(), inbox_rx, bcast_tx.clone());
    let head = self.store.load_head_seq(id).await?;
    tokio::spawn(actor.run());

    // 3. Write the SessionWoken marker into the event log
    let (reply_tx, reply_rx) = oneshot::channel();
    let _ = inbox_tx.send(ActorCommand::EmitEvent {
        event: SessionEvent::SessionWoken { at: now_ms(), prior_head: head },
        reply: reply_tx,
    }).await;
    let new_head = reply_rx.await??;

    // 4. Register in the router
    self.actors.write().await.insert(id.clone(), inbox_tx);

    Ok(SessionHandle { id: id.clone(), head_seq: new_head })
}
```

**Harness crash semantics**:
- Harness dies → its `mpsc::Sender` for commands drops → actor's inbox closes → actor exits (but session's SQLite row intact)
- New Harness boots → calls `wake()` → forced replacement → replay → new actor ready
- Unfinished turn surfaces as `TurnStarted` without matching `TurnEnded` → Harness decides: close with `Error` event, retry, or mark abandoned

## 10. Migration Strategy (Strangler)

Each step is independently shippable; tests pass after every step.

### Step 5.1 — Types only
- Add `src/session/` module scaffold: `mod.rs`, `events.rs`, `service.rs`, `state.rs`, `store.rs`, `actor.rs`
- Define `SessionService` trait + all event/command types
- SQLite migration for `session_events` table
- No runtime wiring
- Exit: `cargo check` green, no warnings

### Step 5.2 — Actor implementation
- Implement `InProcessActorSessionService`
- Unit tests cover: attach → emit → get → subscribe → detach → wake
- Exit: all session-module unit tests green

### Step 5.3 — Dual-write shim
- Wherever `SessionManager` appends a message, also call `SessionService::emit_event` with the matching event
- Read paths unchanged (still via `messages`)
- Consistency test: send 10 messages via SessionManager, assert `session_events` has 10 corresponding records
- Exit: dual-write consistency tests green

### Step 5.4 — agent_loop read migration
- Identify every `agent_loop/**` read of SessionManager; switch to `SessionService::get_events` + local projection helper
- Per-site commit + `cargo test` gate
- Exit: no `SessionManager` read imports in `agent_loop/**`

### Step 5.5 — agent_loop write migration
- Switch `agent_loop/**` writes from `SessionManager::append_message` etc. to `SessionService::emit_event`
- Gateway RPC side unchanged (still goes to SessionManager; dual-write shim keeps both stores in sync)
- Exit: no `SessionManager::append_*` calls in `agent_loop/**`; `grep` verified

### Step 5.6 — wake() integration test
- Spawn session, emit 10 events, abort actor task, call `wake()`, verify new actor returns correct `head_seq` and can continue emitting
- Exit: crash-recovery integration test green

### Step 5.7 — Documentation
- New `docs/reference/SESSION_SERVICE.md`
- Update `docs/reference/ARCHITECTURE.md` to reference SessionService for agent_loop
- Mark `SessionManager` as "Phase 1 compatibility layer" in its module docstring

**Exit gate for Phase 1**:
- `grep -rn 'SessionManager' src/agent_loop/` → zero hits
- `wake(session_id)` crash-recovery integration test green
- All existing `session.*` Gateway RPC method tests pass
- Dual-write consistency test green
- `just test-all` overall green (minus the 2 pre-existing failures inherited from main)

## 11. Testing Strategy

### Unit tests (`src/session/*` `#[cfg(test)]`)

| Area | Cases |
|------|-------|
| Event store | append/load ordering; duplicate `(session_id, seq)` rejected; concurrent append from two sessions |
| Actor replay | Emit N events → kill actor → new actor replay → state-equal assertion |
| Actor concurrency | Two sessions' actors emit 1000 events each → no blocking → head_seq correct per session |
| Wake | `wake()` writes `SessionWoken { prior_head }` with correct prior seq |
| Subscribe | Multiple subscribers all receive fan-out; late subscriber misses history (by design) |
| Idle timeout | Last command + 31 min → actor auto-detach → next attach respawns actor |
| Error events | Emit `Error` → subsequent `get_events` finds it → emit still works after error |
| Schema back-compat | Deserialize older JSON missing a `#[serde(default)]` field succeeds |

### Integration tests (`tests/session_service_integration.rs`)

1. **Crash + wake round-trip**: emit 5 → `task::abort()` → `wake()` → 6th event has seq=6 (plus 7 for the `SessionWoken` marker)
2. **Dual-write consistency**: 5 messages via SessionManager → 5 matching records in `session_events`
3. **Agent turn event shape**: full turn through agent_loop emits the expected event sequence

### Performance baselines (not required to pass, but recorded)

- Single-session append 10k events: < 5s (SQLite WAL mode)
- wake() full replay of 10k events: < 1s
- 1000 idle sessions resident memory: < 100 MB

### Regression gates

- All existing `session.*` Gateway RPC tests continue to pass
- All existing `agent_loop` integration tests continue to pass
- Read results of the `messages` table unchanged before vs after migration

## 12. Risks & Mitigations

| Risk | Likelihood | Impact | Mitigation |
|------|-----------|--------|------------|
| Replay non-determinism (e.g., timestamp embedded in state) | Medium | High | `SessionState::apply` is pure; integration test asserts state equality pre-kill vs post-wake |
| Dual-write divergence (bug writes one side, not the other) | Medium | Medium | Consistency test runs in CI; dual-write is a thin helper, hard to bypass |
| SQLite contention under high write rate | Low | Medium | WAL mode + connection pool + per-session serialization. If issue, Phase-later optimization |
| `#[non_exhaustive]` forces exhaustive match break elsewhere | Low | Low | All internal matches use `_ => {}` guard; trait default method in apply |
| Idle timeout too aggressive (actor churn) | Low | Low | 30 min configurable via `AppConfig.session.idle_timeout_minutes` |
| Subscribe `broadcast` channel lag errors crash consumers | Medium | Low | Consumers call `.resubscribe()` + get_events to backfill; documented pattern |
| Event enum grows too large | Low | Low | Stay disciplined; composite payloads go in nested structs, not new top-level variants |

## 13. Open Questions (deferred to implementation, not blocking)

- Exact SQL for loading head_seq efficiently (probably `SELECT MAX(seq) FROM session_events WHERE session_id = ?`)
- Whether `SessionState` should live in `src/session/state.rs` or `src/session/reducer.rs` (naming — decide during impl)
- Whether to use `sqlx` compile-time checked queries or runtime queries (follow existing project convention)
- Whether `CompactionPerformed` needs a payload for the summary or just a reference (`summary_ref` as string URL/id — finalize during impl)
- Whether `TurnId` collisions across restarted sessions matter (use UUID v7 for time-ordering + uniqueness)

## 14. Success Metrics

- `agent_loop` has zero direct `SessionManager` imports after Phase 1
- `wake()` crash-recovery test passes in CI
- Zero regression on Gateway `session.*` RPC behavior
- New `src/session/` module has ≥ 80% line coverage
- Daily release cadence uninterrupted throughout Phase 1

## 15. Next Action

1. User reviews this spec
2. On approval → invoke `writing-plans` skill to produce a task-level implementation plan
3. Implementation executes via subagent-driven-development (matching Phase 0 pattern)
