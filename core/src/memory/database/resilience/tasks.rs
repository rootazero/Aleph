//! CRUD operations for agent_tasks table
//!
//! Provides database operations for task management with recovery support.

use crate::error::AlephError;
use crate::memory::database::resilience::{AgentTask, Lane, RiskLevel, TaskStatus};
use crate::memory::database::VectorDatabase;
use rusqlite::params;
use rusqlite::OptionalExtension;

impl VectorDatabase {
    // =========================================================================
    // Agent Tasks CRUD
    // =========================================================================

    /// Insert a new agent task
    pub async fn insert_agent_task(&self, task: &AgentTask) -> Result<(), AlephError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        conn.execute(
            r#"
            INSERT INTO agent_tasks (
                id, parent_session_id, agent_id, task_prompt, status,
                risk_level, lane, checkpoint_snapshot_path, last_tool_call_id,
                recursion_depth, parent_task_id, created_at, updated_at,
                started_at, completed_at, metadata_json
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)
            "#,
            params![
                task.id,
                task.parent_session_id,
                task.agent_id,
                task.task_prompt,
                task.status.to_string(),
                task.risk_level.to_string(),
                task.lane.to_string(),
                task.checkpoint_snapshot_path,
                task.last_tool_call_id,
                task.recursion_depth,
                task.parent_task_id,
                task.created_at,
                task.updated_at,
                task.started_at,
                task.completed_at,
                task.metadata_json,
            ],
        )
        .map_err(|e| AlephError::config(format!("Failed to insert agent task: {}", e)))?;
        Ok(())
    }

    /// Get an agent task by ID
    pub async fn get_agent_task(&self, task_id: &str) -> Result<Option<AgentTask>, AlephError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let mut stmt = conn
            .prepare(
                r#"
                SELECT id, parent_session_id, agent_id, task_prompt, status,
                       risk_level, lane, checkpoint_snapshot_path, last_tool_call_id,
                       recursion_depth, parent_task_id, created_at, updated_at,
                       started_at, completed_at, metadata_json
                FROM agent_tasks WHERE id = ?1
                "#,
            )
            .map_err(|e| AlephError::config(format!("Failed to prepare query: {}", e)))?;

        let result = stmt
            .query_row(params![task_id], |row| {
                Ok(AgentTask {
                    id: row.get(0)?,
                    parent_session_id: row.get(1)?,
                    agent_id: row.get(2)?,
                    task_prompt: row.get(3)?,
                    status: TaskStatus::from_str_or_default(&row.get::<_, String>(4)?),
                    risk_level: RiskLevel::from_str_or_default(&row.get::<_, String>(5)?),
                    lane: Lane::from_str_or_default(&row.get::<_, String>(6)?),
                    checkpoint_snapshot_path: row.get(7)?,
                    last_tool_call_id: row.get(8)?,
                    recursion_depth: row.get(9)?,
                    parent_task_id: row.get(10)?,
                    created_at: row.get(11)?,
                    updated_at: row.get(12)?,
                    started_at: row.get(13)?,
                    completed_at: row.get(14)?,
                    metadata_json: row.get(15)?,
                })
            })
            .optional()
            .map_err(|e| AlephError::config(format!("Failed to get agent task: {}", e)))?;

        Ok(result)
    }

    /// Update task status
    pub async fn update_task_status(
        &self,
        task_id: &str,
        status: TaskStatus,
    ) -> Result<(), AlephError> {
        let now = chrono::Utc::now().timestamp();
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());

        // Simple update - timestamps handled separately for clarity
        conn.execute(
            r#"
            UPDATE agent_tasks
            SET status = ?1, updated_at = ?2
            WHERE id = ?3
            "#,
            params![status.to_string(), now, task_id],
        )
        .map_err(|e| AlephError::config(format!("Failed to update task status: {}", e)))?;

        // Update started_at for Running status
        if status == TaskStatus::Running {
            conn.execute(
                "UPDATE agent_tasks SET started_at = ?1 WHERE id = ?2 AND started_at IS NULL",
                params![now, task_id],
            )
            .map_err(|e| AlephError::config(format!("Failed to update started_at: {}", e)))?;
        }

        // Update completed_at for terminal states
        if matches!(status, TaskStatus::Completed | TaskStatus::Failed) {
            conn.execute(
                "UPDATE agent_tasks SET completed_at = ?1 WHERE id = ?2",
                params![now, task_id],
            )
            .map_err(|e| AlephError::config(format!("Failed to update completed_at: {}", e)))?;
        }

        Ok(())
    }

    /// Get all tasks for a session
    pub async fn get_tasks_by_session(
        &self,
        session_id: &str,
    ) -> Result<Vec<AgentTask>, AlephError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let mut stmt = conn
            .prepare(
                r#"
                SELECT id, parent_session_id, agent_id, task_prompt, status,
                       risk_level, lane, checkpoint_snapshot_path, last_tool_call_id,
                       recursion_depth, parent_task_id, created_at, updated_at,
                       started_at, completed_at, metadata_json
                FROM agent_tasks
                WHERE parent_session_id = ?1
                ORDER BY created_at DESC
                "#,
            )
            .map_err(|e| AlephError::config(format!("Failed to prepare query: {}", e)))?;

        let tasks = stmt
            .query_map(params![session_id], |row| {
                Ok(AgentTask {
                    id: row.get(0)?,
                    parent_session_id: row.get(1)?,
                    agent_id: row.get(2)?,
                    task_prompt: row.get(3)?,
                    status: TaskStatus::from_str_or_default(&row.get::<_, String>(4)?),
                    risk_level: RiskLevel::from_str_or_default(&row.get::<_, String>(5)?),
                    lane: Lane::from_str_or_default(&row.get::<_, String>(6)?),
                    checkpoint_snapshot_path: row.get(7)?,
                    last_tool_call_id: row.get(8)?,
                    recursion_depth: row.get(9)?,
                    parent_task_id: row.get(10)?,
                    created_at: row.get(11)?,
                    updated_at: row.get(12)?,
                    started_at: row.get(13)?,
                    completed_at: row.get(14)?,
                    metadata_json: row.get(15)?,
                })
            })
            .map_err(|e| AlephError::config(format!("Failed to query tasks: {}", e)))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| AlephError::config(format!("Failed to collect tasks: {}", e)))?;

        Ok(tasks)
    }

    /// Get all interrupted/running tasks for recovery on startup
    pub async fn get_recoverable_tasks(&self) -> Result<Vec<AgentTask>, AlephError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let mut stmt = conn
            .prepare(
                r#"
                SELECT id, parent_session_id, agent_id, task_prompt, status,
                       risk_level, lane, checkpoint_snapshot_path, last_tool_call_id,
                       recursion_depth, parent_task_id, created_at, updated_at,
                       started_at, completed_at, metadata_json
                FROM agent_tasks
                WHERE status IN ('running', 'interrupted')
                ORDER BY risk_level ASC, created_at ASC
                "#,
            )
            .map_err(|e| AlephError::config(format!("Failed to prepare query: {}", e)))?;

        let tasks = stmt
            .query_map([], |row| {
                Ok(AgentTask {
                    id: row.get(0)?,
                    parent_session_id: row.get(1)?,
                    agent_id: row.get(2)?,
                    task_prompt: row.get(3)?,
                    status: TaskStatus::from_str_or_default(&row.get::<_, String>(4)?),
                    risk_level: RiskLevel::from_str_or_default(&row.get::<_, String>(5)?),
                    lane: Lane::from_str_or_default(&row.get::<_, String>(6)?),
                    checkpoint_snapshot_path: row.get(7)?,
                    last_tool_call_id: row.get(8)?,
                    recursion_depth: row.get(9)?,
                    parent_task_id: row.get(10)?,
                    created_at: row.get(11)?,
                    updated_at: row.get(12)?,
                    started_at: row.get(13)?,
                    completed_at: row.get(14)?,
                    metadata_json: row.get(15)?,
                })
            })
            .map_err(|e| AlephError::config(format!("Failed to query tasks: {}", e)))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| AlephError::config(format!("Failed to collect tasks: {}", e)))?;

        Ok(tasks)
    }

    /// Update task checkpoint for recovery
    pub async fn update_task_checkpoint(
        &self,
        task_id: &str,
        checkpoint_path: &str,
        last_tool_call_id: Option<&str>,
    ) -> Result<(), AlephError> {
        let now = chrono::Utc::now().timestamp();
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        conn.execute(
            r#"
            UPDATE agent_tasks
            SET checkpoint_snapshot_path = ?1, last_tool_call_id = ?2, updated_at = ?3
            WHERE id = ?4
            "#,
            params![checkpoint_path, last_tool_call_id, now, task_id],
        )
        .map_err(|e| AlephError::config(format!("Failed to update checkpoint: {}", e)))?;
        Ok(())
    }

    /// Mark all running tasks as interrupted (for graceful shutdown)
    pub async fn mark_running_as_interrupted(&self) -> Result<u64, AlephError> {
        let now = chrono::Utc::now().timestamp();
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let count = conn
            .execute(
                r#"
                UPDATE agent_tasks
                SET status = 'interrupted', updated_at = ?1
                WHERE status = 'running'
                "#,
                params![now],
            )
            .map_err(|e| AlephError::config(format!("Failed to mark tasks: {}", e)))?;
        Ok(count as u64)
    }
}
