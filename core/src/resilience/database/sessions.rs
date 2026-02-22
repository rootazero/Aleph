//! CRUD operations for subagent_sessions table
//!
//! Provides database operations for long-lived subagent session management
//! (Session-as-a-Service).

use crate::error::AlephError;
use crate::resilience::{SessionStatus, SubagentSession};
use super::StateDatabase;
use rusqlite::params;
use rusqlite::OptionalExtension;

impl StateDatabase {
    // =========================================================================
    // Subagent Sessions CRUD
    // =========================================================================

    /// Insert a new subagent session
    pub async fn insert_session(&self, session: &SubagentSession) -> Result<(), AlephError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        conn.execute(
            r#"
            INSERT INTO subagent_sessions (
                id, agent_type, status, context_path, parent_session_id,
                created_at, last_active_at, total_tokens_used, total_tool_calls
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
            "#,
            params![
                session.id,
                session.agent_type,
                session.status.to_string(),
                session.context_path,
                session.parent_session_id,
                session.created_at,
                session.last_active_at,
                session.total_tokens_used,
                session.total_tool_calls,
            ],
        )
        .map_err(|e| AlephError::config(format!("Failed to insert session: {}", e)))?;
        Ok(())
    }

    /// Get a subagent session by ID
    pub async fn get_session(
        &self,
        session_id: &str,
    ) -> Result<Option<SubagentSession>, AlephError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let mut stmt = conn
            .prepare(
                r#"
                SELECT id, agent_type, status, context_path, parent_session_id,
                       created_at, last_active_at, total_tokens_used, total_tool_calls
                FROM subagent_sessions WHERE id = ?1
                "#,
            )
            .map_err(|e| AlephError::config(format!("Failed to prepare query: {}", e)))?;

        let result = stmt
            .query_row(params![session_id], |row| {
                Ok(SubagentSession {
                    id: row.get(0)?,
                    agent_type: row.get(1)?,
                    status: SessionStatus::from_str_or_default(&row.get::<_, String>(2)?),
                    context_path: row.get(3)?,
                    parent_session_id: row.get(4)?,
                    created_at: row.get(5)?,
                    last_active_at: row.get(6)?,
                    total_tokens_used: row.get(7)?,
                    total_tool_calls: row.get(8)?,
                })
            })
            .optional()
            .map_err(|e| AlephError::config(format!("Failed to get session: {}", e)))?;

        Ok(result)
    }

    /// Update session status
    pub async fn update_session_status(
        &self,
        session_id: &str,
        status: SessionStatus,
        context_path: Option<&str>,
    ) -> Result<(), AlephError> {
        let now = chrono::Utc::now().timestamp();
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        conn.execute(
            r#"
            UPDATE subagent_sessions
            SET status = ?1, context_path = COALESCE(?2, context_path), last_active_at = ?3
            WHERE id = ?4
            "#,
            params![status.to_string(), context_path, now, session_id],
        )
        .map_err(|e| AlephError::config(format!("Failed to update session status: {}", e)))?;
        Ok(())
    }

    /// Get all sessions by parent session ID
    pub async fn get_sessions_by_parent(
        &self,
        parent_session_id: &str,
    ) -> Result<Vec<SubagentSession>, AlephError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let mut stmt = conn
            .prepare(
                r#"
                SELECT id, agent_type, status, context_path, parent_session_id,
                       created_at, last_active_at, total_tokens_used, total_tool_calls
                FROM subagent_sessions
                WHERE parent_session_id = ?1
                ORDER BY last_active_at DESC
                "#,
            )
            .map_err(|e| AlephError::config(format!("Failed to prepare query: {}", e)))?;

        let sessions = stmt
            .query_map(params![parent_session_id], |row| {
                Ok(SubagentSession {
                    id: row.get(0)?,
                    agent_type: row.get(1)?,
                    status: SessionStatus::from_str_or_default(&row.get::<_, String>(2)?),
                    context_path: row.get(3)?,
                    parent_session_id: row.get(4)?,
                    created_at: row.get(5)?,
                    last_active_at: row.get(6)?,
                    total_tokens_used: row.get(7)?,
                    total_tool_calls: row.get(8)?,
                })
            })
            .map_err(|e| AlephError::config(format!("Failed to query sessions: {}", e)))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| AlephError::config(format!("Failed to collect sessions: {}", e)))?;

        Ok(sessions)
    }

    /// Get idle sessions (candidates for swap-out)
    pub async fn get_idle_sessions(&self, limit: u32) -> Result<Vec<SubagentSession>, AlephError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let mut stmt = conn
            .prepare(
                r#"
                SELECT id, agent_type, status, context_path, parent_session_id,
                       created_at, last_active_at, total_tokens_used, total_tool_calls
                FROM subagent_sessions
                WHERE status = 'idle'
                ORDER BY last_active_at ASC
                LIMIT ?1
                "#,
            )
            .map_err(|e| AlephError::config(format!("Failed to prepare query: {}", e)))?;

        let sessions = stmt
            .query_map(params![limit], |row| {
                Ok(SubagentSession {
                    id: row.get(0)?,
                    agent_type: row.get(1)?,
                    status: SessionStatus::Idle,
                    context_path: row.get(3)?,
                    parent_session_id: row.get(4)?,
                    created_at: row.get(5)?,
                    last_active_at: row.get(6)?,
                    total_tokens_used: row.get(7)?,
                    total_tool_calls: row.get(8)?,
                })
            })
            .map_err(|e| AlephError::config(format!("Failed to query sessions: {}", e)))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| AlephError::config(format!("Failed to collect sessions: {}", e)))?;

        Ok(sessions)
    }

    /// Update session resource usage
    pub async fn update_session_usage(
        &self,
        session_id: &str,
        tokens_delta: u64,
        tool_calls_delta: u64,
    ) -> Result<(), AlephError> {
        let now = chrono::Utc::now().timestamp();
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        conn.execute(
            r#"
            UPDATE subagent_sessions
            SET total_tokens_used = total_tokens_used + ?1,
                total_tool_calls = total_tool_calls + ?2,
                last_active_at = ?3
            WHERE id = ?4
            "#,
            params![tokens_delta, tool_calls_delta, now, session_id],
        )
        .map_err(|e| AlephError::config(format!("Failed to update session usage: {}", e)))?;
        Ok(())
    }

    /// Count sessions by status
    pub async fn count_sessions_by_status(
        &self,
        status: SessionStatus,
    ) -> Result<u64, AlephError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM subagent_sessions WHERE status = ?1",
                params![status.to_string()],
                |row| row.get(0),
            )
            .map_err(|e| AlephError::config(format!("Failed to count sessions: {}", e)))?;
        Ok(count as u64)
    }

    /// Delete a session
    pub async fn delete_session(&self, session_id: &str) -> Result<(), AlephError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let deleted = conn
            .execute(
                "DELETE FROM subagent_sessions WHERE id = ?1",
                params![session_id],
            )
            .map_err(|e| AlephError::config(format!("Failed to delete session: {}", e)))?;

        if deleted == 0 {
            return Err(AlephError::config(format!(
                "Session not found: {}",
                session_id
            )));
        }
        Ok(())
    }
}
