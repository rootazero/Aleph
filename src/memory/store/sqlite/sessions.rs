//! DreamStore and CompressionStore implementations for SqliteMemoryBackend.
//!
//! Persists dream daemon status, daily insights, and compression metadata
//! in dedicated SQLite tables so state survives restarts.

use async_trait::async_trait;
use rusqlite::params;

use crate::error::AlephError;
use crate::memory::context::CompressionSession;
use crate::memory::dreaming::{DailyInsight, DreamStatus};
use crate::memory::store::{CompressionStore, DreamStore};

use super::SqliteMemoryBackend;

// ============================================================================
// DreamStore implementation
// ============================================================================

#[async_trait]
impl DreamStore for SqliteMemoryBackend {
    async fn get_dream_status(&self) -> Result<DreamStatus, AlephError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let mut stmt = conn
            .prepare(
                "SELECT last_run_at, last_status, last_duration_ms FROM dream_status WHERE id = 1",
            )
            .map_err(|e| {
                AlephError::config(format!("Failed to prepare dream_status query: {e}"))
            })?;

        let result = stmt.query_row(params![], |row| {
            let last_run_at: Option<i64> = row.get(0)?;
            let last_status: Option<String> = row.get(1)?;
            let last_duration_ms: Option<i64> = row.get(2)?;
            Ok(DreamStatus {
                last_run_at,
                last_status,
                last_duration_ms: last_duration_ms.map(|v| v as u64),
            })
        });

        match result {
            Ok(status) => Ok(status),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(DreamStatus::default()),
            Err(e) => Err(AlephError::config(format!(
                "Failed to get dream status: {e}"
            ))),
        }
    }

    async fn set_dream_status(&self, status: DreamStatus) -> Result<(), AlephError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        conn.execute(
            "INSERT INTO dream_status (id, last_run_at, last_status, last_duration_ms)
             VALUES (1, ?1, ?2, ?3)
             ON CONFLICT(id) DO UPDATE SET
                 last_run_at = excluded.last_run_at,
                 last_status = excluded.last_status,
                 last_duration_ms = excluded.last_duration_ms",
            params![
                status.last_run_at,
                status.last_status,
                status.last_duration_ms.map(|v| v as i64),
            ],
        )
        .map_err(|e| AlephError::config(format!("Failed to set dream status: {e}")))?;
        Ok(())
    }

    async fn upsert_daily_insight(&self, insight: DailyInsight) -> Result<(), AlephError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        conn.execute(
            "INSERT INTO daily_insights (date, content, source_memory_count, created_at)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(date) DO UPDATE SET
                 content = excluded.content,
                 source_memory_count = excluded.source_memory_count,
                 created_at = excluded.created_at",
            params![
                insight.date,
                insight.content,
                insight.source_memory_count,
                insight.created_at,
            ],
        )
        .map_err(|e| AlephError::config(format!("Failed to upsert daily insight: {e}")))?;
        Ok(())
    }

    async fn get_daily_insight(&self, date: &str) -> Result<Option<DailyInsight>, AlephError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let mut stmt = conn
            .prepare("SELECT date, content, source_memory_count, created_at FROM daily_insights WHERE date = ?1")
            .map_err(|e| AlephError::config(format!("Failed to prepare daily_insights query: {e}")))?;

        let result = stmt.query_row(params![date], |row| {
            Ok(DailyInsight {
                date: row.get(0)?,
                content: row.get(1)?,
                source_memory_count: row.get(2)?,
                created_at: row.get(3)?,
            })
        });

        match result {
            Ok(insight) => Ok(Some(insight)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(AlephError::config(format!(
                "Failed to get daily insight: {e}"
            ))),
        }
    }
}

// ============================================================================
// CompressionStore implementation
// ============================================================================

#[async_trait]
impl CompressionStore for SqliteMemoryBackend {
    async fn set_last_compression_timestamp(&self, timestamp: i64) -> Result<(), AlephError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        conn.execute(
            "INSERT INTO compression_metadata (key, value)
             VALUES ('last_timestamp', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![timestamp.to_string()],
        )
        .map_err(|e| AlephError::config(format!("Failed to set compression timestamp: {e}")))?;
        Ok(())
    }

    async fn get_last_compression_timestamp(&self) -> Result<Option<i64>, AlephError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let mut stmt = conn
            .prepare("SELECT value FROM compression_metadata WHERE key = 'last_timestamp'")
            .map_err(|e| {
                AlephError::config(format!(
                    "Failed to prepare compression timestamp query: {e}"
                ))
            })?;

        let result = stmt.query_row(params![], |row| {
            let value: String = row.get(0)?;
            Ok(value)
        });

        match result {
            Ok(value) => {
                let ts = value.parse::<i64>().map_err(|e| {
                    AlephError::config(format!("Failed to parse compression timestamp: {e}"))
                })?;
                Ok(Some(ts))
            }
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(AlephError::config(format!(
                "Failed to get compression timestamp: {e}"
            ))),
        }
    }

    async fn record_compression_session(
        &self,
        session: &CompressionSession,
    ) -> Result<(), AlephError> {
        let json = serde_json::to_string(session).map_err(|e| {
            AlephError::config(format!("Failed to serialize compression session: {e}"))
        })?;

        let key = format!("session_{}", session.id);
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        conn.execute(
            "INSERT INTO compression_metadata (key, value)
             VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, json],
        )
        .map_err(|e| AlephError::config(format!("Failed to record compression session: {e}")))?;

        tracing::info!(
            session_id = %session.id,
            memories = session.source_memory_ids.len(),
            facts = session.extracted_fact_ids.len(),
            provider = %session.provider_used,
            duration_ms = session.duration_ms,
            "Compression session persisted"
        );

        Ok(())
    }
}
