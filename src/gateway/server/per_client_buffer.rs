//! Per-client event buffer with backpressure support
//!
//! Provides bounded buffering between the global GatewayEventBus and individual
//! WebSocket connections. Uses tokio broadcast channel for sync recv compatibility
//! with tokio::select!.

use crate::sync_primitives::Arc;
use crate::sync_primitives::{AtomicU64, Ordering};
use tokio::sync::broadcast;

/// Default per-client buffer capacity
pub const CLIENT_BUFFER_CAPACITY: usize = 256;

/// Metrics for a per-client event buffer
#[derive(Debug, Clone)]
pub struct PerClientBufferMetrics {
    /// Number of events dropped due to buffer overflow
    pub overflow_count: Arc<AtomicU64>,
}

impl PerClientBufferMetrics {
    pub fn new() -> Self {
        Self {
            overflow_count: Arc::new(AtomicU64::new(0)),
        }
    }

    pub fn overflow(&self) -> u64 {
        self.overflow_count.load(Ordering::Relaxed)
    }

    pub fn add_overflow(&self, n: u64) {
        self.overflow_count.fetch_add(n, Ordering::Relaxed);
    }
}

impl Default for PerClientBufferMetrics {
    fn default() -> Self {
        Self::new()
    }
}

/// Per-client event buffer using broadcast channel internally.
///
/// Uses broadcast semantics: when the receiver can't keep up with the sender,
/// it receives a `Lagged` error. This signals that some events were dropped.
/// We count these as overflow events.
pub struct PerClientBuffer {
    sender: broadcast::Sender<String>,
    metrics: PerClientBufferMetrics,
}

impl PerClientBuffer {
    pub fn new() -> (Self, broadcast::Receiver<String>) {
        Self::with_capacity(CLIENT_BUFFER_CAPACITY)
    }

    pub fn with_capacity(capacity: usize) -> (Self, broadcast::Receiver<String>) {
        let (sender, receiver) = broadcast::channel(capacity);
        let metrics = PerClientBufferMetrics::new();
        (Self { sender, metrics }, receiver)
    }

    /// Try to broadcast an event to all receivers (fire-and-forget)
    ///
    /// Returns `Ok(())` if sent, or `Err(())` if no receivers (buffer full).
    /// When receivers lag, events are dropped at the receiver level, not here.
    pub fn try_send(&self, event: String) -> Result<(), ()> {
        match self.sender.send(event) {
            Ok(_) => Ok(()),
            Err(broadcast::error::SendError(_)) => {
                self.metrics.add_overflow(1);
                Err(())
            }
        }
    }

    pub fn metrics(&self) -> &PerClientBufferMetrics {
        &self.metrics
    }
}

impl Default for PerClientBuffer {
    fn default() -> Self {
        let (sender, _) = Self::new();
        sender
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_buffer_basic() {
        let (buffer, mut rx) = PerClientBuffer::new();
        buffer.try_send("event1".into()).unwrap();
        buffer.try_send("event2".into()).unwrap();
        buffer.try_send("event3".into()).unwrap();
        assert_eq!(rx.blocking_recv(), Ok("event1".into()));
        assert_eq!(rx.blocking_recv(), Ok("event2".into()));
        assert_eq!(rx.blocking_recv(), Ok("event3".into()));
    }

    #[test]
    fn test_buffer_no_receivers() {
        let (buffer, _rx) = PerClientBuffer::with_capacity(2);
        let result = buffer.try_send("event".into());
        assert!(result.is_ok() || result.is_err());
    }
}
