//! Storage abstraction layer for the memory system.
//!
//! Provides supporting types and trait definitions used by SQLite-backed
//! storage implementations. The Layer 2 compressed-facts storage trait
//! (`MemoryStore`) has been removed; Knowledge Notes (`NoteStore`) is now
//! the primary persistence layer for compressed memory.
//!
//! ## Remaining traits
//!
//! - **`RawMemoryStore`** -- ephemeral raw memory records (see `raw_memory`).
//! - **`DreamStore`** -- dream daemon state persistence.
//! - **`CompressionStore`** -- compression session metadata.

pub mod raw_memory;
pub mod sqlite;
pub mod types;

pub use raw_memory::{RawMemory, RawMemorySource, RawMemoryStore};

pub use sqlite::SqliteMemoryBackend;

use async_trait::async_trait;

use crate::error::AlephError;
use crate::memory::dreaming::{DailyInsight, DreamStatus};

// ---------------------------------------------------------------------------
// DreamStore -- Dream daemon persistence trait
// ---------------------------------------------------------------------------

/// Abstraction over dream daemon state persistence.
///
/// Provides storage for dream run status and daily insight summaries
/// generated during idle-time memory consolidation.
#[async_trait]
pub trait DreamStore: Send + Sync {
    /// Get the current dream daemon status.
    async fn get_dream_status(&self) -> Result<DreamStatus, AlephError>;

    /// Update the dream daemon status.
    async fn set_dream_status(&self, status: DreamStatus) -> Result<(), AlephError>;

    /// Insert or update a daily insight for the given date.
    async fn upsert_daily_insight(&self, insight: DailyInsight) -> Result<(), AlephError>;

    /// Get the daily insight for a specific date (YYYY-MM-DD format).
    async fn get_daily_insight(&self, date: &str) -> Result<Option<DailyInsight>, AlephError>;

    /// List the most recent daily insights, ordered by date descending,
    /// capped at `limit`. Used by the `dreaming.list_insights` RPC.
    async fn recent_daily_insights(&self, limit: usize) -> Result<Vec<DailyInsight>, AlephError>;
}

// ---------------------------------------------------------------------------
// CompressionStore -- Compression session persistence trait
// ---------------------------------------------------------------------------

/// Abstraction over compression session metadata storage.
///
/// Tracks when compression was last run and stores session records
/// for auditing the memory compression pipeline.
#[async_trait]
pub trait CompressionStore: Send + Sync {
    /// Set the timestamp of the last successful compression run.
    async fn set_last_compression_timestamp(&self, timestamp: i64) -> Result<(), AlephError>;
}

// ---------------------------------------------------------------------------
// MemoryBackend type alias
// ---------------------------------------------------------------------------

use crate::sync_primitives::Arc;

/// Unified memory backend.
///
/// This is the single entry point for all memory storage operations.
/// Wraps `SqliteMemoryBackend` in an `Arc` for shared ownership across
/// the agent loop, thinker, and other subsystems.
pub type MemoryBackend = Arc<sqlite::SqliteMemoryBackend>;
