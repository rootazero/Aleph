//! DAG query methods for `SqliteCoordTaskStore` (dependencies/dependents/newly-unblocked).
//! Free functions delegated to by the thin `impl CoordTaskStore` in `mod.rs`.

use rusqlite::params;

use super::helpers::db_err;
use super::row_decode::{load_dependencies, read_task_row};
use super::SqliteCoordTaskStore;
use crate::agents::swarm::tasks::{CoordTask, CoordTaskId, CoordTaskStatus};

pub(super) async fn get_dependencies(
    store: &SqliteCoordTaskStore,
    id: &str,
) -> crate::error::Result<Vec<CoordTaskId>> {
    let conn = store.conn.lock().await;
    load_dependencies(&conn, id).map_err(db_err)
}

pub(super) async fn get_dependents(
    store: &SqliteCoordTaskStore,
    id: &str,
) -> crate::error::Result<Vec<CoordTaskId>> {
    let conn = store.conn.lock().await;
    let mut stmt = conn
        .prepare_cached("SELECT task_id FROM coord_task_dependencies WHERE depends_on = ?1")
        .map_err(db_err)?;
    let rows = stmt
        .query_map(params![id], |row| row.get(0))
        .map_err(db_err)?;
    let mut ids = Vec::new();
    for r in rows {
        ids.push(r.map_err(db_err)?);
    }
    Ok(ids)
}

/// Dependents of `settled_id` that became runnable because of that transition.
///
/// The caller's `settled_id` is normally a task that just completed, but the
/// same question has an answer when it FAILED: a dependent stamped
/// `tolerate_failed_deps` treats a failed/cancelled upstream as "will never
/// arrive" rather than as a blocker, so a failure can be the transition that
/// releases it. The row must therefore be filtered on both counts — unresolved
/// deps and the dead subset of them — instead of on the strict
/// `NOT EXISTS unresolved` the SQL used to apply to everyone.
pub(super) async fn get_newly_unblocked(
    store: &SqliteCoordTaskStore,
    settled_id: &str,
) -> crate::error::Result<Vec<CoordTask>> {
    let conn = store.conn.lock().await;
    let mut stmt = conn
        .prepare_cached(
            r#"
            SELECT t.id, t.team_id, t.subject, t.description, t.status, t.owner, t.priority, t.result, t.metadata, t.created_at, t.started_at, t.completed_at, t.locked_by, t.locked_at,
              (SELECT COUNT(*) FROM coord_task_dependencies d2
                 JOIN coord_tasks dep ON dep.id = d2.depends_on
                 WHERE d2.task_id = t.id AND dep.status NOT IN ('completed', 'skipped')) AS unresolved,
              (SELECT COUNT(*) FROM coord_task_dependencies d3
                 JOIN coord_tasks dep2 ON dep2.id = d3.depends_on
                 WHERE d3.task_id = t.id AND dep2.status IN ('failed', 'cancelled')) AS dead
            FROM coord_tasks t
            JOIN coord_task_dependencies d ON d.task_id = t.id
            WHERE d.depends_on = ?1
              AND t.status = 'pending'
            "#,
        )
        .map_err(db_err)?;

    let rows = stmt
        .query_map(params![settled_id], |row| {
            let task = read_task_row(row)?;
            let unresolved: i64 = row.get(14)?;
            let dead: i64 = row.get(15)?;
            Ok((task, unresolved, dead))
        })
        .map_err(db_err)?;

    let mut tasks = Vec::new();
    for row in rows {
        let (mut task, unresolved, dead) = row.map_err(db_err)?;
        // Same partition as `row_decode::derive_status` / `crud::list_tasks`:
        // everything satisfied, or — for a tolerant task — everything that is
        // left is dead and therefore no longer worth waiting for.
        let ready = if crate::agents::swarm::tasks::acceptance::tolerate_failed_deps(&task.metadata)
        {
            unresolved - dead == 0
        } else {
            unresolved == 0
        };
        if !ready {
            continue;
        }
        task.dependencies = load_dependencies(&conn, &task.id).map_err(db_err)?;
        // These are newly unblocked → status is Pending (nothing left to wait on)
        task.status = CoordTaskStatus::Pending;
        tasks.push(task);
    }
    Ok(tasks)
}
