//! Comment methods for `SqliteCoordTaskStore` (add/list task comments).
//! Free functions delegated to by the thin `impl CoordTaskStore` in `mod.rs`.

use rusqlite::params;

use super::helpers::{db_err, now_epoch};
use super::SqliteCoordTaskStore;
use crate::agents::swarm::tasks::CoordTaskComment;

/// Maximum number of comment rows one `list_task_comments` call may
/// materialise.
///
/// `coord_task_comments` grows one row per comment and nothing prunes it, so
/// a task with a long comment thread could force an unbounded read while
/// holding the connection mutex. The window keeps the newest comments — the
/// ones the handoff notes section renders — and returns them ascending, so
/// the listing order stays oldest-first (as the consumers expect) while the
/// read stays bounded.
const COMMENT_HISTORY_ROW_CAP: usize = 200;

pub(super) async fn add_task_comment(
    store: &SqliteCoordTaskStore,
    task_id: &str,
    author: &str,
    body: &str,
) -> crate::error::Result<CoordTaskComment> {
    let conn = store.conn.lock().await;
    let id = uuid::Uuid::new_v4().to_string();
    let now = now_epoch();
    conn.execute(
        "INSERT INTO coord_task_comments (id, task_id, author, body, created_at) \
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![id, task_id, author, body, now],
    )
    .map_err(db_err)?;
    Ok(CoordTaskComment {
        id,
        task_id: task_id.to_string(),
        author: author.to_string(),
        body: body.to_string(),
        created_at: now,
    })
}

pub(super) async fn list_task_comments(
    store: &SqliteCoordTaskStore,
    task_id: &str,
) -> crate::error::Result<Vec<CoordTaskComment>> {
    let conn = store.conn.lock().await;
    // `created_at` has whole-second granularity, so two comments written in
    // the same second tie — `rowid` (insertion order) is the deterministic
    // tiebreak that keeps the oldest-first listing order unambiguous.
    let cap = COMMENT_HISTORY_ROW_CAP as i64;
    let mut stmt = conn
        .prepare_cached(
            "SELECT id, task_id, author, body, created_at FROM coord_task_comments \
             WHERE task_id = ?1 \
               AND rowid IN ( \
                   SELECT rowid FROM coord_task_comments \
                   WHERE task_id = ?1 \
                   ORDER BY created_at DESC, rowid DESC \
                   LIMIT ?2 \
               ) \
             ORDER BY created_at ASC, rowid ASC",
        )
        .map_err(db_err)?;
    let rows = stmt
        .query_map(params![task_id, cap], |row| {
            Ok(CoordTaskComment {
                id: row.get(0)?,
                task_id: row.get(1)?,
                author: row.get(2)?,
                body: row.get(3)?,
                created_at: row.get(4)?,
            })
        })
        .map_err(db_err)?;
    let mut comments = Vec::new();
    for r in rows {
        comments.push(r.map_err(db_err)?);
    }
    Ok(comments)
}
