//! Per-agent FIFO wait queue for busy-input message delivery.
//!
//! # Why this exists
//!
//! When a message arrives while its agent's loop is busy and mid-loop steering
//! cannot deliver it (cross-session busy, steering burst at cap, `Queue` /
//! `Interrupt` busy-input modes), the inbound executor used to retry with a
//! fixed exponential back-off capped at 6 attempts (~76 s total) and then
//! **drop the message** with an error reply. Long autonomous runs routinely
//! outlive that window, so a cross-session message sent to a busy agent was
//! silently lost minutes before the agent went idle. Worse, each waiting
//! message slept on its own back-off schedule, so two queued messages could
//! wake out of order and deliver inverted — a later message grabbing the freed
//! slot before an earlier one.
//!
//! All four reference harnesses keep a real FIFO lane instead (openclaw
//! `followup` queue, hermes `queue` mode, Pi `followUpQueue`, `OpenSquilla`
//! per-session pending queue with overflow policy). This module is Aleph's
//! equivalent: every message joins its agent's FIFO lane up front (before its
//! first delivery attempt, so a newcomer can never jump ahead of waiting
//! siblings); only the front ticket attempts delivery while the rest poll
//! cheaply behind it, so bursts deliver in arrival order. Overflow is
//! `REJECT_NEWEST` (`OpenSquilla`
//! parity): past [`MAX_QUEUED_PER_AGENT`] new messages are refused up front —
//! the sender is told immediately instead of waiting half an hour to fail.
//!
//! # R10 compliance
//!
//! Pure scaffolding — mechanical arrival-order bookkeeping. No routing,
//! intent, or completion judgement; the engine's per-agent `try_start_run`
//! gate stays the single authority on whether a run may start. A missing or
//! stale ticket fails open (delivery is attempted), never closed.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;

use crate::sync_primitives::Mutex;

/// Upper bound on messages waiting in one agent's FIFO lane. Past this the
/// newest message is rejected immediately (`OpenSquilla` `REJECT_NEWEST`) so a
/// flooding channel gets prompt feedback instead of a deep silent backlog.
pub(super) const MAX_QUEUED_PER_AGENT: usize = 32;

fn queues() -> &'static Mutex<HashMap<String, VecDeque<u64>>> {
    static QUEUES: OnceLock<Mutex<HashMap<String, VecDeque<u64>>>> = OnceLock::new();
    QUEUES.get_or_init(|| Mutex::new(HashMap::new()))
}

fn next_ticket() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

fn lock() -> crate::sync_primitives::MutexGuard<'static, HashMap<String, VecDeque<u64>>> {
    queues().lock().unwrap_or_else(|e| e.into_inner())
}

/// Join the back of `agent_id`'s FIFO lane. Returns the ticket to poll
/// [`is_front`] with, or `None` when the lane is full (`REJECT_NEWEST`).
pub(super) fn register(agent_id: &str) -> Option<u64> {
    let mut map = lock();
    let queue = map.entry(agent_id.to_string()).or_default();
    if queue.len() >= MAX_QUEUED_PER_AGENT {
        return None;
    }
    let ticket = next_ticket();
    queue.push_back(ticket);
    Some(ticket)
}

/// Whether `ticket` is at the front of `agent_id`'s lane and may attempt
/// delivery. A lane or ticket that no longer exists fails **open** (`true`):
/// the engine's busy gate is the real authority, so the worst case of a stale
/// ticket is one redundant delivery attempt, never a stuck message.
pub(super) fn is_front(agent_id: &str, ticket: u64) -> bool {
    let map = lock();
    match map.get(agent_id).and_then(|q| q.front()) {
        Some(front) => *front == ticket || !map[agent_id].contains(&ticket),
        None => true,
    }
}

/// Drop `ticket` from `agent_id`'s lane (delivered, failed, or gave up).
/// Removes the lane entirely once empty so idle agents leak nothing.
pub(super) fn remove(agent_id: &str, ticket: u64) {
    let mut map = lock();
    if let Some(queue) = map.get_mut(agent_id) {
        queue.retain(|t| *t != ticket);
        if queue.is_empty() {
            map.remove(agent_id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Tests share one process-global queue map, so each test uses a unique
    // agent id to stay isolated under the parallel test runner.

    #[test]
    fn tickets_deliver_in_fifo_order() {
        let agent = "bq-test-fifo";
        let first = register(agent).unwrap();
        let second = register(agent).unwrap();
        let third = register(agent).unwrap();

        assert!(is_front(agent, first));
        assert!(!is_front(agent, second));
        assert!(!is_front(agent, third));

        // Front delivers → next in arrival order is promoted.
        remove(agent, first);
        assert!(is_front(agent, second));
        assert!(!is_front(agent, third));

        remove(agent, second);
        remove(agent, third);
    }

    #[test]
    fn mid_queue_removal_preserves_order_of_the_rest() {
        let agent = "bq-test-mid-removal";
        let first = register(agent).unwrap();
        let second = register(agent).unwrap();
        let third = register(agent).unwrap();

        // A waiter that gives up (deadline) from the middle must not block
        // or reorder the others.
        remove(agent, second);
        assert!(is_front(agent, first));
        assert!(!is_front(agent, third));
        remove(agent, first);
        assert!(is_front(agent, third));
        remove(agent, third);
    }

    #[test]
    fn full_lane_rejects_newest() {
        let agent = "bq-test-overflow";
        let tickets: Vec<u64> = (0..MAX_QUEUED_PER_AGENT)
            .map(|_| register(agent).unwrap())
            .collect();
        assert!(register(agent).is_none(), "lane at cap must reject newest");

        // Draining one slot re-admits new arrivals.
        remove(agent, tickets[0]);
        let readmitted = register(agent).expect("freed slot re-admits");

        for t in &tickets[1..] {
            remove(agent, *t);
        }
        remove(agent, readmitted);
    }

    #[test]
    fn unknown_lane_and_stale_ticket_fail_open() {
        // No lane at all → deliver.
        assert!(is_front("bq-test-no-lane", 999));

        // Lane exists but the ticket is not in it (already removed) → deliver;
        // the engine gate is authoritative, a stale ticket must never wedge.
        let agent = "bq-test-stale";
        let held = register(agent).unwrap();
        assert!(is_front(agent, 12_345_678));
        remove(agent, held);
    }

    #[test]
    fn empty_lane_is_garbage_collected() {
        let agent = "bq-test-gc";
        let t = register(agent).unwrap();
        remove(agent, t);
        assert!(
            !lock().contains_key(agent),
            "empty lane must be removed from the map"
        );
    }
}
