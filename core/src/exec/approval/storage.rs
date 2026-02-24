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
                capabilities_json TEXT,
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
        let result: Result<Option<i64>, _> = conn.query_row(
            "SELECT MAX(timestamp) FROM tool_executions WHERE tool_name = ?1",
            params![tool_name],
            |row| row.get(0),
        );

        match result {
            Ok(time) => Ok(time),
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

    /// Get tool capabilities from the most recent approval
    pub async fn get_tool_capabilities(&self, tool_name: &str) -> SqliteResult<Vec<String>> {
        let conn = self.conn.lock().await;

        // Try to get the most recent capabilities JSON for this tool
        let result = conn.query_row(
            "SELECT capabilities_json FROM capability_approvals
             WHERE tool_name = ?1 AND approved = 1
             ORDER BY approved_at DESC
             LIMIT 1",
            params![tool_name],
            |row| row.get::<_, Option<String>>(0),
        );

        match result {
            Ok(Some(json_str)) => {
                // Parse the JSON to extract capability strings
                if let Ok(json_value) = serde_json::from_str::<serde_json::Value>(&json_str) {
                    let mut capabilities = Vec::new();

                    // Extract filesystem capabilities
                    if let Some(fs_array) = json_value.get("filesystem").and_then(|v| v.as_array()) {
                        for fs_cap in fs_array {
                            if let Some(cap_type) = fs_cap.get("type").and_then(|v| v.as_str()) {
                                capabilities.push(format!("filesystem.{}", cap_type));
                            }
                        }
                    }

                    // Extract network capability
                    if let Some(network) = json_value.get("network") {
                        if let Some(net_str) = network.as_str() {
                            capabilities.push(format!("network.{}", net_str));
                        } else if network.is_object() {
                            // Handle AllowDomains case
                            capabilities.push("network.allow_domains".to_string());
                        }
                    }

                    // Extract process capability
                    if let Some(process) = json_value.get("process").and_then(|v| v.as_object()) {
                        if let Some(no_fork) = process.get("no_fork").and_then(|v| v.as_bool()) {
                            if !no_fork {
                                capabilities.push("process.exec".to_string());
                            }
                        }
                    }

                    Ok(capabilities)
                } else {
                    Ok(Vec::new())
                }
            }
            Ok(None) => Ok(Vec::new()),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(Vec::new()),
            Err(e) => Err(e),
        }
    }

    /// Test helper: Insert capability approval (available for integration tests)
    pub async fn insert_test_capability_approval(
        &self,
        tool_name: &str,
        capabilities_json: &str,
        approved_at: i64,
    ) -> SqliteResult<()> {
        let conn = self.conn.lock().await;
        conn.execute(
            "INSERT INTO capability_approvals
             (tool_name, capabilities_hash, capabilities_json, approved, approved_by, approval_scope, approved_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                tool_name,
                "test_hash",
                capabilities_json,
                true,
                "test_user",
                "session",
                approved_at
            ],
        )?;
        Ok(())
    }

    /// Test helper: Insert escalation (available for integration tests)
    pub async fn insert_test_escalation(
        &self,
        tool_name: &str,
        execution_id: &str,
        escalation_reason: &str,
        decided_at: i64,
    ) -> SqliteResult<()> {
        let conn = self.conn.lock().await;
        conn.execute(
            "INSERT INTO capability_escalations
             (tool_name, execution_id, escalation_reason, decided_at)
             VALUES (?1, ?2, ?3, ?4)",
            params![tool_name, execution_id, escalation_reason, decided_at],
        )?;
        Ok(())
    }

    /// Test helper: Insert tool execution (available for integration tests)
    pub async fn insert_test_execution(
        &self,
        tool_name: &str,
        execution_id: &str,
        parameters: &str,
        timestamp: i64,
    ) -> SqliteResult<()> {
        let conn = self.conn.lock().await;
        conn.execute(
            "INSERT INTO tool_executions
             (execution_id, tool_name, timestamp, parameters, escalation_triggered)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![execution_id, tool_name, timestamp, parameters, false],
        )?;
        Ok(())
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

    #[tokio::test]
    async fn test_get_tool_capabilities_empty() {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("test.db");

        let storage = ApprovalAuditStorage::new(&db_path).await.unwrap();

        // Get capabilities for non-existent tool
        let capabilities = storage.get_tool_capabilities("test_tool").await.unwrap();
        assert_eq!(capabilities.len(), 0);
    }

    #[tokio::test]
    async fn test_get_tool_capabilities_with_data() {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("test.db");

        let storage = ApprovalAuditStorage::new(&db_path).await.unwrap();

        // Insert test data with capabilities JSON
        let capabilities_json = r#"{
            "filesystem": [{"type": "read_write", "path": "/tmp"}],
            "network": "allow_all",
            "process": {"no_fork": false, "max_execution_time": 300, "max_memory_mb": 512},
            "environment": "restricted"
        }"#;

        storage
            .insert_test_capability_approval("test_tool", capabilities_json, 1234567890)
            .await
            .unwrap();

        // Get capabilities
        let capabilities = storage.get_tool_capabilities("test_tool").await.unwrap();
        assert!(!capabilities.is_empty());
        assert!(capabilities.contains(&"filesystem.read_write".to_string()));
        assert!(capabilities.contains(&"network.allow_all".to_string()));
        assert!(capabilities.contains(&"process.exec".to_string()));
    }

    #[tokio::test]
    async fn test_get_tool_capabilities_multiple_approvals() {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("test.db");

        let storage = ApprovalAuditStorage::new(&db_path).await.unwrap();

        // Insert older approval
        let old_capabilities_json = r#"{
            "filesystem": [{"type": "read_only", "path": "/tmp"}],
            "network": "deny",
            "process": {"no_fork": true, "max_execution_time": 300, "max_memory_mb": 512},
            "environment": "restricted"
        }"#;

        // Insert newer approval
        let new_capabilities_json = r#"{
            "filesystem": [{"type": "read_write", "path": "/tmp"}],
            "network": "allow_all",
            "process": {"no_fork": false, "max_execution_time": 300, "max_memory_mb": 512},
            "environment": "restricted"
        }"#;

        storage
            .insert_test_capability_approval("test_tool", old_capabilities_json, 1234567890)
            .await
            .unwrap();

        storage
            .insert_test_capability_approval("test_tool", new_capabilities_json, 1234567900)
            .await
            .unwrap();

        // Get capabilities - should return the most recent one
        let capabilities = storage.get_tool_capabilities("test_tool").await.unwrap();
        assert!(capabilities.contains(&"filesystem.read_write".to_string()));
        assert!(capabilities.contains(&"network.allow_all".to_string()));
        assert!(capabilities.contains(&"process.exec".to_string()));
    }
}
