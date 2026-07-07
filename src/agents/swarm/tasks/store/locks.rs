//! Task-locking methods for `SqliteCoordTaskStore` (acquire/release/release-stale).
//! Free functions delegated to by the thin `impl CoordTaskStore` in `mod.rs`.

use rusqlite::{params, OptionalExtension};

use super::helpers::{db_err, now_epoch};
use super::SqliteCoordTaskStore;

pub(super) async fn acquire_lock(
    store: &SqliteCoordTaskStore,
    task_id: &str,
    agent_id: &str,
) -> crate::error::Result<()> {
    let conn = store.conn.lock().await;
    let now = now_epoch();

    let affected = conn
        .execute(
            "UPDATE coord_tasks SET locked_by = ?1, locked_at = ?2 \
             WHERE id = ?3 AND (locked_by IS NULL OR locked_by = ?1)",
            params![agent_id, now, task_id],
        )
        .map_err(db_err)?;

    if affected == 0 {
        // Check if task exists at all
        let exists: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM coord_tasks WHERE id = ?1)",
                params![task_id],
                |row| row.get(0),
            )
            .map_err(db_err)?;

        if !exists {
            return Err(db_err(format!("task not found: {task_id}")));
        }
        // Task exists but locked by someone else
        return Err(db_err(format!("task {task_id} is locked by another agent")));
    }
    Ok(())
}

pub(super) async fn release_lock(
    store: &SqliteCoordTaskStore,
    task_id: &str,
    agent_id: &str,
) -> crate::error::Result<()> {
    let conn = store.conn.lock().await;

    let affected = conn
        .execute(
            "UPDATE coord_tasks SET locked_by = NULL, locked_at = NULL \
             WHERE id = ?1 AND locked_by = ?2",
            params![task_id, agent_id],
        )
        .map_err(db_err)?;

    if affected == 0 {
        // Check current holder
        let holder: Option<String> = conn
            .query_row(
                "SELECT locked_by FROM coord_tasks WHERE id = ?1",
                params![task_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(db_err)?
            .flatten();

        match holder {
            Some(other) => {
                return Err(db_err(format!(
                    "task {task_id} is locked by {other}, not {agent_id}"
                )));
            }
            None => {
                // Already unlocked — idempotent success
            }
        }
    }
    Ok(())
}

pub(super) async fn release_stale_locks(
    store: &SqliteCoordTaskStore,
    max_age_secs: u64,
) -> crate::error::Result<usize> {
    let conn = store.conn.lock().await;
    let cutoff = now_epoch().saturating_sub(max_age_secs);

    let affected = conn
        .execute(
            "UPDATE coord_tasks SET locked_by = NULL, locked_at = NULL \
             WHERE locked_by IS NOT NULL AND locked_at < ?1",
            params![cutoff],
        )
        .map_err(db_err)?;

    Ok(affected)
}
