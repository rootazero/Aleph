//! Comment methods for `SqliteCoordTaskStore` (add/list task comments).
//! Free functions delegated to by the thin `impl CoordTaskStore` in `mod.rs`.

use rusqlite::params;

use super::helpers::{db_err, now_epoch};
use super::SqliteCoordTaskStore;
use crate::agents::swarm::tasks::CoordTaskComment;

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
    let mut stmt = conn
        .prepare_cached(
            "SELECT id, task_id, author, body, created_at FROM coord_task_comments \
             WHERE task_id = ?1 ORDER BY created_at ASC",
        )
        .map_err(db_err)?;
    let rows = stmt
        .query_map(params![task_id], |row| {
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
