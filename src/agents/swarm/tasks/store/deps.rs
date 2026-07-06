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

pub(super) async fn get_newly_unblocked(
    store: &SqliteCoordTaskStore,
    completed_id: &str,
) -> crate::error::Result<Vec<CoordTask>> {
    let conn = store.conn.lock().await;
    let mut stmt = conn
        .prepare_cached(
            r#"
            SELECT t.id, t.team_id, t.subject, t.description, t.status, t.owner, t.priority, t.result, t.metadata, t.created_at, t.started_at, t.completed_at, t.locked_by, t.locked_at
            FROM coord_tasks t
            JOIN coord_task_dependencies d ON d.task_id = t.id
            WHERE d.depends_on = ?1
              AND t.status = 'pending'
              AND NOT EXISTS (
                SELECT 1 FROM coord_task_dependencies d2
                JOIN coord_tasks dep ON dep.id = d2.depends_on
                WHERE d2.task_id = t.id AND dep.status NOT IN ('completed', 'skipped')
              )
            "#,
        )
        .map_err(db_err)?;

    let rows = stmt
        .query_map(params![completed_id], read_task_row)
        .map_err(db_err)?;

    let mut tasks = Vec::new();
    for row in rows {
        let mut task = row.map_err(db_err)?;
        task.dependencies = load_dependencies(&conn, &task.id).map_err(db_err)?;
        // These are newly unblocked → status is Pending (all deps completed)
        task.status = CoordTaskStatus::Pending;
        tasks.push(task);
    }
    Ok(tasks)
}
