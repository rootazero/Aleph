//! DreamStore and CompressionStore implementations for LanceMemoryBackend.
//!
//! SessionStore has been removed — raw memory entry (Layer 1) storage
//! is no longer provided by LanceDB. Raw conversations are stored
//! in SessionManager's SQLite.

use async_trait::async_trait;

use crate::error::AlephError;
use crate::memory::context::CompressionSession;
use crate::memory::dreaming::{DailyInsight, DreamStatus};
use crate::memory::store::{CompressionStore, DreamStore};

use super::LanceMemoryBackend;

// ============================================================================
// DreamStore implementation
// ============================================================================

#[async_trait]
impl DreamStore for LanceMemoryBackend {
    async fn get_dream_status(&self) -> Result<DreamStatus, AlephError> {
        // NOTE: DreamStore is not yet persisted to LanceDB. Status resets on
        // restart, meaning the DreamDaemon will run again on the first eligible
        // window. This is safe but wasteful — a dedicated metadata table should
        // be added when dream persistence becomes important.
        Ok(DreamStatus::default())
    }

    async fn set_dream_status(&self, _status: DreamStatus) -> Result<(), AlephError> {
        // No-op: dream status is ephemeral (see get_dream_status note).
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
impl CompressionStore for LanceMemoryBackend {
    async fn set_last_compression_timestamp(&self, timestamp: i64) -> Result<(), AlephError> {
        self.last_compression_ts
            .store(timestamp, crate::sync_primitives::Ordering::Release);
        tracing::debug!(timestamp, "Updated compression timestamp");
        Ok(())
    }

    async fn get_last_compression_timestamp(&self) -> Result<Option<i64>, AlephError> {
        let ts = self
            .last_compression_ts
            .load(crate::sync_primitives::Ordering::Acquire);
        if ts == 0 {
            Ok(None)
        } else {
            Ok(Some(ts))
        }
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
