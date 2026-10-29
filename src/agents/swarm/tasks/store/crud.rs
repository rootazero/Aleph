//! CRUD methods for `SqliteCoordTaskStore` (create/get/update/list/delete).
//! Free functions delegated to by the thin `impl CoordTaskStore` in `mod.rs`.

use rusqlite::{params, OptionalExtension};

use super::helpers::{db_err, now_epoch};
use super::row_decode::{load_task, read_task_row};
use super::SqliteCoordTaskStore;
use crate::agents::swarm::tasks::{
    CoordTask, CoordTaskFilter, CoordTaskStatus, CoordTaskUpdate, NewCoordTask,
};

/// Maximum number of task rows a single `list_tasks` call may materialise.
///
/// `coord_tasks` is only pruned by an explicit team deletion, so a
/// long-lived team grows without bound. Without a cap, one call decoded
/// EVERY matching row — each carrying the full description/result/metadata
/// JSON plus a `GROUP_CONCAT` of its dependency ids — while holding the
/// single connection mutex, so one large team blocked every other writer.
///
/// The cap is applied IN SQL (`LIMIT ?N`, the value is bound, never
/// interpolated), so the driver never decodes more rows than the caller
/// receives. The ordering stays deterministic (priority, then oldest-first,
/// then rowid), so a truncated listing is always the same prefix of the same
/// order, and truncation is never silent: the call logs a warning whenever
/// the window comes back full.
///
/// Known trade-off (deferred): consumers that need the COMPLETE task set
/// see a truncated list once a team holds more than `TASK_LIST_ROW_CAP`
/// tasks. The dispatcher settle sweep
/// (`teams/dispatcher/schedule/settle.rs`) groups the unfiltered listing by
/// workflow run id and needs every task of a run to decide "all settled";
/// with a truncated listing it can miss a run entirely or judge a partial
/// run settled. Fixing that needs pagination (a cursor on the filter), i.e.
/// a change to the `CoordTaskStore` trait that this module does not own.
///
/// The cap is therefore deliberately high: it exists to bound the memory a
/// single call can take (the defect being fixed), not to truncate a real
/// team's task set. The truncation is never silent - the call warns - but
/// the correct fix is the cursor above, which is a follow-up.
const TASK_LIST_ROW_CAP: usize = 10_000;

pub(super) async fn create_task(
    store: &SqliteCoordTaskStore,
    mut input: NewCoordTask,
) -> crate::error::Result<CoordTask> {
    // Round 11 (N1): the author the dispatcher later resolves fire-time
    // authority for. Stamped before the lock — it reads task-locals only.
    crate::agents::swarm::tasks::stamp_task_author(&mut input.metadata);
    // Generate the id before cycle-check so we can pass it as the new node.
    let id = uuid::Uuid::new_v4().to_string();

    // Acquire the lock FIRST. B6-04 (vacuous predicate CUT): the previous
    // call to `check_no_cycle_sync` here was structurally constant-true —
    // the new id is a fresh UUID minted three lines earlier and not yet
    // present in `coord_tasks`, so no `coord_task_dependencies` row can
    // reference it and the BFS can never hit `current == new_task_id`.
    // Cost was real (one prepare_cached per visited ancestor node, inside
    // the connection mutex, on every create), benefit was zero. The DAG is
    // acyclic by construction (edges are immutable after creation;
    // dag.rs:3-5). The structural invariant belongs in a doc comment on
    // `NewCoordTask::blocked_by` and a source-level guard that no other
    // INSERT site exists in `coord_task_dependencies`. If a future path
    // mutates edges on existing tasks, wire `check_no_cycle_sync` THERE —
    // that is where it stops being vacuous.
    let conn = store.conn.lock().await;

    let now = now_epoch();
    let metadata_json = serde_json::to_string(&input.metadata).unwrap_or_else(|_| "{}".into());

    // Wrap task + dependency inserts in a transaction so partial
    // failure (e.g. FK violation on a dependency) doesn't leave an
    // orphaned task row. B6-01: use `unchecked_transaction()` instead of
    // hand-rolled BEGIN/COMMIT — the RAII guard rolls back on any early
    // return, INCLUDING a failed `tx.commit()`, which the literal form
    // returned `Err` from without rolling back. The connection is shared
    // process-wide with `SqliteSnapshotStore`, so a dangling transaction
    // poisoned every later write until restart.
    //
    // The transaction is scoped to its own block so the non-`Send` guard
    // cannot live past the `.await` on `emit_task_topic` below —
    // `Transaction` borrows the `Connection` via `RefCell` (rusqlite's
    // own safety net) and so is `!Send`, but `CoordTaskStore` requires
    // `dyn Future + Send`. `Transaction::commit` consumes `self`, so
    // either path through the block drops the guard before we return.
    let inserted: Result<(), crate::error::AlephError> =
        (|| -> Result<(), crate::error::AlephError> {
            let tx = conn.unchecked_transaction().map_err(db_err)?;

            let result: std::result::Result<(), rusqlite::Error> = (|| {
                // Always store as 'pending' — Blocked is derived
                tx.execute(
                r#"
            INSERT INTO coord_tasks (id, team_id, subject, description, status, owner, priority, metadata, created_at)
            VALUES (?1, ?2, ?3, ?4, 'pending', ?5, ?6, ?7, ?8)
            "#,
                params![
                    id,
                    input.team_id,
                    input.subject,
                    input.description,
                    input.owner,
                    input.priority.as_str(),
                    metadata_json,
                    now,
                ],
            )?;

                // Insert dependency edges
                for dep_id in &input.blocked_by {
                    tx.execute(
                        "INSERT INTO coord_task_dependencies (task_id, depends_on) VALUES (?1, ?2)",
                        params![id, dep_id],
                    )?;
                }
                Ok(())
            })();

            match result {
                Ok(()) => tx.commit().map_err(db_err),
                Err(e) => Err(db_err(e)),
            }
        })();
    // `tx` is gone here; the block's return is the commit outcome (or the
    // original error, which left the transaction rolled back via Drop).
    inserted?;

    // Return the fully loaded task (with derived status)
    let task = load_task(&conn, &id)
        .map_err(db_err)?
        .ok_or_else(|| db_err("task disappeared after insert"))?;
    drop(conn); // release lock before emit so subscribers aren't blocked
    store.emit_task_topic(&task, "created").await;
    Ok(task)
}

pub(super) async fn get_task(
    store: &SqliteCoordTaskStore,
    id: &str,
) -> crate::error::Result<Option<CoordTask>> {
    let conn = store.conn.lock().await;
    load_task(&conn, id).map_err(db_err)
}

pub(super) async fn update_task(
    store: &SqliteCoordTaskStore,
    id: &str,
    update: CoordTaskUpdate,
) -> crate::error::Result<CoordTask> {
    // All synchronous SQL work happens inside this block so the non-`Send`
    // `Vec<Box<dyn ToSql>>` scratch state (and the rusqlite connection
    // guard) cannot escape into the `.await` below — the outer trait impl
    // demands a `Send` future.
    let task_opt: Option<CoordTask> = {
        let conn = store.conn.lock().await;
        let now = now_epoch();

        // Build dynamic SET clauses
        let mut sets: Vec<String> = Vec::new();
        let mut values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
        let mut idx = 1usize;

        if let Some(ref status) = update.status {
            // Never store 'blocked' or 'unsatisfiable' (both derived at
            // query time; from_stored rejects them) — map to pending so
            // the row stays readable.
            let store_status = if status.is_blocked_like() {
                "pending"
            } else {
                status.as_str()
            };
            sets.push(format!("status = ?{idx}"));
            values.push(Box::new(store_status.to_string()));
            idx += 1;

            if *status == CoordTaskStatus::InProgress {
                sets.push(format!("started_at = ?{idx}"));
                values.push(Box::new(now));
                idx += 1;
            }
            if *status == CoordTaskStatus::Completed {
                sets.push(format!("completed_at = ?{idx}"));
                values.push(Box::new(now));
                idx += 1;
            }
        }

        if let Some(ref owner) = update.owner {
            sets.push(format!("owner = ?{idx}"));
            values.push(Box::new(owner.clone()));
            idx += 1;
        }

        if let Some(ref result) = update.result {
            sets.push(format!("result = ?{idx}"));
            values.push(Box::new(result.clone()));
            idx += 1;
        }

        if let Some(ref metadata) = update.metadata {
            let json = serde_json::to_string(metadata)
                .map_err(|e| db_err(format!("failed to serialize metadata: {e}")))?;
            // LOST-UPDATE WINDOW (known, deferred to a follow-up pass):
            // `metadata` is a single TEXT column written as a whole-blob
            // replacement, with no version/etag to compare against. The ~37
            // call sites that patch it do read-modify-write against a
            // snapshot they fetched OUTSIDE this connection lock, so two
            // concurrent producers can interleave and the later write
            // silently drops the earlier one's key. The real fix is
            // optimistic concurrency — a `metadata_version` column plus
            // `WHERE metadata_version = ?` — which touches every metadata
            // writer outside this module and is therefore not done here.
            //
            // What this write can still do cheaply and STRICTLY safer:
            // never rewrite a value that is already stored. When the
            // incoming blob is byte-identical to the stored text, leaving
            // the column out of the SET list preserves a concurrent
            // writer's newer metadata instead of restoring our (identical)
            // snapshot over it.
            let stored: Option<String> = conn
                .query_row(
                    "SELECT metadata FROM coord_tasks WHERE id = ?1",
                    params![id],
                    |row| row.get::<_, String>(0),
                )
                .optional()
                .map_err(db_err)?;
            if stored.as_deref() != Some(json.as_str()) {
                sets.push(format!("metadata = ?{idx}"));
                values.push(Box::new(json));
                idx += 1;
            }
        }

        if sets.is_empty() {
            // Nothing to update — load current state, no emit, no await.
            let t = load_task(&conn, id)
                .map_err(db_err)?
                .ok_or_else(|| db_err(format!("task not found: {id}")))?;
            return Ok(t);
        }

        let sql = format!(
            "UPDATE coord_tasks SET {} WHERE id = ?{idx}",
            sets.join(", ")
        );
        values.push(Box::new(id.to_string()));
        let params_ref: Vec<&dyn rusqlite::types::ToSql> =
            values.iter().map(|v| v.as_ref()).collect();
        let affected = conn.execute(&sql, params_ref.as_slice()).map_err(db_err)?;
        if affected == 0 {
            return Err(db_err(format!("task not found: {id}")));
        }
        load_task(&conn, id).map_err(db_err)?
    };

    let task = task_opt.ok_or_else(|| db_err(format!("task not found after update: {id}")))?;
    // The broadcast verb reflects the TRANSITION, not the row's resting state:
    // a metadata/owner/result-only write on an already-terminal task must not
    // re-broadcast TeamTaskCompleted/TeamTaskFailed. Downstream listeners
    // treat those verbs as fresh terminal transitions — TeamNotifier's
    // completion claim would re-fire "Team work complete" (even after a
    // FAILED run) and duplicate failure alerts whenever a marker stamp (e.g.
    // `workflow_notified`, clarify delivery markers) lands on a settled task.
    let verb = if update.status.is_some() {
        match task.status {
            CoordTaskStatus::Completed => "completed",
            CoordTaskStatus::Failed => "failed",
            CoordTaskStatus::Cancelled => "cancelled",
            CoordTaskStatus::WaitingReview => "waiting_review",
            CoordTaskStatus::Skipped => "skipped",
            CoordTaskStatus::Paused => "paused",
            _ => "updated",
        }
    } else {
        "updated"
    };
    store.emit_task_topic(&task, verb).await;
    Ok(task)
}

pub(super) async fn list_tasks(
    store: &SqliteCoordTaskStore,
    filter: CoordTaskFilter,
) -> crate::error::Result<Vec<CoordTask>> {
    let conn = store.conn.lock().await;

    let mut where_clauses: Vec<String> = Vec::new();
    let mut values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
    let mut idx = 1usize;

    let filter_blocked = filter.status == Some(CoordTaskStatus::Blocked);
    let filter_pending = filter.status == Some(CoordTaskStatus::Pending);
    let filter_unsatisfiable = filter.status == Some(CoordTaskStatus::Unsatisfiable);

    if let Some(ref status) = filter.status {
        // Blocked, Unsatisfiable and Pending are all stored as 'pending';
        // they are separated in the post-query pass below.
        if status.is_blocked_like() || *status == CoordTaskStatus::Pending {
            where_clauses.push(format!("t.status = ?{idx}"));
            values.push(Box::new("pending".to_string()));
            idx += 1;
        } else {
            where_clauses.push(format!("t.status = ?{idx}"));
            values.push(Box::new(status.as_str().to_string()));
            idx += 1;
        }
    }

    if let Some(ref team_id) = filter.team_id {
        where_clauses.push(format!("t.team_id = ?{idx}"));
        values.push(Box::new(team_id.clone()));
        idx += 1;
    }

    let where_sql = if where_clauses.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", where_clauses.join(" AND "))
    };

    // SQL side of the derived-status rule. `Blocked`, `Unsatisfiable` and
    // `Pending` are all stored as 'pending' and separated AFTER the query,
    // so a raw `LIMIT` would spend its window on rows the caller then
    // discards (the dispatcher's pending scan would starve on blocked
    // rows). These HAVING clauses pre-filter with a deliberately LOOSE
    // superset of the Rust rule in `row_decode::derive_status`:
    //
    //   pending       -> unresolved == 0 OR the tolerate flag is set
    //   blocked       -> unresolved > 0
    //   unsatisfiable -> dead > 0
    //
    // A loose superset is the safe direction: SQL may return a row the
    // Rust pass then drops, but it can never hide a row the Rust rule
    // would keep. The Rust pass stays authoritative.
    //
    // The tolerate flag mirrors `acceptance::tolerate_failed_deps`: only a
    // JSON boolean `true` counts (`json_type`, not `json_extract`, so the
    // number 1 reads as false exactly like `serde_json::Value::as_bool`).
    let tolerate_expr = format!(
        "(CASE WHEN json_valid(t.metadata) \
         THEN COALESCE(json_type(t.metadata, '$.{key}') = 'true', 0) \
         ELSE 0 END)",
        key = crate::agents::swarm::tasks::acceptance::TOLERATE_FAILED_DEPS_METADATA_KEY
    );
    let having_sql = if filter_blocked {
        "HAVING unresolved_parents > 0".to_string()
    } else if filter_unsatisfiable {
        "HAVING dead_parents > 0".to_string()
    } else if filter_pending {
        format!("HAVING (unresolved_parents = 0 OR {tolerate_expr})")
    } else {
        String::new()
    };

    // Row cap, bound as a parameter (never interpolated into the SQL).
    values.push(Box::new(TASK_LIST_ROW_CAP as i64));
    let limit_idx = idx;

    // Single pass: tasks + unresolved-parent count + a CSV of dependency ids.
    // GROUP_CONCAT keeps the dependency list inline so we avoid the per-task
    // load_dependencies() round-trip used previously.
    //
    // Ordering: highest priority first so a kanban column surfaces the most
    // urgent work at the top, then oldest-created first as a stable
    // tiebreaker, then `rowid` so tasks created within the same second
    // (the timestamps have whole-second granularity) still have ONE
    // deterministic order instead of SQLite's scan order. Priority is
    // stored as text, so a CASE maps it to a rank.
    let sql = format!(
        r#"
        SELECT
            t.id, t.team_id, t.subject, t.description, t.status, t.owner,
            t.priority, t.result, t.metadata, t.created_at, t.started_at,
            t.completed_at, t.locked_by, t.locked_at,
            COALESCE(SUM(CASE WHEN dep.status IS NOT NULL AND dep.status NOT IN ('completed', 'skipped') THEN 1 ELSE 0 END), 0) AS unresolved_parents,
            COALESCE(SUM(CASE WHEN dep.status IN ('failed', 'cancelled') THEN 1 ELSE 0 END), 0) AS dead_parents,
            -- Ordered by insertion, which IS the declared order: `create_task`
            -- inserts `blocked_by` in the order the template listed it, and the
            -- table is a rowid table. Without the ORDER BY, SQLite returned the
            -- covering index's order — dependency-id (uuid) order — so a
            -- three-way fan-in presented its inputs to the synthesis node in an
            -- order that changed every time the same template was
            -- re-materialised. (`GROUP_CONCAT(x ORDER BY y)` needs SQLite
            -- >= 3.44; rusqlite is `bundled`, so the version is ours.)
            GROUP_CONCAT(d.depends_on ORDER BY d.rowid) AS dep_ids
        FROM coord_tasks t
        LEFT JOIN coord_task_dependencies d ON d.task_id = t.id
        LEFT JOIN coord_tasks dep ON dep.id = d.depends_on
        {where_sql}
        GROUP BY t.id
        {having_sql}
        ORDER BY
            CASE t.priority
                WHEN 'critical' THEN 0
                WHEN 'high' THEN 1
                WHEN 'normal' THEN 2
                WHEN 'low' THEN 3
                ELSE 2
            END ASC,
            t.created_at ASC,
            t.rowid ASC
        LIMIT ?{limit_idx}
        "#
    );

    let params_ref: Vec<&dyn rusqlite::types::ToSql> = values.iter().map(|v| v.as_ref()).collect();
    let mut stmt = conn.prepare(&sql).map_err(db_err)?;

    let rows = stmt
        .query_map(params_ref.as_slice(), |row| {
            let mut task = read_task_row(row)?;
            let unresolved: i64 = row.get(14)?;
            let dead: i64 = row.get(15)?;
            let dep_csv: Option<String> = row.get(16)?;
            task.dependencies = dep_csv
                .map(|s| {
                    s.split(',')
                        .filter(|x| !x.is_empty())
                        .map(|x| x.to_string())
                        .collect()
                })
                .unwrap_or_default();
            task.status = if task.status == CoordTaskStatus::Pending && unresolved > 0 {
                if crate::agents::swarm::tasks::acceptance::tolerate_failed_deps(&task.metadata) {
                    // Opted out of the dead-dependency rule: a failed/cancelled
                    // upstream will never deliver, so it stops blocking rather
                    // than killing the branch. `unresolved - dead` is what is
                    // still capable of arriving — the same partition
                    // `row_decode::has_live_unresolved_deps` expresses as a
                    // query, kept as arithmetic here because this pass already
                    // counts both sets in one scan.
                    if unresolved - dead > 0 {
                        CoordTaskStatus::Blocked
                    } else {
                        CoordTaskStatus::Pending
                    }
                } else if dead > 0 {
                    // A terminally-failed dependency makes the task permanently
                    // stuck (Unsatisfiable); otherwise it is merely Blocked.
                    CoordTaskStatus::Unsatisfiable
                } else {
                    CoordTaskStatus::Blocked
                }
            } else {
                task.status
            };
            Ok(task)
        })
        .map_err(db_err)?;

    let mut tasks = Vec::new();
    let mut window_rows = 0usize;
    for row in rows {
        let task = row.map_err(db_err)?;
        window_rows += 1;
        if filter_blocked && task.status != CoordTaskStatus::Blocked {
            continue;
        }
        if filter_unsatisfiable && task.status != CoordTaskStatus::Unsatisfiable {
            continue;
        }
        if filter_pending && task.status != CoordTaskStatus::Pending {
            continue;
        }
        tasks.push(task);
    }

    // Truncation is never silent: a full window means the SQL LIMIT cut
    // the result set, so callers that need every task must be told.
    if window_rows >= TASK_LIST_ROW_CAP {
        tracing::warn!(
            cap = TASK_LIST_ROW_CAP,
            team_id = filter.team_id.as_deref().unwrap_or("<all teams>"),
            status = filter.status.as_ref().map(|s| s.as_str()),
            returned = tasks.len(),
            "list_tasks: row cap reached, the task list is TRUNCATED"
        );
    }

    Ok(tasks)
}

pub(super) async fn delete_team_tasks(
    store: &SqliteCoordTaskStore,
    team_id: &str,
) -> crate::error::Result<usize> {
    let conn = store.conn.lock().await;
    let n = conn
        .execute(
            "DELETE FROM coord_tasks WHERE team_id = ?1",
            params![team_id],
        )
        .map_err(db_err)?;
    Ok(n)
}
