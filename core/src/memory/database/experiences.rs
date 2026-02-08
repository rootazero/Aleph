//! CRUD operations for experience_replays table
//!
//! Provides database operations for experience management,
//! enabling L1.5 experience replay and skill evolution.

use crate::error::AlephError;
use crate::memory::cortex::{EvolutionStatus, Experience};
use crate::memory::database::VectorDatabase;
use rusqlite::{params, OptionalExtension};

impl VectorDatabase {
    // =========================================================================
    // Experience CRUD Operations
    // =========================================================================

    /// Insert a new experience
    pub async fn insert_experience(&self, exp: &Experience) -> Result<(), AlephError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());

        // Insert into experience_replays table
        conn.execute(
            r#"
            INSERT INTO experience_replays (
                id, pattern_hash, intent_vector, user_intent,
                environment_context_json, thought_trace_distilled,
                tool_sequence_json, parameter_mapping, logic_trace_json,
                success_score, token_efficiency, latency_ms, novelty_score,
                evolution_status, usage_count, success_count, last_success_rate,
                created_at, last_used_at, last_evaluated_at
            ) VALUES (
                ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10,
                ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20
            )
            "#,
            params![
                exp.id,
                exp.pattern_hash,
                exp.intent_vector.as_ref().map(|v| Self::serialize_embedding(v)),
                exp.user_intent,
                exp.environment_context_json,
                exp.thought_trace_distilled,
                exp.tool_sequence_json,
                exp.parameter_mapping,
                exp.logic_trace_json,
                exp.success_score,
                exp.token_efficiency,
                exp.latency_ms,
                exp.novelty_score,
                exp.evolution_status.to_string(),
                exp.usage_count,
                exp.success_count,
                exp.last_success_rate,
                exp.created_at,
                exp.last_used_at,
                exp.last_evaluated_at,
            ],
        )
        .map_err(|e| AlephError::config(format!("Failed to insert experience: {}", e)))?;

        // Insert into vector table if intent_vector exists
        if let Some(ref vector) = exp.intent_vector {
            let vector_blob = Self::serialize_embedding(vector);

            conn.execute(
                "INSERT INTO experiences_vec(rowid, embedding) VALUES ((SELECT rowid FROM experience_replays WHERE id = ?1), ?2)",
                params![exp.id, vector_blob],
            )
            .map_err(|e| AlephError::config(format!("Failed to insert vector: {}", e)))?;
        }

        Ok(())
    }

    /// Get experience by ID
    pub async fn get_experience(&self, id: &str) -> Result<Option<Experience>, AlephError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());

        let result = conn
            .query_row(
                r#"
                SELECT id, pattern_hash, intent_vector, user_intent,
                       environment_context_json, thought_trace_distilled,
                       tool_sequence_json, parameter_mapping, logic_trace_json,
                       success_score, token_efficiency, latency_ms, novelty_score,
                       evolution_status, usage_count, success_count, last_success_rate,
                       created_at, last_used_at, last_evaluated_at
                FROM experience_replays
                WHERE id = ?1
                "#,
                params![id],
                |row| {
                    let intent_vector_blob: Option<Vec<u8>> = row.get(2)?;
                    let intent_vector = intent_vector_blob
                        .map(|blob| Self::deserialize_embedding(&blob));

                    Ok(Experience {
                        id: row.get(0)?,
                        pattern_hash: row.get(1)?,
                        intent_vector,
                        user_intent: row.get(3)?,
                        environment_context_json: row.get(4)?,
                        thought_trace_distilled: row.get(5)?,
                        tool_sequence_json: row.get(6)?,
                        parameter_mapping: row.get(7)?,
                        logic_trace_json: row.get(8)?,
                        success_score: row.get(9)?,
                        token_efficiency: row.get(10)?,
                        latency_ms: row.get(11)?,
                        novelty_score: row.get(12)?,
                        evolution_status: EvolutionStatus::from_str(&row.get::<_, String>(13)?),
                        usage_count: row.get(14)?,
                        success_count: row.get(15)?,
                        last_success_rate: row.get(16)?,
                        created_at: row.get(17)?,
                        last_used_at: row.get(18)?,
                        last_evaluated_at: row.get(19)?,
                    })
                },
            )
            .optional()
            .map_err(|e| AlephError::config(format!("Failed to query experience: {}", e)))?;

        Ok(result)
    }

    /// Query experiences by evolution status
    pub async fn query_experiences_by_status(
        &self,
        status: EvolutionStatus,
        limit: u32,
    ) -> Result<Vec<Experience>, AlephError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());

        let mut stmt = conn
            .prepare(
                r#"
                SELECT id, pattern_hash, intent_vector, user_intent,
                       environment_context_json, thought_trace_distilled,
                       tool_sequence_json, parameter_mapping, logic_trace_json,
                       success_score, token_efficiency, latency_ms, novelty_score,
                       evolution_status, usage_count, success_count, last_success_rate,
                       created_at, last_used_at, last_evaluated_at
                FROM experience_replays
                WHERE evolution_status = ?1
                ORDER BY last_used_at DESC
                LIMIT ?2
                "#,
            )
            .map_err(|e| AlephError::config(format!("Failed to prepare query: {}", e)))?;

        let experiences = stmt
            .query_map(params![status.to_string(), limit], |row| {
                let intent_vector_blob: Option<Vec<u8>> = row.get(2)?;
                let intent_vector = intent_vector_blob
                    .map(|blob| Self::deserialize_embedding(&blob));

                Ok(Experience {
                    id: row.get(0)?,
                    pattern_hash: row.get(1)?,
                    intent_vector,
                    user_intent: row.get(3)?,
                    environment_context_json: row.get(4)?,
                    thought_trace_distilled: row.get(5)?,
                    tool_sequence_json: row.get(6)?,
                    parameter_mapping: row.get(7)?,
                    logic_trace_json: row.get(8)?,
                    success_score: row.get(9)?,
                    token_efficiency: row.get(10)?,
                    latency_ms: row.get(11)?,
                    novelty_score: row.get(12)?,
                    evolution_status: EvolutionStatus::from_str(&row.get::<_, String>(13)?),
                    usage_count: row.get(14)?,
                    success_count: row.get(15)?,
                    last_success_rate: row.get(16)?,
                    created_at: row.get(17)?,
                    last_used_at: row.get(18)?,
                    last_evaluated_at: row.get(19)?,
                })
            })
            .map_err(|e| AlephError::config(format!("Failed to query experiences: {}", e)))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| AlephError::config(format!("Failed to collect experiences: {}", e)))?;

        Ok(experiences)
    }

    /// Update experience evolution status
    pub async fn update_experience_status(
        &self,
        id: &str,
        status: EvolutionStatus,
    ) -> Result<(), AlephError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());

        conn.execute(
            "UPDATE experience_replays SET evolution_status = ?1 WHERE id = ?2",
            params![status.to_string(), id],
        )
        .map_err(|e| AlephError::config(format!("Failed to update status: {}", e)))?;

        Ok(())
    }

    /// Increment experience usage count and update success rate
    pub async fn increment_experience_usage(
        &self,
        id: &str,
        success: bool,
    ) -> Result<(), AlephError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());

        // Get current counts
        let (usage_count, success_count, evolution_status): (i64, i64, String) = conn
            .query_row(
                "SELECT usage_count, success_count, evolution_status FROM experience_replays WHERE id = ?1",
                params![id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .map_err(|e| AlephError::config(format!("Failed to query experience: {}", e)))?;

        let new_usage_count = usage_count + 1;
        let new_success_count = if success { success_count + 1 } else { success_count };
        let new_success_rate = new_success_count as f64 / new_usage_count as f64;

        // Update counts and success rate
        conn.execute(
            r#"
            UPDATE experience_replays
            SET usage_count = ?1,
                success_count = ?2,
                last_success_rate = ?3,
                last_used_at = ?4
            WHERE id = ?5
            "#,
            params![
                new_usage_count,
                new_success_count,
                new_success_rate,
                chrono::Utc::now().timestamp(),
                id
            ],
        )
        .map_err(|e| AlephError::config(format!("Failed to update usage: {}", e)))?;

        // Check if ready for hardening (usage_count > 20 && success_rate > 0.95 && status == verified)
        if new_usage_count > 20
            && new_success_rate > 0.95
            && evolution_status == EvolutionStatus::Verified.to_string()
        {
            conn.execute(
                "UPDATE experience_replays SET evolution_status = ?1 WHERE id = ?2",
                params!["ready_for_distillation", id],
            )
            .map_err(|e| AlephError::config(format!("Failed to mark for distillation: {}", e)))?;
        }

        Ok(())
    }

    /// Vector search for similar experiences
    pub async fn vector_search_experiences(
        &self,
        query_vector: &[f32],
        top_k: usize,
        min_score: f64,
        status_filter: Option<Vec<EvolutionStatus>>,
    ) -> Result<Vec<(Experience, f64)>, AlephError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());

        // Serialize query vector
        let query_blob = Self::serialize_embedding(query_vector);

        // Build status filter clause
        let status_clause = if let Some(ref statuses) = status_filter {
            let status_strs: Vec<String> = statuses.iter().map(|s| format!("'{}'", s)).collect();
            format!("AND e.evolution_status IN ({})", status_strs.join(","))
        } else {
            String::new()
        };

        // Vector search query
        let query = format!(
            r#"
            SELECT e.id, e.pattern_hash, e.intent_vector, e.user_intent,
                   e.environment_context_json, e.thought_trace_distilled,
                   e.tool_sequence_json, e.parameter_mapping, e.logic_trace_json,
                   e.success_score, e.token_efficiency, e.latency_ms, e.novelty_score,
                   e.evolution_status, e.usage_count, e.success_count, e.last_success_rate,
                   e.created_at, e.last_used_at, e.last_evaluated_at,
                   vec_distance_L2(v.embedding, ?1) as distance
            FROM experience_replays e
            JOIN experiences_vec v ON v.rowid = e.rowid
            WHERE 1=1 {}
            ORDER BY distance ASC
            LIMIT ?2
            "#,
            status_clause
        );

        let mut stmt = conn
            .prepare(&query)
            .map_err(|e| AlephError::config(format!("Failed to prepare vector search: {}", e)))?;

        let results = stmt
            .query_map(params![query_blob, top_k], |row| {
                let intent_vector_blob: Option<Vec<u8>> = row.get(2)?;
                let intent_vector = intent_vector_blob
                    .map(|blob| Self::deserialize_embedding(&blob));

                let distance: f64 = row.get(19)?;
                let similarity = 1.0 / (1.0 + distance);

                Ok((
                    Experience {
                        id: row.get(0)?,
                        pattern_hash: row.get(1)?,
                        intent_vector,
                        user_intent: row.get(3)?,
                        environment_context_json: row.get(4)?,
                        thought_trace_distilled: row.get(5)?,
                        tool_sequence_json: row.get(6)?,
                        parameter_mapping: row.get(7)?,
                        logic_trace_json: row.get(8)?,
                        success_score: row.get(9)?,
                        token_efficiency: row.get(10)?,
                        latency_ms: row.get(11)?,
                        novelty_score: row.get(12)?,
                        evolution_status: EvolutionStatus::from_str(&row.get::<_, String>(13)?),
                        usage_count: row.get(14)?,
                        success_count: row.get(15)?,
                        last_success_rate: row.get(16)?,
                        created_at: row.get(17)?,
                        last_used_at: row.get(18)?,
                        last_evaluated_at: row.get(19)?,
                    },
                    similarity,
                ))
            })
            .map_err(|e| AlephError::config(format!("Failed to execute vector search: {}", e)))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| AlephError::config(format!("Failed to collect results: {}", e)))?;

        // Filter by min_score
        let filtered: Vec<(Experience, f64)> = results
            .into_iter()
            .filter(|(_, score)| *score >= min_score)
            .collect();

        Ok(filtered)
    }

    /// Delete experience by ID
    pub async fn delete_experience(&self, id: &str) -> Result<(), AlephError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());

        // Delete from vector table first
        conn.execute(
            "DELETE FROM experiences_vec WHERE rowid = (SELECT rowid FROM experience_replays WHERE id = ?1)",
            params![id],
        )
        .map_err(|e| AlephError::config(format!("Failed to delete vector: {}", e)))?;

        // Delete from main table
        conn.execute(
            "DELETE FROM experience_replays WHERE id = ?1",
            params![id],
        )
        .map_err(|e| AlephError::config(format!("Failed to delete experience: {}", e)))?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::cortex::ExperienceBuilder;
    use std::path::PathBuf;

    #[tokio::test]
    async fn test_insert_and_get_experience() {
        let temp_dir = tempfile::tempdir().unwrap();
        let db_path = temp_dir.path().join("test.db");
        let db = VectorDatabase::new(db_path).unwrap();

        let exp = ExperienceBuilder::new(
            "test-1".to_string(),
            "test intent".to_string(),
            r#"{"tools": []}"#.to_string(),
        )
        .pattern_hash("hash123".to_string())
        .success_score(0.95)
        .build();

        // Insert
        db.insert_experience(&exp).await.unwrap();

        // Get
        let retrieved = db.get_experience("test-1").await.unwrap();
        assert!(retrieved.is_some());
        let retrieved = retrieved.unwrap();
        assert_eq!(retrieved.id, "test-1");
        assert_eq!(retrieved.pattern_hash, "hash123");
        assert_eq!(retrieved.success_score, 0.95);
    }

    #[tokio::test]
    async fn test_update_status() {
        let temp_dir = tempfile::tempdir().unwrap();
        let db_path = temp_dir.path().join("test.db");
        let db = VectorDatabase::new(db_path).unwrap();

        let exp = ExperienceBuilder::new(
            "test-2".to_string(),
            "test intent".to_string(),
            r#"{"tools": []}"#.to_string(),
        )
        .pattern_hash("hash456".to_string())
        .success_score(0.90)
        .build();

        db.insert_experience(&exp).await.unwrap();

        // Update status
        db.update_experience_status("test-2", EvolutionStatus::Verified)
            .await
            .unwrap();

        // Verify
        let retrieved = db.get_experience("test-2").await.unwrap().unwrap();
        assert_eq!(retrieved.evolution_status, EvolutionStatus::Verified);
    }

    #[tokio::test]
    async fn test_increment_usage() {
        let temp_dir = tempfile::tempdir().unwrap();
        let db_path = temp_dir.path().join("test.db");
        let db = VectorDatabase::new(db_path).unwrap();

        let exp = ExperienceBuilder::new(
            "test-3".to_string(),
            "test intent".to_string(),
            r#"{"tools": []}"#.to_string(),
        )
        .pattern_hash("hash789".to_string())
        .success_score(0.85)
        .build();

        db.insert_experience(&exp).await.unwrap();

        // Increment usage (success)
        db.increment_experience_usage("test-3", true).await.unwrap();

        // Verify
        let retrieved = db.get_experience("test-3").await.unwrap().unwrap();
        assert_eq!(retrieved.usage_count, 2);
        assert_eq!(retrieved.success_count, 1);
        assert_eq!(retrieved.last_success_rate, Some(0.5));
    }
}
