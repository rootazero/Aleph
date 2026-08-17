# Resilience Module — Static Code Review (2026-08-16)

**Module:** `src/resilience/` (13 files, 5018 LOC)
**Reviewer lens:** seam/wiring, logic, architecture
**Confidence threshold:** > 80% (only high-confidence findings reported)

---

## Executive Summary

The resilience module is **a state.db persistence layer with deep dead-code from past features** that were removed or never wired up. Of the 13 files reviewed, the only production-relevant code is:

- `types.rs` — `AgentTask`, `TaskStatus`, `TaskTrace`, `RiskLevel`, `Lane`
- `database/state_database/mod.rs` + `schema.rs` — schema + `new()` / `new_with_dim()`
- `database/migration.rs` — schema evolution
- `database/tasks.rs` — `agent_tasks` CRUD (only `insert_agent_task`, `update_task_status`, `get_agent_task`, `reconcile_orphaned_tasks` are wired to production)
- `database/traces.rs` — `task_traces` write/read (only `insert_trace`, `get_traces_by_task`, `list_trace_tasks_paged`, `aggregate_usage_by_agents`, `aggregate_moa_advisor_usage` are wired)
- `database/group_chat.rs` — group chat persistence
- `database/channel_offsets.rs` — Telegram offsets
- `database/memory_events.rs` — memory event sourcing (heavily used)

The remaining surface — `agent_events` (Skeleton & Pulse), `poe_*` tables, `experience_replays` table, `memories` table, `MemoryStats`, most `TaskStatus` variants, `with_parent_task` / `Recursive Sentry` machinery, `should_auto_resume` / `needs_resume_confirmation`, recovery data fields (`checkpoint_snapshot_path`, `last_tool_call_id`) — is **either never wired or aspirational**.

---

## Findings

### [High] src/resilience/database/state_database/schema.rs:163-211 — POE tables fully orphaned (3 tables, no readers/writers)

**Category:** architecture (dead schema)
**Confidence:** High
**Description:** The schema creates three POE (Pattern-Oriented Execution) tables — `poe_events`, `poe_trust_scores`, `poe_contracts` — with associated indexes. **No production code anywhere in the tree reads from or writes to these tables.** Verified by `grep -rn "poe_events\|poe_trust_scores\|poe_contracts" --include="*.rs"` returning only the schema file itself. The matching migration `migrate_add_*` does not exist (so these are NOT added by a migration — they were always part of `schema_sql()`). This is three tables with full CRUD space (events, indexes for trust scores, contracts with status CHECK) that nobody on HEAD uses. They cost write amplification (every `CREATE TABLE IF NOT EXISTS` is still parsed on every `new()` / `in_memory()`), WAL bloat, and visual confusion about what the schema actually is.

**Suggested fix:** Either wire the POE subsystem or remove these three tables and their indexes from `schema_sql()`. Note that `poe_events` is structurally similar to `agent_events` — the latter is also unused, so both subsystems can be removed together.

---

### [High] src/resilience/database/migration.rs:24-105 — `experience_replays` migration is dead

**Category:** architecture (dead schema)
**Confidence:** High
**Description:** `migrate_add_experience_replays` creates `experience_replays` + 4 indexes + a `experiences_vec` vec0 virtual table. **No production code reads or writes these.** Verified: `grep -rn "experience_replays\|experiences_vec" --include="*.rs"` returns only migration.rs itself and the test. The migration runs on every `StateDatabase::new()` and `in_memory()` (called via `run_optional_migrations`), so every fresh DB pays for creating and indexing a table nobody queries. The doc-comment claims this is "for Cortex evolution system" — but the Cortex system is no longer present.

**Suggested fix:** Delete `migrate_add_experience_replays` and remove its call from `run_optional_migrations` and `in_memory()`. If the data shape matters, leave a TODO pointing at the actual memory.db (`memory/store/sqlite`).

---

### [High] src/resilience/database/state_database/mod.rs:449-456 — `MemoryStats` struct is fully orphaned

**Category:** architecture (dead type)
**Confidence:** High
**Description:** `pub struct MemoryStats { total_memories, total_apps, database_size_mb, oldest/newest_memory_timestamp }` is defined and re-exported three times: `database::mod.rs:18`, `resilience::mod.rs:12`, `crate::lib.rs:220`. **No production code constructs or reads this struct.** Verified: `grep -rn "MemoryStats" --include="*.rs" | grep -v "webchat"` returns only the definition and the three re-exports. The webchat interface defines its own unrelated `MemoryStats` (in `interfaces/webchat/src/{models.rs, api/memory.rs}`) that is deserialized from JSON, not from this struct. This is exactly the "inert type" form from the seam catalog — a public type that exists only to be re-exported.

**Suggested fix:** Delete the struct, the three re-exports, and the `pub use crate::resilience::database::MemoryStats` line in lib.rs.

---

### [High] src/resilience/database/events.rs — entire `agent_events` Skeleton & Pulse surface is unused

**Category:** architecture (dead subsystem)
**Confidence:** High
**Description:** All 9 public methods on `StateDatabase` related to `agent_events` — `insert_event`, `bulk_insert_events`, `get_events_by_task`, `get_events_since_seq`, `get_events_in_range`, `get_structural_events`, `get_latest_event_seq`, `delete_events_for_task`, `get_event_count` — **have zero callers in production code**. Verified by `grep -rn "\.insert_event\b\|\.get_events_by_task\b\|\.get_events_since_seq\b\|..."`. None of the consumer modules (`gateway/execution_engine/*`, `group_chat/*`, `memory/*`, `builtin_tools/*`) ever writes a `skeleton` or `pulse` event. Likewise, `AgentEvent::TYPE_TASK_STARTED`, `TYPE_TOOL_CALL_STARTED`, `TYPE_TOOL_CALL_COMPLETED`, `TYPE_ARTIFACT_CREATED`, `TYPE_TASK_COMPLETED`, `TYPE_TASK_FAILED`, `TYPE_AI_STREAMING` constants are unused outside their own definition. The `AgentEvent::new` / `::structural` / `::pulse` constructors are equally unused. The `agent_events` table itself is created in `schema_sql()` and the FTS5 triggers + index `idx_agent_events_task_seq` / `idx_agent_events_structural` are installed, but the row source doesn't exist.

The events.rs file's `FIRST_EVENT_LOGGED` OnceLock + atomic first-write log is the smoking gun: it was built for production traffic that never materialized.

**Suggested fix:** Delete `src/resilience/database/events.rs`, the `AgentEvent` struct from `types.rs`, the `agent_events` table from `schema_sql()`, and remove the `pub use AgentEvent` from `resilience/mod.rs`. If kept, this surface needs a documented consumer (the closest in spirit is the task trace event sourcing, which IS wired via `traces.rs`).

---

### [Medium] src/resilience/types.rs:56-71 — `TaskStatus::Idle` and `::Swapped` never produced

**Category:** architecture (dead enum variants)
**Confidence:** High
**Description:** Two of the seven `TaskStatus` variants — `Idle` and `Swapped` — are documented as "Session-as-a-Service" states but **no code path in the tree ever writes them**. Verified: `grep -rn "TaskStatus::Idle\|TaskStatus::Swapped\|\"idle\"\|\"swapped\""` in production code returns nothing (only `FromStr` round-tripping in `types.rs`). `reconcile_orphaned_tasks` and `update_task_status` only know about `Running` → `Interrupted` and `Completed`/`Failed`. `is_recoverable` only checks `Running | Interrupted`. So `from_str_or_default` accepts the strings, the DB column accepts them, but the state machine has no entry edge for them. Any persisted "idle" or "swapped" row would be silently orphaned.

**Suggested fix:** Either implement the Session-as-a-Service transitions that produce these states (writer code path), or remove both variants + their `FromStr` arms + their `Display` arms. The non-functional enum members are a footgun for future "I'll just write this status" contributions.

---

### [Medium] src/resilience/types.rs:281-291 — `should_auto_resume` and `needs_resume_confirmation` are unused

**Category:** architecture (dead API)
**Confidence:** High
**Description:** Both methods on `AgentTask` — `should_auto_resume` (risk=Low + recoverable) and `needs_resume_confirmation` (risk=High + recoverable) — are defined and unit-tested, but **never called in production**. Verified by `grep -rn "should_auto_resume\|needs_resume_confirmation" --include="*.rs"` returning only the definition and the test. The actual restart-time recovery decision is made by `reconcile_orphaned_tasks` (which unconditionally flips Running → Interrupted) plus `orphan_notice::notify_interrupted_tasks` (which sends a message regardless of risk level). These two methods look like the planned policy gate, but the actual policy is "always interrupt, never auto-resume."

**Suggested fix:** Either remove both methods (and their tests) or wire them into a real resumer that reads the risk level before deciding to auto-restart a task.

---

### [Medium] src/resilience/types.rs:213-220 + database/tasks.rs — "Recursive Sentry" fields and `with_parent_task` are unused

**Category:** architecture (dead aspirational feature)
**Confidence:** High
**Description:** `AgentTask::recursion_depth`, `AgentTask::parent_task_id`, `with_parent_task()` builder, and the schema column `recursion_depth INTEGER DEFAULT 0` exist but **no production code enforces, reads, or increments them**. Verified: `grep -rn "with_parent_task\|recursion_depth\|parent_task_id"` outside the resilience module returns nothing. `AgentTask::new` always sets `recursion_depth: 0, parent_task_id: None`, and `with_parent_task` is never called. `get_recoverable_tasks` orders by risk+created, not by depth. The doc-comment "for Recursive Sentry" describes an unbuilt feature.

**Suggested fix:** Implement the recursion-depth check (compare against a config-bound maximum before spawning a subagent) or remove the two fields, the `with_parent_task` builder, and the schema columns. Without enforcement, the columns are write-only noise.

---

### [Medium] src/resilience/types.rs:209-212 + database/tasks.rs — Shadow Replay recovery fields unused

**Category:** architecture (dead aspirational feature)
**Confidence:** High
**Description:** `AgentTask::checkpoint_snapshot_path` and `last_tool_call_id` are written into every persisted task (see `tasks.rs:64-65`) but **never read anywhere**. Verified: `grep -rn "checkpoint_snapshot_path\|last_tool_call_id"` outside the resilience module returns nothing. The doc-comment claims these are "for Shadow Replay recovery," but the actual recovery path (`reconcile_orphaned_tasks` → `mark_running_as_interrupted`) ignores them entirely. `get_traces_from_step` (in traces.rs) sounds like it would use `last_tool_call_id` to resume, but that function itself is also unused.

**Suggested fix:** Wire them — at minimum, `get_recoverable_tasks` should use `last_tool_call_id` to surface "where the task died" — or remove the fields and the schema columns.

---

### [Medium] src/resilience/database/state_database/schema.rs:17-47 — `memories` table + vec0 + FTS5 + triggers are dead in state.db

**Category:** architecture (dead duplicate schema)
**Confidence:** High
**Description:** The resilience state.db creates `memories` (with 3 indexes), `memories_vec` (vec0), `memories_fts` (FTS5 virtual table), and the two `memories_fts_insert` / `memories_fts_delete` triggers — but **no production code writes to or reads from any of them**. Verified: `grep -rn "INSERT INTO memories\|INSERT INTO memories_vec\|FROM memories\|FROM memories_vec" --include="*.rs"` returns only the migration logic, `migrate_to_vec0`, and `tests.rs`. The actual memory store is in `memory/store/sqlite/` writing to `memory.db` (a different file) with `raw_memories` as the table name. The `serialize_embedding` helper is only used in `tests.rs`. The FTS5 triggers will never fire because no row is ever inserted. The schema also creates a `memories_vec` at a fixed 1024-dim — if any code *did* write to `memories` at a different dimension (e.g. legacy 384-dim), the vec0 insert would fail; the dimension-change migration in `state_database/mod.rs:130-160` only exists because of this orphaned layer.

**Suggested fix:** Delete `memories`, `memories_vec`, `memories_fts`, the two triggers, the 3 indexes, `serialize_embedding`, the entire `migrate_to_vec0` function, and the dimension-change handling in `new()` / `new_with_dim()`. Real memory storage lives in `memory/store/sqlite/`. Keeping a parallel, unused vector layer is a maintenance and disk-cost liability (every DB open pays for FTS5 + vec0 init for nothing).

---

### [Medium] src/resilience/database/state_database/schema.rs:38-211 — 7 tables duplicated from memory.db (graph_* / compression_sessions / daily_insights / dream_status / memory_events)

**Category:** architecture (dead duplicate schema)
**Confidence:** High
**Description:** The resilience state.db creates `compression_sessions`, `graph_nodes`, `graph_edges`, `graph_aliases`, `memory_entities`, `daily_insights`, `dream_status`, and `memory_events` — but **all of them are created and actively read/written in a different database** (`memory.db`, opened by `memory::store::sqlite::SqliteMemoryBackend`). Verified by grep — `memory/store/sqlite/sessions.rs` reads/writes `dream_status`, `daily_insights`; `migrations.rs` references `graph_nodes`/`graph_edges`/`memory_entities`. These tables in state.db are dead — every read/write goes to the other DB. Even the comment block in `schema.rs` acknowledges this for `memory_audit_log` ("zero writers anywhere in the tree… dropped by drop_obsolete_tables"). Same condition holds for these 8 tables. `memory_events` in state.db IS used (see below) — but everything else is dead.

Wait: `memory_events` in state.db IS used by `memory/events/handler.rs`, `traveler.rs`, `migration.rs`, `projector.rs`. So that one is alive; the other 7 (`compression_sessions`, `graph_nodes`, `graph_edges`, `graph_aliases`, `memory_entities`, `daily_insights`, `dream_status`) are dead.

**Suggested fix:** Delete the dead 7 tables and their indexes from `schema_sql()`. Keep `memory_events` (it's wired). `drop_obsolete_tables` should add the remaining dead tables (`graph_nodes`, `graph_edges`, `memory_entities`, `daily_insights`, `dream_status`, `compression_sessions`) to its DROP list so existing DBs that already have them get cleaned up on first boot after this change.

---

### [Medium] src/resilience/database/*.rs — `AlephError::config` used for 136+ DB errors with misleading suggestion

**Category:** architecture (error taxonomy)
**Confidence:** High
**Description:** Every database error in `events.rs` (20), `tasks.rs` (13), `traces.rs` (33), `migration.rs` (33), `group_chat.rs` (10), `channel_offsets.rs` (2), `state_database/mod.rs` (25) is wrapped as `AlephError::config("...")`. The variant is `ConfigError` whose `Display` reads `"Configuration/Database error: {message}"` and whose hardcoded suggestion is `"Check your configuration file at ~/.aleph/config.toml"`. For a connection-lost, FK-violation, or serialize-failed DB error, the suggestion points the operator at a TOML file that has nothing to do with their problem. `memory_events.rs` correctly uses `AlephError::other` (which has no misleading suggestion) — proving the inconsistency.

**Suggested fix:** Introduce `AlephError::storage` / `AlephError::database` variants (the enum already has 30+ variants; one more is fine) and migrate the 136 calls. Update the Display impl to read "Storage error" or "Database error" and provide a suggestion that actually fits (e.g. "Check disk space and database file permissions").

---

### [Low] src/resilience/database/tasks.rs:198-222 — `get_tasks_by_session`, `get_recoverable_tasks`, `mark_running_as_interrupted` not consumed externally

**Category:** architecture (dead API surface)
**Confidence:** High
**Description:** Of the 7 CRUD methods on `agent_tasks`, only 4 are wired to production (`insert_agent_task`, `update_task_status`, `get_agent_task`, `reconcile_orphaned_tasks`). The remaining three — `get_tasks_by_session`, `get_recoverable_tasks`, `mark_running_as_interrupted` — are only used by tests and internally by `reconcile_orphaned_tasks`. Verified: `grep -rn "\.get_tasks_by_session\b\|\.get_recoverable_tasks\b\|\.mark_running_as_interrupted\b" --include="*.rs"` returns only `tasks.rs` tests + `tasks.rs` internal call. These are private-looking but `pub`; they look like deliberate API surface, but no consumer exists.

**Suggested fix:** Either make them `pub(crate)` (if intended only for `orphan_notice` and friends) or wire them to a UI surface (the Panel's per-session task list is a natural fit).

---

### [Low] src/resilience/database/traces.rs — 7 of 11 trace methods unused outside tests

**Category:** architecture (dead API surface)
**Confidence:** High
**Description:** `bulk_insert_traces`, `get_last_trace`, `get_traces_from_step`, `delete_traces_for_task`, `get_trace_count`, `list_trace_tasks`, `get_trace_by_id` are all unused in production code. Verified by grep. Only `insert_trace`, `get_traces_by_task`, `list_trace_tasks_paged`, `aggregate_usage_by_agents`, `aggregate_moa_advisor_usage` are wired. The first three unused methods would be exactly the right primitives for a "Shadow Replay resume" feature (which the schema fields advertise) — they were built but the resume consumer never materialized.

**Suggested fix:** Wire them or mark `#[allow(dead_code)]` with a TODO pointing at the planned consumer.

---

### [Low] src/resilience/database/tasks.rs:107-153 — `reconcile_orphaned_tasks` returns a pre-update snapshot

**Category:** logic (minor semantic surprise)
**Confidence:** Medium
**Description:** `reconcile_orphaned_tasks` calls `get_recoverable_tasks()` (snapshot of Running rows), then `mark_running_as_interrupted()` (DB now has Interrupted), and returns the *original* Running snapshot. So the returned `AgentTask` objects report `status = Running` while the DB says `Interrupted`. The current caller (`agent_init/mod.rs:1240`) only uses `task.id`, `task.agent_id`, `task.task_prompt`, and `task.lane`, so the mismatch is harmless today. But a future caller that reads `task.status` from the returned Vec would see stale state and might re-mark `interrupted` (which is idempotent) or compare against DB state and get a "no-op" diagnostic.

**Suggested fix:** Document the snapshot semantics explicitly in the doc-comment, OR re-fetch the updated rows after the UPDATE (`SELECT … WHERE status = 'interrupted' AND updated_at = ?`), OR return a Vec of IDs only and let the caller re-fetch.

---

### [Low] src/resilience/database/tasks.rs:224-237 — `mark_running_as_interrupted` doesn't set `completed_at`

**Category:** logic (consistency)
**Confidence:** Medium
**Description:** `update_task_status` sets `completed_at` for `Completed | Failed`. But `mark_running_as_interrupted` (the bulk reconcile path) flips `Running → Interrupted` without setting `completed_at`. So `Interrupted` rows have `started_at IS NOT NULL` but `completed_at IS NULL`, indistinguishable from "started but never finished." Combined with the previous finding, an Interrupted task now reports `status=Running, completed_at=NULL` in the returned Vec — the very symbol of "still in flight."

**Suggested fix:** In `mark_running_as_interrupted`, also set `completed_at = ?1` to make the terminal-state invariant explicit.

---

### [Low] src/resilience/mod.rs — `mod.rs` claims reconnect/retry/backoff scope that does not exist

**Category:** architecture (documentation drift)
**Confidence:** High
**Description:** The audit brief asks for review of "reconnect/retry/backoff paths in mod.rs." The module's actual scope, per `mod.rs`, is "Database and Core Types" — and there is **no retry, backoff, circuit breaker, or reconnect logic anywhere in `src/resilience/`**. Verified: `grep -rn "retry\|backoff\|reconnect\|circuit.breaker" src/resilience/` returns nothing. The brief's expectation matches what the original "Multi-Agent Resilience" doc-headers promised (lines like "Risk level for task recovery decisions" in `types.rs:97`), but the code only persists state — it does not act on it.

**Suggested fix:** Either rename the module (e.g. `persistence`) to match the actual scope, OR scope-up by adding the retry/reconnect logic. The current name sets false expectations.

---

## Not Findings (Confirmed Wired)

The following are the **actually-wired** surface — flagged here to clarify what NOT to flag in future audits:

- `StateDatabase::new` / `new_with_dim` / `in_memory` — used in `bin/aleph-server/.../subsystems.rs:265`, `agent_init/mod.rs:822`, tests, and gateway `set_state_database`.
- `task_traces`: `insert_trace`, `get_traces_by_task`, `list_trace_tasks_paged`, `aggregate_usage_by_agents`, `aggregate_moa_advisor_usage` — wired via `execution_engine/callback.rs`, `handlers/trace_replay.rs`, `handlers/teams/snapshot.rs`, `builtin_tools/team/usage.rs`.
- `agent_tasks`: `insert_agent_task`, `update_task_status`, `get_agent_task`, `reconcile_orphaned_tasks` — wired via `execution_engine/persistence.rs`, `handlers/trace_replay.rs`, `agent_init/mod.rs`.
- `memory_events`: `append_memory_event(s)`, `get_memory_events_*` — heavily wired via `memory/events/{handler,traveler,migration}.rs`.
- `group_chat_*` CRUD — wired via `group_chat/orchestrator.rs`, `group_chat/executor.rs`.
- `channel_offsets` CRUD — wired via `gateway/interfaces/telegram/offset.rs`.
- `sticker_descriptions` CRUD — wired via `gateway/interfaces/telegram/sticker.rs`.
- `AgentTask::with_lane` — wired by `execution_engine/persistence.rs:38`.
- `Lane::Main`, `Lane::Subagent` — wired by `gateway/orphan_notice.rs`.
- `RiskLevel::High` — wired by `execution_engine/persistence.rs:36`.
- `TaskStatus::Running / Completed / Failed / Interrupted` — all four written by `execution_engine/{persistence,fast_path,execute}.rs`.
- `AgentUsageTotal::cache_hit_ratio` — wired by `builtin_tools/team/usage.rs:171`, `gateway/handlers/teams/snapshot.rs:307`.
- `migrate_task_traces_to_agent_trace` savepoint + orphan filter — tested and protects real boot paths.

## Cross-Module Notes (not findings, but interesting)

- `src/lib.rs:220` re-exports `MemoryStats` for backwards-compat — this is now dead and should be removed with the struct (finding #3).
- The `agent_tasks` table has `created_at`, `updated_at` written by the API, but the `tasks.rs` SELECT statements don't return them in some paths — verify by reading the column list. (Not a finding — schema matches API.)
- `TaskTrace::with_timestamp` was added in commit `983fc10a0` (post-fix) — that addition is wired via tests at `types.rs:560`.

## Negative-Space (What this review did NOT cover)

- No runtime test was executed; this is purely static. Memory-pressure / OOM paths in bulk-insert are not exercised here.
- No benchmark was run; latency claims in doc-comments are taken on faith.
- No cross-process locking test was performed; the `Mutex` recovery via `unwrap_or_else(|e| e.into_inner())` is assumed correct.
- The new test directory `tests/gateway_trace_replay_rpc.rs` was not opened; production wiring through that integration path was confirmed via grep but not behaviorally verified.