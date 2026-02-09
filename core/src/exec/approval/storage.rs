use rusqlite::{Connection, Result as SqliteResult, params};
use std::path::Path;
use tokio::sync::Mutex;

pub struct ApprovalAuditStorage {
    conn: Mutex<Connection>,
}

impl ApprovalAuditStorage {
    pub async fn new(db_path: &Path) -> SqliteResult<Self> {
        let conn = Connection::open(db_path)?;

        // Create capability_approvals table
        conn.execute(
            "CREATE TABLE IF NOT EXISTS capability_approvals (
                id INTEGER PRIMARY KEY,
                tool_name TEXT NOT NULL,
                capabilities_hash TEXT NOT NULL,
                approved BOOLEAN NOT NULL,
                approved_by TEXT NOT NULL,
                approval_scope TEXT NOT NULL,
                approved_at INTEGER NOT NULL,
                reason TEXT
            )",
            [],
        )?;

        // Create capability_escalations table
        conn.execute(
            "CREATE TABLE IF NOT EXISTS capability_escalations (
                id INTEGER PRIMARY KEY,
                tool_name TEXT NOT NULL,
                execution_id TEXT NOT NULL,
                escalation_reason TEXT NOT NULL,
                requested_path TEXT,
                approved_paths TEXT,
                user_decision TEXT,
                decided_at INTEGER NOT NULL
            )",
            [],
        )?;

        // Create tool_executions table for tracking execution history
        conn.execute(
            "CREATE TABLE IF NOT EXISTS tool_executions (
                id INTEGER PRIMARY KEY,
                execution_id TEXT NOT NULL UNIQUE,
                tool_name TEXT NOT NULL,
                timestamp INTEGER NOT NULL,
                parameters TEXT,
                escalation_triggered BOOLEAN NOT NULL DEFAULT 0
            )",
            [],
        )?;

        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    /// Get escalation count for a tool
    pub async fn get_escalation_count(&self, tool_name: &str) -> SqliteResult<u32> {
        let conn = self.conn.lock().await;
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM capability_escalations WHERE tool_name = ?1",
            params![tool_name],
            |row| row.get(0),
        )?;
        Ok(count as u32)
    }

    /// Get execution count for a tool
    pub async fn get_execution_count(&self, tool_name: &str) -> SqliteResult<u32> {
        let conn = self.conn.lock().await;
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM tool_executions WHERE tool_name = ?1",
            params![tool_name],
            |row| row.get(0),
        )?;
        Ok(count as u32)
    }

    /// Get last execution timestamp for a tool
    pub async fn get_last_execution_time(&self, tool_name: &str) -> SqliteResult<Option<i64>> {
        let conn = self.conn.lock().await;
        let result = conn.query_row(
            "SELECT MAX(timestamp) FROM tool_executions WHERE tool_name = ?1",
            params![tool_name],
            |row| row.get(0),
        );

        match result {
            Ok(time) => Ok(Some(time)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Get execution history for a tool
    pub async fn get_execution_history(
        &self,
        tool_name: &str,
        limit: usize,
    ) -> SqliteResult<Vec<(String, i64, String, bool)>> {
        let conn = self.conn.lock().await;
        let mut stmt = conn.prepare(
            "SELECT execution_id, timestamp, parameters, escalation_triggered
             FROM tool_executions
             WHERE tool_name = ?1
             ORDER BY timestamp DESC
             LIMIT ?2",
        )?;

        let rows = stmt.query_map(params![tool_name, limit as i64], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
            ))
        })?;

        rows.collect()
    }

    /// Get escalation details for an execution
    pub async fn get_escalation_details(
        &self,
        execution_id: &str,
    ) -> SqliteResult<Option<(String, Option<String>, Option<String>)>> {
        let conn = self.conn.lock().await;
        let result = conn.query_row(
            "SELECT escalation_reason, requested_path, user_decision
             FROM capability_escalations
             WHERE execution_id = ?1",
            params![execution_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        );

        match result {
            Ok(data) => Ok(Some(data)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Get all escalations with limit
    pub async fn get_all_escalations(
        &self,
        limit: usize,
    ) -> SqliteResult<Vec<(String, String, i64, String, Option<String>, Option<String>)>> {
        let conn = self.conn.lock().await;
        let mut stmt = conn.prepare(
            "SELECT e.execution_id, e.tool_name, e.decided_at, e.escalation_reason,
                    e.requested_path, e.user_decision
             FROM capability_escalations e
             ORDER BY e.decided_at DESC
             LIMIT ?1",
        )?;

        let rows = stmt.query_map(params![limit as i64], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get(5)?,
            ))
        })?;

        rows.collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_create_audit_tables() {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("test.db");

        let storage = ApprovalAuditStorage::new(&db_path).await.unwrap();

        // Verify tables exist
        let conn = storage.conn.lock().await;
        let tables: Vec<String> = conn
            .prepare("SELECT name FROM sqlite_master WHERE type='table'")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();

        assert!(tables.contains(&"capability_approvals".to_string()));
        assert!(tables.contains(&"capability_escalations".to_string()));
    }
}
