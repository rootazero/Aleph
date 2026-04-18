# SessionService

> Append-only event log per session, with an in-process tokio actor.
> Phase 1 of the [managed-agents refactor](../superpowers/specs/2026-04-18-managed-agents-refactor-roadmap.md).

## Public surface

`src/session/service.rs::SessionService` — async trait with:
- `attach(id) → SessionHandle` — ensure actor is running; returns current head seq
- `emit_event(id, event) → EventSeq` — append + sync persist; returns the new seq
- `get_events(id, from, to) → Vec<SessionEventRecord>` — half-open read range
- `subscribe(id) → broadcast::Receiver<SessionEventRecord>` — live fan-out
- `wake(id) → SessionHandle` — force-replace actor (crash recovery)
- `detach(id) → ()` — stop actor, keep events

`SessionId` is an alias for `crate::routing::session_key::SessionKey` — sessions are identified by the same key used everywhere else in the gateway.

## Implementation

`src/session/in_process.rs::InProcessActorSessionService` spawns one tokio task per session. Each task (`SessionActor`, defined in `src/session/actor.rs`) replays events from SQLite on start, then serves commands until its inbox closes or an idle timeout fires (default 30 min). Per-actor state lives in `src/session/state.rs`; event types live in `src/session/events.rs`.

## Storage

SQLite table `session_events` (created by `migrate_add_session_events` in `src/session/store.rs`):

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

Plus two supporting indexes (`idx_session_events_session_turn`, `idx_session_events_session_type`). Writes are synchronous; SQLite runs in WAL mode; the `(session_id, seq)` primary key enforces monotonic ordering per session.

## Event schema

See `src/session/events.rs::SessionEvent` (`#[non_exhaustive]` enum). Variants cover session lifecycle, turn boundaries, messages, LLM interaction, tool calls, subagent delegation, budget/compaction, and errors. Helper types: `EventSeq`, `TurnId`, `MessageContent`, `ToolOutput`, `TurnOutcome`, `TurnTrigger`, `ApprovalSource`, `ErrorKind`, `Timestamp`.

A read-side projection helper `src/session/projection.rs::project_messages` turns an event range into `Vec<ProjectedMessage>` for consumers that want a classic message-history view rather than raw events.

## `wake(session_id)` semantics

1. Shut down the old actor (if any); grace period 5s.
2. Spawn a fresh actor; it replays all persisted events from SQLite.
3. Write a `SessionWoken { prior_head }` event into the log.
4. Return a new `SessionHandle`.

A Harness that crashed mid-turn will surface as a `TurnStarted` with no matching `TurnEnded` — the replacement Harness decides whether to retry, abandon, or close the turn with an `Error` event. The crash-recovery path has an integration test (`tests/session_wake_recovery.rs`).

## Gateway RPC relationship

Phase 1 does **not** migrate Gateway `session.*` RPC methods. `SessionManager` (`src/gateway/session_manager/`) remains the public face for those methods. A dual-write shim (`src/session/shim.rs`) mirrors each `SessionManager` append into `SessionService` so `session_events` stays populated in parallel with the legacy `messages` table. Phase 6 removes the shim and points Gateway RPC at `SessionService` directly.

## Consumer migration status

| Consumer | Status |
|----------|--------|
| `agent_loop` | No direct use of `SessionManager` *or* `SessionService` yet. Phase 1 verified `agent_loop/**` was already decoupled from `SessionManager` — no imports, no calls. Phase 4 rewrites the loop on top of `SessionService`. |
| Gateway `session.*` RPC | On `SessionManager`; every append dual-writes into `SessionService` via `src/session/shim.rs`. Phase 6 migrates Gateway RPC directly and removes the shim. |
| Memory / Dream / other | Read-only `SessionService::get_events` available; adoption on a case-by-case basis. |

## Non-goals (Phase 1)

- Migrating Gateway `session.*` RPC methods (Phase 6).
- Cross-process Session daemon.
- Deleting the legacy `messages` column (it remains the Gateway-read materialized view).
- Snapshot-based `wake()` optimization — full replay is adequate in v1.
- Changing `SessionKey` variants or routing semantics.
