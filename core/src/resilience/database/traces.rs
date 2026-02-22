//! CRUD operations for task_traces table
//!
//! Provides database operations for execution trace management,
//! enabling Shadow Replay for deterministic task recovery.

use crate::error::AlephError;
use crate::resilience::{TaskTrace, TraceRole};
use super::StateDatabase;
use rusqlite::params;
use rusqlite::OptionalExtension;

impl StateDatabase {
    // =========================================================================
    // Task Traces CRUD
    // =========================================================================

    /// Insert a single trace entry
    pub async fn insert_trace(&self, trace: &TaskTrace) -> Result<i64, AlephError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        conn.execute(
            r#"
            INSERT INTO task_traces (task_id, step_index, role, content_json, timestamp)
            VALUES (?1, ?2, ?3, ?4, ?5)
            "#,
            params![
                trace.task_id,
                trace.step_index,
                trace.role.to_string(),
                trace.content_json,
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
                INSERT INTO task_traces (task_id, step_index, role, content_json, timestamp)
                VALUES (?1, ?2, ?3, ?4, ?5)
                "#,
            )
            .map_err(|e| AlephError::config(format!("Failed to prepare statement: {}", e)))?;

        for trace in traces {
            stmt.execute(params![
                trace.task_id,
                trace.step_index,
                trace.role.to_string(),
                trace.content_json,
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
                SELECT id, task_id, step_index, role, content_json, timestamp
                FROM task_traces
                WHERE task_id = ?1
                ORDER BY step_index ASC
                "#,
            )
            .map_err(|e| AlephError::config(format!("Failed to prepare query: {}", e)))?;

        let traces = stmt
            .query_map(params![task_id], |row| {
                Ok(TaskTrace {
                    id: row.get(0)?,
                    task_id: row.get(1)?,
                    step_index: row.get(2)?,
                    role: TraceRole::from_str_or_default(&row.get::<_, String>(3)?),
                    content_json: row.get(4)?,
                    timestamp: row.get(5)?,
                })
            })
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
                SELECT id, task_id, step_index, role, content_json, timestamp
                FROM task_traces
                WHERE task_id = ?1
                ORDER BY step_index DESC
                LIMIT 1
                "#,
            )
            .map_err(|e| AlephError::config(format!("Failed to prepare query: {}", e)))?;

        let result = stmt
            .query_row(params![task_id], |row| {
                Ok(TaskTrace {
                    id: row.get(0)?,
                    task_id: row.get(1)?,
                    step_index: row.get(2)?,
                    role: TraceRole::from_str_or_default(&row.get::<_, String>(3)?),
                    content_json: row.get(4)?,
                    timestamp: row.get(5)?,
                })
            })
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
                SELECT id, task_id, step_index, role, content_json, timestamp
                FROM task_traces
                WHERE task_id = ?1 AND step_index >= ?2
                ORDER BY step_index ASC
                "#,
            )
            .map_err(|e| AlephError::config(format!("Failed to prepare query: {}", e)))?;

        let traces = stmt
            .query_map(params![task_id, from_step], |row| {
                Ok(TaskTrace {
                    id: row.get(0)?,
                    task_id: row.get(1)?,
                    step_index: row.get(2)?,
                    role: TraceRole::from_str_or_default(&row.get::<_, String>(3)?),
                    content_json: row.get(4)?,
                    timestamp: row.get(5)?,
                })
            })
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
}
