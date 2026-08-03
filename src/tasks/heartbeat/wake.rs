//! Wake Queue
//!
//! A priority-coalescing queue for waking heartbeat tasks out-of-band.
//! Multiple wake requests for the same task are collapsed into the one
//! with the highest priority, preventing redundant executions while
//! preserving the most urgent intent.

use crate::sync_primitives::Mutex;
use std::collections::HashMap;

use tokio::sync::Notify;

// ── WakePriority ──────────────────────────────────────────────────────────────

/// Priority level for a wake request.
///
/// Higher numeric value = higher priority. When two requests target the
/// same task, the one with the higher priority wins.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum WakePriority {
    /// Triggered by the regular interval timer
    #[allow(dead_code)]
    Interval = 0,
    /// Triggered by a system event (e.g. network change, OS wake)
    #[allow(dead_code)]
    SystemEvent = 1,
    /// Triggered by explicit user action (highest priority)
    UserAction = 2,
}

// ── WakeRequest ───────────────────────────────────────────────────────────────

/// A request to wake a specific heartbeat task.
#[derive(Debug, Clone)]
pub struct WakeRequest {
    pub task_id: String,
    pub priority: WakePriority,
    /// Optional human-readable reason (for logging / observability)
    pub reason: Option<String>,
    /// When this request was created (Unix ms)
    pub requested_at: i64,
}

// ── WakeQueue ─────────────────────────────────────────────────────────────────

/// Thread-safe priority-coalescing wake queue.
///
/// Maintains at most one pending `WakeRequest` per task, keeping whichever
/// has the higher priority. Callers await `notified()` to sleep until at
/// least one request is enqueued, then call `drain()` to retrieve and
/// clear all pending requests atomically.
pub struct WakeQueue {
    pending: Mutex<HashMap<String, WakeRequest>>,
    notify: Notify,
}

impl WakeQueue {
    #[must_use]
    pub fn new() -> Self {
        Self {
            pending: Mutex::new(HashMap::new()),
            notify: Notify::new(),
        }
    }

    /// Enqueue a wake request.
    ///
    /// If a request for the same `task_id` already exists with equal or
    /// higher priority, the existing request is kept unchanged.
    /// Otherwise the incoming request replaces it.
    pub fn enqueue(&self, req: WakeRequest) {
        let mut map = self.pending.lock().unwrap_or_else(|e| e.into_inner());
        match map.get(&req.task_id) {
            // Keep the existing entry if it has equal or higher priority
            Some(existing) if existing.priority >= req.priority => {}
            _ => {
                map.insert(req.task_id.clone(), req);
            }
        }
        self.notify.notify_one();
    }

    /// Drain all pending wake requests atomically.
    ///
    /// Returns all pending requests and clears the internal map.
    pub fn drain(&self) -> Vec<WakeRequest> {
        let mut map = self.pending.lock().unwrap_or_else(|e| e.into_inner());
        map.drain().map(|(_, v)| v).collect()
    }

    /// Wait until at least one wake request has been enqueued.
    ///
    /// This is a thin wrapper around `tokio::sync::Notify::notified()`.
    /// The caller should call `drain()` after being woken.
    pub async fn notified(&self) {
        self.notify.notified().await;
    }
}

impl Default for WakeQueue {
    fn default() -> Self {
        Self::new()
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_req(task_id: &str, priority: WakePriority, reason: Option<&str>) -> WakeRequest {
        WakeRequest {
            task_id: task_id.to_string(),
            priority,
            reason: reason.map(|s| s.to_string()),
            requested_at: 0,
        }
    }

    #[test]
    fn test_enqueue_and_drain() {
        let q = WakeQueue::new();
        q.enqueue(make_req("t1", WakePriority::Interval, None));
        let d = q.drain();
        assert_eq!(d.len(), 1);
        assert!(q.drain().is_empty());
    }

    #[test]
    fn test_coalesce_keeps_higher_priority() {
        let q = WakeQueue::new();
        q.enqueue(make_req("t1", WakePriority::Interval, Some("timer")));
        q.enqueue(make_req("t1", WakePriority::UserAction, Some("manual")));
        let d = q.drain();
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].priority, WakePriority::UserAction);
    }

    #[test]
    fn test_coalesce_does_not_downgrade_priority() {
        let q = WakeQueue::new();
        q.enqueue(make_req("t1", WakePriority::UserAction, Some("manual")));
        q.enqueue(make_req("t1", WakePriority::Interval, Some("timer")));
        let d = q.drain();
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].priority, WakePriority::UserAction);
        assert_eq!(d[0].reason.as_deref(), Some("manual"));
    }

    #[test]
    fn test_multiple_tasks() {
        let q = WakeQueue::new();
        q.enqueue(make_req("t1", WakePriority::Interval, None));
        q.enqueue(make_req("t2", WakePriority::SystemEvent, None));
        assert_eq!(q.drain().len(), 2);
    }

    #[test]
    fn test_drain_clears_queue() {
        let q = WakeQueue::new();
        q.enqueue(make_req("t1", WakePriority::Interval, None));
        q.drain();
        assert!(q.drain().is_empty());
    }

    #[test]
    fn test_priority_ordering() {
        assert!(WakePriority::Interval < WakePriority::SystemEvent);
        assert!(WakePriority::SystemEvent < WakePriority::UserAction);
    }

    #[test]
    fn test_same_priority_keeps_existing() {
        // When priority is equal, the existing request is preserved
        let q = WakeQueue::new();
        q.enqueue(make_req("t1", WakePriority::SystemEvent, Some("first")));
        q.enqueue(make_req("t1", WakePriority::SystemEvent, Some("second")));
        let d = q.drain();
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].reason.as_deref(), Some("first"));
    }
}
