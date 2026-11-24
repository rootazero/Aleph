//! Persistent, per-account progress for Telegram polling.

use crate::resilience::StateDatabase;
use crate::sync_primitives::{Arc, AtomicBool, AtomicI64, Ordering};

pub struct OffsetTracker {
    db: Arc<StateDatabase>,
    channel_id: String,
    current: AtomicI64,
    initialized: AtomicBool,
}

impl OffsetTracker {
    pub async fn new(db: Arc<StateDatabase>, channel_id: String) -> Self {
        let persisted = db.get_channel_offset(&channel_id).await.unwrap_or_else(|error| {
            tracing::warn!(%channel_id, %error, "Cannot load Telegram offset; retaining pending updates");
            // Unknown initialization is not permission to discard pending updates.
            Some(0)
        });
        Self {
            db,
            channel_id,
            current: AtomicI64::new(persisted.unwrap_or(0)),
            initialized: AtomicBool::new(persisted.is_some()),
        }
    }

    /// Each bot has an independent update-id sequence.
    pub async fn for_account(&self, account_id: &str) -> Self {
        let tracker = Self::new(
            self.db.clone(),
            format!(
                "{}:telegram-account:{}:{}",
                self.channel_id,
                account_id.len(),
                account_id
            ),
        )
        .await;
        // A legacy channel cursor cannot be copied to a different bot. Retain
        // pending updates when upgrading an already initialized channel instead.
        if self.initialized.load(Ordering::Acquire) {
            tracker.initialize(account_id).await;
        }
        tracker
    }

    /// Persist the initialization fact BEFORE allowing a one-time backlog drop.
    /// A crash in between favors retaining old updates over losing new ones.
    pub async fn initialize(&self, bot_id: &str) -> bool {
        if self.initialized.swap(true, Ordering::AcqRel) {
            return false;
        }
        match self
            .db
            .set_channel_offset(&self.channel_id, bot_id, self.load())
            .await
        {
            Ok(()) => true,
            Err(error) => {
                tracing::warn!(%error, "Cannot persist Telegram initialization; retaining pending updates");
                false
            }
        }
    }

    pub fn load(&self) -> i64 {
        self.current.load(Ordering::Acquire)
    }

    /// Call only after ordered handler completion, never from a diagnostic fetch.
    pub async fn advance(&self, update_id: i64, bot_id: &str) {
        let previous = self.current.fetch_max(update_id, Ordering::AcqRel);
        if update_id <= previous {
            return;
        }
        if let Err(error) = self
            .db
            .set_channel_offset(&self.channel_id, bot_id, update_id)
            .await
        {
            tracing::error!(%error, update_id, "Failed to persist Telegram offset");
        }
        self.initialized.store(true, Ordering::Release);
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
        tracker.advance(5, "bot-1").await;
        tracker.advance(10, "bot-1").await;
        assert_eq!(tracker.load(), 10);
        tracker.advance(20, "bot-1").await;
        assert_eq!(tracker.load(), 20);
    }

    #[tokio::test]
    async fn test_offset_tracker_persists_and_reloads() {
        let db = Arc::new(StateDatabase::in_memory().unwrap());
        let tracker = OffsetTracker::new(db.clone(), "test-channel".to_string()).await;
        tracker.advance(42, "bot-1").await;
        assert_eq!(
            OffsetTracker::new(db, "test-channel".to_string())
                .await
                .load(),
            42
        );
    }

    #[tokio::test]
    async fn zero_offset_initializes_only_once_and_accounts_do_not_share_progress() {
        let db = Arc::new(StateDatabase::in_memory().unwrap());
        let root = OffsetTracker::new(db.clone(), "channel".into()).await;
        let a = root.for_account("a").await;
        let b = root.for_account("b").await;
        assert!(a.initialize("a").await);
        assert!(!a.initialize("a").await);
        let reloaded = root.for_account("a").await;
        assert_eq!(reloaded.load(), 0);
        assert!(!reloaded.initialize("a").await);
        a.advance(200, "a").await;
        b.advance(3, "b").await;
        assert_eq!(root.for_account("a").await.load(), 200);
        assert_eq!(root.for_account("b").await.load(), 3);
    }
}
