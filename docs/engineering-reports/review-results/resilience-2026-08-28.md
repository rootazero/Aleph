# Logic Review Report — src/resilience
**Date**: 2026-08-28
**Mode**: strict
**Files reviewed**:
- `src/resilience/mod.rs`
- `src/resilience/types.rs`
- `src/resilience/database/mod.rs`
- `src/resilience/database/state_database/mod.rs`
- `src/resilience/database/state_database/schema.rs`
- `src/resilience/database/state_database/tests.rs`
- `src/resilience/database/migration.rs`
- `src/resilience/database/tasks.rs`
- `src/resilience/database/traces.rs`
- `src/resilience/database/memory_events.rs`
- `src/resilience/database/group_chat.rs`
- `src/resilience/database/channel_offsets.rs`

**Prior audit (`resilience.md`, 2026-05-31)**: covered type-cast safety (i64 overflow) and DRY row-mapping. Those are still clean (overflows now use `i64::try_from(...).map_err(...)`; helper extractors remain in place).

This audit focuses on state-machine correctness, SQL safety, lock hierarchy, dead code, and missing wiring.

---

## Findings — Verified from prior partial run

### [Critical] `set_channel_offset` unconditionally overwrites `last_update_id`
- **Location**: `src/resilience/database/channel_offsets.rs:51-55` (also `:37` doc comment)
- **Trigger condition**: a writer with an out-of-order or stale `update_id` calls `set_channel_offset` after a higher value has already been persisted.
- **Expected behavior**: `last_update_id` is monotonic across all writers and restart cycles.
- **Actual behavior**: `INSERT OR REPLACE` overwrites whatever value is in the row, so a stale writer can regress the offset. The caller-side `OffsetTracker::advance` (gateway/interfaces/telegram/offset.rs:53-92) guards against this in-process via CAS, but does NOT guard across processes or across restart-then-replay; an out-of-band writer (e.g., a future backfill job, or a debug replay tool) can clobber a higher offset.
- **Severity rationale**: regression causes **message re-processing or duplication on Telegram** — the very failure mode the persistence layer was built to prevent.
- **Suggested fix**:
  ```sql
  INSERT INTO channel_offsets (channel_id, bot_id, last_update_id, updated_at)
  VALUES (?1, ?2, ?3, ?4)
  ON CONFLICT(channel_id) DO UPDATE SET
    last_update_id = MAX(last_update_id, excluded.last_update_id),
    bot_id         = excluded.bot_id,
    updated_at     = excluded.updated_at;
  ```
  (Keep `bot_id` assignment unconditional — it is metadata, not the cursor.)

---

### [Critical] `get_memory_events_since_seq` lacks actor filter and LIMIT
- **Location**: `src/resilience/database/memory_events.rs:185-217`
- **Trigger condition**: a non-wildcard caller (an agent or external tool) calls this method with a `since_seq` they observed for their own fact.
- **Expected behavior**: the response is scoped to events the caller is authorized to read; the page size is bounded.
- **Actual behavior**:
  1. No `actor` filter — compare `get_memory_events_for_fact` (line 149) which accepts an `agent_id` and filters with `(?2 = '' OR actor = ?2)`. `since_seq` is the cross-agent variant of the same query and inherits the read-side authorization gap.
  2. No `LIMIT` — an unbounded number of rows materializes per call. The sister `get_memory_events_in_range` (line 234) bounds itself with a `LIMIT ?3`.
- **Severity rationale**: cross-agent event leak (privacy) + memory blowup. The sister method `get_memory_events_for_fact` deliberately gates on `actor`; the parallel implementation bypasses that gate.
- **Suggested fix**: add the `agent_id` parameter and apply `(?N = '' OR actor = ?N)` plus a default `LIMIT 1000` (or accept it as a parameter, mirroring `get_memory_events_in_range`).

---

### [Critical] `list_trace_tasks_paged` uses strict `<` cursor — silently drops rows on collision
- **Location**: `src/resilience/database/traces.rs:326-376` (HAVING clause on `:355`)
- **Trigger condition**: two or more tasks share the same `MAX(timestamp)` (epoch seconds; `TaskTrace::new` stamps `Utc::now().timestamp()`).
- **Expected behavior**: pagination visits every task exactly once.
- **Actual behavior**: cursor advancement uses `HAVING MAX(timestamp) < ?1`. The cursor is the LAST entry's `last_timestamp` — equal to, not strictly less than, every other entry in the same second. So every task whose `last_timestamp == cursor` is **silently dropped** on the next page.
- **Severity rationale**: callers (`gateway/handlers/trace_replay.rs:278`) eventually report "trace history is incomplete"; in production the median second-resolution collision rate is high for short tasks (the existing test `list_paged_cursor_advances_without_overlap` at `:727` already documents this hazard — "rapid inserts collide").
- **Suggested fix**: cursor over a monotonic tie-breaker (compound `(timestamp, task_id)`), e.g.
  ```sql
  HAVING MAX(timestamp) < ?1
     OR (MAX(timestamp) = ?1 AND task_id > ?2)
  ORDER BY MAX(timestamp) DESC, task_id DESC
  LIMIT ?3
  ```
  (Tie-break requires the caller to thread a second cursor value.)

---

### [Critical] `reconcile_orphaned_tasks` has a window where newly-running tasks get silently clobbered
- **Location**: `src/resilience/database/tasks.rs:308-313`
- **Trigger condition**: a task transitions from non-`running` (e.g., `pending` or `swapped`) to `running` between `get_recoverable_tasks` and `mark_running_as_interrupted`.
- **Expected behavior**: tasks in `running` at boot get exactly one terminal-classification pass; nothing else is touched.
- **Actual behavior**: the two-step pattern (SELECT running → UPDATE WHERE status = 'running') runs across two `with_conn` invocations. The SELECT returns orphans at time T; the UPDATE applies at T+δ. Any task that became `running` in (T, T+δ] is mutated by the bulk UPDATE but is NOT in the returned `orphans` vec, so the reconciliation log / `orphan_notice` (gateway/orphan_notice.rs) silently misses a freshly-restarted task while also stamping it `interrupted`.
- **Severity rationale**: the daemon's boot path (`bin/aleph-server/.../agent_init/mod.rs:1319`) treats this list as the user-facing receipt; missing entries mean a real restart is misclassified.
- **Suggested fix**: do the SELECT and UPDATE in a single transaction with `RETURNING` semantics, or rewrite as:
  ```sql
  UPDATE agent_tasks
     SET status = 'interrupted', updated_at = ?1
   WHERE status = 'running'
  RETURNING id, parent_session_id, agent_id, task_prompt, status,
            risk_level, lane, checkpoint_snapshot_path, last_tool_call_id,
            recursion_depth, parent_task_id, created_at, updated_at,
            started_at, completed_at, metadata_json;
  ```
  Then map the rows via `agent_task_from_row`.

---

### [Critical] `update_task_status` silently no-ops on missing tasks, allows illegal transitions, and clobbers `completed_at`
- **Location**: `src/resilience/database/tasks.rs:154-216`
- **Trigger conditions**:
  1. Caller supplies a `task_id` that doesn't exist (or already matches `status`) → UPDATE matches 0 rows but the function returns `Ok(())`. The caller has no way to detect the miss.
  2. Caller asks for `Completed → Running`, `Pending → Swapped`, etc. — the DB layer permits any transition. Caller-side guard lives at `gateway/execution_engine/persistence.rs:53-67`, but it's defense-in-depth-thin and bypassed by any direct DB caller (e.g., a future debug tool).
  3. A second `update_task_status(.., Completed)` (or `Failed`) call **unconditionally overwrites** `completed_at` with `now`. The schema column never records "originally completed at T₀".
- **Expected behavior**: `Ok(())` should report whether the row was actually mutated; legal transitions only; `completed_at` preserved on idempotent updates.
- **Actual behavior**: `UPDATE … WHERE id = ?3` returns rowcount but it is discarded; the `completed_at` SET has no `AND completed_at IS NULL` guard (compare to the symmetric `started_at IS NULL` guard at `:188-191`).
- **Suggested fix**:
  1. Read `conn.execute(...)?` rowcount and return an error / typed result when zero.
  2. Either reject illegal transitions in the DB (CHECK constraint) or document the caller-only contract with a runtime check.
  3. Guard `completed_at` SET with `AND completed_at IS NULL`, mirroring the `started_at` pattern.

---

### [Critical] `task_traces` lacks `UNIQUE(task_id, step_index)` — replay-monotonic invariant can be violated
- **Location**: `src/resilience/database/state_database/schema.rs:226-235` (and the recreated copy in `migration.rs:79-87`)
- **Trigger condition**: caller invokes `insert_trace` or `bulk_insert_traces` twice for the same `(task_id, step_index)`, or supplies an out-of-order batch where a later batch repeats an earlier index.
- **Expected behavior**: the schema rejects duplicate / regressing `step_index` (the invariant `memory_events` enforces with `UNIQUE(fact_id, seq)` at schema.rs:80).
- **Actual behavior**: both inserts succeed. `get_traces_by_task` orders by `step_index ASC`, so a duplicate produces two rows with the same replay position — Shadow Replay re-applies the same event. The migration recreation (`migration.rs:79-87`) deliberately re-creates the table without UNIQUE.
- **Severity rationale**: monotonic replay invariant is the foundational promise of Shadow Replay; the asymmetry vs `memory_events` is almost certainly an oversight.
- **Suggested fix**:
  ```sql
  CREATE TABLE task_traces (
      ...,
      UNIQUE(task_id, step_index)
  );
  ```
  Apply to both the main schema and the migration recreation.

---

### [Warning] `in_memory()` does not enable `PRAGMA foreign_keys=ON`
- **Location**: `src/resilience/database/state_database/mod.rs:132-158`
- **Trigger condition**: a unit test inserts a row violating a declared `FOREIGN KEY` (e.g., a `task_traces.task_id` with no matching `agent_tasks.id`).
- **Expected behavior**: the in-memory DB enforces FK constraints just like the on-disk DB.
- **Actual behavior**: `Connection::open_in_memory()` (line 135) bypasses `crate::utils::sqlite_open::open_sqlite_safe`, which is the only place that runs `PRAGMA foreign_keys=ON` (utils/sqlite_open.rs:30). On-disk production DBs have FK enforcement; tests do not — meaning FK regressions can land undetected.
- **Severity rationale**: medium. Hidden regressions can affect production migrations (`migrate_task_traces_to_agent_trace` already deals with the legacy FK orphan problem — see `count_orphaned_legacy_traces` at migration.rs:60-69; that mitigation is real but not exhaustive).
- **Suggested fix**: in `in_memory()`, execute `PRAGMA foreign_keys=ON;` right after opening the connection, OR refactor so `in_memory` shares the same pragma setup as `open_sqlite_safe`.

---

### [Warning] `TaskStatus` state machine: `is_recoverable` says yes; reconciliation says no
- **Location**: `src/resilience/types.rs:62-65` (`is_recoverable`), `src/resilience/database/tasks.rs:255-279` (`get_recoverable_tasks`), `src/resilience/types.rs:280-290` (`should_auto_resume`)
- **Trigger condition**: any reboot where an `Interrupted` row exists in `agent_tasks`.
- **Expected behavior**: consistent — either `Interrupted` is recoverable (and reconciliation touches it) or terminal (and `is_recoverable` excludes it).
- **Actual behavior**: three different definitions coexist:
  1. `is_recoverable` — `Running | Interrupted`
  2. `should_auto_resume` — `is_recoverable() && risk_level == Low`
  3. `get_recoverable_tasks` SQL — `WHERE status = 'running'` only
  The reconciliation boot path (`bin/aleph-server/.../agent_init/mod.rs:1319`) treats `Interrupted` as terminal — exactly the docstring claim at `tasks.rs:303-307` ("orphans are not resumed; `interrupted` is a terminal state"). The type-layer `is_recoverable` says the opposite. Worse: `should_auto_resume` and `needs_resume_confirmation` have **zero production callers** (only tests at types.rs:446-447); they are vestigial and never drive a code path.
- **Severity rationale**: warning, not critical, because no production code consumes `should_auto_resume` today — the bug is dormant. It will become critical the moment someone wires `should_auto_resume` into a recovery loop.
- **Suggested fix**: align the definitions. Two clean choices:
  - Make `Interrupted` a terminal state and remove it from `is_recoverable` / `should_auto_resume`. Delete the dead methods if no caller materializes.
  - Make `Interrupted` resumable and have `get_recoverable_tasks` include it (and have the boot reconciliation either skip `interrupted` rows or re-mark them based on a separate "needs review" column).

---

### [Warning] All async DB methods serialize on a single connection mutex
- **Location**: `src/resilience/database/state_database/mod.rs:170-202` (`with_conn`)
- **Trigger condition**: any load above a handful of concurrent in-flight requests (multi-channel Telegram polling, parallel agent dispatch).
- **Expected behavior**: async writes progress in parallel up to the SQLite write-lock boundary.
- **Actual behavior**: every async method acquires `self.conn: Arc<Mutex<Connection>>` via `spawn_blocking`. The Mutex is the bottleneck — even on a multi-threaded blocking pool, all writes serialize. The `with_conn` doc comment at `:177-185` acknowledges this as Risk 4 of the review backlog.
- **Severity rationale**: warning, not critical — SQLite has a single-writer model anyway, but multi-reader / write-coalescing patterns (e.g., per-connection with `BEGIN IMMEDIATE` only when needed) would unlock significant throughput. The current design also makes deadlock-detection and cancellation awkward.
- **Suggested fix**: open with `OpenFlags::SQLITE_OPEN_FULL_MUTEX` and use a small r2d2-style pool (2-4 connections) keyed by an `Arc<Mutex<VecDeque<Connection>>>`. Alternatively, offload only writes to the pool and use the read methods directly (rusqlite's `Connection` is `!Sync` so the pool must enforce this).

---

### [Warning] `insert_group_chat_turn` lacks UNIQUE(session_id, round, sequence)
- **Location**: `src/resilience/database/state_database/schema.rs:256-266`
- **Trigger condition**: caller supplies a `(round, sequence)` that already exists, or an out-of-order write race in the orchestrator (`group_chat/executor.rs:150` is the sole caller).
- **Expected behavior**: a schema-level constraint surfaces the duplicate.
- **Actual behavior**: nothing in the schema prevents duplicate `(session_id, round, sequence)` rows. Replay / history (`get_group_chat_turns` at `:181-216`) returns them ordered ASC, so a duplicate is presented as two turns in the same position — confusing the conversation view.
- **Severity rationale**: same root cause as the `task_traces` UNIQUE miss.
- **Suggested fix**: `UNIQUE(session_id, round, sequence)` plus `idx_gc_turns_seq` on `(session_id, round, sequence)` for ordered lookups.

---

### [Warning] `update_group_chat_session_status` and `insert_group_chat_turn` accept free-form `&str`
- **Location**: `src/resilience/database/group_chat.rs:111-131` (status) and `:138-176` (turn `speaker_type`)
- **Trigger condition**: caller passes a typo or stale string for either field.
- **Expected behavior**: the DB layer refuses values not in the enum (`GroupChatStatus = {Active, Ended}` per `group_chat/protocol.rs:222-227`).
- **Actual behavior**: any string is stored. The orchestrator (`group_chat/orchestrator.rs:271`) correctly calls `.as_str()` to convert from the enum, but a typo in any other call site silently writes `"activ"` or `"completeed"`.
- **Severity rationale**: silent data corruption; downstream filtering (e.g., `list_active_group_chats` WHERE `status = 'active'` at group_chat.rs:228) would skip such sessions.
- **Suggested fix**: take the typed enum (`status: GroupChatStatus`, `speaker_type: GroupChatSpeaker`) and convert at the boundary, OR add CHECK constraints to the DDL.

---

### [Warning] `MemoryEventEnvelope::new` accepts a `seq` that the DB ignores, leaving the in-memory envelope stale
- **Location**: `src/memory/events/mod.rs:282-298` (constructor) consumed at `src/resilience/database/memory_events.rs:31-71` (`append_memory_event`)
- **Trigger condition**: caller constructs `MemoryEventEnvelope::new(fact_id, 1, …)` twice in a row; both envelopes have `seq = 1` in memory, but the second insert persists with `seq = 2`.
- **Expected behavior**: the envelope's `seq` matches what was persisted.
- **Actual behavior**: the INSERT is `INSERT INTO memory_events (..., seq, ...) SELECT ?1, COALESCE((MAX(seq) WHERE fact_id = ?1),0) + 1, …` — `seq` is computed inline, never read from a parameter. `conn.last_insert_rowid()` returns `id`, not `seq`. The envelope's `.seq` is untouched, and the docs explicitly tell callers to "re-read via `get_memory_events_for_fact`" — but most callers won't bother.
- **Severity rationale**: subtle correctness footgun. The atomic allocation prevents the UNIQUE-collision race (verified by `test_concurrent_append_assigns_unique_seqs` at `:790-841`), but the per-envelope `seq` field becomes a misleading token.
- **Suggested fix**: change the constructor to omit `seq` (allocate on read), or have `append_memory_event` rehydrate the envelope's `seq` via a follow-up `SELECT seq WHERE id = last_insert_rowid()` before returning it (single statement, e.g. via `RETURNING seq` on SQLite ≥3.35).

---

### [Warning] `in_memory()` does not call `drop_obsolete_tables`
- **Location**: `src/resilience/database/state_database/mod.rs:132-158` (function body) vs `:241` and `:293` (production paths)
- **Trigger condition**: a test starts using `in_memory()` while obsolete tables still exist in the source-of-truth schema (none today, but the migration table is growing).
- **Expected behavior**: every DB construction path applies the same DDL cleanup.
- **Actual behavior**: `new()` and `new_with_dim()` both call `drop_obsolete_tables` (lines 241, 293); `in_memory()` does not. If a future migration drops a real table that today has only in-memory test data, those tests will diverge.
- **Severity rationale**: low today; a future drift bomb.
- **Suggested fix**: extract a shared `bootstrap_in_memory(conn: &Connection)` helper and call it from both `in_memory()` and the new() paths.

---

## Findings — New

### [Warning] `task_traces.task_id` FK has no `ON DELETE` policy → orphan rows on parent deletion
- **Location**: `src/resilience/database/state_database/schema.rs:233` (`FOREIGN KEY(task_id) REFERENCES agent_tasks(id)`)
- **Trigger condition**: an `agent_tasks` row is deleted while `task_traces` rows reference it (with `foreign_keys=ON` in production, deletion errors — no `delete_task` exists today, so this is dormant).
- **Expected behavior**: either cascade-delete traces, restrict deletion, or set the FK column to NULL — explicit choice.
- **Actual behavior**: the FK has no `ON DELETE` clause. With `foreign_keys=OFF` (the default for in-memory tests, per finding #7 above), nothing rejects the deletion. With `foreign_keys=ON` (production), deletion is rejected by SQLite, but the error message is opaque. The legacy migration at `migration.rs:60-69` even has a `count_orphaned_legacy_traces` helper — evidence that the orphan case has actually happened in real databases.
- **Suggested fix**: declare the intent explicitly. `ON DELETE CASCADE` is the natural choice (trace lifetime ⊆ task lifetime) but requires documenting that "deleting a task means erasing its forensic history". `ON DELETE RESTRICT` is safer for forensic integrity.

---

### [Warning] `task_traces.step_index` is signed INTEGER with no CHECK
- **Location**: `src/resilience/database/state_database/schema.rs:229`
- **Trigger condition**: a direct-SQL caller (test, future migration, ad-hoc admin script) writes a negative `step_index`.
- **Expected behavior**: rejected by the schema.
- **Actual behavior**: `INTEGER` is signed in SQLite; the column accepts negative values. `row.get::<_, u32>(2)?` in `task_trace_from_row` (traces.rs:71-93) would surface this as a rusqlite conversion error on read, but only after the bad row exists.
- **Suggested fix**: `step_index INTEGER NOT NULL CHECK (step_index >= 0)`. (Combine with the UNIQUE constraint from finding #6 in a single ALTER-TABLE migration.)

---

### [Warning] `event_kind`/`event_json` mismatch in `task_traces` only warns — silent data corruption
- **Location**: `src/resilience/database/traces.rs:79-87`
- **Trigger condition**: an inserted row has `event_kind = "text_emitted"` but `event_json` deserializes to a different `AgentTraceEvent` variant.
- **Expected behavior**: either reject on insert (CHECK) or refuse on read.
- **Actual behavior**: `task_trace_from_row` parses `event_json`, compares `event.kind()` against `event_kind`, and emits only a `tracing::warn!` on mismatch. The row passes through unchanged. Downstream consumers (`list_trace_tasks_paged`, `aggregate_usage_by_agents` which uses `WHERE event_kind = 'provider_usage'` at traces.rs:438) silently skip or include the wrong rows.
- **Suggested fix**: a stronger contract — either compute `event_kind` from `event_json` on insert and refuse to take a separate parameter, or add a CHECK constraint comparing the two (not enforceable directly, but a runtime assertion in the insert path catches drift).

---

### [Warning] `MemoryEventRow::into_envelope` silently skips unparseable rows
- **Location**: `src/resilience/database/memory_events.rs:431-454`
- **Trigger condition**: a stored row's `event_json` no longer matches any `MemoryEvent` variant (legacy data after a variant rename that lacked an `#[serde(alias)]`).
- **Expected behavior**: replay surfaces the corruption to the operator (count, sample, last-N-ids) rather than dropping rows invisibly.
- **Actual behavior**: a `tracing::warn!` is emitted and the row is skipped via `return Ok(None)`. The caller (`get_memory_events_for_fact` at `:165-172`) pushes a `None` and continues; no metric, no counter, no surfaced alert.
- **Suggested fix**: maintain a `count_unparsed` atomic in `StateDatabase` (or a struct-method companion) so callers can report "N rows skipped during replay" in their operational logs. The existing test `replay_skips_unknown_event_variants` at `:699` already exercises the path; pairing it with a counter assertion is straightforward.

---

### [Warning] `should_auto_resume` / `needs_resume_confirmation` are dead public API
- **Location**: `src/resilience/types.rs:280-290`
- **Trigger condition**: none today — no production caller exists.
- **Expected behavior**: either a documented part of the recovery surface, or absent.
- **Actual behavior**: only `types.rs:446-447` references them — both in unit tests. The methods are `pub` and expose a recovery decision the rest of the system has not wired up. They will mislead the next contributor who reads the resilience module surface as "what the daemon can do".
- **Suggested fix**: either (a) mark `#[allow(dead_code)]` with a comment naming the planned integration site, or (b) delete them. Combined with finding #8, choose a recovery policy and remove the contradiction.

---

### [Warning] `aggregate_moa_advisor_usage` hardcodes the `'moa:%'` naming pattern
- **Location**: `src/resilience/database/traces.rs:511-549`
- **Trigger condition**: the MoA adapter renames its synthetic `agent_id` prefix (e.g., to `moa_advisor:` for clarity).
- **Expected behavior**: a single source of truth.
- **Actual behavior**: the LIKE pattern is a string literal in this function. A separate constant or `pub const MOA_AGENT_ID_PREFIX: &str = "moa:"` should be defined and used both here and at the producer (`MeteringProvider` for MoA advisors) — otherwise the producer and aggregator drift silently.
- **Suggested fix**: hoist the prefix to a `pub const` and reference it on both sides. (Pure refactor; no behavior change.)

---

## Suggested Tests

### [Suggested Test] `set_channel_offset_rejects_regression`
```rust
#[tokio::test]
async fn set_channel_offset_rejects_regression() {
    let db = StateDatabase::in_memory().unwrap();
    db.set_channel_offset("chan-1", "bot-1", 100).await.unwrap();
    // Even an out-of-order writer must NOT regress the persisted offset.
    db.set_channel_offset("chan-1", "bot-1", 50).await.unwrap();
    assert_eq!(db.get_channel_offset("chan-1").await.unwrap(), Some(100));
    // Equal is allowed (idempotent ack).
    db.set_channel_offset("chan-1", "bot-1", 100).await.unwrap();
    assert_eq!(db.get_channel_offset("chan-1").await.unwrap(), Some(100));
    // Strict monotonic write must succeed.
    db.set_channel_offset("chan-1", "bot-1", 200).await.unwrap();
    assert_eq!(db.get_channel_offset("chan-1").await.unwrap(), Some(200));
}
```

### [Suggested Test] `get_memory_events_since_seq_filters_by_actor_and_bounds_results`
```rust
#[tokio::test]
async fn get_memory_events_since_seq_is_actor_scoped_and_bounded() {
    let db = StateDatabase::in_memory().unwrap();
    // 1500 events owned by Agent, 1500 by User, all on the same fact_id.
    for i in 0..1500u64 {
        let actor = if i % 2 == 0 { EventActor::Agent } else { EventActor::User };
        let env = MemoryEventEnvelope::new("shared".into(), i, make_created_event("shared"), actor, None);
        db.append_memory_event(&env).await.unwrap();
    }
    // An Agent-only caller must NOT see User rows.
    let agent_only = db.get_memory_events_since_seq_scoped("shared", 0, "agent").await.unwrap();
    assert!(agent_only.iter().all(|e| matches!(e.actor, EventActor::Agent)));
    assert_eq!(agent_only.len(), 750);
    // The unbounded call must not blow up at >1000 rows.
    let all = db.get_memory_events_since_seq("shared", 0).await.unwrap();
    assert_eq!(all.len(), 1500);
}
```

### [Suggested Test] `list_trace_tasks_paged_visits_every_task_under_timestamp_collision`
```rust
#[tokio::test]
async fn list_trace_tasks_paged_does_not_drop_tasks_with_collision() {
    let db = StateDatabase::in_memory().unwrap();
    // Five tasks whose only trace has the SAME timestamp (forces a collision).
    let pinned_ts = 1_700_000_000i64;
    for i in 0..5 {
        let tid = format!("task-{i}");
        db.insert_agent_task(&AgentTask::new(&tid, "s", "coder", "x", RiskLevel::Low))
            .await
            .unwrap();
        let trace = TaskTrace {
            id: 0,
            task_id: tid,
            step_index: 0,
            event: AgentTraceEvent::TextEmitted {
                iteration: 0,
                stream: AgentTraceTextKind::Final,
                text: "x".into(),
            },
            timestamp: pinned_ts,
        };
        db.insert_trace(&trace).await.unwrap();
    }

    // Paginate with limit=2: must visit all 5 across 3 pages, no drops, no dupes.
    let mut seen: Vec<String> = Vec::new();
    let mut cursor: Option<i64> = None;
    loop {
        let page = db.list_trace_tasks_paged(2, cursor).await.unwrap();
        if page.is_empty() { break; }
        cursor = Some(page.last().unwrap().last_timestamp);
        for info in &page {
            assert!(!seen.contains(&info.task_id), "duplicate task_id {}", info.task_id);
            seen.push(info.task_id.clone());
        }
        if page.len() < 2 { break; }
    }
    assert_eq!(seen.len(), 5, "all 5 tasks must be visited; got {seen:?}");
}
```

### [Suggested Test] `reconcile_orphaned_tasks_captures_concurrent_becoming_running`
```rust
#[tokio::test]
async fn reconcile_orphaned_tasks_marks_becoming_running() {
    let db = StateDatabase::in_memory().unwrap();
    // Insert a Pending task, then have a background thread mark it Running
    // between SELECT and UPDATE. The reconcile must either capture it in
    // the returned list OR leave it Running — never silently flip it to
    // Interrupted without reporting.
    db.insert_agent_task(&AgentTask::new("race-1", "s", "a", "p", RiskLevel::Low)).await.unwrap();

    let db_clone = std::sync::Arc::new(db);
    let race_db = db_clone.clone();
    let racy_handle = tokio::spawn(async move {
        // Sleep briefly to land between SELECT and UPDATE in reconcile_orphaned_tasks.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        race_db.update_task_status("race-1", TaskStatus::Running).await.unwrap();
    });

    let orphans = db_clone.reconcile_orphaned_tasks().await.unwrap();
    racy_handle.await.unwrap();
    // After reconciliation, the task must be either:
    //   (a) returned in orphans AND marked interrupted, OR
    //   (b) still running (the race window closed before the UPDATE).
    let final_status = db_clone.get_agent_task("race-1").await.unwrap().unwrap().status;
    let was_returned = orphans.iter().any(|t| t.id == "race-1");
    assert!(
        (was_returned && final_status == TaskStatus::Interrupted)
            || (!was_returned && final_status == TaskStatus::Running),
        "unexpected state: was_returned={was_returned}, final_status={final_status:?}"
    );
}
```

### [Suggested Test] `task_traces_rejects_duplicate_step_index_via_unique_constraint`
```rust
#[tokio::test]
async fn task_traces_unique_constraint_rejects_duplicate_step_index() {
    let db = StateDatabase::in_memory().unwrap();
    db.insert_agent_task(&AgentTask::new("task-1", "s", "a", "p", RiskLevel::Low))
        .await
        .unwrap();
    let trace = TaskTrace::new(
        "task-1", 0,
        AgentTraceEvent::TextEmitted {
            iteration: 0,
            stream: AgentTraceTextKind::Final,
            text: "x".into(),
        },
    );
    db.insert_trace(&trace).await.unwrap();
    // A second trace at the same (task_id, step_index) must fail with UNIQUE.
    let dup = TaskTrace::new(
        "task-1", 0,
        AgentTraceEvent::TextEmitted {
            iteration: 0,
            stream: AgentTraceTextKind::Final,
            text: "y".into(),
        },
    );
    let err = db.insert_trace(&dup).await;
    assert!(err.is_err(), "UNIQUE(task_id, step_index) must fire on duplicate step_index");
    let traces = db.get_traces_by_task("task-1").await.unwrap();
    assert_eq!(traces.len(), 1, "only the first trace must persist");
}
```

---

## Summary

| Level | Count |
|-------|-------|
| Critical | 6 |
| Warning | 9 |
| Suggested Test | 5 |

## Notes

- Verified fixes from the prior `resilience.md` audit (i64 try_from conversions, DRY row extractors, lock-poisoning recovery) are still in place and clean.
- The `events.rs` file referenced by the prior audit's "AgentEvent row mapping" no longer exists in `src/resilience/database/`; the current module is `memory_events.rs` only. Anything still mentioning `agent_events` is dead.
- `task_traces` and `memory_events` use different constraints for what is conceptually the same invariant (per-entity monotonic sequence). This asymmetry is the proximate cause of findings #6 and #10 — one is critical, one warning, but both deserve the same UNIQUE treatment.
- The reconciliation / state-machine drift (findings #4 and #8) compounds: a freshly-running task gets silently interrupted (#4), and even if it survived, the dormant `should_auto_resume` path would never have caught it (#8). Fixing #4 via `RETURNING` is the higher leverage change; #8 is a cleanup that becomes urgent if anyone wires `should_auto_resume` into a recovery loop.
- `in_memory()` divergence (findings #7 and #13) is small in absolute terms but the kind of drift that lets FK / migration regressions land unnoticed. Both fixes are mechanical.
- The dead public API (`should_auto_resume`, `needs_resume_confirmation`) is not a bug today, but the comment-free `pub fn` invites a contributor to wire it up without realizing the recovery story disagrees with `is_recoverable`. Recommend marking `#[allow(dead_code)]` with a TODO pointing to the chosen resolution of #8, or deleting outright.