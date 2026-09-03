//! Row decoding + DAG-derived status helpers.
//!
//! These translate raw `rusqlite::Row`s and parent-status data into the
//! domain types defined in [`super::super`]. `Blocked` is never stored — it
//! is derived here at query time from unresolved dependency edges.

use rusqlite::{params, Connection, OptionalExtension};

use super::super::{CoordTask, CoordTaskId, CoordTaskStatus, Priority};

/// Read a task row from a rusqlite Row. Caller must ensure column order matches.
pub(super) fn read_task_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<CoordTask> {
    let status_str: String = row.get(4)?;
    let priority_str: String = row.get(6)?;
    let result_val: Option<String> = row.get(7)?;
    let metadata_str: String = row.get(8)?;

    let status = CoordTaskStatus::from_stored(&status_str).ok_or_else(|| {
        rusqlite::Error::FromSqlConversionFailure(
            4,
            rusqlite::types::Type::Text,
            Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("unknown task status: {status_str}"),
            )),
        )
    })?;
    let priority = Priority::from_stored(&priority_str).ok_or_else(|| {
        rusqlite::Error::FromSqlConversionFailure(
            6,
            rusqlite::types::Type::Text,
            Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("unknown task priority: {priority_str}"),
            )),
        )
    })?;

    Ok(CoordTask {
        id: row.get(0)?,
        team_id: row.get(1)?,
        subject: row.get(2)?,
        description: row.get(3)?,
        status,
        owner: row.get(5)?,
        priority,
        result: result_val,
        metadata: serde_json::from_str(&metadata_str).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(
                8,
                rusqlite::types::Type::Text,
                Box::new(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("invalid metadata JSON: {e}"),
                )),
            )
        })?,
        dependencies: Vec::new(),
        created_at: row.get(9)?,
        started_at: row.get(10)?,
        completed_at: row.get(11)?,
        locked_by: row.get(12)?,
        locked_at: row.get(13)?,
    })
}

/// Load dependency list for a task.
pub(super) fn load_dependencies(
    conn: &Connection,
    task_id: &str,
) -> rusqlite::Result<Vec<CoordTaskId>> {
    // `ORDER BY rowid` = insertion order = the order the template declared
    // `blocked_by` in. Keeps this reader in step with the `GROUP_CONCAT(…
    // ORDER BY d.rowid)` in `crud.rs`, so both paths agree AND both are the
    // declared order rather than uuid order.
    let mut stmt = conn.prepare_cached(
        "SELECT depends_on FROM coord_task_dependencies WHERE task_id = ?1 ORDER BY rowid",
    )?;
    let rows = stmt.query_map(params![task_id], |row| row.get(0))?;
    rows.collect()
}

/// Determine if a pending task should display as Blocked (has unresolved deps).
///
/// A dep is "satisfied" when its status is one of (completed, skipped).
/// Skipped was added in Phase C (workflow parity); it means an operator
/// (lead agent / user) decided the step is not required, so downstream
/// tasks should still unblock.
pub(super) fn has_unresolved_deps(conn: &Connection, task_id: &str) -> rusqlite::Result<bool> {
    let blocked: bool = conn.query_row(
        r#"
        SELECT EXISTS(
            SELECT 1 FROM coord_task_dependencies d
            JOIN coord_tasks dep ON dep.id = d.depends_on
            WHERE d.task_id = ?1 AND dep.status NOT IN ('completed', 'skipped')
        )
        "#,
        params![task_id],
        |row| row.get(0),
    )?;
    Ok(blocked)
}

/// Determine if a pending task has at least one dependency in a terminal
/// non-satisfying state (`failed` or `cancelled`). Such a dep will never
/// flip to completed/skipped, so the dependent is permanently stuck — this
/// distinguishes `Unsatisfiable` from a still-waiting `Blocked`.
pub(super) fn has_dead_deps(conn: &Connection, task_id: &str) -> rusqlite::Result<bool> {
    conn.query_row(
        r#"
        SELECT EXISTS(
            SELECT 1 FROM coord_task_dependencies d
            JOIN coord_tasks dep ON dep.id = d.depends_on
            WHERE d.task_id = ?1 AND dep.status IN ('failed', 'cancelled')
        )
        "#,
        params![task_id],
        |row| row.get(0),
    )
}

/// Determine if a pending task has at least one unresolved dependency that is
/// still ALIVE — neither satisfying (`completed`/`skipped`) nor terminally dead
/// (`failed`/`cancelled`). This is what still blocks a task whose metadata
/// tolerates dead deps: a dead upstream will never deliver, so waiting on it is
/// waiting forever, but a `pending`/`in_progress`/`waiting_review` upstream
/// still might.
///
/// Both status sets are the SAME literals the strict pair above uses, copied
/// verbatim rather than re-partitioned, so the drift-guard in
/// `tasks/mod.rs::dependency_resolution_rule_is_pinned_across_all_statuses`
/// still covers every literal in this file.
pub(super) fn has_live_unresolved_deps(
    conn: &Connection,
    task_id: &str,
) -> rusqlite::Result<bool> {
    conn.query_row(
        r#"
        SELECT EXISTS(
            SELECT 1 FROM coord_task_dependencies d
            JOIN coord_tasks dep ON dep.id = d.depends_on
            WHERE d.task_id = ?1
              AND dep.status NOT IN ('completed', 'skipped')
              AND dep.status NOT IN ('failed', 'cancelled')
        )
        "#,
        params![task_id],
        |row| row.get(0),
    )
}

/// Derive the effective status for a task (`Blocked`/`Unsatisfiable` are
/// computed, not stored). A pending task with unresolved deps is
/// `Unsatisfiable` when one of those deps terminally failed, otherwise
/// `Blocked`.
///
/// `metadata` is the task's own row metadata: a task stamped
/// [`TOLERATE_FAILED_DEPS_METADATA_KEY`](crate::agents::swarm::tasks::acceptance::TOLERATE_FAILED_DEPS_METADATA_KEY)
/// opts out of the dead-dependency rule — a failed/cancelled upstream stops
/// blocking it (and can never make it `Unsatisfiable`), while any dep still
/// capable of delivering keeps it `Blocked`. The flag is per-task and covers
/// only its DIRECT dependencies; it is not inherited down the DAG.
pub(super) fn derive_status(
    conn: &Connection,
    task_id: &str,
    stored: CoordTaskStatus,
    metadata: &serde_json::Value,
) -> rusqlite::Result<CoordTaskStatus> {
    if stored == CoordTaskStatus::Pending && has_unresolved_deps(conn, task_id)? {
        if crate::agents::swarm::tasks::acceptance::tolerate_failed_deps(metadata) {
            if has_live_unresolved_deps(conn, task_id)? {
                Ok(CoordTaskStatus::Blocked)
            } else {
                Ok(CoordTaskStatus::Pending)
            }
        } else if has_dead_deps(conn, task_id)? {
            Ok(CoordTaskStatus::Unsatisfiable)
        } else {
            Ok(CoordTaskStatus::Blocked)
        }
    } else {
        Ok(stored)
    }
}

/// Fully load a task including dependencies and derived status.
pub(super) fn load_task(conn: &Connection, task_id: &str) -> rusqlite::Result<Option<CoordTask>> {
    let mut stmt = conn.prepare_cached(
        "SELECT id, team_id, subject, description, status, owner, priority, result, metadata, created_at, started_at, completed_at, locked_by, locked_at FROM coord_tasks WHERE id = ?1",
    )?;
    let task_opt: Option<CoordTask> = stmt.query_row(params![task_id], read_task_row).optional()?;

    match task_opt {
        Some(mut task) => {
            task.dependencies = load_dependencies(conn, &task.id)?;
            task.status = derive_status(conn, &task.id, task.status, &task.metadata)?;
            Ok(Some(task))
        }
        None => Ok(None),
    }
}
