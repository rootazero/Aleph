//! Statistics and utility operations for memory facts

use crate::error::AetherError;
use crate::memory::context::FactStats;
use crate::memory::database::core::VectorDatabase;

impl VectorDatabase {
    /// Get fact statistics
    pub async fn get_fact_stats(&self) -> Result<FactStats, AetherError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());

        // Total facts
        let total_facts: u64 = conn
            .query_row("SELECT COUNT(*) FROM memory_facts", [], |row| row.get(0))
            .unwrap_or(0);

        // Valid facts
        let valid_facts: u64 = conn
            .query_row(
                "SELECT COUNT(*) FROM memory_facts WHERE is_valid = 1",
                [],
                |row| row.get(0),
            )
            .unwrap_or(0);

        // Facts by type
        let mut facts_by_type = std::collections::HashMap::new();
        let mut stmt = conn
            .prepare(
                "SELECT fact_type, COUNT(*) FROM memory_facts WHERE is_valid = 1 GROUP BY fact_type",
            )
            .map_err(|e| AetherError::config(format!("Failed to prepare query: {}", e)))?;

        let rows = stmt
            .query_map([], |row| {
                let fact_type: String = row.get(0)?;
                let count: u64 = row.get(1)?;
                Ok((fact_type, count))
            })
            .map_err(|e| AetherError::config(format!("Failed to query fact types: {}", e)))?;

        for (fact_type, count) in rows.flatten() {
            facts_by_type.insert(fact_type, count);
        }

        // Timestamps
        let oldest_fact_timestamp: Option<i64> = conn
            .query_row(
                "SELECT MIN(created_at) FROM memory_facts WHERE is_valid = 1",
                [],
                |row| row.get(0),
            )
            .ok();

        let newest_fact_timestamp: Option<i64> = conn
            .query_row(
                "SELECT MAX(created_at) FROM memory_facts WHERE is_valid = 1",
                [],
                |row| row.get(0),
            )
            .ok();

        Ok(FactStats {
            total_facts,
            valid_facts,
            facts_by_type,
            oldest_fact_timestamp,
            newest_fact_timestamp,
        })
    }

    /// Clear all facts (for testing or reset)
    pub async fn clear_facts(&self) -> Result<u64, AetherError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let rows_affected = conn
            .execute("DELETE FROM memory_facts", [])
            .map_err(|e| AetherError::config(format!("Failed to clear facts: {}", e)))?;

        Ok(rows_affected as u64)
    }
}
