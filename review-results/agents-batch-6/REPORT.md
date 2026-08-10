# Review Report — Batch 6 (Swarm SQLite task store)

**Scope:** `src/agents/swarm/tasks/store/{mod,crud,schema,journal,row_decode,runs,locks,deps,comments,helpers}.rs`
**Date:** 2026-08-10
**Reviewer:** static (4-perspective protocol)

## Summary

| Severity | Count |
|----------|-------|
| Critical | 0 |
| High     | 1 |
| Medium   | 4 |
| Low      | 9 |
| **Total** | **14** |

**Clean bills of health (verified, not assumed):**

- **No SQL injection.** Every user-controlled value is bound. The three sites that build SQL with `format!` — `crud.rs::update_task` (SET list), `crud.rs::list_tasks` (WHERE list), `runs.rs::abandon_orphaned_runs` (IN placeholders) — interpolate only `?N` placeholders and literal column names; values go through `ToSql`. `schema.rs::add_column_if_missing` interpolates identifiers but validates them with `is_safe_identifier`/`is_safe_type_decl` first, and all call sites pass literals.
- **Foreign keys are enforced.** `PRAGMA foreign_keys = ON` is issued in `migrate()` on the same long-lived `Connection` the store keeps (connection-scoped, so it persists), and `coord.db` is opened exactly once (`agent_init/coord_stores.rs:41`). `delete_team_tasks`' reliance on `ON DELETE CASCADE` is therefore sound.
- **No lock-ordering / reentrancy hazard from `connection_handle()`.** The only production consumer is `SqliteSnapshotStore::new_from_shared` (`teams/snapshots/store.rs:38`). All five of its methods acquire and release the mutex within one statement; `snapshots/operations.rs` calls `coord_store.list_tasks(...)` *before* `snapshot_store.insert(...)`, never nested. There is no path that holds the connection mutex across a call back into the other store, so the non-reentrant `tokio::sync::Mutex` cannot self-deadlock.
- **Migrations are forward-compatible.** No `DROP` outside the guarded one-shot legacy `coord_teams` rebuild; new columns go through `add_column_if_missing`; all `CREATE` are `IF NOT EXISTS`.
- **The run-row janitor is race-free as documented.** `dispatcher/schedule/mod.rs` inserts into `self.running` *before* spawning (line 178) and removes *after* `finish_task_run` (line 363 → 484), so a live run row is always covered by `live_task_ids`. `abandon_orphaned_runs` is called from exactly one globally-unique `TeamDispatcher`, so its `NOT IN (live)` sweep cannot close a sibling's rows.
- **`GROUP_CONCAT(... ORDER BY ...)`** (crud.rs:264) needs SQLite ≥ 3.44; `Cargo.toml:47` pins `rusqlite = { version = "0.37", features = ["bundled"] }`, so the version is in-tree. The comment's claim checks out.
- **`helpers.rs`** is correct: `summarize` truncates on `chars()` (UTF-8 safe, cf. P7), `now_epoch` uses `unwrap_or`, `db_err` is total.
- **`comments.rs`** is correct: fully parameterised, indexed by `(task_id, created_at)`, no unwrap.

---

## Findings

### [HIGH] crud.rs:67 — `COMMIT` failure leaves an open transaction on the process-wide shared connection
**Category:** Logic / Quality
**Confidence:** High

**Description:** `create_task` drives the transaction with raw `BEGIN`/`COMMIT` strings. The error arm (line 69-72) issues a `ROLLBACK`, but the `Ok(())` arm does not:

```rust
Ok(()) => { conn.execute("COMMIT", []).map_err(db_err)?; }   // ← `?` returns with the txn still open
```

If `COMMIT` fails (`SQLITE_BUSY`, `SQLITE_FULL`, an I/O error, or a deferred-FK violation surfacing at commit time), the function returns `Err` **without rolling back**. The connection is not per-request — it is a single `Arc<Mutex<Connection>>` created once at boot (`agent_init/coord_stores.rs:41`) and shared with `SqliteSnapshotStore`. A dangling transaction therefore persists for the lifetime of the process: every subsequent `create_task` fails at `BEGIN` with *"cannot start a transaction within a transaction"*, and every other write on that connection (snapshot insert, `update_task`, run rows, comments) silently joins the never-committed transaction. The failure mode is permanent and self-amplifying, and it is only reachable through the one error path nobody exercises.

**Suggested fix:** Use rusqlite's RAII transaction instead of literal SQL — the guard derefs to `&mut Connection`, so `unchecked_transaction()` is available and its `Drop` rolls back on any early return:

```rust
let tx = conn.unchecked_transaction().map_err(db_err)?;
tx.execute(INSERT_TASK, params![...]).map_err(db_err)?;
for dep_id in &input.blocked_by { tx.execute(INSERT_DEP, params![id, dep_id]).map_err(db_err)?; }
tx.commit().map_err(db_err)?;   // rusqlite rolls back on Drop if this is skipped
```
Minimum fix if the literal form is kept: `if let Err(e) = conn.execute("COMMIT", []) { let _ = conn.execute("ROLLBACK", []); return Err(db_err(e)); }`.

---

### [MEDIUM] crud.rs:147-153 — `metadata` is a whole-blob overwrite with no CAS; concurrent stampers lose updates silently
**Category:** Logic (data integrity)
**Confidence:** High

**Description:** `update_task` writes `metadata` as a single replacing column value. The store offers no version/etag, no `json_patch`, and no compare-and-set, so every caller must do read-modify-write against a snapshot it fetched earlier. There are at least **nine** independent producers doing exactly that, spread across three subsystems that run concurrently:

- dispatcher tick — `schedule/settle.rs:206,329`, `schedule/reclaim.rs:285,461`, `schedule/failure.rs:135`, `dispatcher/clarify.rs:78,130`
- model tool calls — `builtin_tools/workflow_tool.rs:935,1046,1074,1105,1181,1227`, `builtin_tools/team/task_control.rs:201,234`
- panel/RPC — `gateway/handlers/teams/workflow.rs:584,640`

Concretely: the scheduler stamps `stale_review_warned_at` on a `WaitingReview` task (`reclaim.rs:285`) from a snapshot taken at the top of the tick, while the operator's `teams.workflow.approve_step` RPC stamps its own marker from its own snapshot. Last writer wins the whole blob. The lost key is not noticed anywhere — it produces a duplicate "Team work complete" notification (lost `workflow_notified`) or a resume that re-executes instead of restoring the review gate (lost `PAUSED_FROM_KEY`). Nothing errors and nothing logs.

Note this is the same shape the code's own comment at `crud.rs:178-184` is worried about, but that comment only addresses the *broadcast verb*, not the write itself.

**Suggested fix:** Either (a) add an `expected_metadata_rev`/`updated_at` guard to `CoordTaskUpdate` and make the UPDATE conditional (`WHERE id = ? AND metadata_rev = ?`, returning a typed conflict on `affected == 0`), or (b) expose a merge-shaped write at the store boundary that does the read-modify-write **inside the connection mutex**, e.g. `update_task_metadata_patch(id, patch: &Value)` implemented with SQLite's `json_patch(metadata, ?)`. (b) is the smaller change and makes every existing `merge_metadata_patch` caller correct by construction.

---

### [MEDIUM] schema.rs:112-118 — no index on `coord_task_dependencies(depends_on)`; three hot paths full-scan the edge table
**Category:** Quality (efficiency) / Logic
**Confidence:** High

**Description:** The edge table's only index is the implicit `PRIMARY KEY (task_id, depends_on)` autoindex. By the leftmost-prefix rule that index cannot serve any lookup keyed on `depends_on`, and there is no second index. Three production paths are keyed exactly that way:

- `deps.rs:25` — `get_dependents`: `WHERE depends_on = ?1` → full scan of the edge table, per call.
- `deps.rs:48` — `get_newly_unblocked`: `JOIN coord_task_dependencies d ON d.task_id = t.id WHERE d.depends_on = ?1` → same, and this runs on **every** task completion.
- `schema.rs:117` — the `FOREIGN KEY (depends_on) REFERENCES coord_tasks(id) ON DELETE CASCADE` child scan. SQLite has no index to find matching child rows, so `delete_team_tasks` (`crud.rs:339`) degrades to O(deleted_tasks × total_edges).

**Suggested fix:** Add to the standard-schema batch (idempotent, additive, no rebuild):
```sql
CREATE INDEX IF NOT EXISTS idx_coord_task_deps_depends_on
    ON coord_task_dependencies(depends_on);
```

---

### [MEDIUM] crud.rs:27 — the create-time cycle check is structurally vacuous, and it is not free
**Category:** Architecture (always-true predicate) / Quality
**Confidence:** High

**Description:** `create_task` generates a fresh `Uuid::new_v4()` at line 18 and then calls `check_no_cycle_sync(&conn, &id, &input.blocked_by)`. That function (`dag.rs:57`) BFS-walks the ancestors of `blocked_by` looking for `new_task_id`. Because `id` was minted three lines earlier, **no row in `coord_task_dependencies` can possibly reference it**, so the BFS can never hit the `current == new_task_id` branch — the guard can never return `Err`. This is CLAUDE.md §0's "恒真的谓词等于没判": a predicate whose only observable behaviour is its cost.

And the cost is real: the BFS issues one `prepare_cached` query **per visited ancestor node**, inside the connection mutex, on every single create. `workflow::compile` materialises a chain of tasks each depending on the previous, so materialising an *n*-step template is O(n²) edge queries with the global DB lock held.

The underlying invariant is fine — edges are immutable after creation (`dag.rs:3-5`), and a node with no incoming edges cannot close a cycle — so the DAG really is acyclic by construction. The problem is that the code presents a runtime guard where the actual guarantee is structural, which both costs and misleads (a future "add dependency to an existing task" path would look protected when it is not).

**Suggested fix:** Drop the call from `create_task` and record the real invariant in a doc comment on `NewCoordTask::blocked_by` ("acyclic by construction: edges are only ever inserted at creation, when the node has no dependents"). Keep `check_no_cycle_sync` and wire it to the first path that mutates edges on an existing task — that is where it stops being vacuous. If it is kept as belt-and-braces, at minimum bound it and say in the doc that it is unreachable today.

---

### [MEDIUM] crud.rs:202-330 — `list_tasks` is unbounded: no `LIMIT`, no cursor, no row cap
**Category:** Quality (unbounded growth)
**Confidence:** High

**Description:** `CoordTaskFilter` carries only `status` and `team_id`. `list_tasks` materialises **every** matching row, each carrying its full `description`, `result` and `metadata` JSON, plus a `GROUP_CONCAT` of its dependency ids, and decodes them all into a `Vec<CoordTask>` while holding the global connection mutex. `coord_tasks` is never pruned except by explicit team deletion, so a long-lived team's task table only grows.

Two consequences: (1) the mutex is held for the whole decode, blocking every other coord/snapshot operation including the dispatcher tick; (2) callers that only need ids pay the full price — e.g. `gateway/handlers/teams/crud.rs:189` calls `list_tasks` purely to `map(|t| t.id)` for artifact cleanup, and `snapshots/operations.rs:83` embeds the whole result in a snapshot payload.

Also note the post-query filter (lines 317-325) discards rows *after* SQL returned them, so a `Blocked` filter still pays for every `pending` row in the team — a `LIMIT` pushed into SQL would silently truncate before that pass, which is why the fix has to be a real cursor, not a bare `LIMIT`.

**Suggested fix:** Add `limit: Option<usize>` + `created_before: Option<u64>` (keyset cursor on the existing `ORDER BY priority_rank, created_at`) to `CoordTaskFilter`; apply the derived-status post-filter inside the paging loop so `Blocked`/`Unsatisfiable` pages stay correct. Separately, give the id-only callers a `list_task_ids(team_id)` that selects one column.

---

### [LOW] crud.rs:29 — metadata serialization failure is silently swallowed on create but errors on update
**Category:** Logic
**Confidence:** High

**Description:** `create_task` does `serde_json::to_string(&input.metadata).unwrap_or_else(|_| "{}".into())` — a failure silently replaces the caller's metadata with an empty object and the task is created as if nothing happened. `update_task:148` handles the identical operation by returning `db_err("failed to serialize metadata: …")`. Two answers to the same question; the create-side one loses data without a trace (not even a `warn!`).

**Suggested fix:** Make create match update — `serde_json::to_string(&input.metadata).map_err(|e| db_err(format!("failed to serialize metadata: {e}")))?`.

---

### [LOW] runs.rs:114, 115, 129 — `row.get(N).ok()` conflates NULL, type error, and missing column
**Category:** Quality (silent error swallowing)
**Confidence:** High

**Description:** `list_task_runs` reads the three Phase-C columns as `row.get(8).ok()` / `row.get(9).ok()` / `row.get(10).ok()`. Type inference makes these `Result<String>` → `Option<String>`, so a NULL column produces `Err(InvalidColumnType)` which `.ok()` maps to `None`. It happens to give the right answer for NULL, but it also swallows a genuine decode failure and a missing-column error identically. Compounding it, lines 125-128 pass the strings through `ReviewVerdict::from_stored` / `ReviewerKind::from_stored`, which return `None` for an unrecognised value — so a verdict written by a newer build reads back as "never reviewed", with no warning, on the audit trail whose whole job is to say who approved what.

Contrast `read_task_row` (row_decode.rs:18-37), which correctly raises `FromSqlConversionFailure` on an unknown status. Two policies for the same class of corruption.

**Suggested fix:** Use `row.get::<_, Option<String>>(8)?` for nullability, and log (`tracing::warn!`) when `from_stored` rejects a non-NULL value.

---

### [LOW] runs.rs:161 / runs.rs:98 — run ordering relies on second-granularity `started_at`; ties are resolved only by SQLite's scan order
**Category:** Logic
**Confidence:** High

**Description:** `started_at` is `now_epoch()` — whole seconds (`helpers.rs:9`). `record_run_review` stamps the verdict on `ORDER BY started_at DESC LIMIT 1` among ended runs, and `list_task_runs` returns `ORDER BY started_at ASC`, which is what `retry::recovery_abandons_since` and the panel drawer read. Two attempts of the same task starting inside the same second — routine for the busy-deferral path, which files an `Abandoned` row and immediately re-dispatches — are order-ambiguous. In practice SQLite resolves the tie by rowid via `idx_coord_task_runs_task(task_id, started_at)`, which happens to be insertion order, but that is an implementation detail of the query plan, not a guarantee: a plan change (or an added index) silently stamps the verdict on the wrong attempt.

**Suggested fix:** Make the tiebreak explicit — `ORDER BY started_at DESC, rowid DESC` in `record_run_review` and `ORDER BY started_at ASC, rowid ASC` in `list_task_runs`. (Storing millisecond timestamps would be the deeper fix but changes the column's unit — cf. the `MessageRecord.timestamp` ambiguity already recorded in CLAUDE.md §10.)

---

### [LOW] crud.rs:230-233 — `idx` is not incremented after the `team_id` clause
**Category:** Quality (latent bug)
**Confidence:** High

**Description:** Every other clause in `list_tasks`/`update_task` does `idx += 1` after pushing. The `team_id` branch does not, because it is currently last. The next filter appended to this builder will reuse the same `?N` and bind the wrong value — the query will still *run* (correct placeholder count is not checked against distinct indices), just against the wrong parameter, so the failure is a silently wrong result set.

**Suggested fix:** Add `idx += 1;` after line 232 (and `#[allow(unused_assignments)]` or a trailing `let _ = idx;` if clippy objects).

---

### [LOW] journal.rs:49 — the UPSERT overwrites `created_at`, so the column means "last written"
**Category:** Logic (naming/semantics drift)
**Confidence:** High

**Description:** `ON CONFLICT(task_id) DO UPDATE SET … created_at = excluded.created_at` replaces the original insert time on every rewrite. The column, the public `TaskExitJournal::created_at` field, and `list_team_journals`' `ORDER BY j.created_at DESC` (line 126) all read as creation time, but after the first retry rewrites the journal the value is an update time — so the "team journals, newest first" listing is actually ordered by most-recently-edited. The schema comment (`schema.rs:217-219`) says only the latest snapshot is canonical, which justifies the overwrite but not the name.

**Suggested fix:** Either keep the original (`created_at = coord_task_journals.created_at` in the DO UPDATE, i.e. don't touch it) and add a separate `updated_at`, or rename the field to `updated_at` throughout. Whichever way, `ORDER BY` should name the one that matches the caller's intent.

---

### [LOW] schema.rs:127-136 — the locking columns bypass `add_column_if_missing` and can half-apply
**Category:** Quality
**Confidence:** High

**Description:** `locked_by`/`locked_at` are added by a bespoke probe (`conn.prepare("SELECT locked_by … LIMIT 0").is_ok()`) plus a two-statement `execute_batch`, while `add_column_if_missing` — written for exactly this, six lines below at `schema.rs:165-167` — already exists. Two problems: the probe swallows *every* prepare error (a corrupt db or a missing table reads the same as "column absent"), and `execute_batch` is not transactional here, so if the second `ALTER` fails the table keeps `locked_by` without `locked_at`. The next migration run then sees `has_locked_by == true` and never retries — every subsequent `locks.rs` query fails on the missing column, permanently.

**Suggested fix:** Replace lines 127-136 with two `add_column_if_missing(conn, "coord_tasks", "locked_by", "TEXT")?` / `…"locked_at", "INTEGER"…` calls. Each is independently idempotent, so a partial application self-heals on the next boot.

---

### [LOW] schema.rs:83 — a failed legacy migration can leave FK enforcement OFF for the connection's lifetime
**Category:** Logic
**Confidence:** High

**Description:** The legacy rebuild disables `PRAGMA foreign_keys` (line 36) and restores it either on the success path (line 88) or through the best-effort batch `"ROLLBACK; PRAGMA foreign_keys = ON;"` (line 83). `execute_batch` aborts at the first failing statement, so if `ROLLBACK` errors — no transaction active, because the batch failed at `BEGIN` itself (e.g. a transaction was already dangling; see the HIGH finding) — the `PRAGMA` never runs and the connection keeps FK enforcement OFF. The subsequent `return Err` does currently prevent the store from being published (`coord_stores.rs:66` returns `(None, None)`), so today the damage is contained; the coupling is fragile and undocumented.

**Suggested fix:** Split into two calls so the pragma restore cannot be skipped:
```rust
let _ = conn.execute_batch("ROLLBACK;");
let _ = conn.execute_batch("PRAGMA foreign_keys = ON;");
```

---

### [LOW] locks.rs:19-20 — `acquire_lock` is not mutually exclusive against the same `agent_id`, and silently refreshes the staleness clock
**Category:** Logic
**Confidence:** High

**Description:** The claim is `WHERE id = ?3 AND (locked_by IS NULL OR locked_by = ?1)`. The `OR` arm makes a second acquire by the *same* agent id succeed and re-stamp `locked_at`. The dispatcher documents this call as an atomic claim that "loses harmlessly to a racing claimer" (`schedule/mod.rs:153`) — true for a different owner, false for the same one. Today the double-claim window appears closed by dispatcher-side bookkeeping (`select_schedulable` only sees `pending` rows and `self.running` gates re-selection), so this is a contract gap rather than a live bug; but the safety lives entirely outside the store, in a caller comment that asserts the opposite. The `locked_at` refresh is the second half: it means "how long has this been held" is really "how long since the last acquire attempt", which is the input `release_stale_locks` keys on.

**Suggested fix:** Split the two intents — `acquire_lock` should take the lock only when `locked_by IS NULL` and return a typed `AlreadyHeld(agent_id)`; add an explicit `renew_lock(task_id, agent_id)` for the refresh case if a caller actually needs it (none does today → R10 says don't add it until one does). At minimum, correct the dispatcher's comment.

---

### [LOW] locks.rs:90 — stale-lock expiry is wall-clock on both sides
**Category:** Logic
**Confidence:** High

**Description:** `locked_at` is `now_epoch()` at acquire and the cutoff is `now_epoch().saturating_sub(max_age_secs)` at sweep. Both are `SystemTime` (`helpers.rs:6`), so an NTP step backwards larger than `lock_ttl_secs` makes every existing lock look fresh and stale-release stops working until wall clock catches up; a forward step releases live locks early. There is no monotonic component and no heartbeat, so a task whose lock is wrongly released is re-claimable while its run is still executing — the exact scenario `select.rs:163-168` documents as producing a superseding claim.

**Suggested fix:** Not worth a clock redesign for a janitor, but the sweep should refuse a negative interval: skip the pass (and `warn!`) when `now_epoch()` is less than the maximum `locked_at` in the table, so a backwards jump degrades to "no expiry this tick" rather than a silent behaviour change. Document the wall-clock dependency on `release_stale_locks`.

---

## Cross-cutting observations

**1. Two separate error-handling policies for corrupt columns, and the boundary is arbitrary.** `read_task_row` (row_decode.rs) fails *hard* on an unknown status/priority/metadata — one bad row makes `list_tasks` return `Err` for the entire team. `journal.rs::decode_journal_field` and `runs.rs`'s `from_stored` calls fail *soft* — corrupt data reads as empty/absent, with a `warn!` in the journal case and nothing at all in the runs case. Both policies are defensible; having them chosen per-file rather than per-consequence is not. Worth one decision recorded at the module level.

**2. `helpers::now_epoch` is the store's only clock, and it is seconds + wall clock.** It stamps `created_at`, `started_at`, `completed_at`, `locked_at`, run `started_at`/`ended_at`, comment `created_at`, and journal `created_at`. Three findings above (run tie-breaking, stale-lock skew, journal ordering) are downstream of that single choice. Note also that `SqliteSnapshotStore::insert` uses `chrono::Utc::now().timestamp()` for the same database's `created_at` — same unit, different source; worth converging on one helper.

**3. The derived-status predicate is implemented twice and the two copies currently agree — that is luck, not structure.** `row_decode.rs::derive_status` (`has_unresolved_deps` + `has_dead_deps`, two `EXISTS` queries) and `crud.rs::list_tasks`' inline `SUM(CASE …)` aggregate encode the same rule: pending + unresolved > 0 → `Unsatisfiable` if any dep is `failed`/`cancelled`, else `Blocked`. The satisfying set `('completed','skipped')` and the dead set `('failed','cancelled')` are string literals repeated in **five** places across `row_decode.rs:95,114` and `crud.rs:254,255`, `deps.rs:53` — while `CoordTaskStatus::satisfies_dependency()` (mod.rs) already exists as the single source and is not used by any of them. A sixth status added to the enum will update the Rust predicate and none of the SQL. Suggest generating the `IN (…)` lists from the enum (e.g. a `const SATISFYING_SQL_LIST: &str` derived next to `satisfies_dependency`, plus a source-level test that the SQL literals match).

**4. `delete_team_tasks` is the only mutation that emits nothing.** `create_task` and `update_task` both go through `emit_task_topic`; the bulk delete does not publish a `team.<id>.task.*` topic and does not broadcast an `AlephEvent`. The caller (`gateway/handlers/teams/crud.rs:219`) does send `notify_team_changed(Deleted)` afterwards, so the panel is not left completely blind — but any consumer subscribed to the task topics specifically (the kanban drawer's timeline is fed from `team_events`, which `TeamEventLogger` populates from those broadcasts) sees the tasks vanish with no corresponding event. Flagging rather than filing because the correct behaviour depends on what the kanban actually subscribes to, which is outside this batch.

**5. `CoordTaskUpdate`'s `Option<T>` fields cannot express "clear this".** `owner`, `result` and `metadata` all use `None` to mean "leave alone", so there is no way to un-assign an owner or clear a stale result through the store API. Not a bug today (no caller wants to), noted because the type will need `Option<Option<T>>` or a dedicated verb the first time one does.

**6. Positional row decoding is load-bearing and unguarded.** `read_task_row` is called from three sites with three separately hand-written `SELECT` column lists (`row_decode.rs:145`, `crud.rs:251-253`, `deps.rs:45`), and `list_tasks` additionally hardcodes indices 14/15/16 for its aggregate columns. All four currently agree — verified column by column. A single reordering in any one of them mis-decodes silently (types mostly line up: `TEXT` into `TEXT`). A shared `const TASK_COLUMNS: &str` used by all three SELECTs would make the drift impossible instead of merely absent.

---

## Files reviewed

| File | LOC | Notes |
|------|-----|-------|
| `src/agents/swarm/tasks/store/mod.rs` | 619 | production impl ends at line 335; lines 337-619 are `#[cfg(test)] mod review_tests`, not reviewed per instructions |
| `src/agents/swarm/tasks/store/crud.rs` | 344 | 1 HIGH, 3 MEDIUM, 2 LOW |
| `src/agents/swarm/tasks/store/schema.rs` | 284 | 1 MEDIUM, 2 LOW |
| `src/agents/swarm/tasks/store/runs.rs` | 179 | 2 LOW |
| `src/agents/swarm/tasks/store/row_decode.rs` | 157 | clean; contributes to cross-cutting #3 and #6 |
| `src/agents/swarm/tasks/store/journal.rs` | 156 | 1 LOW |
| `src/agents/swarm/tasks/store/locks.rs` | 101 | 2 LOW |
| `src/agents/swarm/tasks/store/deps.rs` | 72 | clean; index gap filed against `schema.rs` |
| `src/agents/swarm/tasks/store/comments.rs` | 61 | clean |
| `src/agents/swarm/tasks/store/helpers.rs` | 27 | clean (16 LOC excluding doc comments) |

**Consulted for verification (not in scope, not reviewed):** `src/agents/swarm/tasks/mod.rs` (status/verdict enums, `RUN_ABANDONED_BY_JANITOR_ERROR`), `src/agents/swarm/tasks/dag.rs`, `src/agents/swarm/tasks/retry.rs`, `src/teams/snapshots/{store,operations}.rs`, `src/teams/dispatcher/schedule/{mod,select,reclaim}.rs`, `src/gateway/handlers/teams/crud.rs`, `src/bin/aleph-server/commands/start/builder/agent_init/coord_stores.rs`, `Cargo.toml`.
