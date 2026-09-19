# Aleph persistence / crash-recovery census (read-only scan, 2026-09-12, main @ 5e85060b8)

Scope: durable facts -> derived run state -> derived model context -> derived UI -> re-reduction after crash.
Method: graphify orientation per area, then source read. Anchors are `file:line` on this checkout.

## 0. Verdict

`session_events` (SQLite, `<data_dir>/sessions.db`, `journal_mode=WAL`, `synchronous=NORMAL`, `busy_timeout=5000` — `src/utils/sqlite_open.rs:12-30`) is a genuine write-ahead SSOT: every `SessionService::emit_event` awaits the INSERT before the caller proceeds (`src/session/in_process.rs:326-345` -> `src/session/actor.rs:161-184`, reply sent only after `store.append` returns `Ok`). A `kill -9` therefore loses nothing that was acknowledged to the harness (WAL + NORMAL is crash-safe against process death; only power loss can drop the last commits).

The weak spots are not the log. They are:
(a) facts written AFTER their side-effect (`AssistantRunMeta`, fast-path slash rows, spend ledger, `SubagentSpawned`);
(b) multi-step durable sequences with no boot completion (manual `/compact`, session-split);
(c) a second `messages` writer whose verdict contradicts the resume arm (`orphan_notice`);
(d) the resume crash-loop ratchet living on the *re-triggered run's own* marker;
(e) per-row strict event decode with no schema version, whose blast radius is the whole boot resume scan.

---

## 1. Session event log

**Variants** (`src/session/events.rs:230-449`, `#[non_exhaustive]`, serde `tag="type"`), 18 total:

| Variant | Line | Fact or derived | Producer |
|---|---|---|---|
| `SessionWoken {at, prior_head}` | 231 | derived/bookkeeping | actor spawn `in_process.rs:178` |
| `RunStarted {run_id, at, project_root?, envelope?}` | 237 | fact (+knob snapshot) | `harness_bridge/runner_impl.rs:978`; split child `session_split.rs:122` |
| `RunFinished {run_id, outcome, at}` | 257 | fact | bridge; split parent; `abandon`/`delegated` closers; `marker_balance` |
| `TurnStarted {turn_id, trigger, at}` | 269 | fact (no `TurnEnded` by design) | `session_seed.rs`; `subagent_spawner/mod.rs:675` |
| `UserMessage {turn_id, content, at, synthetic, author_user_id?}` | 274 | fact | seed; steering; fast path; subagent child |
| `AssistantMessage {turn_id, content, usage?, at}` | 308 | fact (1:1 with LLM call) | harness think |
| `AssistantRunMeta {turn_id(fresh uuid), run_id, context_*, tokens, cost_usd?, model?, at}` | 332 | **denormalized** — run_id + occupancy + session spend counters | `execution_engine/execute.rs:966-999` AFTER the run |
| `SystemMessage {turn_id, content, at}` | 365 | fact (summary carrier, degrade note) | `manual.rs:455`; split summary; `announce_degrade` |
| `ToolCallRequested {turn_id, call_id, name, input, at}` | 371 | fact | `harness/agent/act.rs:484` |
| `ToolCallApproved {turn_id, call_id, by, at}` | 378 | fact | `tools/scoped/dispatch.rs:1191` |
| `ToolCallDenied {turn_id, call_id, reason, at}` | 384 | fact | `dispatch.rs:1185` |
| `ToolResult {turn_id, call_id, output, at}` | 390 | fact | act.rs |
| `ToolError {turn_id, call_id, error, at}` | 396 | fact (+ synthetic repairs) | act.rs; `boundary_repair.rs` |
| `SubagentSpawned {turn_id, child_id, flow, at}` | 403 | fact | `subagent_spawner/mod.rs:773` (after permit) |
| `SubagentReturned {turn_id, child_id, summary, at}` | 409 | fact | `subagent_spawner/mod.rs:1062` |
| `CompactionPerformed {from_seq, to_seq, summary_ref, at}` | 416 | derived checkpoint | `context/compact/manual.rs:470` only |
| `SessionForked {parent_session_id, at}` | 426 | fact | `session_split.rs:79` |
| `Error {turn_id?, kind: Guardrail, message, recoverable, at}` | 441 | fact (receipt) | harness_bridge guardrail block |

**Append durability** (`src/session/store.rs`):
- `append` = one auto-commit INSERT into `session_events` + best-effort INSERT into `session_events_fts` (301-350); FTS failure never blocks.
- `retire_from` = `BEGIN IMMEDIATE`, UPDATE `retired_at`, DELETE FTS rows, COMMIT (430-480) — idempotent via `retired_at IS NULL`.
- `retire_through` = single UPDATE, keeps FTS (483-520).
- All reads filter `retired_at IS NULL` (378, 546). `load_run_markers` is one cross-session query ordered `session_id, seq` (537-590).
- Actor self-heals a `(session_id, seq)` UNIQUE collision once by resyncing `head_seq` (`actor.rs:166-180`, drain arm 239-258).
- Production construction: dedicated connection to `SessionManagerConfig::default().db_path` — the same `sessions.db` the SQLite `SessionManager` uses, **also when the SessionStore backend is `file`** (`src/bin/aleph-server/commands/start/helpers.rs:336-372`; `start/mod.rs:426-452`). Two connections to one DB in the SQLite-backend case.

**Schema / migration**: `migrate_add_session_events` under `SAVEPOINT` (182-224), idempotent; `add_retired_at_column` is "add column if missing" (232-245). There is **no `PRAGMA user_version`, no event-schema version**, no per-row tolerance: rows decode with `serde_json::from_str(&payload)?` (396, 569). New fields are `serde(default)`/`skip_serializing_if` so old logs stay readable; the reverse (older binary reading a newer variant) fails the whole `load_*` call. Because `load_run_markers` spans all sessions, one undecodable marker row anywhere => `resume scan failed; skipping resume` (`src/gateway/resume_coordinator.rs:686-692`) and the reconciler's marker-union candidate set is lost (`projection_reconciler.rs:142+`).

**Corruption handling**: `LogContradiction` closed set of 9 (`src/session/reduction.rs:54-107`) — 2 REJECT (`OutOfOrderSlice`, `NonMarkerInMarkerSlice`), 7 REPORT with a corrected reading (`UnmarkedActivity`, `FinishWithoutStart`, `DuplicateDispatch`, `ReceiptWithoutDispatch`, `DuplicateReceipt`, `DanglingDeniedCall`, `ClockAnomaly`). `ClockAnomaly` => neither resume nor abandon (`resume_coordinator.rs:966-976`, `skipped_unknown_age`). Doctor: `core/session-log` report-only, `core/projection-holes` repairable (`src/diagnostics/checks/{session_log,projection_holes}.rs`).

**File backend**: `transcript.jsonl` append + `flush()` (no fsync — fine for process death) (`src/gateway/session_store/file_backend/mod.rs:405-432`); `metadata.json` tmp+fsync+rename under per-key lock (`meta.rs:18, 64`). Boot repairs: `sweep_archive_events`, `normalize_session_dir_names`, `repair_session_metadata`, legacy SQLite->file export (`helpers.rs:232-305`).

**Lost on kill -9**: nothing acknowledged. In-flight: the one event being appended (caller never got `Ok`), the projector's in-memory `MissedSeqs`, actor broadcast subscribers.

---

## 2. Run lifecycle & tool calls

Order inside one bridge run (`src/orchestrator/harness_bridge/runner_impl.rs::run` starts 105):
1. resilience `tasks` row `running` (`execution_engine/execute.rs:481` -> `persistence.rs:11 persist_run_task_started`, `insert_agent_task_if_absent`) — before anything else
2. `seed_session`: `TurnStarted` + `UserMessage` (342)
3. prompt build, memory recall, hooks (343-977) — can take seconds
4. `RunStarted {envelope}` (978) — one construction, pinned by a test at 1763-1790
5. `harness.run` (1000)
6. `RunFinished` (~1111 region)
7. `AssistantRunMeta` (`execute.rs:966-999`)

- **`ToolCallRequested` is awaited before execute** (`src/harness/agent/act.rs:484-491`, doc 97-99); `ToolResult`/`ToolError` after (556+). Non-crash terminations close unexecuted calls themselves (`close_unexecuted_tool_uses`, deferred `ToolResult` 287-340).
- **`ToolCallApproved`/`Denied`** written after the gate decision, before execute (`src/tools/scoped/dispatch.rs:1160-1200`). Silently skipped when the global session service is absent (1159-1170) or no ambient `CallIdentity` (1173-1180).
- **Pending approvals**: `ExecApprovalManager.pending: Arc<RwLock<HashMap>>` (`src/exec/manager.rs:300, 359-390`) — memory only. **Pending clarifications** (`ask_user`, scratchpad plan-approval): `ClarificationManager.pending` HashMap (`src/clarification/session.rs:256`) — memory only.
- Crash while parked: the call has `ToolCallRequested`, no `Approved`, no receipt => dangling => `boundary_repair_text` says "OUTCOME UNKNOWN ... may have completed" (`src/session/boundary_repair.rs:97-121`). The log cannot distinguish "parked at a gate, never ran" from "ungated, mid-execution" — only `ToolCallDenied` gets the honest "did not run" arm (`denied` flag, `reduction.rs:229-233`).
- Long background tools: bash jobs journaled at spawn (`src/builtin_tools/process_journal.rs:415 init_and_reconcile`, `525 init_and_announce`; **no pid recorded**, doc 34-48); sub-agents sidecar (`src/agents/background_persistence.rs:543 record_start` BEFORE the task is spawned, `585 record_settled`, `571 record_activity` trail).
- `subagent{wait}` park is not itself recoverable — it is a dangling `ToolCallRequested`; the resumed parent must re-ask and `subagent_tool/recovery.rs` answers from the parent log (`SubagentSpawned/Returned`) merged with the sidecar (`Recovered::{Completed, Interrupted, Sidecar}`; module doc 1-40: "does NOT restart anything").

---

## 3. Compaction / model context derivation

- **The prompt is built from events, never from `messages`**: `src/harness/agent/think.rs:326 get_events(session_id, None, None)` -> `src/harness/agent/prompt.rs:29 build_prompt` (walks `SessionEventRecord`s; drops orphan `tool_use`). `agent.rs:430/626/830` do seq-ranged reads for the tail. Retirement (`retired_at`) is the only thing that shrinks it.
- **Manual `/compact`** (`src/context/compact/manual.rs:1-60` doc; body 430-520): re-read span-intact check -> `SystemMessage`(summary, 455) -> `CompactionPerformed`(470) -> `retire_through`(482) -> `cache_monitor.notify_compaction`(499). Three durable writes, no boot completion. `CompactionPerformed` has one producer and its fields are read only by `manual.rs` tests (1051-1063) and the `fork.rs:241` filter.
- **Auto in-place compaction** (`CompactAndContinue`/`CompactToFit`): mutates the in-flight `messages: Vec<UnifiedMessage>` only (`src/context/compact/directive.rs:95-228`, `fit.rs`); `CompactStrategy::CacheReuse` is per-run process memory (`compactor.rs:25-40`); `SessionMemoryReuse` reads the memory DB (durable). After restart the next Think re-reads the full live log and re-summarizes — cost, not correctness; on a log past the window the first turn goes through the reactive rescue (`rescue.rs:195/263`).
- **Session-split** (`src/context/compact/session_split.rs:50-135`): 1 mint child `epoch+1` -> 2 summarize (LLM) -> 3 `register_epoch` (only impl is `SessionManager`, `sqlite_backend/mod.rs:665`; `None` on file backend, `start/mod.rs:385-395`) -> 3b `retire_superseded` (btw side session) -> 4 child `SessionForked`, `SystemMessage`, verbatim tail -> 5 parent `RunFinished`, child `RunStarted` (116-135). On file backend the `SplitSession` directive degrades to compact-to-fit (`directive.rs:207-226`).
- **Prefix cache**: manual compaction tells the watchdog the break was deliberate (`manual.rs:499-506`). The transient tail (cwd, project CLAUDE.md/AGENTS.md, `UserPromptSubmit` hook output) is merged into a transient recall message every Think and never persisted (`runner_impl.rs:563-577`); a resumed run has `input: ""` (`resume_coordinator.rs:1238`) so those hooks recompute against an empty input.

---

## 4. UI projection

- `MessageProjector` (`src/gateway/session_projector.rs:185`) is the `SessionEventObserver` installed at boot (`start/mod.rs:439-452`); it drains an mpsc into `messages` (SQLite) / `transcript.jsonl` (file); `MissedSeqs` (146) is process memory. Row ids carry the source seq (`parse_source_seq`, heal at 475-520).
- `ProjectionReconciler` (`src/gateway/projection_reconciler.rs:81`, doc 1-44): candidate set = activity window (`[resume] max_age_secs`) UNION every session whose markers reduce to Interrupted; calls `MessageProjector::request_repair` (289) so the projector's drain task stays the single writer. Covers **both backends**. Older sessions: doctor `core/projection-holes` (unbounded). No durable watermark by ruling A6.
- `AssistantRunMeta` pairing: "last assistant row BETWEEN the run's `RunStarted` and this meta" (`session_projector.rs:797-803`); `NoRowInRange` => `Retry`, spend accumulated exactly once on `Stamped` (815-830).
- **Divergences events vs messages**:
  1. `src/gateway/orphan_notice.rs:61-64` writes an assistant `MessageRecord` directly via `store.append_message` — a **second `messages` writer**, not in `session_events`, invisible to the model, random id so not seq-repairable; called from `start/builder/agent_init/mod.rs:1347-1375`.
  2. `AgentEnd` hook `updated_output` rewrites the returned text after `AssistantMessage` was already durable (`execution_engine/run_loop/mod.rs:649-657`).
  3. `BeforeAgentStart` hook deny / `prevent_continuation` returns text with no event at all, and before `seed_session` (`run_loop/mod.rs:484-520` vs `runner_impl.rs:342`).
  4. `AgentInstance::add_message*` is another direct writer with zero production callers (`agent_instance.rs:454-500`).

---

## 5. Subagents

- Child session log: `TurnStarted` + `UserMessage` (`src/agents/subagent_spawner/mod.rs:675, 744`) then harness events; **no `RunStarted`/`RunFinished`** (child harness built at 1009, `NoopHarnessCallback`). Parent: `SubagentSpawned` after the concurrency permit (773), `SubagentReturned` after (1062). `BackgroundAgentTracker` is memory (`background_tracker`).
- Sidecar (`src/agents/background_persistence.rs`): `<dir>/<slug>/state.json` written at start and terminal via `write_atomic`, `result.txt` append trail (masked, 40-50); `init_and_reconcile` (279) tombstones `Running` rows, 7-day retention; `init_and_announce_orphans` (373) broadcasts one grouped `SubAgentCompleted` per parent session (wired `start/mod.rs:1631-1650`, after the announcer subscribes).
- On crash: nothing re-triggers a child (R7; `recovery.rs:36-40`). Completed-before-crash children are reported from `SubagentReturned` + sidecar; children queued behind `max_concurrent_subagents` exist only in the sidecar (`recovery.rs:146-153`); foreground `run` children and the parent's dangling `subagent` call go through ordinary boundary repair. Child logs reduce as marker-free "one run's worth" (`reduction.rs:70-73`) — their dangling calls are read as `EarlierRun` and shown as `in_flight` on the detail face only.
- Resume coordinator does not filter Subagent/Ephemeral keys (`resume_from_markers` 760-880 checks only `has_own_scheduler`); since children write no markers they never appear in `load_run_markers`.

---

## 6. Slash commands

- Resolution -> tool (`src/gateway/execution_engine/slash_command.rs:134-312`); `is_continuation_driven_slash` (46) keeps `/loop` `/goal` off the fast path.
- **L0 fast path executes the tool first, then writes `UserMessage` + `AssistantMessage`** (`fast_path.rs:43-80`, second site 163-200); no `ToolCallRequested`/`ToolResult`; both dropped with a warn when the global session service is absent (81-88).
- `/model` -> `select_model` -> `StoreBackedPinSink` onto the session row (`start/mod.rs:420`, `session_model_pin.rs`). Knobs (`exec_tier`, `session_mode`, `think_level`, `memory_mode`, pin) live in `SessionMetadata.identity_meta.custom` — row state, no event (writers `session_manager/ops/modify.rs:363, 454`, `file_backend/mod.rs:1381`; `docs/reference/SESSION_KNOBS.md`). Recovery reads the frozen `RunStarted.envelope` instead (snapshot > session > global; `exec_tier` ceiling only).
- `/clear` = `retire_from(1)` then projection reset (`handlers/chat.rs:719-760`) — retires the markers too, so nothing dangles. `/undo`/rewind = `retire_from(seq)` + `balance_run_markers_after_retire` (`chat.rs:815-831`, `handlers/mod.rs:164`, `session/marker_balance.rs:47`); also `session.truncate` (`db_handlers/modify.rs:752`).
- `/btw`: derived ephemeral side session, stamp `BTW_METADATA_KEY` in request metadata only (`gateway/btw/mod.rs`, `slash_command.rs:67`). `/<skill>`: `allowed_tools` scope lifted into request metadata (`slash_skill_scope.rs`). Neither is in `RunEnvelopeSnapshot`; `resume_metadata` (`resume_coordinator.rs:501-537`) rebuilds only `resume`, `project_root`, scope attribution, `caller_role`.
- MoA preset not persisted (ruling A5).

---

## 7. Skills

- Skill index: `SkillSystem` in `ExtensionManager` (rebuilt at boot from the filesystem, `helpers.rs:377-395`); enable state `<data_dir>/skills.toml`.
- Body reaches the model only through the `skill_read` tool => a durable `ToolResult` — survives restart until retired by compaction. `/<skill>` injects **no** skill text (`slash_command.rs:258-268`), only `record_use` (durable stat) and the tool scope.
- "Skill X was loaded" is therefore durable *as a tool result*, not as a fact; after `/compact` retires that turn the body is gone and only the summary mentions it (recoverable via `recall_events`).
- Loss on resume: the `/<skill>` run's `allowed-tools` narrowing (section 6) — a resumed `allowed-tools: []` run gets the agent's full surface.

---

## 8. Plugins & hooks

- Enabled/disabled: `<data_dir>/plugins.toml` (`src/extension/plugin_state.rs:1-30`, replaces the `.disabled` marker that had 4 writers and 0 readers). Installed = plugin directories + marketplace registry (`hub/install.rs`, `extension/marketplace`); `InstallRegistry::save` has no fsync (review-results BUNDLED-R4-10). `utils::atomic_io::write_atomic` fsyncs the file, dir-fsync best-effort (`atomic_io.rs:63, 84-88`).
- Plugin-provided tools/agents/MCP: re-derived at boot from manifests (`extension/registrar`, `mcp_config.rs`); live reconcile via `hub/reconcile.rs`.
- Hooks (`src/extension/hooks`): `BeforeToolCall` `additional_contexts` are folded into the `ToolResult` value (`tools/scoped/dispatch.rs:62 wrap_value_with_hook_contexts`) => durable. `UserPromptSubmit`/`SessionStart` contexts ride the transient tail (never persisted). Lifecycle hooks hold no state; a crash mid-`BeforeToolCall` hook = dangling call handled by boundary repair.

---

## 9. MCP

- Server configs persisted to JSON on add/remove (`src/mcp/manager/actor.rs:515-560`), transient servers never persisted (598-620), `auto_start` servers restarted at boot (189-196); tool lists re-fetched on connect (memory). OAuth tokens in `auth.json` with mtime-based cache invalidation (`src/mcp/auth/storage.rs:162-200, 389`). stdio pending-request map is memory (`transport/stdio.rs:253`).
- In-flight MCP call on crash = ordinary dangling `ToolCallRequested` (MCP tools go through the same `act.rs` path) => same boundary repair. R8 `mcp_*` config tools write the persisted config. Not verified: `kill_on_drop` on stdio children (orphaning after SIGKILL is likely, same as bash).

---

## 10. Cron / heartbeat / tasks / teams

- Cron (`src/tasks/cron/service/concurrency.rs`): phase 1 sets `running_at_ms` **and** advances `next_run` then `persist()` BEFORE execution (95-127, at-most-once: a crash mid-run does not re-fire the trigger); phase 3 writes `last_run_*`, `run_count`, `last_output` (170-200) and `insert_run` history (288); partial-result carry-over file per job (`carryover.rs`). Store is SQLite with in-memory working copy (`store.rs:1-30`, `CURRENT_VERSION = 1`).
- Heartbeat: twin shape (`src/tasks/heartbeat/store.rs`, `heartbeat.db`, `start/mod.rs:1199-1280`).
- Resume hands cron/heartbeat/team sessions back to their own scheduler after `repair_and_close_abandoned` (`resume_coordinator.rs:243 has_own_scheduler`, 738 `hand_back_to_scheduler`).
- Teams (`src/teams/dispatcher/schedule/reclaim.rs`): `coord_task` rows carry `started_at`/`locked_by`; boot `reclaim_zombies` (28) then `reclaim_orphaned` (108) resets InProgress -> Pending bounded by `MAX_TASK_RECOVERIES`, repairing the member session first; `abandon_orphaned_runs` (366, `swarm/tasks/store/runs.rs:53`) tombstones run rows; `WaitingReview` has a stale-review janitor (`is_stale_review`).
- Resilience `tasks` table (state DB): `reconcile_orphaned_tasks` (`resilience/database/tasks.rs:353`) flips `running` -> `Interrupted` at boot, then `orphan_notice`.

---

## 11. Background processes & PTY

- Background bash: journal `<dir>/job-<id>/state.json` + `output.txt` + `partial.txt` (`process_journal.rs:1-80`); boot tombstones `Running` -> `interrupted_by_restart_liveness_unknown` and announces (525; wired `start/mod.rs:1653-1672`). **No pid** => a SIGKILLed daemon orphans real OS processes and the tombstone can only say "did not check" (doc 34-48).
- PTY / runtime agents: `PtyManager` is a `LazyLock` in-memory registry (`src/gateway/pty/manager.rs:1-12, 218-224`); sessions are killed via `ChildKiller` only on orderly close (`session.rs:283`). Nothing on disk. On core restart: pty children orphaned, and every pty session id the model holds (in durable `ToolResult`s) answers "not found" — indistinguishable from "never existed". No tombstone, no announce.

---

## 12. Canvas / workspace trace / todo-plan / notes

- Canvas: one dir per canvas, `doc.json` with `base_revision` optimistic concurrency under `DocLocks` (in-memory) (`src/canvas/store.rs:1-10`, `doc_io.rs`). Durable per apply.
- Todo-plan (§3.13): scratchpad markdown on disk is SSOT; session -> `project_id` binding mirrored write-through to JSON and reloaded at boot (`src/builtin_tools/scratchpad_registry.rs:9-27 init_persistence`).
- Notes: SQLite via `open_sqlite_safe` (`memory/store/sqlite`).
- Workspace trace + context gauge: ride `AssistantRunMeta` (after the run) and are re-stamped by the reconciler if the event exists (`execute.rs:963-1005`).

---

## 13. Boot sequence (`src/bin/aleph-server/commands/start/mod.rs`)

1. `spend::install_policy` (207)
2. Session store build + file-backend repairs/migrations (`helpers.rs:230-320`)
3. `StoreBackedPinSink::install` (420); `MessageProjector::new` + global (439-447); `build_sqlite_session_service` (449); global event store / session service or `decline_*` (462-485); prompt-size registry (491)
4. Vault + `spend::install_ledger` (497-510)
5. Agent init: `reconcile_orphaned_tasks` + `orphan_notice::notify_interrupted_tasks` (`builder/agent_init/mod.rs:1347-1375`) — before the engine accepts requests
6. Extension manager (`helpers.rs:377`)
7. Sub-agent sidecar `init_and_announce_orphans` (1640) — after the completion announcer subscribes; bash journal `init_and_announce` (1662); busy-queue `durable::init` (1677, record-only)
8. Heartbeat/cron services (1199+, 2700+); task reaper (2960)
9. **Detached** task (2978-3117): `ProjectionReconciler::reconcile_candidates` -> `set_global_resume_coordinator` (unconditional) -> if `[resume] enabled`: `wait_for_channel_config_snapshot(30s)` -> `resume_interrupted_runs` -> `busy_queue::durable::reinject_survivors`
10. Group chat, channels, inbound router, serve.

- Resume is **sequential** (`resume_interrupted_runs` 678-712 `for` loop) and `retrigger` awaits `execution_adapter.execute` which runs the whole loop inline (`execute.rs:885`, no spawn) — so N interrupted sessions recover one after another, and queued user messages re-enter only after all resumed runs finish. The `max_concurrent=4` semaphore (`config/types/resume.rs:40`) is only contended by on-demand `agent.resume`.
- Second crash during recovery: counted by `trailing_starts` (consecutive `RunStarted` after the last `RunFinished`, `reduction.rs:190`), abandoned at `max_attempts = 3` (`config/types/resume.rs:36`, `resume_coordinator.rs:989-999`, `abandon` 1072-1100 writes `RunFinished{Abandoned}`). Age cutoff `max_age_secs = 86400` (recency = max(last marker, last activity)).

---

## A. Durable-fact ledger

| # | Fact | Where persisted | Written before/after effect | Reducer on boot | Lost on kill -9 | Anchor |
|---|---|---|---|---|---|---|
| 1 | User turn | `session_events` UserMessage | before run | `reduce_run` / `build_prompt` | no | `runner_impl.rs:342` |
| 2 | Run open (+knobs) | RunStarted{envelope} | after seed, before harness | `reduce_disposition` / `plan_resume` | seed->start window | `runner_impl.rs:978` |
| 3 | Run close | RunFinished | after | `reduce_disposition` | dangling => resume | `runner_impl.rs:~1111` |
| 4 | Tool dispatch | ToolCallRequested | **before** execute | dangling set | no | `act.rs:484` |
| 5 | Tool receipt | ToolResult / ToolError | after | pairing | in-flight result | `act.rs:556` |
| 6 | Gate decision | ToolCallApproved / Denied | after decision, before execute | `denied` flag | parked gate => "unknown" | `dispatch.rs:1185` |
| 7 | Pending approval | `ExecApprovalManager.pending` | — | none | **yes** | `exec/manager.rs:300` |
| 8 | Pending clarification | `ClarificationManager.pending` | — | none | **yes** | `clarification/session.rs:256` |
| 9 | Assistant text + usage | AssistantMessage | after LLM call | prompt / projector | the call | `think.rs` |
| 10 | Run occupancy, session spend counters | AssistantRunMeta | **after RunFinished** | projector stamp | **yes** | `execute.rs:966` |
| 11 | Per-call spend | `SpendLedger` SQLite | after response | none needed | last call | `metering.rs:153` |
| 12 | Model pin / 5 knobs | session row `identity_meta.custom` | at set | row read | no | `start/mod.rs:420`, `modify.rs:363` |
| 13 | Slash fast-path turn | UserMessage+AssistantMessage | **after tool effect** | projector | yes (effect kept) | `fast_path.rs:43` |
| 14 | Manual compaction | SystemMessage -> CompactionPerformed -> retire_through | 3 writes | none | torn states persist | `manual.rs:455-482` |
| 15 | Auto in-place compaction | memory `Vec<UnifiedMessage>` | — | recomputed | yes (cost) | `directive.rs:95` |
| 16 | Session split | epoch reg + child events + marker pair | 7 writes | `reduce_run` (parent only) | torn split | `session_split.rs:96-135` |
| 17 | `/clear` `/undo` | `retired_at` | atomic | `reduce_run` | no | `chat.rs:726/815` |
| 18 | Messages projection | `messages` / `transcript.jsonl` | async after event | `ProjectionReconciler` | queue (healed) | `session_projector.rs:185` |
| 19 | Sub-agent spawn/return | parent SubagentSpawned/Returned | after permit / after | `recovery.rs` | queued child | `subagent_spawner/mod.rs:773` |
| 20 | Sub-agent sidecar | `state.json` + `result.txt` | before spawn | `init_and_reconcile` | no | `background_persistence.rs:543` |
| 21 | Bash bg job | `state.json` + trails | at spawn | `init_and_reconcile` | pid/liveness | `process_journal.rs:415` |
| 22 | PTY session | memory | — | none | **yes** | `pty/manager.rs:218` |
| 23 | Busy-queue message | `run-<id>/state.json` | at enqueue; tombstone at admission | `reinject_survivors` | no | `busy_queue/durable.rs` |
| 24 | Steer | live-log UserMessage | before wake | prompt | no | `steer_signal.rs` |
| 25 | Resilience task row | `tasks` table | before seed | `reconcile_orphaned_tasks` | no | `persistence.rs:11` |
| 26 | Cron claim | `running_at_ms` + `next_run` | **before** run | boot reset | result only | `concurrency.rs:95` |
| 27 | Cron result | phase-3 writeback + history | after | — | yes | `concurrency.rs:170` |
| 28 | Team task lease | `coord_task` | before run | `reclaim_orphaned` | no | `reclaim.rs:108` |
| 29 | Plugin enabled | `plugins.toml` | at toggle | load | no | `plugin_state.rs` |
| 30 | MCP server cfg / tokens | JSON / `auth.json` | at change | `auto_start` | in-flight call | `actor.rs:516`, `storage.rs:389` |
| 31 | Scratchpad binding | JSON write-through | at touch | `init_persistence` | no | `scratchpad_registry.rs` |
| 32 | Canvas doc | `doc.json` + revision | at apply | load | no | `canvas/store.rs` |
| 33 | Hook stop message | none | — | none | yes (+ user msg) | `run_loop/mod.rs:484` |
| 34 | Transient tail (cwd, CLAUDE.md, prompt hooks) | none (recomputed) | — | recomputed | n/a | `runner_impl.rs:563` |

---

## B. Top 15 gaps (ranked by "silently wrong after crash")

1. **Two boot arms, two verdicts on one crash.** `orphan_notice::notify_interrupted_tasks` writes "interrupted, please re-send" straight into `messages` (`src/gateway/orphan_notice.rs:61-64`, caller `start/builder/agent_init/mod.rs:1347-1375`), then the detached `ResumeCoordinator` re-triggers the same session. The user is told to resend a run that is being continued; the row bypasses SSOT (model never sees it; projector cannot heal or attribute it). Root: three unjoined run ids (ruling A5, `runner_impl.rs:962-970`).
2. **Crash-loop ratchet on the wrong side of the action.** The attempt counter is the re-triggered run's own `RunStarted` (`runner_impl.rs:978`). Any crash before that line — `admit_run`, `BeforeAgentStart` hook, seed, memory recall — leaves `trailing_starts` unchanged, so `max_attempts` never trips and every boot re-repairs and re-triggers forever. `retrigger` writes no intent stamp before `execute` (`resume_coordinator.rs:1199-1300`; cap check 989).
3. **Manual `/compact` torn.** Crash after `CompactionPerformed` before `retire_through` (`manual.rs:470-482`): log claims a compaction, prefix still live, summary + full history both in every future prompt; nothing replays the checkpoint (`CompactionPerformed` has no recovery consumer). Crash after `SystemMessage` before `CompactionPerformed`: duplicate context, no checkpoint.
4. **Session-split torn.** `register_epoch` (routing now resolves to the child) precedes parent `RunFinished` / child `RunStarted` (`session_split.rs:116-135`). Crash in between: parent reads Interrupted and is re-triggered while the child already carries the copied tail; child has no open run; a resumed parent now double-executes the tail's work. SQLite backend only.
5. **Strict decode, global blast radius.** No event-schema version; one undecodable row (older binary after a variant was added, or torn JSON) fails `load_run_markers` for all sessions (`store.rs:569`) => `resume scan failed; skipping resume` (`resume_coordinator.rs:686-692`) and the reconciler's interrupted-union — silently, with one warn.
6. **Seed -> RunStarted window.** `UserMessage` durable, no open run => reducer reads `Clean` (`UnmarkedActivity` counts only tool dispatches, `reduction.rs:65-73`); never resumed, never repaired; the user's message sits unanswered. Only the resilience `tasks` row + gap #1 notice covers it. `runner_impl.rs:342 -> 978` (prompt build/recall in between).
7. **Parked gate reported as "may have completed".** Approval-parked or `ask_user`-parked calls are memory-only (`exec/manager.rs:300`, `clarification/session.rs:256`); their dangling `ToolCallRequested` is indistinguishable from an in-flight one and gets the side-effects-may-have-landed sentence (`boundary_repair.rs:97-121`), sending the model to verify state that never existed. The `denied` arm exists; a "never approved" arm does not, and the log has no "gate opened/parked" fact to derive it from.
8. **Resume widens scope.** `/<skill>` `allowed_tools` (`slash_skill_scope.rs`) and the `/btw` read-only stamp (`btw/mod.rs`) live only in request metadata; `resume_metadata` (`resume_coordinator.rs:501-537`) rebuilds neither, so a resumed `allowed-tools: []` run gets the full tool surface — the one direction recovery is supposed never to move (criterion #14).
9. **Hook stop erases the turn.** `BeforeAgentStart` deny/`prevent_continuation` returns text before `seed_session` (`run_loop/mod.rs:484-520` vs `runner_impl.rs:342`): neither the user's message nor the stop text reaches the log; reload shows nothing happened. Same class the `Error{Guardrail}` receipt fixed for guardrails.
10. **Fast-path slash written after effect.** L0 executes the tool, then writes `UserMessage`+`AssistantMessage` (`fast_path.rs:43-80`, 163-200). Crash between: `select_model` pin / `session_rename` / `agent_switch` applied with no transcript trace; no `ToolCallRequested` so no repair either.
11. **Session spend counters ride a post-run event.** `AssistantRunMeta` is emitted after `RunFinished` (`execute.rs:966-999`); a crash before it under-counts the session's tokens/cost permanently (ledger vs session row diverge; the reconciler can only re-apply stamps that exist). Per-call `SpendLedger` is also after-response (`metering.rs:153`).
12. **PTY sessions vanish without tombstone.** `PtyManager` is memory (`pty/manager.rs:218`); the model holds pty ids in durable `ToolResult`s; after restart `pty.*` answers "not found" (= never existed), OS children orphaned, no boot announce. Every other background family has a sidecar; this one does not.
13. **Bash bg jobs record no pid.** SIGKILLed daemon orphans real processes; tombstone can only say `liveness_unknown` (`process_journal.rs:34-48`, decided against "for this round").
14. **Serial resume + delayed reinjection.** Boot scan awaits each resumed run to completion (`execute.rs:885` inline) before the next session and before `reinject_survivors` (`start/mod.rs:3008-3090`); one long resumed run silently holds every other session's recovery and every queued user message.
15. **`AgentEnd` hook rewrite diverges from the log.** `updated_output` replaces the delivered text after `AssistantMessage` is durable (`run_loop/mod.rs:649-657`); UI/channel show one thing, replay another.

Honourable mentions: `UserPromptSubmit` hooks re-run against `input: ""` on resume (`runner_impl.rs:563`, `resume_coordinator.rs:1238`); `SubagentSpawned` after permit (documented, sidecar covers); `ToolCallApproved` silently skipped without ambient identity (`dispatch.rs:1173`); `agent_instance.reset_session` retires SSOT without `balance_run_markers_after_retire` (no prod caller today).

---

## C. Severed wires / stale comments / dead code

- `AgentInstance::add_message` / `add_message_with_run_id` — direct `messages` writer, zero production callers (only `agent_instance.rs:1023, 1054` tests). CUT candidate. `agent_instance.rs:454-500`.
- `AgentInstance::reset_session` — "No production caller today" per its own doc. `agent_instance.rs:557`.
- `CompactionPerformed` — one producer (`manual.rs:470`), no recovery consumer; only `fork.rs:241` (filter) and `manual.rs` tests read it. Either a boot completer consumes it (gap #3) or it is a checkpoint nobody checks.
- Stale comment `start/mod.rs:391`: "split degrades to FinalReply there" — code degrades to `compact_to_fit_and_note` (`directive.rs:207-226`).
- Stale comment `helpers.rs:327-334`: "Phase 1 dual-write ... the legacy `messages` table remains authoritative" — `messages` is a projection; `start/mod.rs:402` says so.
- Stale doc `projection_reconciler.rs:29-31`: "cron and heartbeat sessions ... emit no run markers" — cron goes `JobSnapshot -> ExecutionAdapter` (`tasks/cron/executor.rs:3, 40`) -> bridge, which writes `RunStarted` unconditionally (`runner_impl.rs:978`); `has_own_scheduler` (`resume_coordinator.rs:243`) exists precisely to close those markers. The `sub-bg-*` half of the sentence is correct.
- `service.rs:67-80` hard-codes "nine sites" for the `ConsumerDecides` handle — a count that rots (its own comment admits it).
- `orphan_notice.rs` — a consumer (`messages`) with a producer that contradicts another producer (resume); the notice text is also wrong whenever resume is enabled (default).
- Three run ids per run never joined (gateway `RunRequest.run_id` / bridge `run_marker_id` / resilience task id) — `runner_impl.rs:962-970`; gap #1 and the "which scheduler run wrote this marker" question both hang on it.
- `background_persistence` announce vs `orphan_notice` vs `ResumeCoordinator` degrade note — three boot-time "tell the user" channels with three carriers (GlobalBus event / direct `messages` row / `SystemMessage` event).

---

## D. Existing infrastructure to reuse (do not re-invent)

- `session::reduction::{reduce_run, reduce_disposition, LogContradiction, RunReduction, DanglingCall}` (`reduction.rs:54, 190, 220, 264, 316, 361`) — the single derivation any new boot arm must read.
- `session::boundary_repair::{repair_boundary, repairs_for, boundary_repair_text, repair_and_close_abandoned, DegradeNote}` (`boundary_repair.rs:47, 87, 138, 166, 238`) — the only place to answer a dangling call; add a fourth arm here for "never approved".
- `session::marker_balance::close_open_run_after_retire` (`marker_balance.rs:47`) + `handlers::balance_run_markers_after_retire` (`handlers/mod.rs:164`) — fail-closed on unknown run manager.
- `RunEnvelopeSnapshot` + `RUN_ENVELOPE_KNOB_KEYS` census + `plan_resume` (`events.rs:174`, `session_snapshot.rs`, `resume_coordinator.rs:371`) — the carrier for anything a resume must replay (`allowed_tools`, `btw`), with a census test that goes red when the two key sets drift.
- `ResumeCoordinator.in_flight` / `ResumeSlot`, `resume_session` on-demand face, `ResumeReport.{refused, degraded, unsnapshotted, skipped_unknown_age}` (`resume_coordinator.rs:88-200, 881`) — buckets already exist for new refusal kinds.
- `MessageProjector::request_repair` + `ProjectionReconciler` + doctor `core/projection-holes` (`session_projector.rs:289`, `projection_reconciler.rs:108`, `diagnostics/checks/projection_holes.rs`) — projection self-heal; never add a second `messages` writer.
- `SessionEvent::Error` receipt + `project_row` system-row projection (`events.rs:441`, `session/projection.rs`) — reuse for hook-stop receipts (gap #9) instead of a new variant.
- Sidecar pattern, four implementations to copy for PTY: `agents/background_persistence.rs`, `builtin_tools/process_journal.rs`, `gateway/busy_queue/durable.rs`, `builtin_tools/scratchpad_registry.rs` (state.json twice via `write_atomic`, append trail, boot tombstone, 7-day sweep, opt-in `init`).
- `utils::atomic_io::write_atomic` (`atomic_io.rs:18-90`), `utils::sqlite_open::open_sqlite_safe` (`sqlite_open.rs:12`), `migrate_add_session_events` SAVEPOINT + add-column-if-missing (`store.rs:182, 232`) — the migration idiom to extend with a `user_version`.
- `store::retire_live_events` / `set_global_session_event_store` / `global_session_service` capability slots with `decline_*` reasons (`store.rs:837`, `service.rs`) — the three-state "absent vs declined vs present" handle shape.
- `session_events_fts` + `recall_events` (`store.rs:329-348, 590-650`) — post-compaction continuity already exists; a torn `/compact` recovery can lean on it.
- `SubAgentCompletionEvent` grouped announce with `request_ids` + `on_delivered` stamping (`background_persistence.rs:373, 641`) — the one boot-notice shape that reports counts instead of verdicts; the right template for a PTY/bash orphan notice.

---

## Not done / unverified

- No runtime probes (read-only scan); `qa/resume_boundary/run.sh` not re-run.
- Did not read `src/gateway/session_manager/ops/*` beyond the knob writers, nor the SQLite `messages` DDL.
- Did not verify MCP stdio child `kill_on_drop` behaviour, nor `interfaces/webchat` (§6.9 client-side reconnect half), nor `desktop/`.
- Line numbers are from this checkout (main @ 5e85060b8); several large files (`resume_coordinator.rs`, `session_projector.rs`, `start/mod.rs`) shift often — re-anchor by symbol name.
