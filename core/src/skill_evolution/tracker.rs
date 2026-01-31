//! EvolutionTracker - logs skill executions and maintains metrics.
//!
//! Uses SQLite for persistence with similar schema to memory module.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, RwLock};

use rusqlite::{params, Connection, OptionalExtension};
use tracing::{debug, info};

use crate::error::{AetherError, Result};

use super::types::{ExecutionStatus, SkillExecution, SkillMetrics, SolidificationConfig};

/// SQL schema for skill evolution tables
const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS skill_executions (
    id TEXT PRIMARY KEY,
    skill_id TEXT NOT NULL,
    session_id TEXT NOT NULL,
    invoked_at INTEGER NOT NULL,
    duration_ms INTEGER NOT NULL,
    status TEXT NOT NULL,
    satisfaction REAL,
    context TEXT NOT NULL,
    input_summary TEXT NOT NULL,
    output_length INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_executions_skill ON skill_executions(skill_id);
CREATE INDEX IF NOT EXISTS idx_executions_time ON skill_executions(invoked_at);

CREATE TABLE IF NOT EXISTS skill_metrics (
    skill_id TEXT PRIMARY KEY,
    total_executions INTEGER NOT NULL,
    successful_executions INTEGER NOT NULL,
    avg_duration_ms REAL NOT NULL,
    avg_satisfaction REAL,
    failure_rate REAL NOT NULL,
    last_used INTEGER NOT NULL,
    first_used INTEGER NOT NULL,
    context_frequency TEXT NOT NULL
);
"#;

/// Tracker for skill executions and metrics
pub struct EvolutionTracker {
    conn: Arc<RwLock<Connection>>,
    /// In-memory cache for hot metrics
    metrics_cache: Arc<RwLock<HashMap<String, SkillMetrics>>>,
}

impl EvolutionTracker {
    /// Create a new tracker with database at the given path
    pub fn new(db_path: impl AsRef<Path>) -> Result<Self> {
        let conn = Connection::open(db_path.as_ref()).map_err(|e| AetherError::ConfigError {
            message: format!("Failed to open evolution database: {}", e),
            suggestion: Some("Check database path permissions".to_string()),
        })?;

        conn.execute_batch(SCHEMA)
            .map_err(|e| AetherError::ConfigError {
                message: format!("Failed to create schema: {}", e),
                suggestion: None,
            })?;

        info!(path = %db_path.as_ref().display(), "Evolution tracker initialized");

        Ok(Self {
            conn: Arc::new(RwLock::new(conn)),
            metrics_cache: Arc::new(RwLock::new(HashMap::new())),
        })
    }

    /// Create an in-memory tracker (for testing)
    pub fn in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory().map_err(|e| AetherError::ConfigError {
            message: format!("Failed to create in-memory database: {}", e),
            suggestion: None,
        })?;

        conn.execute_batch(SCHEMA)
            .map_err(|e| AetherError::ConfigError {
                message: format!("Failed to create schema: {}", e),
                suggestion: None,
            })?;

        Ok(Self {
            conn: Arc::new(RwLock::new(conn)),
            metrics_cache: Arc::new(RwLock::new(HashMap::new())),
        })
    }

    /// Log a skill execution
    pub fn log_execution(&self, execution: &SkillExecution) -> Result<()> {
        let conn = self.conn.write().map_err(|_| AetherError::Other {
            message: "Failed to acquire database lock".to_string(),
            suggestion: None,
        })?;

        let status_str = match execution.status {
            ExecutionStatus::Success => "success",
            ExecutionStatus::PartialSuccess => "partial_success",
            ExecutionStatus::Failed => "failed",
            ExecutionStatus::Error => "error",
        };

        conn.execute(
            "INSERT INTO skill_executions (id, skill_id, session_id, invoked_at, duration_ms, status, satisfaction, context, input_summary, output_length)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                execution.id,
                execution.skill_id,
                execution.session_id,
                execution.invoked_at,
                execution.duration_ms,
                status_str,
                execution.satisfaction,
                execution.context,
                execution.input_summary,
                execution.output_length,
            ],
        ).map_err(|e| AetherError::ConfigError {
            message: format!("Failed to insert execution: {}", e),
            suggestion: None,
        })?;

        debug!(skill_id = %execution.skill_id, status = %status_str, "Logged execution");

        // Update metrics
        drop(conn);
        self.update_metrics(&execution.skill_id)?;

        Ok(())
    }

    /// Get metrics for a skill
    pub fn get_metrics(&self, skill_id: &str) -> Result<Option<SkillMetrics>> {
        // Check cache first
        {
            let cache = self.metrics_cache.read().map_err(|_| AetherError::Other {
                message: "Failed to acquire cache lock".to_string(),
                suggestion: None,
            })?;
            if let Some(metrics) = cache.get(skill_id) {
                return Ok(Some(metrics.clone()));
            }
        }

        // Load from database
        let conn = self.conn.read().map_err(|_| AetherError::Other {
            message: "Failed to acquire database lock".to_string(),
            suggestion: None,
        })?;

        let result: Option<(i64, i64, f64, Option<f64>, f64, i64, i64, String)> = conn
            .query_row(
                "SELECT total_executions, successful_executions, avg_duration_ms, avg_satisfaction, failure_rate, last_used, first_used, context_frequency
                 FROM skill_metrics WHERE skill_id = ?1",
                params![skill_id],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                        row.get(6)?,
                        row.get(7)?,
                    ))
                },
            )
            .optional()
            .map_err(|e| AetherError::ConfigError {
                message: format!("Failed to query metrics: {}", e),
                suggestion: None,
            })?;

        match result {
            Some((
                total,
                successful,
                avg_duration,
                avg_satisfaction,
                failure_rate,
                last_used,
                first_used,
                context_json,
            )) => {
                let context_frequency: HashMap<String, u32> =
                    serde_json::from_str(&context_json).unwrap_or_default();
                Ok(Some(SkillMetrics {
                    skill_id: skill_id.to_string(),
                    total_executions: total as u64,
                    successful_executions: successful as u64,
                    avg_duration_ms: avg_duration as f32,
                    avg_satisfaction: avg_satisfaction.map(|s| s as f32),
                    failure_rate: failure_rate as f32,
                    last_used,
                    first_used,
                    context_frequency,
                }))
            }
            None => Ok(None),
        }
    }

    /// Get all skills that meet the solidification threshold
    pub fn get_solidification_candidates(
        &self,
        config: &SolidificationConfig,
    ) -> Result<Vec<SkillMetrics>> {
        let conn = self.conn.read().map_err(|_| AetherError::Other {
            message: "Failed to acquire database lock".to_string(),
            suggestion: None,
        })?;

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;

        let min_age_secs = config.min_age_days as i64 * 86400;
        let max_idle_secs = config.max_idle_days as i64 * 86400;

        let mut stmt = conn
            .prepare(
                "SELECT skill_id, total_executions, successful_executions, avg_duration_ms, avg_satisfaction, failure_rate, last_used, first_used, context_frequency
                 FROM skill_metrics
                 WHERE successful_executions >= ?1
                   AND (1.0 - failure_rate) >= ?2
                   AND (?3 - first_used) >= ?4
                   AND (?3 - last_used) <= ?5",
            )
            .map_err(|e| AetherError::ConfigError {
                message: format!("Failed to prepare query: {}", e),
                suggestion: None,
            })?;

        let rows = stmt
            .query_map(
                params![
                    config.min_success_count,
                    config.min_success_rate,
                    now,
                    min_age_secs,
                    max_idle_secs,
                ],
                |row| {
                    let context_json: String = row.get(8)?;
                    let context_frequency: HashMap<String, u32> =
                        serde_json::from_str(&context_json).unwrap_or_default();
                    Ok(SkillMetrics {
                        skill_id: row.get(0)?,
                        total_executions: row.get::<_, i64>(1)? as u64,
                        successful_executions: row.get::<_, i64>(2)? as u64,
                        avg_duration_ms: row.get::<_, f64>(3)? as f32,
                        avg_satisfaction: row.get::<_, Option<f64>>(4)?.map(|s| s as f32),
                        failure_rate: row.get::<_, f64>(5)? as f32,
                        last_used: row.get(6)?,
                        first_used: row.get(7)?,
                        context_frequency,
                    })
                },
            )
            .map_err(|e| AetherError::ConfigError {
                message: format!("Failed to query candidates: {}", e),
                suggestion: None,
            })?;

        let candidates: Vec<SkillMetrics> = rows.filter_map(|r| r.ok()).collect();
        info!(count = candidates.len(), "Found solidification candidates");
        Ok(candidates)
    }

    /// Update metrics for a skill based on all executions
    fn update_metrics(&self, skill_id: &str) -> Result<()> {
        let conn = self.conn.write().map_err(|_| AetherError::Other {
            message: "Failed to acquire database lock".to_string(),
            suggestion: None,
        })?;

        // Aggregate from executions
        let stats: (i64, i64, f64, Option<f64>, i64, i64) = conn
            .query_row(
                "SELECT
                    COUNT(*) as total,
                    SUM(CASE WHEN status = 'success' OR status = 'partial_success' THEN 1 ELSE 0 END) as successful,
                    AVG(duration_ms) as avg_duration,
                    AVG(satisfaction) as avg_satisfaction,
                    MAX(invoked_at) as last_used,
                    MIN(invoked_at) as first_used
                 FROM skill_executions WHERE skill_id = ?1",
                params![skill_id],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                    ))
                },
            )
            .map_err(|e| AetherError::ConfigError {
                message: format!("Failed to aggregate stats: {}", e),
                suggestion: None,
            })?;

        let (total, successful, avg_duration, avg_satisfaction, last_used, first_used) = stats;
        let failure_rate = if total > 0 {
            1.0 - (successful as f64 / total as f64)
        } else {
            0.0
        };

        // Get context frequency
        let mut context_freq: HashMap<String, u32> = HashMap::new();
        {
            let mut stmt = conn
                .prepare("SELECT context FROM skill_executions WHERE skill_id = ?1")
                .map_err(|e| AetherError::ConfigError {
                    message: format!("Failed to prepare context query: {}", e),
                    suggestion: None,
                })?;
            let contexts = stmt
                .query_map(params![skill_id], |row| row.get::<_, String>(0))
                .map_err(|e| AetherError::ConfigError {
                    message: format!("Failed to query contexts: {}", e),
                    suggestion: None,
                })?;
            for ctx in contexts.filter_map(|r| r.ok()) {
                *context_freq.entry(ctx).or_insert(0) += 1;
            }
        }

        let context_json =
            serde_json::to_string(&context_freq).unwrap_or_else(|_| "{}".to_string());

        // Upsert metrics
        conn.execute(
            "INSERT INTO skill_metrics (skill_id, total_executions, successful_executions, avg_duration_ms, avg_satisfaction, failure_rate, last_used, first_used, context_frequency)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
             ON CONFLICT(skill_id) DO UPDATE SET
                total_executions = ?2,
                successful_executions = ?3,
                avg_duration_ms = ?4,
                avg_satisfaction = ?5,
                failure_rate = ?6,
                last_used = ?7,
                context_frequency = ?9",
            params![
                skill_id,
                total,
                successful,
                avg_duration,
                avg_satisfaction,
                failure_rate,
                last_used,
                first_used,
                context_json,
            ],
        )
        .map_err(|e| AetherError::ConfigError {
            message: format!("Failed to upsert metrics: {}", e),
            suggestion: None,
        })?;

        // Update cache
        drop(conn);
        let metrics = SkillMetrics {
            skill_id: skill_id.to_string(),
            total_executions: total as u64,
            successful_executions: successful as u64,
            avg_duration_ms: avg_duration as f32,
            avg_satisfaction: avg_satisfaction.map(|s| s as f32),
            failure_rate: failure_rate as f32,
            last_used,
            first_used,
            context_frequency: context_freq,
        };

        let mut cache = self.metrics_cache.write().map_err(|_| AetherError::Other {
            message: "Failed to acquire cache lock".to_string(),
            suggestion: None,
        })?;
        cache.insert(skill_id.to_string(), metrics);

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_log_and_get_metrics() {
        let tracker = EvolutionTracker::in_memory().unwrap();

        // Log some executions
        for i in 0..5 {
            let exec = SkillExecution::success(
                "test-skill",
                format!("session-{}", i),
                "testing",
                "test input",
                100,
                500,
            );
            tracker.log_execution(&exec).unwrap();
        }

        let metrics = tracker.get_metrics("test-skill").unwrap().unwrap();
        assert_eq!(metrics.total_executions, 5);
        assert_eq!(metrics.successful_executions, 5);
        assert_eq!(metrics.success_rate(), 1.0);
    }

    #[test]
    fn test_solidification_candidates() {
        let tracker = EvolutionTracker::in_memory().unwrap();
        let config = SolidificationConfig {
            min_success_count: 3,
            min_success_rate: 0.8,
            min_age_days: 0, // For testing
            max_idle_days: 100,
        };

        // Log enough executions
        for i in 0..4 {
            let exec = SkillExecution::success(
                "candidate-skill",
                format!("session-{}", i),
                "testing",
                "test input",
                100,
                500,
            );
            tracker.log_execution(&exec).unwrap();
        }

        let candidates = tracker.get_solidification_candidates(&config).unwrap();
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].skill_id, "candidate-skill");
    }
}
