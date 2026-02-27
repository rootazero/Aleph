//! CRUD operations for poe_contracts table

use crate::error::AlephError;
use super::StateDatabase;
use rusqlite::params;
use rusqlite::OptionalExtension;

/// Row from the poe_contracts table.
#[derive(Debug, Clone)]
pub struct PoeContractRow {
    pub id: String,
    pub task_id: String,
    pub instruction: String,
    pub manifest_json: String,
    pub context_json: Option<String>,
    pub status: String,
    pub created_at: i64,
    pub signed_at: Option<i64>,
    pub expires_at: Option<i64>,
}

impl StateDatabase {
    /// Insert a new pending contract.
    pub async fn insert_poe_contract(
        &self,
        id: &str,
        task_id: &str,
        instruction: &str,
        manifest_json: &str,
        context_json: Option<&str>,
        created_at: i64,
        expires_at: Option<i64>,
    ) -> Result<(), AlephError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        conn.execute(
            r#"
            INSERT INTO poe_contracts (id, task_id, instruction, manifest_json, context_json, status, created_at, expires_at)
            VALUES (?1, ?2, ?3, ?4, ?5, 'pending', ?6, ?7)
            "#,
            params![id, task_id, instruction, manifest_json, context_json, created_at, expires_at],
        )
        .map_err(|e| AlephError::other(format!("Failed to insert contract: {e}")))?;
        Ok(())
    }

    /// Get a contract by ID.
    pub async fn get_poe_contract(
        &self,
        id: &str,
    ) -> Result<Option<PoeContractRow>, AlephError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let result = conn
            .query_row(
                r#"
                SELECT id, task_id, instruction, manifest_json, context_json, status, created_at, signed_at, expires_at
                FROM poe_contracts
                WHERE id = ?1
                "#,
                params![id],
                |row| {
                    Ok(PoeContractRow {
                        id: row.get(0)?,
                        task_id: row.get(1)?,
                        instruction: row.get(2)?,
                        manifest_json: row.get(3)?,
                        context_json: row.get(4)?,
                        status: row.get(5)?,
                        created_at: row.get(6)?,
                        signed_at: row.get(7)?,
                        expires_at: row.get(8)?,
                    })
                },
            )
            .optional()
            .map_err(|e| AlephError::other(format!("Failed to get contract: {e}")))?;
        Ok(result)
    }

    /// Update contract status.
    pub async fn update_poe_contract_status(
        &self,
        id: &str,
        status: &str,
        signed_at: Option<i64>,
    ) -> Result<bool, AlephError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let rows = conn
            .execute(
                "UPDATE poe_contracts SET status = ?1, signed_at = ?2 WHERE id = ?3",
                params![status, signed_at, id],
            )
            .map_err(|e| AlephError::other(format!("Failed to update contract status: {e}")))?;
        Ok(rows > 0)
    }

    /// Delete a contract by ID. Returns true if a row was deleted.
    pub async fn delete_poe_contract(
        &self,
        id: &str,
    ) -> Result<bool, AlephError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let rows = conn
            .execute(
                "DELETE FROM poe_contracts WHERE id = ?1",
                params![id],
            )
            .map_err(|e| AlephError::other(format!("Failed to delete contract: {e}")))?;
        Ok(rows > 0)
    }

    /// List all pending contracts.
    pub async fn list_pending_poe_contracts(&self) -> Result<Vec<PoeContractRow>, AlephError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let mut stmt = conn
            .prepare(
                r#"
                SELECT id, task_id, instruction, manifest_json, context_json, status, created_at, signed_at, expires_at
                FROM poe_contracts
                WHERE status = 'pending'
                ORDER BY created_at ASC
                "#,
            )
            .map_err(|e| AlephError::other(format!("Failed to prepare statement: {e}")))?;

        let rows = stmt
            .query_map([], |row| {
                Ok(PoeContractRow {
                    id: row.get(0)?,
                    task_id: row.get(1)?,
                    instruction: row.get(2)?,
                    manifest_json: row.get(3)?,
                    context_json: row.get(4)?,
                    status: row.get(5)?,
                    created_at: row.get(6)?,
                    signed_at: row.get(7)?,
                    expires_at: row.get(8)?,
                })
            })
            .map_err(|e| AlephError::other(format!("Failed to list contracts: {e}")))?;

        let mut results = Vec::new();
        for row in rows {
            results.push(row.map_err(|e| AlephError::other(format!("Row error: {e}")))?);
        }
        Ok(results)
    }

    /// Delete expired pending contracts. Returns count deleted.
    pub async fn delete_expired_poe_contracts(
        &self,
        now: i64,
    ) -> Result<usize, AlephError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let rows = conn
            .execute(
                "DELETE FROM poe_contracts WHERE status = 'pending' AND expires_at IS NOT NULL AND expires_at < ?1",
                params![now],
            )
            .map_err(|e| AlephError::other(format!("Failed to delete expired contracts: {e}")))?;
        Ok(rows)
    }

    /// Count pending contracts.
    pub async fn count_pending_poe_contracts(&self) -> Result<usize, AlephError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM poe_contracts WHERE status = 'pending'",
                [],
                |row| row.get(0),
            )
            .map_err(|e| AlephError::other(format!("Failed to count contracts: {e}")))?;
        Ok(count as usize)
    }
}

#[cfg(test)]
mod tests {
    use crate::resilience::database::StateDatabase;

    #[tokio::test]
    async fn test_insert_and_get_contract() {
        let db = StateDatabase::in_memory().unwrap();
        db.insert_poe_contract(
            "c1", "task-1", "Do something", r#"{"task_id":"task-1","objective":"test"}"#,
            None, 1000, None,
        ).await.unwrap();

        let row = db.get_poe_contract("c1").await.unwrap().unwrap();
        assert_eq!(row.id, "c1");
        assert_eq!(row.task_id, "task-1");
        assert_eq!(row.status, "pending");
    }

    #[tokio::test]
    async fn test_update_status() {
        let db = StateDatabase::in_memory().unwrap();
        db.insert_poe_contract(
            "c1", "task-1", "test", "{}", None, 1000, None,
        ).await.unwrap();

        let updated = db.update_poe_contract_status("c1", "signed", Some(2000)).await.unwrap();
        assert!(updated);

        let row = db.get_poe_contract("c1").await.unwrap().unwrap();
        assert_eq!(row.status, "signed");
        assert_eq!(row.signed_at, Some(2000));
    }

    #[tokio::test]
    async fn test_delete_contract() {
        let db = StateDatabase::in_memory().unwrap();
        db.insert_poe_contract(
            "c1", "task-1", "test", "{}", None, 1000, None,
        ).await.unwrap();

        assert!(db.delete_poe_contract("c1").await.unwrap());
        assert!(!db.delete_poe_contract("c1").await.unwrap()); // Already deleted
        assert!(db.get_poe_contract("c1").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_list_pending() {
        let db = StateDatabase::in_memory().unwrap();
        db.insert_poe_contract("c1", "t1", "i1", "{}", None, 1000, None).await.unwrap();
        db.insert_poe_contract("c2", "t2", "i2", "{}", None, 2000, None).await.unwrap();

        // Sign one
        db.update_poe_contract_status("c1", "signed", Some(3000)).await.unwrap();

        let pending = db.list_pending_poe_contracts().await.unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].id, "c2");
    }

    #[tokio::test]
    async fn test_delete_expired() {
        let db = StateDatabase::in_memory().unwrap();
        db.insert_poe_contract("c1", "t1", "i1", "{}", None, 1000, Some(5000)).await.unwrap();
        db.insert_poe_contract("c2", "t2", "i2", "{}", None, 1000, Some(15000)).await.unwrap();
        db.insert_poe_contract("c3", "t3", "i3", "{}", None, 1000, None).await.unwrap(); // No expiry

        let deleted = db.delete_expired_poe_contracts(10000).await.unwrap();
        assert_eq!(deleted, 1); // Only c1 expired

        let pending = db.list_pending_poe_contracts().await.unwrap();
        assert_eq!(pending.len(), 2); // c2 and c3 remain
    }

    #[tokio::test]
    async fn test_count_pending() {
        let db = StateDatabase::in_memory().unwrap();
        assert_eq!(db.count_pending_poe_contracts().await.unwrap(), 0);

        db.insert_poe_contract("c1", "t1", "i1", "{}", None, 1000, None).await.unwrap();
        db.insert_poe_contract("c2", "t2", "i2", "{}", None, 2000, None).await.unwrap();
        assert_eq!(db.count_pending_poe_contracts().await.unwrap(), 2);
    }
}
