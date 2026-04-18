//! CancellationToken — drop-in replacement for `std::task::Poll` cancellation.
//!
//! Provides a shared, multi-subscriber cancellation flag with tokio broadcast
//! integration. Channels and long-running operations use this to coordinate
//! graceful shutdown without `std::task:: CancellationToken` (unstable).

use crate::sync_primitives::Arc;
use crate::sync_primitives::{AtomicBool, Ordering};
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
    pub fn is_cancelled(&self) -> bool {
        self.inner.load(Ordering::SeqCst)
    }

    /// Subscribe to cancellation events.
    /// The returned receiver will receive `()` exactly once when `cancel` is called.
    pub fn subscribe(&self) -> broadcast::Receiver<()> {
        self.cancel_broadcast.subscribe()
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
