# SessionService

> Append-only event log per session, with an in-process tokio actor.
> Phase 1 of the [managed-agents refactor](../superpowers/specs/2026-04-18-managed-agents-refactor-roadmap.md).

## Public surface

`src/session/service.rs::SessionService` — async trait with:
- `attach(id) → SessionHandle` — ensure actor is running; returns current head seq
- `emit_event(id, event) → EventSeq` — append + sync persist; returns the new seq
- `emit_batch(id, events, retire: Option<Retire>) → Vec<EventSeq>` — append `events` and apply `retire` in ONE store transaction (round-3, T2). A **provided** method whose default is `Err("this SessionService cannot append atomically")`, not a loop of `emit_event`: a service that cannot commit atomically must say so rather than report a batch that can tear. `InProcessActorSessionService` is the only production override (`ActorCommand::EmitBatch`); test doubles override it too. See "Batches, retirement and durability" below.
- `get_events(id, from, to) → Vec<SessionEventRecord>` — half-open read range. Fails with `SessionError::UndecodableRecord` when a live row of the session cannot be decoded by this build (see "Per-row decode").
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

Plus a `retired_at INTEGER` column added by migration (`ALTER TABLE … ADD COLUMN` — the schema only ever adds columns and never gates on `user_version`: a version gate refuses the whole file for every session at once, while an added column reads as NULL on rows written before it; pinned by `store.rs::the_event_table_migration_only_ever_adds_columns`), and two supporting indexes (`idx_session_events_session_turn`, `idx_session_events_session_type`). Writes are synchronous and SQLite runs in WAL mode at `PRAGMA synchronous=NORMAL`; a batch that carries a Barrier-class event commits under `synchronous=FULL` (see "Batches, retirement and durability").

`payload_json` is `store::encode_row`'s output — the serde object of the `SessionEvent` plus a row envelope: `"v"` (`SESSION_EVENT_SCHEMA_VERSION`, 1) on every row, and `"ignorable": true` only when `events::ignorable` says so for that variant (nothing declares it today; the key is absent otherwise, never a literal `false`). The envelope is what lets an OLDER build skip a row a newer build marked skippable instead of refusing the session — see "Per-row decode". The `(session_id, seq)` primary key enforces **uniqueness** of a seq within a session — it does not enforce monotonicity, and no index could: a primary key accepts 5, 3, 4 in that insertion order without complaint. Monotonicity is the allocator's promise, and the reducer does not take it on faith: `session::reduction::validate_slice` REJECTS a slice whose `seq` decreases (`LogContradiction::OutOfOrderSlice`), because a reducer that proceeded would derive the run anchor and the disposition from a false order.

## Event schema

See `src/session/events.rs::SessionEvent` (`#[non_exhaustive]` enum). Variants cover session lifecycle, turn boundaries, messages, LLM interaction, tool calls, subagent delegation, budget/compaction, and errors. Helper types: `EventSeq`, `TurnId`, `MessageContent`, `ToolOutput`, `TurnTrigger`, `ApprovalSource`, `ErrorKind`, `Timestamp`.

Two rules keep this list honest, both learned the same way:

- **A variant with no producer is a claim the enum cannot honour** (`ApprovalSource::Autoconfirm`'s own doc). `TurnOutcome` and `SessionEvent::TurnEnded` were removed on those grounds — nothing ever constructed them in any build, so nothing could read one back, while the sentence below documented a crash-recovery contract on top of them for as long as they existed.
- `ErrorKind` is therefore **not open vocabulary**: it carries only kinds that a producer actually emits — `Guardrail` (the input-block receipt) and, since round-3 (T9), `HookStop` (a pre-seed lifecycle hook refused or stopped the run: `run_loop::hook_stop_receipt` writes it inside a closed run bracket, so the log says "a run happened and the hook stopped it" and never reads `Unanswered`; the set of writing seams is derived by `hook_stop_tests::every_pre_seed_hook_exit_journals_the_stop`, not listed). A new kind lands in the same commit as the code that writes it.

A read-side projection helper `src/session/projection.rs::project_row` turns one event into at most one message-shaped row (`ProjectedRow`), for consumers that want a classic message-history view rather than raw events. It is what `MessageProjector` materialises into the `messages` table, so an event that projects to `None` is an event `chat.history` will never serve.

## Batches, retirement and durability (round-3, T1–T4)

**One write path.** `SessionEventStore::append_batch(session, first_seq, events, retire: Option<Retire>, durability)` appends `events` at consecutive seqs and applies `retire` in the SAME transaction; `append` is a batch of one. An empty `events` with no `retire` is refused (a "batch" that could do nothing would report success for nothing); an empty `events` WITH a `retire` is a legal retire-only batch. `retire` runs BEFORE the inserts so the batch's own rows stay live. Inside the store, `retire_in_txn` takes the `rusqlite::Transaction` itself, not a `Connection` — "inside the transaction" is a type invariant, and a caller that wanted to retire in autocommit mode has no value of that type to hand over. Pinned by `store.rs::retire_and_insert_are_one_transaction` (a retire committed after a successful batch would retire the batch's own seq; that is the red). The previous shape — manual `/compact` as three independent calls, `/undo` as two, session split as seven — is why this exists: a crash between the steps left a log that was internally consistent and false (summary present AND the compacted turns still live; tail retired AND its `RunStarted` still open, so every later boot re-ran a deleted turn). Full text: [FEATURE_LOCATOR 附录 D.4.49](FEATURE_LOCATOR.md).

**Soft retirement (`retired_at`).** Events are never deleted. The `retired_at` column takes them out of the **live conversation** — every live reader (`load_all_events`, `load_events_range`, `load_run_markers`, `search_events`) filters `retired_at IS NULL` — while the rows, and therefore seq allocation, stay intact. `Retire` has two arms with one deliberate asymmetry:

| `Retire` | range (inclusive) | BM25 mirror (`session_events_fts`) | driven by |
|---|---|---|---|
| `Retire::From(seq)` | `seq >= n` (tail) | **deleted**, in the same transaction | Two drivers, two paths. **Balanced** (a cut that can leave a `RunStarted` open): `chat.rewind` (`handlers/chat.rs:833`) and `session.truncate` (`db_handlers/modify.rs:747`) → `handlers::retire_events_and_balance` (`handlers/mod.rs:171`) → `session::marker_balance::retire_from_and_close_run`, which appends the `RunFinished { outcome: Cancelled }` closer the cut would otherwise leave owed **in the same batch** (`open_run_after_retire` is the pure builder over the surviving prefix; its `is_running` predicate fails CLOSED — "I do not know whether a run is live" retires but does not close). **Standalone** (the cut starts at seq 1, so no prefix survives and nothing is owed): `chat.clear` (`handlers/chat.rs:738`), `session.reset` (`modify.rs:122`) and `session.delete` (`modify.rs:306`) → `session::store::retire_live_events(key, 1)` (`store.rs:1288`) → the trait's standalone `retire_from`, no closer, no `is_running` gate. Line numbers measured at `dded43c7f`. |
| `Retire::Through(seq)` | `seq <= n` (head) | **kept** | manual `/compact` (`context::compact::manual::compact_session`: summary row + checkpoint row + `Retire::Through(cut)` in one batch). Reached ONLY as the `retire` argument of `append_batch` — the old `retire_through` trait method was CUT in T3 (zero production callers after the batch landed). |

The asymmetry is the point. Clearing is erasure: leaving the content searchable would let `recall_events` hand the model the very turns the user just wiped. Compaction is *relocation*: the turns leave the live prompt but must stay recallable, which is what makes "compaction is not a net loss" true. Regression: `store.rs::retire_through_keeps_the_search_index_unlike_clear`. A third, single-row primitive, `retire_record(session, seq) -> Result<bool>`, is the doctor's `fix=true` exit for an undecodable row (idempotent; `Ok(false)` merges "already retired" with "no such row"; the FTS mirror row is left alone — that record is one this build cannot read). Its default, like `load_rows`', is a refusal: a store that cannot expose or retire raw rows must say so rather than answer "no rows".

**Durability policy (U3).** `events::durability_of(&SessionEvent) -> Durability::{Normal, Barrier}` is THE table — one `const fn`, no wildcard arm, so a new variant does not compile until it states its column. Barrier = exactly `ToolCallRequested` / `RunStarted` / `ResumeAttempted` / `UserMessage`, pinned by a source-derived equality census in `events.rs` (T1, `a4fac37bd`). A batch is as durable as its most durable member (`batch_durability`); a Barrier batch commits under `PRAGMA synchronous=FULL` for that one transaction (`with_synchronous_full`), and the connection is put back to `NORMAL` after commit AND after rollback. `ToolCallParked` is deliberately Normal: losing it reads as "outcome unknown", the safe direction, and does not buy an fsync. The harness never chooses durability — it calls `emit_event` / `emit_batch` and the policy lives in the store (R10).

**Session split is the one multi-step operation that cannot be one transaction** (`context::compact::session_split::perform_session_split`, T4): the child's epoch is written to the routing table on the gateway `SessionStore`'s own connection, so it rides neither batch. The order IS the crash contract — parent `[RunFinished{Completed}]` first, then the child `[SessionForked, summary, tail…, RunStarted{parent's envelope}]`, then `register_epoch` / `retire_superseded` — and the torn state (log says split, routing still names the parent) is healed at the next boot by `ProjectionReconciler::heal_split_epochs`, which runs BEFORE the resume pass; a child whose marker slice this build cannot decode is skipped by the heal, not guessed at. If the parent's open `RunStarted` is not in the log the split is REFUSED (`SplitError::NoOpenRun`, compact-to-fit instead): a child opener claiming an empty envelope would be a lie.

## `wake(session_id)` semantics

1. Shut down the old actor (if any); grace period 5s.
2. Spawn a fresh actor; it replays all persisted events from SQLite.
3. Write a `SessionWoken { prior_head }` event into the log.
4. Return a new `SessionHandle`.

Crash recovery reads the **run** markers, not the turn markers: a run that never finished surfaces as a `RunStarted` after the last `RunFinished`, which `session::reduction::reduce_disposition` reads as `Interrupted { attempts }`. Since round-3 (T6) the crash-loop counter is NOT the number of trailing `RunStarted`s but the number of `ResumeAttempted { target, attempt }` stamps in that same tail — the intent stamp `ResumeCoordinator` writes (Barrier durability) BEFORE each re-trigger, so a resume that dies before its own `RunStarted` still counts; a stamp that does not land refuses the resume without re-triggering. (The wire field on `LastRunState` keeps its historical spelling `trailing_starts`; its value is the attempt count and no client renders it.) A second disposition, `Unanswered { user_seq, attempts }` (T7), names a real `UserMessage` after the last `RunFinished` with no `RunStarted` and no `AssistantMessage` after it — the crash landed between the seed and the run's own marker; markers alone cannot show it, so the coordinator reads the message tail of every clean candidate and of every marker-less session in the activity window, then stamps and re-triggers it with no boundary repair. `ResumeCoordinator` otherwise repairs the boundary — a synthetic `ToolError` per dangling tool call, in four arms (`denied` / parked-at-a-gate / this restart / an earlier run — `session::boundary_repair`) — and re-triggers each surviving candidate, `[resume] max_concurrent` (default 2) at a time. Full text: [FEATURE_LOCATOR §4.13a](FEATURE_LOCATOR.md) ⑱ ⑲ ㉑ ㉔.

There is deliberately no `TurnEnded` marker to pair with `TurnStarted`; a turn ends when the next one opens or when the run does. An earlier version of this section described the crash predicate in terms of that pair, which no code ever emitted, so every turn matched "crashed mid-turn" — and cited an integration test (`tests/session_wake_recovery.rs`) that does not exist. Both are gone.

## Per-row decode and `UndecodableRecord` (round-3, T14)

Every row is decoded on its own by `store::decode_row(seq, created_at_ms, json) -> DecodedRow::{Event, Skipped{seq, kind_tag}, Undecodable(UndecodableRecord{seq, kind_tag, error})}`. `Skipped` is reached only when the `type` tag ITSELF is unknown to this build AND the row carries `"ignorable": true` (the skip guard keys on serde's own `unknown variant `<tag>`` wording via `names_unknown_outer_variant`); a known `type` whose body will not parse is corruption, never skippable. Two readings sit on top of that:

- **Model-facing readers fold strictly.** `fold_strict` drops `Skipped` rows (debug-traced) and returns the FIRST `Undecodable` as `Err`, so `load_all_events` / `load_events_range` / `get_events` refuse the whole session — the reducer never sees a record it cannot read and derives nothing from a false order. `load_run_markers` is decoded per session: `Vec<(SessionId, MarkerSlice)>` with `MarkerSlice = Result<Vec<SessionEventRecord>, UndecodableRecord>`, so one bad row refuses ITS session and leaves every other session's slice whole (the outer `Err` is the query itself failing). Before T14 the cross-session marker query failed as a whole and no session was resumed at boot.
- **The doctor reads rows as values.** `load_rows` hands every row over as a `DecodedRow`, so `core/session-log` can name each undecodable record by seq and type (uncapped — they are exactly the set `fix=true` touches) and `fix=true` retires exactly those via `retire_record`. Contradictions stay report-only; this is the one mechanical repair the check has.

On the contradiction face the refusal is `LogContradiction::UndecodableRecord { seq }` (`session-log-undecodable-record`), the third REJECT kind, lifted by `reduce_marker_slice`; the resume coordinator logs ONE refusal under that tag and leaves the session exactly as found; the attach face reads `log_inconsistent` with that tag; the harness refuses the session naming the record. Known limit (FOLLOW-UP): `/undo` on a session holding an undecodable record fails at `retire_from_and_close_run` → `get_events` → `fold_strict`; the doctor is the only exit. Real-machine stage: `qa/resume_boundary/run.sh undecodable` — note it guards the SHARED `fold_strict` (a lossy fold in `load_run_markers` alone is caught by `handle_interrupted`'s second read); the per-session `Result` is pinned by `store.rs::one_bad_row_refuses_only_its_own_session` and `projection_reconciler.rs::a_child_whose_marker_slice_did_not_decode_gets_no_epoch_heal_and_is_counted`.

## Gateway RPC relationship

Gateway `session.*` RPC methods remain on `SessionManager` (`src/gateway/session_manager/`). **There is no dual-write shim.** This section described `src/session/shim.rs` — a file that does not exist and whose mirroring was removed when `session_events` became the SSOT — so anyone reading it went looking for a mirror of the `messages` table. There is no mirror: `MessageProjector` (`src/gateway/session_projector.rs`) materialises `messages` from `session_events` asynchronously, and since 2026-09-13 it is the table's **only production writer** — pinned to the tree by `session_projector::tests::the_projector_is_the_only_production_writer_of_the_messages_table` (equality on the set of files whose production code calls `.append_message(`, the store's own tree excluded). Until that day this sentence was a lie every doc repeated: two production paths appended straight to `messages` with `source_seq` NULL — `AgentInstance::add_message` (`src/gateway/agent_instance.rs`, zero production callers, deleted by T17) and the boot orphan notice (`src/gateway/orphan_notice.rs`, folded into the resume arm by T16) — the 「另两个生产者」 FEATURE_LOCATOR §6.9 named. Rows with a NULL `source_seq` still exist in old databases (legacy transcripts, past orphan notices); `map_message_row` (`src/gateway/session_manager/ops/crud.rs`) reads them as "not event-sourced, leave it alone", which keeps them out of the projection's seq-set arithmetic.

The projection is **self-healing rather than lossy**. Back-pressure or a stopped drain records the event's `seq` (payload stays in the SSOT) and the next heal pass re-reads it from the log; a heal is a seq-set difference against the transcript's own row ids, so a hole below the newest row is filled, not only a missing tail. `missed` is process memory, so a crash between an append and its drain is repaired at the NEXT boot: `ProjectionReconciler` asks the projector to repair every session in the activity window (`[resume] max_age_secs`) plus every session whose markers read as interrupted, and the `core/projection-holes` doctor check does the unbounded sweep for anything older.

## Re-attaching a client (`chat.history`'s `session` snapshot)

A conversation's **durable settings** do not live in the event log — they live
on the session row and in `SessionMetadata.identity_meta.custom`:

| fact | where |
|---|---|
| `session_mode` / `exec_tier` / `think_level` / `memory_mode` / `model_pin` / `project_root` | `identity_meta.custom[…]`, written by the `turn_*` resolvers and `sessions.patch` |
| `model` / `model_provider` / `input_tokens` / `output_tokens` / `total_tokens` / `estimated_cost_usd` | the `sessions` row, accumulated per run by `session_projector::bill_run_from_fold` when the run's `AssistantRunMeta` lands — tokens from `session::usage_fold::run_usage_totals` over the run's `AssistantMessage.usage` (the meta carries NO token counters since T5a), dollars from the meta's `cost_usd` — or, for a finished run whose meta never landed, by the whole-session heal that synthesizes the stamp at boot (tokens only, never dollars). The row is a face of the fold, not a second derivation; the row's dollars and its tokens therefore count different populations until per-call cost rides the message (Phase 2 F). |

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
| Gateway `session.*` RPC | On `SessionManager`. **No dual write** — see the section above: `session_events` is the SSOT and `MessageProjector` is the ONLY production writer of `messages` (since 2026-09-13, T16/T17: the two direct appenders that once left `source_seq` NULL are deleted; rows with a NULL `source_seq` in old databases are legacy and left alone). The `src/session/shim.rs` this row used to name never survived that change. |
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
