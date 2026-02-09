use rusqlite::{Connection, Result as SqliteResult};
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

        Ok(Self {
            conn: Mutex::new(conn),
        })
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
