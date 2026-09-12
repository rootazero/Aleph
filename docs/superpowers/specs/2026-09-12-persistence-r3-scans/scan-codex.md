# codex persistence / recovery scan — `T:\Github\codex\codex-rs`

Scope note: every anchor is `file:line` under `codex-rs/`. Graphify was used to orient; conclusions come from reading the sources it pointed at. Nothing was built or run.

## 1. The durable log (rollout)

- **Format**: JSONL, one `RolloutLine { timestamp, ordinal: Option<u64>, #[flatten] item: RolloutItem }` per line (`history/src/lib.rs:266-271`); wire tag is `{"type": "<snake_case>", "payload": …}` via `RolloutItemWire` (`history/src/rollout_payload.rs:22-61`). `RolloutLine` deliberately does **not** derive `Deserialize` — readers must go through `codex_rollout::parse_rollout_line` (`rollout/src/lib.rs:75`) so nested decimals survive the flattened envelope.
- **Location**: `~/.codex/sessions/YYYY/MM/DD/rollout-<ts>-<thread_id>[_<rollout_id>].jsonl` (`rollout/src/recorder.rs:1707-1719`), `archived_sessions/` sibling (`rollout/src/lib.rs:84-85`). The `_<rollout_id>` suffix exists only after `thread/revert` (`recorder.rs:98-105`).
- **Header**: first line is `SessionMeta` (+ optional `git`) — `SessionMetaLine` (`protocol/src/protocol.rs:3167-3171`); `SessionMeta` carries id/session_id, `forked_from_id`, `parent_thread_id`, cwd, cli_version, `base_instructions`, `dynamic_tools`, `history_mode` (Legacy|Paginated), `history_base: Option<HistoryPosition>` (zero-copy fork pointer), `subagent_history_start_ordinal`, `context_window` (`protocol.rs:3065-3131`). Git info is collected async at write time (`recorder.rs:1946-1965`).
- **Writer**: one tokio task per recorder owning the `tokio::fs::File`, fed by a bounded `mpsc<RolloutCmd>(256)` with `AddItems / Persist / Flush / Shutdown / Discard` (`recorder.rs:126-142`, `1909-1943`). `AddItems` writes immediately if the file is materialized (`flush_if_materialized`, `1768-1775`); **deferred creation** — a new rollout file is not created until the first `Persist` (`RolloutWriterState.deferred_creation`, `1751-1761`), so empty threads leave no file.
- **Flush semantics**: `flush()` = `tokio::fs::File::flush` after `write_all` (`recorder.rs:2067-2074`) — that is a page-cache write, **no fsync**. `sync_all` appears only in compression/migration publish paths (`compression.rs:105,112,673`; `rollout_migration/publish.rs:136-257`). Write failure → `enter_recovery_mode` drops the handle, keeps `pending_items`, retries once on the next command (`1794-1850`).
- **Ordinals**: Paginated mode stamps a monotonically increasing `ordinal` per record, seeded from `history_base.end_ordinal_exclusive` or the last valid record found by a reverse scan on reopen (`rollout/src/ordinal.rs:23-100`). A subagent whose inherited prefix was cut short refuses to append (`ordinal.rs:84-94`).
- **Writer lock**: cross-process per-thread `flock` files under `~/.codex/thread-writer-locks/<thread_id>.lock`, all creation/removal serialized by `.coordination.lock`; stale locks are swept once per coordinator (`rollout/src/writer_lock.rs:16-17, 43-87, 124-170`). The writer **task**, not the caller, owns the guard (`recorder.rs:144-147`).
- **Rotation/archival**: no size-based rotation. Cold rollouts are `.jsonl.zst`-compressed by a fire-and-forget worker (`compression.rs:29-31`), transparently decompressed for read (`45-57`) and re-materialized for append (`79-…`). Archive = move to `archived_sessions/` + SQLite `archived_at` (`thread-store/src/local/archive_thread.rs`).
- **Corruption**: torn tail is repaired by appending a newline on reopen (`recorder.rs:2020-2033`), so the partial JSON becomes one rejected line; readers skip unparseable lines and count them (`load_rollout_items`, `recorder.rs:1083-1108`); the reverse scanner tolerates oversized/garbage records (`reverse_jsonl_scanner.rs:75-133`). Unknown `history_mode` in the **first** SessionMeta is fatal (`recorder.rs:1096-1099`).
- **Versioning**: no schema version field; forward-compat via `#[serde(default)]` everywhere plus a legacy→paginated migration state machine with a durable `.pending` journal per thread (`thread-store/src/local/rollout_migration.rs:1-9`; `rollout_migration/publish.rs:1-9,17-23`) and SQLite `rollout_migration_state` (`state/migrations/0047_…`).

## 2. Event vocabulary

- `RolloutItem` variants (`history/src/lib.rs:101-116`): `SessionMeta`, `ResponseItem(+CodexHarnessMetadata)`, `InterAgentCommunication(+Metadata)`, `Compacted`, `TurnContext`, `TokenUsageRecord`, `WorldState`, `SecurityRiskScore`, `RetainedContext`, `EventMsg`, `RealtimeItem`.
- **Facts vs derived**: `ResponseItem` and `InterAgentCommunication` are the model-visible facts; `TurnContext` / `SessionMeta` / `ThreadSettingsApplied` are knob facts; `Compacted` is a **materialized derived checkpoint** written back into the log (it carries `replacement_history`, `retained_context`, `guardian_history`, window ids and a token-usage snapshot so resume need not scan past it — `history/src/lib.rs:189-206`); `WorldState` is an explicit snapshot+merge-patch chain (`protocol.rs:3205-3209`); `EventMsg` subset is UI-derivation input only.
- Persistence policy is one function: `is_persisted_rollout_item` / `should_persist_response_item` / `should_persist_event_msg` (`rollout/src/policy.rs:10-206`).
- Persisted `EventMsg`: `TokenCount`, `ThreadGoalUpdated`, `ThreadRolledBack`, `TurnAborted`, `TurnStarted`, `TurnComplete`, `ThreadSettingsApplied` always; `ItemCompleted` always in Paginated; the legacy `UserMessage/AgentMessage/AgentReasoning/McpToolCallEnd/…` only in Legacy mode (`policy.rs:99-133`).
- **Deliberately NOT persisted** (`policy.rs:137-205`): all deltas, `ExecCommandBegin/End`, `ExecCommandOutputDelta`, `McpToolCallBegin`, `ExecApprovalRequest`, `ApplyPatchApprovalRequest`, `RequestUserInput`, `ElicitationRequest`, `SessionConfigured`, `McpStartupUpdate/Complete`, `Error`, `StreamError`, `TurnDiff`, `PatchApplyBegin`, all `Collab*Begin/End`, `HookStarted/Completed`. Rationale in-file: "Transient, non-durable" — the durable fact for a tool is the `FunctionCall`/`FunctionCallOutput` `ResponseItem`, not the exec lifecycle.
- **Approvals: NOT FOUND in the log.** Neither the request nor the decision is persisted; per-session "approve for session" caches are in-memory only.

## 3. Fork / branch / resume

- `InitialHistory::{New, Cleared, Resumed(ResumedHistory), Forked(Vec<RolloutItem>)}` (`history/src/lib.rs:280-285`). Resume keeps the same thread id and appends to the same file; fork creates a new thread with `forked_from_id` + `forked_from_ordinal_exclusive`.
- Two fork persistence modes (`core/src/session/mod.rs:1558-1580`): `ForkPersistence::Referenced` — the child file starts with `history_base: HistoryPosition{rollout_id, end_ordinal_exclusive, end_byte_offset}` and does **not** copy the parent (`protocol.rs:3037-3051`); readers follow the pointer chain via `RolloutLineage` (`thread-store/src/local/rollout_lineage.rs:15-30`). `Copied` — parent prefix is re-appended into the child.
- `thread/revert` = new immutable rollout file for the same thread id; the only mutable cut-over is the SQLite `rollout_path` pointer, CAS'd against the expected path (`thread-store/src/local/revert_thread.rs:15-60`).
- `thread/rollback` = append `ThreadRolledBack{num_turns}` and re-reduce (`protocol.rs:3630-3633`; reduction at `rollout_reconstruction.rs:208-211,398-400`).
- What a resumed turn restores: `TurnContextItem` (cwd, approval_policy, sandbox/permission profile, model, effort, collaboration mode, plugins — `protocol.rs:3232-3289`) via `PreviousTurnSettings` + `reference_context_item` (`rollout_reconstruction.rs:240-262`), token usage from the last `TokenCount`/`TokenUsageRecord` (`session/mod.rs:1519-1526`), effective settings from the last `ThreadSettingsApplied` **owned by this thread id** (`thread_processor.rs:3743-3755`). A model mismatch on resume is a warning, not a block (`mod.rs:1499-1517`).
- Resume of an already-running thread is serialized through the thread listener: `ThreadListenerCommand::SendThreadResumeResponse` is executed inside the same `select!` loop that forwards events, so "send history, then subscribe" is atomic w.r.t. the event stream (`app-server/src/thread_state.rs:57-63`; `thread_processor.rs:4437-4464`; `thread_lifecycle.rs:279-302`).
- Daemon crash handoff: loaded root thread ids are snapshotted to a recovery file and replayed through the normal cold-resume path on restart (`app-server/src/daemon_thread_recovery.rs:11-51`).
- `resume_agent` is a tool (`core/src/tools/handlers/multi_agents/resume_agent.rs`).

## 4. Reduction to run state

- Reducer = `Session::reconstruct_history_from_rollout` (`core/src/session/rollout_reconstruction.rs:135-462`). Two-pass: **reverse scan** newest→oldest, grouping items into turn segments bounded by `TurnStarted`, stopping once it has (a) the newest surviving `Compacted` with `replacement_history`, (b) previous turn settings, (c) a reference `TurnContext` (`rollout_reconstruction.rs:326-334`); then a **forward replay** of only the surviving suffix into `ContextManager` (`339-407`). Rollback markers skip N user-turn segments during reverse scan (`77-88`).
- `WorldState` is rebuilt by replaying full snapshot + merge patches chronologically, reset at each `Compacted` (`427-452`).
- **Dangling calls**: `normalize_history` runs on every history mutation: `ensure_call_outputs_present` inserts a synthetic `FunctionCallOutput("aborted")` with a deterministic UUID-v5 id (so prompt caches stay stable) for any `FunctionCall`/`CustomToolCall`/`LocalShellCall`/`ToolSearchCall` without output; `remove_orphan_outputs` drops outputs without a call (`context_manager/normalize.rs:18-120`; `history.rs:707-717`).
- **Interrupted turns**: a real interrupt writes a model-visible marker (`interrupted_turn_history_marker`, `tasks/mod.rs:98-118`) and flushes it **before** emitting `TurnAborted` because clients re-read the file on abort (`tasks/mod.rs:~937-940`). A **crash** (TurnStarted, no end) leaves no marker; resume only sets `AgentStatus::Interrupted` if the last status event was already `TurnAborted` (`session/mod.rs:1486-1494`, `agent/status.rs:6-24`) — otherwise the run state starts idle with a synthetic "aborted" output only.
- Partial streaming output: deltas are never persisted; an `OutputItemDone` is recorded only when complete (`stream_events_utils.rs:308-345`), so a mid-stream crash loses the partial item entirely.

## 5. Reduction to model context

- What is sent = `ContextManager` contents after reconstruction + normalization; base instructions come from `SessionMeta.base_instructions` (`history/src/lib.rs:348-359`).
- Compaction writes a `Compacted` item containing the **complete replacement history** plus window lineage (`window_number`, `first/previous/window_id` UUIDv7) and `latest_token_usage_record`; followed by a full `WorldState`, the frozen `TurnContext`, and a fresh `ThreadSettingsApplied` — all in one append under a settings persistence lock (`core/src/session/mod.rs:3943-4021`). Legacy `Compacted` without `replacement_history` is rebuilt from user messages + summary (`rollout_reconstruction.rs:371-397`).
- Ghost snapshots: the old `ghost_snapshot` `ResponseItem` is **stripped on read** (`recorder.rs:1091-1093, 1216-1276`); today "undo" is a git-side feature (`GhostSnapshotConfig`, `core/src/config/mod.rs:220`) and is not a rollout item.
- Truncation by user-turn boundary honours `ThreadRolledBack` (`core/src/thread_rollout_truncation.rs:39-65`).
- Paginated threads resume from a bounded reverse scan across the lineage that stops at the newest checkpoint (`thread-store/src/local/model_context.rs:27-77`), instead of loading the whole file.

## 6. Reduction to UI

- Thread listing: SQLite `threads` table first (`recorder.rs:399-430`), filesystem head-scan fallback that reads only the first `HEAD_RECORD_LIMIT` lines for meta + preview + first user message (`rollout/src/list.rs:793-826, 1120-1235`), with read-repair back into SQLite (`rollout/src/state_db.rs:519-604`). SQLite is derived: `apply_rollout_item` is the metadata reducer (`state/src/extract.rs:15-36`), and only `SessionMeta`/`TurnContext`/a few events can mutate it (`extract.rs:39-63`).
- Transcript projection = `ThreadHistoryBuilder` in the protocol crate (`app-server-protocol/src/protocol/thread_history.rs:85-91, 380-430`), producing `Turn{status: InProgress|Completed|Failed|Interrupted}`; a `TurnStarted` with no terminal event stays `InProgress` in the pure projection (`1253-1260, 1357-1364`) and is reclassified to `Interrupted` only at the read layer when the thread is not live (`thread_processor.rs:5751-5766`).
- Paginated mode additionally materializes the projection into SQLite incrementally with a `(next_byte_offset, next_ordinal)` cursor (`thread-store/src/local/thread_history_materialization.rs:22-62`), always **after** the JSONL flush barrier ("SQLite is a rebuildable view … can lag JSONL … can never get ahead", `live_writer.rs:343-354`).
- The TUI is now an app-server client: it renders from `ThreadResumeResponse.thread.turns` and a replay module that rehydrates items without live side effects (`tui/src/chatwidget/replay.rs:1-6`); the legacy `SessionConfigured.initial_messages` path still exists (`protocol.rs:3943-3944`, `session/session.rs:1661`).

## 7. MCP / plugins / skills

- MCP connection state, tool lists, startup events: **not persisted**; `McpStartupUpdate/Complete`, `McpToolCallBegin` are transient (`policy.rs:172-197`). Tool catalogs are a process-scoped LRU with 30-min TTL (`codex-mcp/src/tool_catalog_cache.rs:31-38`).
- OAuth tokens live outside the rollout in a keyring/file credential store with refresh locks and transactions (`rmcp-client/src/oauth/{credential_store,refresh_lock,refresh_transaction,store_lock}.rs`). Only `McpResourceOriginCheckpoint` (widget-read provenance) is snapshotted into `Compacted` (`protocol/src/mcp.rs:55-62`).
- Restart mid-MCP-call: the `FunctionCall` is on disk, the output is not → synthetic "aborted" output on resume (§4). No re-invocation.
- Skills: filesystem + in-memory snapshot cache keyed by root (`skills/src/loading.rs:53-73`); a watcher notifies clients (`app-server/src/skills_watcher.rs`). Nothing in the rollout except `TurnContextItem.disabled_plugin_ids` / selected capability roots in `SessionMeta`.

## 8. Subagents / collab

- Children are ordinary threads with `SessionSource::SubAgent(ThreadSpawn{parent_thread_id})`, `SessionMeta.parent_thread_id`, and (Paginated) `history_base` + `subagent_history_start_ordinal` so inherited parent context is referenced, not copied (`protocol.rs:3120-3126`).
- Parent↔child edges are persisted in SQLite `thread_spawn_edges(parent, child PK, status Open|Closed)` (`state/migrations/0021_thread_spawn_edges.sql`), written at spawn when not ephemeral (`core/src/agent/control.rs:870-900`).
- Recovery: subagents run **in-process**, so a parent crash kills them. On parent resume, V1 BFS-resumes every `Open` child (`agent/control/spawn.rs:1160-1185`); V2 restores only metadata and reloads a child lazily through its loaded parent (`spawn.rs:174-215`; `thread_manager.rs:1141-1168`; the app-server refuses direct resume of a V2 child, `thread_processor.rs:3700-3740`).
- `InterAgentCommunication` items are persisted and count as user-turn boundaries in reduction (`rollout_reconstruction.rs:301-305`).

## 9. Recoverable side-effects

- Intent-before-execution: yes at the model-item level — `record_completed_response_item` is awaited (recorder `AddItems` + `flush` ack, page cache) **before** the tool future is spawned (`stream_events_utils.rs:309-345`; `live_writer.rs:361-370`). Not fsync'd.
- `ExecCommandBegin/End`, PTY/unified-exec sessions, `TerminalInteraction`: **not persisted**; `unified_exec/process_manager.rs` is in-memory only. There is no classification of "began but no end" other than the missing `FunctionCallOutput`; no idempotency keys for tool calls (the only idempotency key in the tree is project creation, `thread-store/src/local/projects.rs:70-77`).
- Approvals: NOT FOUND (see §2).

## 10. Slash commands

- Command set in `tui/src/slash_command.rs:15-67`. None is logged as a command; only their effects are: `/compact` → `Compacted` (+WorldState+TurnContext+ThreadSettingsApplied); `/model`, `/permissions`, `/personality`, `/cd` → `ThreadSettingsApplied{thread_id, ThreadSettingsSnapshot{model, provider, approval_policy, permission_profile, cwd, effort, collaboration_mode, disabled_plugin_ids}}` (`protocol.rs:2191-2230`) and the next turn's `TurnContext`; `/new` → new rollout; `/fork` → fork; `/resume` → listing+resume; `/archive`, `/delete`, `/rename`, `/title` → SQLite metadata (+ `append_rollout_item_to_path` for unloaded threads, `recorder.rs:1983-1993`); `/goal` → `ThreadGoalUpdated`. `/status`, `/diff`, `/mcp`, `/usage` are pure reads.

## 11. Durability primitives

- No generic WAL `intent→done` record type. The only two-phase machinery is the migration `.pending` journal + atomic publish with parent-dir `sync_all` (`rollout_migration/publish.rs`), and the migration rollback plan (`rollout_migration/rollback*.rs`).
- SQLite (`codex-state`): WAL mode, `synchronous=NORMAL`, 5 s busy timeout, bundled SQLite pinned ≥3.51.3 for the WAL-reset fix (`state/src/sqlite.rs:302-305`; `state/src/lib.rs:7-10`); 55 numbered migrations. Holds threads metadata, spawn edges, durable queued user submissions (`state/src/model/queued_item.rs`), projects, thread attachments, remote-control enrollments.
- `~/.codex/history.jsonl` cross-session composer history: `O_APPEND` + `try_lock`, byte-capped trim (`message-history/src/lib.rs:3-14, 52-55, 148-171`).
- Locks: per-thread writer `flock` + coordination lock (§1); in-process `live_writer_locks` lifecycle locks (`revert_thread.rs:31-34`).

---

## Top 8 patterns worth porting to another Rust core

1. **One persistence-policy function** as the single truth of "what is a fact" (`policy.rs`) — every writer filters through `is_persisted_rollout_item`, so transient events can never leak into the log.
2. **Materialized checkpoint in the log** (`Compacted.replacement_history` + window lineage + `latest_token_usage_record`) so resume is a bounded reverse scan, not a full replay.
3. **Reverse-scan reducer with turn segmentation and early exit** (`rollout_reconstruction.rs`) — and the pattern of "rollback = skip N segments during reverse replay".
4. **Deterministic synthetic outputs for dangling calls** (`normalize.rs`, UUID-v5 namespace) — keeps prompt caches stable across crash recovery.
5. **Zero-copy fork by `HistoryPosition{rollout_id, end_ordinal, end_byte_offset}` + `RolloutLineage`** — the only place that dereferences fork pointers; revert = new immutable file + CAS on one SQLite pointer.
6. **SQLite as a strictly-lagging projection** with a `(byte_offset, ordinal)` cursor and a read-repair path from the JSONL (`live_writer.rs:343-354`, `state_db.rs:519-604`).
7. **Writer task owns the cross-process lock**, torn-tail newline repair on reopen, bounded writer channel + retry-with-reopen instead of panicking.
8. **Atomic "history + subscribe" via a listener-command channel** in the same `select!` as event forwarding (`SendThreadResumeResponse`), plus a daemon recovery file consumed-then-deleted on boot.

## Top 5 things codex does WRONG or leaves unrecovered

1. **No fsync on the rollout** — `flush()` is a page-cache flush (`recorder.rs:2072`); an OS crash can lose the `FunctionCall` that a side-effecting tool already executed.
2. **Tool side-effects have no begin/end facts** — `ExecCommandBegin/End`, PTY sessions and approvals are transient; a restart cannot tell "never ran" from "ran and lost the output", and simply tells the model "aborted".
3. **A mid-turn crash leaves no `TurnAborted` / interruption marker** in the log; run state comes back idle, and only the read-layer view rewrites `InProgress→Interrupted` (`thread_processor.rs:5762`) — the model never learns the turn died.
4. **Partial streamed items are lost entirely** (deltas not persisted, `OutputItemDone` is the first durable point), so long assistant messages/reasoning vanish on crash.
5. **SQLite dual-write with best-effort projection**: `materialize_to_sqlite` failures are `warn!`-and-continue (`live_writer.rs:346-354`); consistency depends on later read-repair, and `reconcile_rollout` refuses to touch a row whose `rollout_path` differs (`state_db.rs:565-571`), so a stale pointer after a failed revert publish is silently kept.

---

**What was not done**: no code was run or compiled; Windows-specific lock semantics were not verified beyond the `try_lock` calls; `rollout-trace/` and the `external-agent-migration` importer were not audited; the "no fsync" claim is based on grep across `rollout/`, `thread-store/`, `state/`, `core/` (excluding tests) — a `sync_all` hidden behind a helper in another crate would be missed.
