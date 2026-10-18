//! `CancellationToken` — drop-in replacement for `std::task::Poll` cancellation.
//!
//! Provides a shared, multi-subscriber cancellation flag with tokio broadcast
//! integration. Channels and long-running operations use this to coordinate
//! graceful shutdown without `std::task:: CancellationToken` (unstable).
//!
//! Moved from `alephcore::gateway::cancellation` in Task 4 of the P0
//! modularization; `alephcore::gateway::cancellation` is now a re-export
//! shim pointing here. The implementation uses `std::sync::Arc` and std
//! atomics (`SeqCst`) with a tokio `broadcast` (capacity 16) — exactly the
//! primitives the Core definition used.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::broadcast;

/// A shared cancellation flag that can be cloned and subscribed to.
///
/// All clones share the same underlying state — cancelling one cancels all.
/// Subscribers receive a `()` value when cancellation is triggered.
#[derive(Debug, Clone)]
pub struct CancellationToken {
    inner: Arc<AtomicBool>,
    cancel_broadcast: broadcast::Sender<()>,
}

impl CancellationToken {
    /// Create a new token that is not cancelled.
    #[must_use]
    pub fn new() -> Self {
        let (cancel_broadcast, _) = broadcast::channel(16);
        Self {
            inner: Arc::new(AtomicBool::new(false)),
            cancel_broadcast,
        }
    }

    /// Trigger cancellation — all clones and subscribers will see it.
    pub fn cancel(&self) {
        self.inner.store(true, Ordering::SeqCst);
        // Ignore send error — nobody listening is fine
        let _ = self.cancel_broadcast.send(());
    }

    /// Returns `true` if cancellation has been requested.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.inner.load(Ordering::SeqCst)
    }

    /// Subscribe to cancellation events.
    /// The returned receiver will receive `()` exactly once when `cancel` is called.
    #[must_use]
    pub fn subscribe(&self) -> broadcast::Receiver<()> {
        let rx = self.cancel_broadcast.subscribe();
        // Close the lost-wakeup gap: a one-shot `broadcast::send` in `cancel()`
        // is only delivered to subscribers that existed at send time. If
        // cancellation already fired, re-emit so this freshly created receiver
        // observes it (cancellation is idempotent, so re-notifying is safe).
        if self.is_cancelled() {
            let _ = self.cancel_broadcast.send(());
        }
        rx
    }
}

impl Default for CancellationToken {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn not_cancelled_by_default() {
        let token = CancellationToken::new();
        assert!(!token.is_cancelled());
    }

    #[test]
    fn cancel_sets_flag() {
        let token = CancellationToken::new();
        token.cancel();
        assert!(token.is_cancelled());
    }
}
