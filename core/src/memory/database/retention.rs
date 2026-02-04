/// Retention policy operations
///
/// Delete old memories and facts based on retention policies.
use crate::error::AlephError;
use rusqlite::params;

use super::core::VectorDatabase;

impl VectorDatabase {
    /// Delete memories older than timestamp (for retention policy)
    pub async fn delete_older_than(&self, cutoff_timestamp: i64) -> Result<u64, AlephError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let rows_affected = conn
            .execute(
                "DELETE FROM memories WHERE timestamp < ?1",
                params![cutoff_timestamp],
            )
            .map_err(|e| AlephError::config(format!("Failed to delete old memories: {}", e)))?;

        Ok(rows_affected as u64)
    }

    /// Delete old facts based on retention policy
    pub async fn delete_old_facts(&self, cutoff_timestamp: i64) -> Result<u64, AlephError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let rows_affected = conn
            .execute(
                "DELETE FROM memory_facts WHERE created_at < ?1",
                params![cutoff_timestamp],
            )
            .map_err(|e| AlephError::config(format!("Failed to delete old facts: {}", e)))?;

        Ok(rows_affected as u64)
    }
}
