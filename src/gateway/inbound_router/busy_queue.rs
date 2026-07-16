//! Per-session FIFO wait queue for busy-input message delivery.
//!
//! # Why this exists
//!
//! When a message arrives while its session's loop is busy and mid-loop steering
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
//! equivalent: every message joins its session's FIFO lane up front (before its
//! first delivery attempt, so a newcomer can never jump ahead of waiting
//! siblings); only the front ticket attempts delivery while the rest poll
//! cheaply behind it, so bursts deliver in arrival order. Overflow is
//! `REJECT_NEWEST` (`OpenSquilla`
//! parity): past [`MAX_QUEUED_PER_SESSION`] new messages are refused up front —
//! the sender is told immediately instead of waiting half an hour to fail.
//!
//! # R10 compliance
//!
//! Pure scaffolding — mechanical arrival-order bookkeeping. No routing,
//! intent, or completion judgement; the engine's per-session `SessionRunRegistry`
//! gate stays the single authority on whether a run may start. A missing or
//! stale ticket fails open (delivery is attempted), never closed.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;

use crate::sync_primitives::Mutex;

/// Upper bound on messages waiting in one session's FIFO lane. Past this the
/// newest message is rejected immediately (`OpenSquilla` `REJECT_NEWEST`) so a
/// flooding channel gets prompt feedback instead of a deep silent backlog.
pub(super) const MAX_QUEUED_PER_SESSION: usize = 32;

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

/// Join the back of the session key's FIFO lane. Returns the ticket to poll
/// [`TicketGuard::is_front`] with, or `None` when the lane is full
/// (`REJECT_NEWEST`).
pub(super) fn register(session_key: &str) -> Option<TicketGuard> {
    let mut map = lock();
    let queue = map.entry(session_key.to_string()).or_default();
    if queue.len() >= MAX_QUEUED_PER_SESSION {
        return None;
    }
    let ticket = next_ticket();
    queue.push_back(ticket);
    Some(TicketGuard {
        session_key: session_key.to_string(),
        ticket,
    })
}

/// RAII lane membership: joins on [`register`], leaves on `Drop`. The `Drop`
/// impl is load-bearing — a panic anywhere while the ticket is held (e.g.
/// inside the execution adapter the waiter awaits) unwinds the task and would
/// otherwise leave a corpse ticket in the lane; once that corpse reached the
/// front, every ticket behind it polled `is_front == false` forever (the
/// fail-open clause only rescues tickets NOT in the queue) and the session's
/// delivery lane was wedged until daemon restart. The other half of this
/// delivery pipeline made the same move for the session claim
/// (`gate.rs::RunSlot`).
pub(super) struct TicketGuard {
    session_key: String,
    ticket: u64,
}

impl TicketGuard {
    /// The raw ticket number, for log correlation only.
    pub(super) fn id(&self) -> u64 {
        self.ticket
    }

    /// Whether this ticket is at the front of its lane and may attempt
    /// delivery (see [`is_front`]).
    pub(super) fn is_front(&self) -> bool {
        is_front(&self.session_key, self.ticket)
    }
}

impl Drop for TicketGuard {
    fn drop(&mut self) {
        remove(&self.session_key, self.ticket);
    }
}

/// Whether `ticket` is at the front of the session key's lane and may attempt
/// delivery. A lane or ticket that no longer exists fails **open** (`true`):
/// the engine's busy gate is the real authority, so the worst case of a stale
/// ticket is one redundant delivery attempt, never a stuck message.
fn is_front(session_key: &str, ticket: u64) -> bool {
    let map = lock();
    match map.get(session_key).and_then(|q| q.front()) {
        Some(front) => *front == ticket || !map[session_key].contains(&ticket),
        None => true,
    }
}

/// Drop `ticket` from the session key's lane (delivered, failed, or gave up).
/// Private: production code leaves a lane only via [`TicketGuard`]'s `Drop`.
/// Removes the lane entirely once empty so idle sessions leak nothing.
fn remove(session_key: &str, ticket: u64) {
    let mut map = lock();
    if let Some(queue) = map.get_mut(session_key) {
        queue.retain(|t| *t != ticket);
        if queue.is_empty() {
            map.remove(session_key);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Tests share one process-global queue map, so each test uses a unique
    // session key to stay isolated under the parallel test runner.

    #[test]
    fn tickets_deliver_in_fifo_order() {
        let agent = "bq-test-fifo";
        let first = register(agent).unwrap();
        let second = register(agent).unwrap();
        let third = register(agent).unwrap();

        assert!(first.is_front());
        assert!(!second.is_front());
        assert!(!third.is_front());

        // Front delivers (guard dropped) → next in arrival order is promoted.
        drop(first);
        assert!(second.is_front());
        assert!(!third.is_front());
    }

    #[test]
    fn mid_queue_removal_preserves_order_of_the_rest() {
        let agent = "bq-test-mid-removal";
        let first = register(agent).unwrap();
        let second = register(agent).unwrap();
        let third = register(agent).unwrap();

        // A waiter that gives up (deadline) from the middle must not block
        // or reorder the others.
        drop(second);
        assert!(first.is_front());
        assert!(!third.is_front());
        drop(first);
        assert!(third.is_front());
    }

    #[test]
    fn full_lane_rejects_newest() {
        let agent = "bq-test-overflow";
        let mut guards: Vec<TicketGuard> = (0..MAX_QUEUED_PER_SESSION)
            .map(|_| register(agent).unwrap())
            .collect();
        assert!(register(agent).is_none(), "lane at cap must reject newest");

        // Draining one slot re-admits new arrivals.
        guards.remove(0); // dropped
        let _readmitted = register(agent).expect("freed slot re-admits");
    }

    #[test]
    fn unknown_lane_and_stale_ticket_fail_open() {
        // No lane at all → deliver.
        assert!(is_front("bq-test-no-lane", 999));

        // Lane exists but the ticket is not in it (never registered) → deliver;
        // the engine gate is authoritative, a stale ticket must never wedge.
        let agent = "bq-test-stale";
        let _held = register(agent).unwrap();
        assert!(is_front(agent, 12_345_678));
    }

    #[test]
    fn empty_lane_is_garbage_collected() {
        let agent = "bq-test-gc";
        let t = register(agent).unwrap();
        drop(t);
        assert!(
            !lock().contains_key(agent),
            "empty lane must be removed from the map"
        );
    }

    #[test]
    fn panic_while_holding_ticket_releases_the_lane() {
        // The RAII guard's whole reason to exist: a front waiter that panics
        // while holding its ticket must not leave a corpse at the head of the
        // lane (a leaked front ticket blocked everyone behind it forever).
        let agent = "bq-test-panic";
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _front = register(agent).unwrap();
            panic!("simulated adapter panic while holding the front ticket");
        }));
        // Had the front ticket leaked, this later arrival would sit behind the
        // corpse (`is_front == false`) until daemon restart.
        let waiting = register(agent).unwrap();
        assert!(waiting.is_front(), "corpse ticket must not wedge the lane");
    }

    #[test]
    fn distinct_session_keys_do_not_block_each_other() {
        // Two different sessions (same agent in production) get independent
        // lanes → both are immediately front (true cross-session parallelism).
        let s1 = "bq-test-agentX|conv-1";
        let s2 = "bq-test-agentX|conv-2";
        let t1 = register(s1).unwrap();
        let t2 = register(s2).unwrap();
        assert!(t1.is_front(), "session-1 lane is its own front");
        assert!(
            t2.is_front(),
            "session-2 lane is its own front — not blocked by session-1"
        );
    }
}
