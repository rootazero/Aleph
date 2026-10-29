//! Task-locking methods for `SqliteCoordTaskStore` (acquire/release/release-stale).
//! Free functions delegated to by the thin `impl CoordTaskStore` in `mod.rs`.

use rusqlite::{params, OptionalExtension};

use super::helpers::{db_err, locked_err, not_found_err, now_epoch};
use super::SqliteCoordTaskStore;

pub(super) async fn acquire_lock(
    store: &SqliteCoordTaskStore,
    task_id: &str,
    agent_id: &str,
) -> crate::error::Result<()> {
    let conn = store.conn.lock().await;
    let now = now_epoch();

    // EXCLUSIVE claim: a row may only be claimed while it is UNLOCKED.
    //
    // The old predicate (`locked_by IS NULL OR locked_by = ?1`) let a second
    // caller using the SAME agent id re-stamp a held lock: `locked_at` was
    // reset to "now", so the FIRST claimer's hold looked fresh and a second
    // dispatcher loop running under the same agent identity silently took
    // over a task that was already in flight. Re-claiming — even by the
    // same id — is now a domain rejection, like claiming someone else's.
    let affected = conn
        .execute(
            "UPDATE coord_tasks SET locked_by = ?1, locked_at = ?2 \
             WHERE id = ?3 AND locked_by IS NULL",
            params![agent_id, now, task_id],
        )
        .map_err(db_err)?;

    if affected == 0 {
        // Either the row does not exist, or it exists and is already held.
        // Read the HOLDER instead of a bare EXISTS so the rejection can say
        // who holds it, and so "not found" and "locked" stay two distinct
        // domain rejections instead of one opaque failure.
        let holder: Option<String> = conn
            .query_row(
                "SELECT locked_by FROM coord_tasks WHERE id = ?1",
                params![task_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(db_err)?
            .flatten();

        return match holder {
            None => Err(not_found_err(&format!("task {task_id}"))),
            Some(holder) => Err(locked_err(&format!("task {task_id}"), &holder)),
        };
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
                return Err(locked_err(&format!("task {task_id}"), &other));
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
    let now = now_epoch();
    let cutoff = now.saturating_sub(max_age_secs);

    // Wall-clock guard. A lock stamped in the FUTURE means the clock moved
    // backwards after that lock was taken, so `now - max_age` no longer
    // describes the lock's age and the sweep would act on a broken clock:
    // a forward jump frees locks that are still live, a backward jump strands
    // every lock forever. Refuse to sweep and surface the anomaly instead of
    // acting on it.
    //
    // Only the backward step is detectable here (it is visible in the stored
    // stamps). A forward jump needs a persisted "last seen now" reference,
    // which is a schema change — see the follow-up note in the report.
    let newest_lock: Option<i64> = conn
        .query_row(
            "SELECT MAX(locked_at) FROM coord_tasks WHERE locked_by IS NOT NULL",
            [],
            |row| row.get(0),
        )
        .map_err(db_err)?;

    if let Some(newest) = newest_lock {
        if newest > now as i64 {
            tracing::warn!(
                newest_locked_at = newest,
                now,
                "coord task locks: wall clock moved backwards (a lock is stamped in the \
                 future) — skipping the stale-lock sweep"
            );
            return Ok(0);
        }
    }

    let affected = conn
        .execute(
            "UPDATE coord_tasks SET locked_by = NULL, locked_at = NULL \
             WHERE locked_by IS NOT NULL AND locked_at < ?1",
            params![cutoff],
        )
        .map_err(db_err)?;

    Ok(affected)
}
