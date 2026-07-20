//! Exit-journal methods for `SqliteCoordTaskStore` (R3 ClawTeam parity).
//! Free functions delegated to by the thin `impl CoordTaskStore` in `mod.rs`.

use rusqlite::params;

use super::helpers::{db_err, now_epoch};
use super::SqliteCoordTaskStore;

/// Decode a JSON-encoded journal array column. A corrupt payload would
/// otherwise look like a legitimately empty list — log a warning so the
/// operator can spot disk/encoding drift instead of silently losing history.
fn decode_journal_field(task_id: &str, field: &str, raw: &str) -> Vec<String> {
    serde_json::from_str(raw).unwrap_or_else(|e| {
        tracing::warn!(
            task_id,
            field,
            error = %e,
            "coord_task_journals: corrupt JSON, treating as empty list"
        );
        Vec::new()
    })
}

pub(super) async fn upsert_task_journal(
    store: &SqliteCoordTaskStore,
    input: crate::agents::swarm::tasks::NewTaskExitJournal,
) -> crate::error::Result<crate::agents::swarm::tasks::TaskExitJournal> {
    let decisions_json = serde_json::to_string(&input.decisions)
        .map_err(|e| db_err(format!("decisions serialize failed: {e}")))?;
    let artifacts_json = serde_json::to_string(&input.artifacts_ref)
        .map_err(|e| db_err(format!("artifacts_ref serialize failed: {e}")))?;
    let next_steps_json = serde_json::to_string(&input.next_steps)
        .map_err(|e| db_err(format!("next_steps serialize failed: {e}")))?;
    let now = now_epoch();
    let confidence_i: Option<i64> = input.confidence.map(|v| i64::from(v.min(100)));
    let conn = store.conn.lock().await;
    conn.execute(
        r#"
        INSERT INTO coord_task_journals
          (task_id, agent_id, summary, decisions, artifacts_ref, next_steps, confidence, created_at)
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
        ON CONFLICT(task_id) DO UPDATE SET
          agent_id      = excluded.agent_id,
          summary       = excluded.summary,
          decisions     = excluded.decisions,
          artifacts_ref = excluded.artifacts_ref,
          next_steps    = excluded.next_steps,
          confidence    = excluded.confidence,
          created_at    = excluded.created_at
        "#,
        params![
            input.task_id,
            input.agent_id,
            input.summary,
            decisions_json,
            artifacts_json,
            next_steps_json,
            confidence_i,
            now,
        ],
    )
    .map_err(db_err)?;
    Ok(crate::agents::swarm::tasks::TaskExitJournal {
        task_id: input.task_id,
        agent_id: input.agent_id,
        summary: input.summary,
        decisions: input.decisions,
        artifacts_ref: input.artifacts_ref,
        next_steps: input.next_steps,
        confidence: input.confidence,
        created_at: now,
    })
}

pub(super) async fn get_task_journal(
    store: &SqliteCoordTaskStore,
    task_id: &str,
) -> crate::error::Result<Option<crate::agents::swarm::tasks::TaskExitJournal>> {
    let conn = store.conn.lock().await;
    let mut stmt = conn
        .prepare_cached(
            "SELECT task_id, agent_id, summary, decisions, artifacts_ref, next_steps, \
                    confidence, created_at \
             FROM coord_task_journals WHERE task_id = ?1",
        )
        .map_err(db_err)?;
    let mut rows = stmt
        .query_map(params![task_id], |row| {
            let decisions_raw: String = row.get(3)?;
            let artifacts_raw: String = row.get(4)?;
            let next_raw: String = row.get(5)?;
            let confidence_i: Option<i64> = row.get(6)?;
            let decisions = decode_journal_field(task_id, "decisions", &decisions_raw);
            let artifacts_ref =
                decode_journal_field(task_id, "artifacts_ref", &artifacts_raw);
            let next_steps = decode_journal_field(task_id, "next_steps", &next_raw);
            Ok(crate::agents::swarm::tasks::TaskExitJournal {
                task_id: row.get(0)?,
                agent_id: row.get(1)?,
                summary: row.get(2)?,
                decisions,
                artifacts_ref,
                next_steps,
                confidence: confidence_i.map(|v| v.clamp(0, 100) as u8),
                created_at: row.get::<_, i64>(7)? as u64,
            })
        })
        .map_err(db_err)?;
    match rows.next() {
        Some(r) => Ok(Some(r.map_err(db_err)?)),
        None => Ok(None),
    }
}

pub(super) async fn list_team_journals(
    store: &SqliteCoordTaskStore,
    team_id: &str,
) -> crate::error::Result<Vec<crate::agents::swarm::tasks::TaskExitJournal>> {
    let conn = store.conn.lock().await;
    let mut stmt = conn
        .prepare_cached(
            "SELECT j.task_id, j.agent_id, j.summary, j.decisions, j.artifacts_ref, \
                    j.next_steps, j.confidence, j.created_at \
             FROM coord_task_journals j \
             JOIN coord_tasks t ON t.id = j.task_id \
             WHERE t.team_id = ?1 \
             ORDER BY j.created_at DESC",
        )
        .map_err(db_err)?;
    let rows = stmt
        .query_map(params![team_id], |row| {
            let decisions_raw: String = row.get(3)?;
            let artifacts_raw: String = row.get(4)?;
            let next_raw: String = row.get(5)?;
            let confidence_i: Option<i64> = row.get(6)?;
            let row_task_id: String = row.get(0)?;
            let decisions = decode_journal_field(&row_task_id, "decisions", &decisions_raw);
            let artifacts_ref =
                decode_journal_field(&row_task_id, "artifacts_ref", &artifacts_raw);
            let next_steps = decode_journal_field(&row_task_id, "next_steps", &next_raw);
            Ok(crate::agents::swarm::tasks::TaskExitJournal {
                task_id: row_task_id,
                agent_id: row.get(1)?,
                summary: row.get(2)?,
                decisions,
                artifacts_ref,
                next_steps,
                confidence: confidence_i.map(|v| v.clamp(0, 100) as u8),
                created_at: row.get::<_, i64>(7)? as u64,
            })
        })
        .map_err(db_err)?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(db_err)?);
    }
    Ok(out)
}
