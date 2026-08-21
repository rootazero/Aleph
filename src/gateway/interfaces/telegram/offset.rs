//! Persistent offset tracking for Telegram polling.
//!
//! `OffsetTracker` wraps an atomic counter backed by `SQLite`, ensuring that
//! restart cycles resume from the last successfully processed `update_id`
//! rather than dropping or re-processing messages.

use crate::resilience::StateDatabase;
use crate::sync_primitives::Arc;
use crate::sync_primitives::{AtomicI64, Ordering};

/// Tracks the Telegram getUpdates offset with DB persistence.
///
/// The in-memory `AtomicI64` provides lock-free reads on the hot path,
/// while writes are durably persisted to `channel_offsets` in `SQLite`.
pub struct OffsetTracker {
    db: Arc<StateDatabase>,
    channel_id: String,
    current: AtomicI64,
}

impl OffsetTracker {
    /// Create a new tracker, loading the persisted offset from the database.
    ///
    /// Returns offset 0 if no prior offset exists (first startup).
    pub async fn new(db: Arc<StateDatabase>, channel_id: String) -> Self {
        let persisted = db
            .get_channel_offset(&channel_id)
            .await
            .unwrap_or_else(|e| {
                tracing::warn!(
                    channel_id = %channel_id,
                    error = %e,
                    "Failed to load channel offset, defaulting to 0"
                );
                None
            })
            .unwrap_or(0);

        if persisted == 0 {
            tracing::info!(
                channel_id = %channel_id,
                "OffsetTracker initialized (first startup, no persisted offset)",
            );
        } else {
            tracing::info!(
                channel_id = %channel_id,
                offset = persisted,
                "OffsetTracker initialized with persisted offset",
            );
        }

        Self {
            db,
            channel_id,
            current: AtomicI64::new(persisted),
        }
    }

    /// Return the current offset value.
    pub fn load(&self) -> i64 {
        self.current.load(Ordering::Acquire)
    }

    /// Advance the offset if `update_id` exceeds the current value.
    ///
    /// The advance is monotonic: stale or duplicate `update_ids` are ignored.
    /// On success the new offset is persisted to `SQLite`.
    pub async fn advance(&self, update_id: i64, bot_id: &str) {
        loop {
            let prev = self.current.load(Ordering::Acquire);
            if update_id <= prev {
                return; // Already at or past this offset
            }
            // CAS: only one winner when concurrent updates race
            if self
                .current
                .compare_exchange(prev, update_id, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                tracing::debug!(
                    channel_id = %self.channel_id,
                    prev_offset = prev,
                    new_offset = update_id,
                    "Offset advanced"
                );
                if let Err(e) = self
                    .db
                    .set_channel_offset(&self.channel_id, bot_id, update_id)
                    .await
                {
                    tracing::error!(
                        channel_id = %self.channel_id,
                        update_id = update_id,
                        error = %e,
                        "Failed to persist channel offset"
                    );
                }
                return;
            }
            // CAS failed — another thread advanced concurrently; retry
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_offset_tracker_starts_at_zero_without_db_entry() {
        let db = Arc::new(StateDatabase::in_memory().unwrap());
        let tracker = OffsetTracker::new(db, "test-channel".to_string()).await;
        assert_eq!(tracker.load(), 0);
    }

    #[tokio::test]
    async fn test_offset_tracker_advances_monotonically() {
        let db = Arc::new(StateDatabase::in_memory().unwrap());
        let tracker = OffsetTracker::new(db, "test-channel".to_string()).await;

        tracker.advance(10, "bot-1").await;
        assert_eq!(tracker.load(), 10);

        // Stale update should be ignored
        tracker.advance(5, "bot-1").await;
        assert_eq!(tracker.load(), 10);

        // Equal update should be ignored
        tracker.advance(10, "bot-1").await;
        assert_eq!(tracker.load(), 10);

        // Higher update should advance
        tracker.advance(20, "bot-1").await;
        assert_eq!(tracker.load(), 20);
    }

    #[tokio::test]
    async fn test_offset_tracker_persists_and_reloads() {
        let db = Arc::new(StateDatabase::in_memory().unwrap());

        // Advance offset
        {
            let tracker = OffsetTracker::new(db.clone(), "test-channel".to_string()).await;
            tracker.advance(42, "bot-1").await;
            assert_eq!(tracker.load(), 42);
        }

        // New tracker should load persisted value
        {
            let tracker = OffsetTracker::new(db, "test-channel".to_string()).await;
            assert_eq!(tracker.load(), 42);
        }
    }
}
