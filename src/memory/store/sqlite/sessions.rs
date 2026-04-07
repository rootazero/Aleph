//! DreamStore and CompressionStore implementations for SqliteMemoryBackend.
//!
//! These are minimal no-op implementations matching the previous SQLite
//! behaviour. Dream status and compression timestamps are ephemeral
//! (reset on restart). A proper persistent implementation can be added
//! later using dedicated SQLite tables.

use async_trait::async_trait;

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
        // Dream status is ephemeral — resets on restart.
        // The DreamDaemon will run again on the first eligible window.
        Ok(DreamStatus::default())
    }

    async fn set_dream_status(&self, _status: DreamStatus) -> Result<(), AlephError> {
        // No-op: dream status is ephemeral.
        Ok(())
    }

    async fn upsert_daily_insight(&self, _insight: DailyInsight) -> Result<(), AlephError> {
        // No-op: daily insights are not yet persisted.
        Ok(())
    }

    async fn get_daily_insight(&self, _date: &str) -> Result<Option<DailyInsight>, AlephError> {
        // No-op: daily insights are not yet persisted.
        Ok(None)
    }
}

// ============================================================================
// CompressionStore implementation
// ============================================================================

#[async_trait]
impl CompressionStore for SqliteMemoryBackend {
    async fn set_last_compression_timestamp(&self, timestamp: i64) -> Result<(), AlephError> {
        // Ephemeral — log only, not persisted across restarts.
        tracing::debug!(timestamp, "Updated compression timestamp (ephemeral)");
        let _ = timestamp;
        Ok(())
    }

    async fn get_last_compression_timestamp(&self) -> Result<Option<i64>, AlephError> {
        // Ephemeral — always returns None after restart.
        Ok(None)
    }

    async fn record_compression_session(
        &self,
        session: &CompressionSession,
    ) -> Result<(), AlephError> {
        tracing::info!(
            memories = session.source_memory_ids.len(),
            facts = session.extracted_fact_ids.len(),
            provider = %session.provider_used,
            duration_ms = session.duration_ms,
            "Compression session recorded"
        );
        Ok(())
    }
}
