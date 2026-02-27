//! CRUD operations for poe_trust_scores table

use crate::error::AlephError;
use super::StateDatabase;
use rusqlite::params;
use rusqlite::OptionalExtension;

/// Row from the poe_trust_scores table.
#[derive(Debug, Clone)]
pub struct TrustScoreRow {
    pub pattern_id: String,
    pub total_executions: u32,
    pub successful_executions: u32,
    pub trust_score: f32,
    pub last_updated: i64,
}

impl StateDatabase {
    /// Upsert trust score for a pattern. Returns the new trust score.
    ///
    /// Trust score formula: successful_executions / total_executions
    /// (simple success rate, can be enhanced with time decay later)
    pub async fn upsert_trust_score(
        &self,
        pattern_id: &str,
        success: bool,
    ) -> Result<f32, AlephError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let now = chrono::Utc::now().timestamp_millis();
        let success_inc: i64 = if success { 1 } else { 0 };

        conn.execute(
            r#"
            INSERT INTO poe_trust_scores (pattern_id, total_executions, successful_executions, trust_score, last_updated)
            VALUES (?1, 1, ?2, ?3, ?4)
            ON CONFLICT(pattern_id) DO UPDATE SET
                total_executions = total_executions + 1,
                successful_executions = successful_executions + ?2,
                trust_score = CAST((successful_executions + ?2) AS REAL) / CAST((total_executions + 1) AS REAL),
                last_updated = ?4
            "#,
            params![pattern_id, success_inc, success_inc as f64, now],
        )
        .map_err(|e| AlephError::other(format!("Failed to upsert trust score: {e}")))?;

        // Read back the updated score
        let score: f64 = conn
            .query_row(
                "SELECT trust_score FROM poe_trust_scores WHERE pattern_id = ?1",
                params![pattern_id],
                |row| row.get(0),
            )
            .map_err(|e| AlephError::other(format!("Failed to read trust score: {e}")))?;

        Ok(score as f32)
    }

    /// Get trust score for a pattern.
    pub async fn get_trust_score(
        &self,
        pattern_id: &str,
    ) -> Result<Option<TrustScoreRow>, AlephError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let result = conn
            .query_row(
                r#"
                SELECT pattern_id, total_executions, successful_executions, trust_score, last_updated
                FROM poe_trust_scores
                WHERE pattern_id = ?1
                "#,
                params![pattern_id],
                |row| {
                    Ok(TrustScoreRow {
                        pattern_id: row.get(0)?,
                        total_executions: row.get::<_, i64>(1)? as u32,
                        successful_executions: row.get::<_, i64>(2)? as u32,
                        trust_score: row.get::<_, f64>(3)? as f32,
                        last_updated: row.get(4)?,
                    })
                },
            )
            .optional()
            .map_err(|e| AlephError::other(format!("Failed to get trust score: {e}")))?;

        Ok(result)
    }

    /// Get all trust scores.
    pub async fn list_trust_scores(&self) -> Result<Vec<TrustScoreRow>, AlephError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let mut stmt = conn
            .prepare(
                r#"
                SELECT pattern_id, total_executions, successful_executions, trust_score, last_updated
                FROM poe_trust_scores
                ORDER BY trust_score DESC
                "#,
            )
            .map_err(|e| AlephError::other(format!("Failed to prepare statement: {e}")))?;

        let rows = stmt
            .query_map([], |row| {
                Ok(TrustScoreRow {
                    pattern_id: row.get(0)?,
                    total_executions: row.get::<_, i64>(1)? as u32,
                    successful_executions: row.get::<_, i64>(2)? as u32,
                    trust_score: row.get::<_, f64>(3)? as f32,
                    last_updated: row.get(4)?,
                })
            })
            .map_err(|e| AlephError::other(format!("Failed to list trust scores: {e}")))?;

        let mut results = Vec::new();
        for row in rows {
            results.push(row.map_err(|e| AlephError::other(format!("Row error: {e}")))?);
        }
        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use crate::resilience::database::StateDatabase;

    #[tokio::test]
    async fn test_upsert_trust_score_success() {
        let db = StateDatabase::in_memory().unwrap();
        let score = db.upsert_trust_score("pattern-1", true).await.unwrap();
        assert_eq!(score, 1.0); // 1/1

        let score = db.upsert_trust_score("pattern-1", true).await.unwrap();
        assert_eq!(score, 1.0); // 2/2

        let score = db.upsert_trust_score("pattern-1", false).await.unwrap();
        assert!((score - 2.0 / 3.0).abs() < 0.01); // 2/3
    }

    #[tokio::test]
    async fn test_get_trust_score() {
        let db = StateDatabase::in_memory().unwrap();
        assert!(db.get_trust_score("nonexistent").await.unwrap().is_none());

        db.upsert_trust_score("pattern-1", true).await.unwrap();
        let row = db.get_trust_score("pattern-1").await.unwrap().unwrap();
        assert_eq!(row.pattern_id, "pattern-1");
        assert_eq!(row.total_executions, 1);
        assert_eq!(row.successful_executions, 1);
    }

    #[tokio::test]
    async fn test_list_trust_scores() {
        let db = StateDatabase::in_memory().unwrap();
        db.upsert_trust_score("a", true).await.unwrap();
        db.upsert_trust_score("b", false).await.unwrap();

        let scores = db.list_trust_scores().await.unwrap();
        assert_eq!(scores.len(), 2);
        // Should be ordered by trust_score DESC
        assert!(scores[0].trust_score >= scores[1].trust_score);
    }
}
