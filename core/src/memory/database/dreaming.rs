//! DreamDaemon persistence helpers.

use crate::error::AlephError;
use crate::memory::context::{ContextAnchor, FactType, MemoryEntry};
use crate::memory::database::core::VectorDatabase;
use crate::memory::decay::{DecayConfig, MemoryStrength};
use crate::memory::dreaming::{DailyInsight, DreamStatus, MemoryDecayReport};
use rusqlite::{params, OptionalExtension};

impl VectorDatabase {
    /// Fetch memories since a timestamp (descending by timestamp).
    pub async fn get_memories_since(
        &self,
        since_timestamp: i64,
        limit: u32,
    ) -> Result<Vec<MemoryEntry>, AlephError> {
        if limit == 0 {
            return Ok(Vec::new());
        }

        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let mut stmt = conn
            .prepare(
                r#"
            SELECT id, app_bundle_id, window_title, user_input, ai_output, timestamp, topic_id
            FROM memories
            WHERE timestamp >= ?1
            ORDER BY timestamp DESC
            LIMIT ?2
            "#,
            )
            .map_err(|e| AlephError::config(format!("Failed to prepare memory query: {}", e)))?;

        let memories = stmt
            .query_map(params![since_timestamp, limit], |row| {
                let id: String = row.get(0)?;
                let app_id: String = row.get(1)?;
                let window: String = row.get(2)?;
                let user_input: String = row.get(3)?;
                let ai_output: String = row.get(4)?;
                let timestamp: i64 = row.get(5)?;
                let topic_id: String = row.get(6)?;

                Ok(MemoryEntry::new(
                    id,
                    ContextAnchor {
                        app_bundle_id: app_id,
                        window_title: window,
                        timestamp,
                        topic_id,
                    },
                    user_input,
                    ai_output,
                ))
            })
            .map_err(|e| AlephError::config(format!("Failed to query memories: {}", e)))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| AlephError::config(format!("Failed to parse memories: {}", e)))?;

        Ok(memories)
    }

    /// Insert or replace a daily insight entry.
    pub async fn upsert_daily_insight(&self, insight: DailyInsight) -> Result<(), AlephError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        conn.execute(
            r#"
            INSERT OR REPLACE INTO daily_insights (date, content, source_memory_count, created_at)
            VALUES (?1, ?2, ?3, ?4)
            "#,
            params![
                insight.date,
                insight.content,
                insight.source_memory_count,
                insight.created_at,
            ],
        )
        .map_err(|e| AlephError::config(format!("Failed to store daily insight: {}", e)))?;

        Ok(())
    }

    /// Retrieve a daily insight by date or return the latest one when date is None.
    pub async fn get_daily_insight(
        &self,
        date: Option<&str>,
    ) -> Result<Option<DailyInsight>, AlephError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());

        let row = if let Some(date) = date {
            conn.query_row(
                "SELECT date, content, source_memory_count, created_at FROM daily_insights WHERE date = ?1",
                params![date],
                |row| {
                    Ok(DailyInsight {
                        date: row.get(0)?,
                        content: row.get(1)?,
                        source_memory_count: row.get::<_, i64>(2)? as u32,
                        created_at: row.get(3)?,
                    })
                },
            )
            .optional()
            .map_err(|e| AlephError::config(format!("Failed to query daily insight: {}", e)))?
        } else {
            conn.query_row(
                "SELECT date, content, source_memory_count, created_at FROM daily_insights ORDER BY date DESC LIMIT 1",
                [],
                |row| {
                    Ok(DailyInsight {
                        date: row.get(0)?,
                        content: row.get(1)?,
                        source_memory_count: row.get::<_, i64>(2)? as u32,
                        created_at: row.get(3)?,
                    })
                },
            )
            .optional()
            .map_err(|e| AlephError::config(format!("Failed to query daily insight: {}", e)))?
        };

        Ok(row)
    }

    /// Retrieve DreamDaemon status.
    pub async fn get_dream_status(&self) -> Result<DreamStatus, AlephError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());

        let row: Option<(Option<i64>, Option<String>, Option<i64>)> = conn
            .query_row(
                "SELECT last_run_at, last_status, last_duration_ms FROM dream_status WHERE id = 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()
            .map_err(|e| AlephError::config(format!("Failed to query dream status: {}", e)))?;

        if let Some((last_run_at, last_status, last_duration_ms)) = row {
            Ok(DreamStatus {
                last_run_at,
                last_status,
                last_duration_ms: last_duration_ms.map(|v| v as u64),
            })
        } else {
            Ok(DreamStatus::default())
        }
    }

    /// Update DreamDaemon status row (singleton).
    pub async fn set_dream_status(&self, status: DreamStatus) -> Result<(), AlephError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let duration_ms: Option<i64> = status.last_duration_ms.map(|v| v as i64);

        conn.execute(
            r#"
            INSERT OR REPLACE INTO dream_status (id, last_run_at, last_status, last_duration_ms)
            VALUES (1, ?1, ?2, ?3)
            "#,
            params![status.last_run_at, status.last_status, duration_ms],
        )
        .map_err(|e| AlephError::config(format!("Failed to update dream status: {}", e)))?;

        Ok(())
    }

    /// Apply decay to memory facts and prune below threshold.
    pub async fn apply_fact_decay(
        &self,
        config: &DecayConfig,
    ) -> Result<MemoryDecayReport, AlephError> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;

        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let mut stmt = conn
            .prepare(
                r#"
                SELECT id, fact_type, created_at, updated_at, confidence
                FROM memory_facts
                WHERE is_valid = 1
                "#,
            )
            .map_err(|e| AlephError::config(format!("Failed to prepare fact decay query: {}", e)))?;

        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, f32>(4)?,
                ))
            })
            .map_err(|e| AlephError::config(format!("Failed to query facts: {}", e)))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| AlephError::config(format!("Failed to read facts: {}", e)))?;

        let mut report = MemoryDecayReport::default();

        for (fact_id, fact_type_str, created_at, updated_at, confidence) in rows {
            let fact_type = FactType::from_str_or_other(&fact_type_str);
            if config.is_protected(&fact_type) {
                continue;
            }

            let strength = MemoryStrength {
                access_count: 0,
                last_accessed: updated_at,
                creation_time: created_at,
            };

            let decay_strength = strength.calculate_strength(config, now);
            let new_confidence = (confidence * decay_strength).clamp(0.0, 1.0);

            if new_confidence < config.min_strength {
                conn.execute(
                    r#"
                    UPDATE memory_facts
                    SET is_valid = 0, invalidation_reason = 'decay_prune',
                        updated_at = ?2, confidence = ?3
                    WHERE id = ?1
                    "#,
                    params![fact_id, now, new_confidence],
                )
                .map_err(|e| {
                    AlephError::config(format!("Failed to prune decayed fact: {}", e))
                })?;
                report.pruned_facts += 1;
            } else if (new_confidence - confidence).abs() > 0.0001 {
                conn.execute(
                    "UPDATE memory_facts SET confidence = ?2 WHERE id = ?1",
                    params![fact_id, new_confidence],
                )
                .map_err(|e| {
                    AlephError::config(format!("Failed to update decayed fact: {}", e))
                })?;
                report.updated_facts += 1;
            }
        }

        Ok(report)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_daily_insight_roundtrip() {
        let temp_dir = tempdir().unwrap();
        let db_path = temp_dir.path().join("dreaming.db");
        let db = VectorDatabase::new(db_path).unwrap();

        let insight1 = DailyInsight {
            date: "2026-02-01".to_string(),
            content: "Insight A".to_string(),
            source_memory_count: 3,
            created_at: 100,
        };
        let insight2 = DailyInsight {
            date: "2026-02-02".to_string(),
            content: "Insight B".to_string(),
            source_memory_count: 5,
            created_at: 200,
        };

        db.upsert_daily_insight(insight1.clone()).await.unwrap();
        db.upsert_daily_insight(insight2.clone()).await.unwrap();

        let latest = db.get_daily_insight(None).await.unwrap().unwrap();
        assert_eq!(latest.date, "2026-02-02");

        let by_date = db
            .get_daily_insight(Some("2026-02-01"))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(by_date.content, "Insight A");
    }
}
