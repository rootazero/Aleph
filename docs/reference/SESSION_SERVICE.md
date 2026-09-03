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

Plus two supporting indexes (`idx_session_events_session_turn`, `idx_session_events_session_type`). Writes are synchronous and SQLite runs in WAL mode. The `(session_id, seq)` primary key enforces **uniqueness** of a seq within a session — it does not enforce monotonicity, and no index could: a primary key accepts 5, 3, 4 in that insertion order without complaint. Monotonicity is the allocator's promise, and the reducer does not take it on faith: `session::reduction::validate_slice` REJECTS a slice whose `seq` decreases (`LogContradiction::OutOfOrderSlice`), because a reducer that proceeded would derive the run anchor and the disposition from a false order.

## Event schema

See `src/session/events.rs::SessionEvent` (`#[non_exhaustive]` enum). Variants cover session lifecycle, turn boundaries, messages, LLM interaction, tool calls, subagent delegation, budget/compaction, and errors. Helper types: `EventSeq`, `TurnId`, `MessageContent`, `ToolOutput`, `TurnTrigger`, `ApprovalSource`, `ErrorKind`, `Timestamp`.

Two rules keep this list honest, both learned the same way:

- **A variant with no producer is a claim the enum cannot honour** (`ApprovalSource::Autoconfirm`'s own doc). `TurnOutcome` and `SessionEvent::TurnEnded` were removed on those grounds — nothing ever constructed them in any build, so nothing could read one back, while the sentence below documented a crash-recovery contract on top of them for as long as they existed.
- `ErrorKind` is therefore **not open vocabulary**: it carries only kinds that a producer actually emits (today `Guardrail`, from the input-block receipt). A new kind lands in the same commit as the code that writes it.

A read-side projection helper `src/session/projection.rs::project_row` turns one event into at most one message-shaped row (`ProjectedRow`), for consumers that want a classic message-history view rather than raw events. It is what `MessageProjector` materialises into the `messages` table, so an event that projects to `None` is an event `chat.history` will never serve.

## Soft retirement (`retired_at`)

Events are never deleted. The `retired_at` column takes them out of the **live conversation** — every live reader (`load_all_events`, `load_events_range`, `load_run_markers`, `search_events`) filters `retired_at IS NULL` — while the rows, and therefore seq allocation, stay intact. Two mirrored primitives, with one deliberate asymmetry:

| | range | BM25 mirror (`session_events_fts`) | driven by |
|---|---|---|---|
| `retire_from(from_seq)` | `seq >= from_seq` (tail) | **deleted**, in the same transaction | `chat.clear` / `chat.rewind` |
| `retire_through(through_seq)` | `seq <= through_seq` (head) | **kept** | manual `/compact` (`context::compact::manual`) |

The asymmetry is the point. Clearing is erasure: leaving the content searchable would let `recall_events` hand the model the very turns the user just wiped. Compaction is *relocation*: the turns leave the live prompt but must stay recallable, which is what makes "compaction is not a net loss" true. Regression: `store.rs::retire_through_keeps_the_search_index_unlike_clear`.

`retire_through` has no `Ok(0)` default — a store that cannot retire must say so, because its caller has already appended the summary and needs to report "summary recorded, context unchanged" rather than a compaction that silently did nothing.

## `wake(session_id)` semantics

1. Shut down the old actor (if any); grace period 5s.
2. Spawn a fresh actor; it replays all persisted events from SQLite.
3. Write a `SessionWoken { prior_head }` event into the log.
4. Return a new `SessionHandle`.

Crash recovery reads the **run** markers, not the turn markers: a run that never finished surfaces as one or more trailing `RunStarted` events with no `RunFinished` after the last of them, which `session::reduction::reduce_disposition` counts (the count doubles as the crash-loop attempt counter). `ResumeCoordinator` then repairs the boundary — a synthetic `ToolError` per dangling tool call — and re-triggers each surviving candidate.

There is deliberately no `TurnEnded` marker to pair with `TurnStarted`; a turn ends when the next one opens or when the run does. An earlier version of this section described the crash predicate in terms of that pair, which no code ever emitted, so every turn matched "crashed mid-turn" — and cited an integration test (`tests/session_wake_recovery.rs`) that does not exist. Both are gone.

## Gateway RPC relationship

Gateway `session.*` RPC methods remain on `SessionManager` (`src/gateway/session_manager/`). **There is no dual-write shim.** This section described `src/session/shim.rs` — a file that does not exist and whose mirroring was removed when `session_events` became the SSOT — so anyone reading it went looking for a mirror of the `messages` table. There is no mirror: `MessageProjector` (`src/gateway/session_projector.rs`) materialises `messages` from `session_events` asynchronously and is the only writer of the rows it projects — the ones carrying a `source_seq`. It is **not** the table's only writer, and every doc that said so (this section included) was lying: two production paths append straight to `messages` and leave `source_seq` NULL — `AgentInstance::add_message` (`src/gateway/agent_instance.rs`) and the boot orphan notice (`src/gateway/orphan_notice.rs`) — the 「另两个生产者」 FEATURE_LOCATOR §6.9 names. `map_message_row` (`src/gateway/session_manager/ops/crud.rs`) reads a NULL `source_seq` back as "not event-sourced, leave it alone", which is what keeps those rows out of the projection's seq-set arithmetic.

The projection is **self-healing rather than lossy**. Back-pressure or a stopped drain records the event's `seq` (payload stays in the SSOT) and the next heal pass re-reads it from the log; a heal is a seq-set difference against the transcript's own row ids, so a hole below the newest row is filled, not only a missing tail. `missed` is process memory, so a crash between an append and its drain is repaired at the NEXT boot: `ProjectionReconciler` asks the projector to repair every session in the activity window (`[resume] max_age_secs`) plus every session whose markers read as interrupted, and the `core/projection-holes` doctor check does the unbounded sweep for anything older.

## Re-attaching a client (`chat.history`'s `session` snapshot)

A conversation's **durable settings** do not live in the event log — they live
on the session row and in `SessionMetadata.identity_meta.custom`:

| fact | where |
|---|---|
| `session_mode` / `exec_tier` / `think_level` / `memory_mode` / `model_pin` / `project_root` | `identity_meta.custom[…]`, written by the `turn_*` resolvers and `sessions.patch` |
| `model` / `model_provider` / `input_tokens` / `output_tokens` / `total_tokens` / `estimated_cost_usd` | the `sessions` row, accumulated per run by `session_projector` on `AssistantRunMeta` |

All of it has been durable for a long time; until 2026-08-11 **none of it was
readable by a client attaching to a session by key**. `sessions.list` decoded
three of the knobs inline (and not `think_level`), and `chat.history` decoded
none — so a client that reopened a conversation painted the *install* defaults
over one the run loop was still governing by its own stored values.

`chat.history` now carries a `session` object typed by
`aleph_protocol::SessionSnapshot`, built by the single decoder
`gateway::session_snapshot::snapshot_from_metadata` that `sessions.list` also
uses. It rides on this response, not a new method, for the reason the handler's
own doc already gives for `active_run` and `plan`: they are **one snapshot**, a
second call opens a window in which a client holds the transcript but not the
settings that govern it, and the authorization is free (the handler has already
resolved the metadata and passed `visibility::session_visible`).

Contract rules for anything added to that snapshot:

- **`None` means "follow the global default", never "off".** The server resolves
  globals per turn from live config; baking today's value into a snapshot would
  go stale while still looking authoritative.
- **Add the decode and the `sessions.patch` validation in the same change.**
  Two source-derived census tests enforce it
  (`session_snapshot.rs::no_session_knob_constant_is_left_unread`,
  `modify.rs::every_session_knob_is_validated_on_patch`).
- **A field with no client renderer is the defect, not a head start.**

See [FEATURE_LOCATOR §5.23](FEATURE_LOCATOR.md).

## Consumer migration status

| Consumer | Status |
|----------|--------|
| `AgentHarness` | Reads and writes history exclusively through `SessionService` (Phase 6 completed). |
| `agents::runtime` (SubagentTool) | Harness-based subagent spawning uses `SessionService` for ephemeral child sessions (Phase 7 completed). |
| Gateway `session.*` RPC | On `SessionManager`. **No dual write** — see the section above: `session_events` is the SSOT and `MessageProjector` is the only writer of the rows `messages` projects from it (`source_seq` non-NULL). The two direct appenders named there leave `source_seq` NULL, so `messages` as a *table* has three production writers. The `src/session/shim.rs` this row used to name never survived that change. |
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
