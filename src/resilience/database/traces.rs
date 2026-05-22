//! CRUD operations for task_traces table
//!
//! Provides database operations for execution trace management,
//! enabling Shadow Replay for deterministic task recovery.

use super::StateDatabase;
use crate::error::AlephError;
use crate::resilience::{TaskTrace, TaskTraceInfo};
use aleph_protocol::AgentTraceEvent;
use rusqlite::params;
use rusqlite::types::Type;
use rusqlite::OptionalExtension;

/// Construct TaskTrace from a rusqlite row.
/// Expected column order: id, task_id, step_index, event_kind, event_json, timestamp
fn task_trace_from_row(row: &rusqlite::Row) -> rusqlite::Result<TaskTrace> {
    let event_kind: String = row.get(3)?;
    let event_json: String = row.get(4)?;
    let event = serde_json::from_str::<AgentTraceEvent>(&event_json)
        .map_err(|err| rusqlite::Error::FromSqlConversionFailure(4, Type::Text, Box::new(err)))?;

    if event.kind() != event_kind {
        tracing::warn!(
            stored_kind = %event_kind,
            parsed_kind = event.kind(),
            "task_traces row has mismatched event_kind and event_json"
        );
    }

    Ok(TaskTrace {
        id: row.get(0)?,
        task_id: row.get(1)?,
        step_index: row.get(2)?,
        event,
        timestamp: row.get(5)?,
    })
}

impl StateDatabase {
    // =========================================================================
    // Task Traces CRUD
    // =========================================================================

    /// Insert a single trace entry
    pub async fn insert_trace(&self, trace: &TaskTrace) -> Result<i64, AlephError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let event_json = serde_json::to_string(&trace.event)
            .map_err(|e| AlephError::config(format!("Failed to serialize trace event: {}", e)))?;
        conn.execute(
            r#"
            INSERT INTO task_traces (task_id, step_index, event_kind, event_json, timestamp)
            VALUES (?1, ?2, ?3, ?4, ?5)
            "#,
            params![
                trace.task_id,
                trace.step_index,
                trace.event_kind(),
                event_json,
                trace.timestamp,
            ],
        )
        .map_err(|e| AlephError::config(format!("Failed to insert trace: {}", e)))?;

        Ok(conn.last_insert_rowid())
    }

    /// Bulk insert traces (for efficient batch writes)
    pub async fn bulk_insert_traces(&self, traces: &[TaskTrace]) -> Result<(), AlephError> {
        if traces.is_empty() {
            return Ok(());
        }

        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let mut stmt = conn
            .prepare(
                r#"
                INSERT INTO task_traces (task_id, step_index, event_kind, event_json, timestamp)
                VALUES (?1, ?2, ?3, ?4, ?5)
                "#,
            )
            .map_err(|e| AlephError::config(format!("Failed to prepare statement: {}", e)))?;

        for trace in traces {
            let event_json = serde_json::to_string(&trace.event).map_err(|e| {
                AlephError::config(format!("Failed to serialize trace event: {}", e))
            })?;
            stmt.execute(params![
                trace.task_id,
                trace.step_index,
                trace.event_kind(),
                event_json,
                trace.timestamp,
            ])
            .map_err(|e| AlephError::config(format!("Failed to insert trace: {}", e)))?;
        }

        Ok(())
    }

    /// Get all traces for a task (ordered by step_index)
    pub async fn get_traces_by_task(&self, task_id: &str) -> Result<Vec<TaskTrace>, AlephError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let mut stmt = conn
            .prepare(
                r#"
                SELECT id, task_id, step_index, event_kind, event_json, timestamp
                FROM task_traces
                WHERE task_id = ?1
                ORDER BY step_index ASC
                "#,
            )
            .map_err(|e| AlephError::config(format!("Failed to prepare query: {}", e)))?;

        let traces = stmt
            .query_map(params![task_id], task_trace_from_row)
            .map_err(|e| AlephError::config(format!("Failed to query traces: {}", e)))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| AlephError::config(format!("Failed to collect traces: {}", e)))?;

        Ok(traces)
    }

    /// Get the last trace entry for a task (for recovery checkpoint)
    pub async fn get_last_trace(&self, task_id: &str) -> Result<Option<TaskTrace>, AlephError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let mut stmt = conn
            .prepare(
                r#"
                SELECT id, task_id, step_index, event_kind, event_json, timestamp
                FROM task_traces
                WHERE task_id = ?1
                ORDER BY step_index DESC
                LIMIT 1
                "#,
            )
            .map_err(|e| AlephError::config(format!("Failed to prepare query: {}", e)))?;

        let result = stmt
            .query_row(params![task_id], task_trace_from_row)
            .optional()
            .map_err(|e| AlephError::config(format!("Failed to get last trace: {}", e)))?;

        Ok(result)
    }

    /// Get traces from a specific step index (for resuming from checkpoint)
    pub async fn get_traces_from_step(
        &self,
        task_id: &str,
        from_step: u32,
    ) -> Result<Vec<TaskTrace>, AlephError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let mut stmt = conn
            .prepare(
                r#"
                SELECT id, task_id, step_index, event_kind, event_json, timestamp
                FROM task_traces
                WHERE task_id = ?1 AND step_index >= ?2
                ORDER BY step_index ASC
                "#,
            )
            .map_err(|e| AlephError::config(format!("Failed to prepare query: {}", e)))?;

        let traces = stmt
            .query_map(params![task_id, from_step], task_trace_from_row)
            .map_err(|e| AlephError::config(format!("Failed to query traces: {}", e)))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| AlephError::config(format!("Failed to collect traces: {}", e)))?;

        Ok(traces)
    }

    /// Delete all traces for a task (cleanup)
    pub async fn delete_traces_for_task(&self, task_id: &str) -> Result<u64, AlephError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let count = conn
            .execute(
                "DELETE FROM task_traces WHERE task_id = ?1",
                params![task_id],
            )
            .map_err(|e| AlephError::config(format!("Failed to delete traces: {}", e)))?;
        Ok(count as u64)
    }

    /// Get trace count for a task
    pub async fn get_trace_count(&self, task_id: &str) -> Result<u64, AlephError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM task_traces WHERE task_id = ?1",
                params![task_id],
                |row| row.get(0),
            )
            .map_err(|e| AlephError::config(format!("Failed to count traces: {}", e)))?;
        Ok(count as u64)
    }

    /// List all distinct task IDs that have traces
    pub async fn list_trace_tasks(&self) -> Result<Vec<TaskTraceInfo>, AlephError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let mut stmt = conn
            .prepare(
                r#"
                SELECT task_id, COUNT(*) as event_count, MAX(timestamp) as last_timestamp
                FROM task_traces
                GROUP BY task_id
                ORDER BY last_timestamp DESC
                "#,
            )
            .map_err(|e| AlephError::config(format!("Failed to prepare query: {}", e)))?;

        let tasks = stmt
            .query_map([], |row| {
                Ok(TaskTraceInfo {
                    task_id: row.get(0)?,
                    event_count: row.get(1)?,
                    last_timestamp: row.get(2)?,
                })
            })
            .map_err(|e| AlephError::config(format!("Failed to query traces: {}", e)))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| AlephError::config(format!("Failed to collect traces: {}", e)))?;

        Ok(tasks)
    }

    /// Paginated sibling of `list_trace_tasks`. Returns at most `limit`
    /// (clamped to 1..200) trace-task summaries whose `last_timestamp` is
    /// strictly less than `before_timestamp` (when set), ordered DESC.
    ///
    /// Keeps each page O(limit) regardless of total trace volume, so
    /// callers can paginate without scanning the whole table on every
    /// request. The existing `list_trace_tasks` is preserved for callers
    /// that want everything in one shot.
    pub async fn list_trace_tasks_paged(
        &self,
        limit: usize,
        before_timestamp: Option<i64>,
    ) -> Result<Vec<TaskTraceInfo>, AlephError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let clamped_limit = limit.clamp(1, 200) as i64;

        let row_map = |row: &rusqlite::Row<'_>| {
            Ok(TaskTraceInfo {
                task_id: row.get(0)?,
                event_count: row.get(1)?,
                last_timestamp: row.get(2)?,
            })
        };

        let collect_err =
            |e: rusqlite::Error| AlephError::config(format!("Failed to collect paged traces: {e}"));

        match before_timestamp {
            Some(ts) => {
                let mut stmt = conn
                    .prepare(
                        r#"
                        SELECT task_id, COUNT(*) as event_count, MAX(timestamp) as last_timestamp
                        FROM task_traces
                        GROUP BY task_id
                        HAVING MAX(timestamp) < ?1
                        ORDER BY last_timestamp DESC
                        LIMIT ?2
                        "#,
                    )
                    .map_err(|e| {
                        AlephError::config(format!("Failed to prepare paged query: {e}"))
                    })?;
                let rows = stmt
                    .query_map(params![ts, clamped_limit], row_map)
                    .map_err(|e| {
                        AlephError::config(format!("Failed to query paged traces: {e}"))
                    })?;
                let collected: Result<Vec<_>, _> = rows.collect();
                collected.map_err(collect_err)
            }
            None => {
                let mut stmt = conn
                    .prepare(
                        r#"
                        SELECT task_id, COUNT(*) as event_count, MAX(timestamp) as last_timestamp
                        FROM task_traces
                        GROUP BY task_id
                        ORDER BY last_timestamp DESC
                        LIMIT ?1
                        "#,
                    )
                    .map_err(|e| {
                        AlephError::config(format!("Failed to prepare paged query: {e}"))
                    })?;
                let rows = stmt
                    .query_map(params![clamped_limit], row_map)
                    .map_err(|e| {
                        AlephError::config(format!("Failed to query paged traces: {e}"))
                    })?;
                let collected: Result<Vec<_>, _> = rows.collect();
                collected.map_err(collect_err)
            }
        }
    }

    /// Get a trace by its ID
    pub async fn get_trace_by_id(&self, trace_id: i64) -> Result<Option<TaskTrace>, AlephError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let mut stmt = conn
            .prepare(
                r#"
                SELECT id, task_id, step_index, event_kind, event_json, timestamp
                FROM task_traces
                WHERE id = ?1
                "#,
            )
            .map_err(|e| AlephError::config(format!("Failed to prepare query: {}", e)))?;

        let result = stmt
            .query_row(params![trace_id], task_trace_from_row)
            .optional()
            .map_err(|e| AlephError::config(format!("Failed to get trace: {}", e)))?;

        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resilience::{AgentTask, RiskLevel};
    use aleph_protocol::{AgentTraceEvent, AgentTraceTextKind};

    #[tokio::test]
    async fn test_insert_and_get_structured_trace() {
        let db = StateDatabase::in_memory().unwrap();
        db.insert_agent_task(&AgentTask::new(
            "task-1",
            "session-1",
            "coder",
            "replay trace",
            RiskLevel::Low,
        ))
        .await
        .unwrap();

        let trace = TaskTrace::new(
            "task-1",
            0,
            AgentTraceEvent::TextEmitted {
                iteration: 0,
                stream: AgentTraceTextKind::Final,
                text: "hello".to_string(),
            },
        );

        db.insert_trace(&trace).await.unwrap();

        let traces = db.get_traces_by_task("task-1").await.unwrap();
        assert_eq!(traces.len(), 1);
        assert_eq!(traces[0].event.kind(), "text_emitted");
        assert_eq!(
            traces[0].event,
            AgentTraceEvent::TextEmitted {
                iteration: 0,
                stream: AgentTraceTextKind::Final,
                text: "hello".to_string(),
            }
        );
    }

    // -------------------------------------------------------------------------
    // P1 — paginated trace task listing
    // -------------------------------------------------------------------------

    async fn seed_n_tasks_each_one_trace(db: &StateDatabase, n: usize) {
        for i in 0..n {
            let tid = format!("task-{i}");
            db.insert_agent_task(&AgentTask::new(
                &tid,
                "session",
                "coder",
                "seeded",
                RiskLevel::Low,
            ))
            .await
            .unwrap();
            db.insert_trace(&TaskTrace::new(
                &tid,
                0,
                AgentTraceEvent::TextEmitted {
                    iteration: 0,
                    stream: AgentTraceTextKind::Final,
                    text: format!("payload-{i}"),
                },
            ))
            .await
            .unwrap();
        }
    }

    #[tokio::test]
    async fn list_paged_returns_at_most_limit() {
        let db = StateDatabase::in_memory().unwrap();
        seed_n_tasks_each_one_trace(&db, 5).await;

        let page = db.list_trace_tasks_paged(3, None).await.unwrap();
        assert_eq!(page.len(), 3);
    }

    #[tokio::test]
    async fn list_paged_clamps_oversize_limit() {
        let db = StateDatabase::in_memory().unwrap();
        seed_n_tasks_each_one_trace(&db, 5).await;

        // 9999 must clamp to <=200 (and we only have 5 tasks, so we get 5).
        let page = db.list_trace_tasks_paged(9999, None).await.unwrap();
        assert_eq!(page.len(), 5);
    }

    #[tokio::test]
    async fn list_paged_cursor_advances_without_overlap() {
        let db = StateDatabase::in_memory().unwrap();
        // Timestamps in TaskTrace::new() come from chrono::Utc::now().timestamp()
        // — Unix epoch SECONDS — so rapid inserts collide. We need strictly
        // differing timestamps because the cursor uses HAVING MAX(timestamp) < ?.
        // Build TaskTrace by hand with explicit increasing timestamps.
        let base_ts = chrono::Utc::now().timestamp();
        for i in 0..4i64 {
            let tid = format!("task-{i}");
            db.insert_agent_task(&AgentTask::new(&tid, "s", "coder", "x", RiskLevel::Low))
                .await
                .unwrap();
            let trace = TaskTrace {
                id: 0,
                task_id: tid.clone(),
                step_index: 0,
                event: AgentTraceEvent::TextEmitted {
                    iteration: 0,
                    stream: AgentTraceTextKind::Final,
                    text: "x".into(),
                },
                timestamp: base_ts + i,
            };
            db.insert_trace(&trace).await.unwrap();
        }

        let page_a = db.list_trace_tasks_paged(2, None).await.unwrap();
        assert_eq!(page_a.len(), 2);
        let cursor = page_a.last().unwrap().last_timestamp;

        let page_b = db.list_trace_tasks_paged(2, Some(cursor)).await.unwrap();
        assert!(!page_b.is_empty());
        for r in &page_b {
            assert!(
                page_a.iter().all(|p| p.task_id != r.task_id),
                "page B leaked page A row: {}",
                r.task_id
            );
        }
    }
}
