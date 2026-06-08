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

Gateway `session.*` RPC methods remain on `SessionManager` (`src/gateway/session_manager/`). A dual-write shim (`src/session/shim.rs`) mirrors each `SessionManager` append into `SessionService` so `session_events` stays populated in parallel with the legacy `messages` table. A future phase will migrate Gateway RPC directly and remove the shim.

## Consumer migration status

| Consumer | Status |
|----------|--------|
| `AgentHarness` | Reads and writes history exclusively through `SessionService` (Phase 6 completed). |
| `agents::runtime` (SubagentTool) | Harness-based subagent spawning uses `SessionService` for ephemeral child sessions (Phase 7 completed). |
| Gateway `session.*` RPC | On `SessionManager`; every append dual-writes into `SessionService` via `src/session/shim.rs`. Future phase will migrate Gateway RPC directly and remove the shim. |
| Memory / Dream / other | Read-only `SessionService::get_events` available; adoption on a case-by-case basis. |

## Non-goals

- Migrating Gateway `session.*` RPC methods (future phase).
- Cross-process Session daemon.
- Deleting the legacy `messages` column (it remains the Gateway-read materialized view).
- Snapshot-based `wake()` optimization — full replay is adequate in v1.
- Changing `SessionKey` variants or routing semantics.

## DM Scope —— 单用户「一脑多端」连续上下文

`[session] dm_scope` 控制 DM 如何映射到会话：

| 值 | 语义 |
|---|---|
| `per-peer`（默认） | 每个发送者独立会话（跨 channel 按 peer） |
| `per-channel-peer` | 每个 channel × 发送者独立会话（多用户推荐） |
| `main` | 所有 DM 坍缩到该 agent 的 `Main` 会话 |

**单用户 owner**（只有你本人会 DM 这个 bot，由 allowlist/pairing 保证）建议设：

```toml
[session]
dm_scope = "main"
```

效果：你在 Telegram / Slack / WebChat Panel 等各 channel 与**同一 agent** 的 DM 共享同一段
`agent:<id>:main` 上下文——agent 记得你在任意 channel 说过的话；打开 Panel 即见完整历史。
绑定到**不同 agent** 的 channel 各自 `agent:<id>:main` 隔离（工作 / 个人不串味）。回复仍只回到
你发问的那个 channel（不向其他 channel 推送）。

**注意事项**
- **多用户警示**：若 owner 之外还有人被 allowlist 也能 DM，`main` 会把所有人并进同一会话。
  多用户请用 `per-channel-peer`（owner 专属 Main 的判定本期未实现）。
- **迁移断点**：从 `per-peer` 切到 `main` 后，旧的 `agent:<id>:dm:<peer>` 会话停在原地（不迁移），
  新消息走 `agent:<id>:main`，会有一次性上下文断点。
- 群组消息不受影响，始终按 `Group` 会话隔离。
