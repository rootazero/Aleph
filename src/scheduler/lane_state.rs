//! LaneState - Per-lane queue and semaphore management
//!
//! Provides FIFO queue, concurrency control via semaphore, and priority boost
//! calculation based on wait time.

use crate::sync_primitives::Arc;
use std::collections::{HashSet, VecDeque};
use tokio::sync::{RwLock, Semaphore, SemaphorePermit};

/// A queued run with metadata
#[derive(Debug, Clone)]
pub struct QueuedRun {
    pub run_id: String,
    pub enqueued_at: i64,
}

impl QueuedRun {
    pub fn new(run_id: String) -> Self {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64;

        Self {
            run_id,
            enqueued_at: now,
        }
    }

    pub fn with_timestamp(run_id: String, enqueued_at: i64) -> Self {
        Self {
            run_id,
            enqueued_at,
        }
    }
}

/// State for a single scheduling lane
///
/// Manages a FIFO queue of pending runs, tracks running runs, and enforces
/// concurrency limits via a semaphore.
pub struct LaneState {
    /// FIFO queue of pending runs
    queue: RwLock<VecDeque<QueuedRun>>,
    /// Set of currently running run IDs
    running: RwLock<HashSet<String>>,
    /// Semaphore for concurrency control
    semaphore: Arc<Semaphore>,
}

impl LaneState {
    /// Create a new LaneState with the given concurrency limit.
    ///
    /// `max_concurrent == 0` would block every run forever, so we clamp it
    /// to 1 and emit a `warn!` instead of panicking. The configuration bug
    /// upstream still needs fixing, but a single typo in `LaneConfig`
    /// should not crash the daemon (P7 — defensive design at boundaries).
    pub fn new(max_concurrent: usize) -> Self {
        let effective = if max_concurrent == 0 {
            tracing::warn!(
                "LaneState constructed with max_concurrent=0; clamping to 1 \
                 — fix the upstream LaneConfig"
            );
            1
        } else {
            max_concurrent
        };
        Self {
            queue: RwLock::new(VecDeque::new()),
            running: RwLock::new(HashSet::new()),
            semaphore: Arc::new(Semaphore::new(effective)),
        }
    }

    /// Enqueue a run with current timestamp
    pub async fn enqueue(&self, run_id: String) {
        let queued_run = QueuedRun::new(run_id);
        self.queue.write().await.push_back(queued_run);
    }

    /// Enqueue a run with a specific timestamp (for testing)
    pub async fn enqueue_at(&self, run_id: String, timestamp: i64) {
        let queued_run = QueuedRun::with_timestamp(run_id, timestamp);
        self.queue.write().await.push_back(queued_run);
    }

    /// Try to dequeue a run (does not check semaphore)
    pub async fn try_dequeue(&self) -> Option<String> {
        self.queue.write().await.pop_front().map(|qr| qr.run_id)
    }

    /// Try to acquire a semaphore permit
    pub fn try_acquire_permit(&self) -> Option<SemaphorePermit<'_>> {
        self.semaphore.try_acquire().ok()
    }

    /// Mark a run as running
    pub async fn mark_running(&self, run_id: String) {
        self.running.write().await.insert(run_id);
    }

    /// Complete a run (removes from running set)
    pub async fn complete(&self, run_id: &str) {
        self.running.write().await.remove(run_id);
    }

    /// Get the number of queued runs
    pub async fn queue_len(&self) -> usize {
        self.queue.read().await.len()
    }

    /// Get the number of running runs
    pub async fn running_count(&self) -> usize {
        self.running.read().await.len()
    }

    /// Get the number of available semaphore permits
    pub fn available_permits(&self) -> usize {
        self.semaphore.available_permits()
    }

    /// Check if a run is currently running
    pub async fn is_running(&self, run_id: &str) -> bool {
        self.running.read().await.contains(run_id)
    }

    /// Get a reference to the semaphore (for testing)
    pub fn semaphore(&self) -> &Arc<Semaphore> {
        &self.semaphore
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_lane_state_enqueue_dequeue() {
        let state = LaneState::new(2);
        state.enqueue("run-1".to_string()).await;
        state.enqueue("run-2".to_string()).await;

        assert_eq!(state.queue_len().await, 2);

        let run_id = state.try_dequeue().await;
        assert_eq!(run_id, Some("run-1".to_string()));
        assert_eq!(state.queue_len().await, 1);
    }

    #[tokio::test]
    async fn test_lane_state_semaphore() {
        let state = LaneState::new(2);

        let permit1 = state.try_acquire_permit();
        assert!(permit1.is_some());

        let permit2 = state.try_acquire_permit();
        assert!(permit2.is_some());

        let permit3 = state.try_acquire_permit();
        assert!(permit3.is_none());

        drop(permit1);
        let permit4 = state.try_acquire_permit();
        assert!(permit4.is_some());
    }

    #[tokio::test]
    async fn test_lane_state_running_tracking() {
        let state = LaneState::new(2);

        state.mark_running("run-1".to_string()).await;
        assert_eq!(state.running_count().await, 1);
        assert!(state.is_running("run-1").await);

        state.complete("run-1").await;
        assert_eq!(state.running_count().await, 0);
        assert!(!state.is_running("run-1").await);
    }

    #[tokio::test]
    async fn lane_state_with_zero_max_concurrent_clamps_to_one() {
        // Previously panicked via assert!; daemon must survive a config typo.
        let state = LaneState::new(0);

        let p1 = state.try_acquire_permit();
        assert!(p1.is_some(), "clamp should grant exactly one permit");
        let p2 = state.try_acquire_permit();
        assert!(p2.is_none(), "clamp must not exceed 1 concurrent permit");

        drop(p1);
        let p3 = state.try_acquire_permit();
        assert!(p3.is_some(), "permit recycles after drop");
    }
}
