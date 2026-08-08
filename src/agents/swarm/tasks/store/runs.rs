//! Run-history methods for `SqliteCoordTaskStore` (start/finish/list runs, record review).
//! Free functions delegated to by the thin `impl CoordTaskStore` in `mod.rs`.

use rusqlite::params;

use super::helpers::{db_err, now_epoch};
use super::SqliteCoordTaskStore;
use crate::agents::swarm::tasks::{CoordTaskRun, ReviewVerdict, ReviewerKind, TaskRunStatus};

pub(super) async fn start_task_run(
    store: &SqliteCoordTaskStore,
    task_id: &str,
    agent_id: &str,
) -> crate::error::Result<String> {
    let conn = store.conn.lock().await;
    let id = uuid::Uuid::new_v4().to_string();
    let now = now_epoch();
    conn.execute(
        "INSERT INTO coord_task_runs (id, task_id, agent_id, started_at, status) \
         VALUES (?1, ?2, ?3, ?4, 'running')",
        params![id, task_id, agent_id, now],
    )
    .map_err(db_err)?;
    Ok(id)
}

pub(super) async fn finish_task_run(
    store: &SqliteCoordTaskStore,
    run_id: &str,
    status: TaskRunStatus,
    summary: Option<String>,
    error: Option<String>,
) -> crate::error::Result<()> {
    if run_id.is_empty() {
        return Ok(());
    }
    let conn = store.conn.lock().await;
    let now = now_epoch();
    let affected = conn
        .execute(
            "UPDATE coord_task_runs \
             SET ended_at = ?1, status = ?2, summary = ?3, error = ?4 \
             WHERE id = ?5",
            params![now, status.as_str(), summary, error, run_id],
        )
        .map_err(db_err)?;
    if affected == 0 {
        return Err(db_err(format!("task run not found: {run_id}")));
    }
    Ok(())
}

pub(super) async fn abandon_orphaned_runs(
    store: &SqliteCoordTaskStore,
    live_task_ids: &[String],
) -> crate::error::Result<usize> {
    let conn = store.conn.lock().await;
    let now = now_epoch();
    // Set-based close keyed on the runs table itself: a `running` row whose
    // task is not currently in flight in-process can never finish (its tokio
    // worker died with a previous daemon incarnation, or finish_task_run's
    // UPDATE was lost). Task status is deliberately NOT consulted — that is
    // what lets cancel-then-crash orphans (terminal task, stuck row) close.
    // The stamped `error` is bound from the shared
    // `RUN_ABANDONED_BY_JANITOR_ERROR` constant, not inlined: it is the ONLY
    // thing that tells a crash-closed row apart from a deliberately-deferred
    // one (a busy-target attempt is `abandoned` too), and the crash-recovery
    // budget counts on that distinction.
    let placeholders = vec!["?"; live_task_ids.len()].join(", ");
    let sql = format!(
        "UPDATE coord_task_runs \
         SET status = 'abandoned', ended_at = ?1, error = ?2 \
         WHERE status = 'running'{}",
        if live_task_ids.is_empty() {
            String::new()
        } else {
            format!(" AND task_id NOT IN ({placeholders})")
        }
    );
    let janitor_error = crate::agents::swarm::tasks::RUN_ABANDONED_BY_JANITOR_ERROR;
    let mut params: Vec<&dyn rusqlite::ToSql> = vec![&now, &janitor_error];
    for id in live_task_ids {
        params.push(id);
    }
    let affected = conn.execute(&sql, params.as_slice()).map_err(db_err)?;
    Ok(affected)
}

pub(super) async fn list_task_runs(
    store: &SqliteCoordTaskStore,
    task_id: &str,
) -> crate::error::Result<Vec<CoordTaskRun>> {
    let conn = store.conn.lock().await;
    let mut stmt = conn
        .prepare_cached(
            "SELECT id, task_id, agent_id, started_at, ended_at, status, summary, error, \
                    review_verdict, reviewer_kind, reviewer_id \
             FROM coord_task_runs WHERE task_id = ?1 ORDER BY started_at ASC",
        )
        .map_err(db_err)?;
    let rows = stmt
        .query_map(params![task_id], |row| {
            let status_str: String = row.get(5)?;
            let status = TaskRunStatus::from_stored(&status_str).ok_or_else(|| {
                rusqlite::Error::FromSqlConversionFailure(
                    5,
                    rusqlite::types::Type::Text,
                    Box::new(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        format!("unknown task run status: {status_str}"),
                    )),
                )
            })?;
            let review_verdict: Option<String> = row.get(8).ok();
            let reviewer_kind: Option<String> = row.get(9).ok();
            Ok(CoordTaskRun {
                id: row.get(0)?,
                task_id: row.get(1)?,
                agent_id: row.get(2)?,
                started_at: row.get(3)?,
                ended_at: row.get(4)?,
                status,
                summary: row.get(6)?,
                error: row.get(7)?,
                review_verdict: review_verdict
                    .as_deref()
                    .and_then(ReviewVerdict::from_stored),
                reviewer_kind: reviewer_kind.as_deref().and_then(ReviewerKind::from_stored),
                reviewer_id: row.get(10).ok(),
            })
        })
        .map_err(db_err)?;
    let mut runs = Vec::new();
    for r in rows {
        runs.push(r.map_err(db_err)?);
    }
    Ok(runs)
}

pub(super) async fn record_run_review(
    store: &SqliteCoordTaskStore,
    task_id: &str,
    verdict: ReviewVerdict,
    reviewer_kind: ReviewerKind,
    reviewer_id: Option<&str>,
) -> crate::error::Result<()> {
    let conn = store.conn.lock().await;
    // Stamp the most recent completed run for this task. We choose the
    // latest-by-started_at row that has already ended — a still-running
    // attempt cannot be reviewed.
    let affected = conn
        .execute(
            r#"
            UPDATE coord_task_runs
            SET review_verdict = ?1,
                reviewer_kind  = ?2,
                reviewer_id    = ?3
            WHERE id = (
                SELECT id FROM coord_task_runs
                WHERE task_id = ?4 AND ended_at IS NOT NULL
                ORDER BY started_at DESC
                LIMIT 1
            )
            "#,
            params![
                verdict.as_str(),
                reviewer_kind.as_str(),
                reviewer_id,
                task_id,
            ],
        )
        .map_err(db_err)?;
    if affected == 0 {
        return Err(db_err(format!(
            "no finished run to review for task {task_id}"
        )));
    }
    Ok(())
}
