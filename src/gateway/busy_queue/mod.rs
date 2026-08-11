//! Per-session FIFO wait lane for busy-input message delivery.
//!
//! # Why this exists
//!
//! When a message arrives while its session's loop is busy and mid-loop
//! steering cannot deliver it (cross-session busy, steering burst at cap,
//! `Queue` / `Interrupt` busy-input modes), the inbound executor used to retry
//! with a fixed exponential back-off capped at 6 attempts (~76 s total) and
//! then **drop the message** with an error reply. Long autonomous runs
//! routinely outlive that window, so a message sent to a busy agent was
//! silently lost minutes before the agent went idle. Worse, each waiting
//! message slept on its own back-off schedule, so two queued messages could
//! wake out of order and deliver inverted.
//!
//! All four reference harnesses keep a real FIFO lane instead (openclaw
//! `followup` queue, hermes `queue` mode, Pi `followUpQueue`, `OpenSquilla`
//! per-session pending queue with overflow policy). This module is Aleph's
//! equivalent: every message joins its session's FIFO lane up front (before its
//! first delivery attempt, so a newcomer can never jump ahead of waiting
//! siblings); only the front ticket attempts delivery while the rest park on a
//! wake signal behind it, so bursts deliver in arrival order. Overflow is
//! `REJECT_NEWEST` (`OpenSquilla` parity): past the configured cap new messages
//! are refused up front — the sender is told immediately instead of waiting
//! half an hour to fail.
//!
//! # Why it lives at the gateway level, not under `inbound_router`
//!
//! The lane has three stakeholders: the inbound router (channel messages), the
//! `aleph-server` RPC handlers (Panel / CLI `agent.run` + `chat.send`), and the
//! execution engine (which *signals* the lane when a session's run slot frees).
//! Keeping the lane under `inbound_router` would have made the Panel path
//! depend on the channel router (P1). Delivery ordering is a gateway-wide I/O
//! concern, so it lives beside the other gateway I/O seams.
//!
//! # The lane holds *waiting* messages only
//!
//! A ticket leaves the lane the moment its message stops waiting — which is
//! when the engine **admits** it ([`mark_admitted`], called from
//! `SessionRunRegistry::try_claim`), not when the resulting run finishes.
//! `deliver_with_ticket` holds its guard across the whole `attempt()`, and
//! `attempt()` is the entire agent run; keeping the ticket enqueued for that
//! long made every follow-up park behind the very run it wanted to change, so
//! the `Steer` and `Interrupt` busy-input modes could never reach the engine.
//! See [`mark_admitted`] for the full failure shape.
//!
//! # Wake model (codex `watch::Sender<InputQueueActivity>` parity)
//!
//! Waiters do **not** poll. They park on a per-session [`tokio::sync::Notify`]
//! that fires when:
//! * the engine releases the session's run slot
//!   ([`crate::gateway::execution_engine`]'s `SessionRunRegistry::release`
//!   calls [`notify_slot_free`]) — the authoritative "you may try now" edge;
//! * the engine admits a queued run ([`mark_admitted`]) — the symmetric
//!   "the lane just got shorter" edge; or
//! * a ticket leaves the lane ([`TicketGuard`]'s `Drop`), promoting the next.
//!
//! A bounded fallback tick (`wake_fallback_secs`) still runs, so a missed
//! signal degrades to a slightly later delivery rather than a wedged lane.
//!
//! # R10 compliance
//!
//! Pure scaffolding — mechanical arrival-order bookkeeping. No routing, intent,
//! or completion judgement; the engine's per-session `SessionRunRegistry` gate
//! stays the single authority on whether a run may start. A missing or stale
//! ticket fails open (delivery is attempted), never closed.

mod config;
mod deliver;
mod spawn;

pub use config::BusyQueueConfig;
pub use deliver::{deliver_with_ticket, DeliveryOutcome};
pub use spawn::spawn_queued_run;

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Instant;

use tokio::sync::Notify;

use crate::sync_primitives::Mutex;

/// One waiting message.
struct Ticket {
    /// Arrival-order id, unique process-wide.
    id: u64,
    /// The run this message would become. Lets an explicit stop reach a
    /// message that is still queued — such a run is not in the engine's
    /// `active_runs` yet, so `ExecutionEngine::cancel` cannot see it and the
    /// user would face an unstoppable pending bubble.
    run_id: String,
    /// When this message joined the lane, on the monotonic clock.
    ///
    /// Read back by [`waiting_since`] so the admission gate can answer
    /// "was the run I am about to cancel already running when I got here?".
    /// Wall time would do for a log line but not for a decision: the
    /// comparison is against another in-process instant, and a clock step
    /// would silently flip the answer.
    enqueued_at: Instant,
    /// Set by [`purge`] / [`cancel_queued_run`]: this waiter should abandon
    /// its message.
    cancelled: bool,
}

/// One session's wait lane: arrival-ordered tickets plus the wake handle every
/// waiter on this session parks on.
#[derive(Default)]
struct Lane {
    /// Tickets in arrival order; the front one may attempt delivery.
    tickets: VecDeque<Ticket>,
    /// Fires on slot-free, on ticket departure, and on cancellation. Shared by
    /// every waiter on this session so one signal wakes all of them (each
    /// re-checks its own state, so only the front proceeds).
    wake: Arc<Notify>,
}

fn lanes() -> &'static Mutex<HashMap<String, Lane>> {
    static LANES: OnceLock<Mutex<HashMap<String, Lane>>> = OnceLock::new();
    LANES.get_or_init(|| Mutex::new(HashMap::new()))
}

fn next_ticket() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

fn lock() -> crate::sync_primitives::MutexGuard<'static, HashMap<String, Lane>> {
    lanes().lock().unwrap_or_else(|e| e.into_inner())
}

/// Join the back of the session key's FIFO lane.
///
/// Returns `None` when the lane is at `max_per_session` (`REJECT_NEWEST`).
///
/// **Call this synchronously on the arrival path, before spawning the delivery
/// task.** Registering inside the spawned task made lane order depend on task
/// scheduling rather than arrival order, so two messages sent a millisecond
/// apart could enqueue inverted — defeating the FIFO guarantee this module
/// exists to provide.
#[must_use]
pub fn register(session_key: &str, max_per_session: usize, run_id: &str) -> Option<TicketGuard> {
    let mut map = lock();
    let lane = map.entry(session_key.to_string()).or_default();
    if lane.tickets.len() >= max_per_session {
        return None;
    }
    let ticket = next_ticket();
    lane.tickets.push_back(Ticket {
        id: ticket,
        run_id: run_id.to_string(),
        enqueued_at: Instant::now(),
        cancelled: false,
    });
    Some(TicketGuard {
        session_key: session_key.to_string(),
        ticket,
    })
}

/// Signal that `session_key`'s run slot is free, so its front waiter may try.
///
/// Called from the engine's `SessionRunRegistry::release` — the one place a
/// session's claim is actually given up. Cheap and lock-scoped: a session with
/// no lane does nothing.
pub fn notify_slot_free(session_key: &str) {
    let wake = {
        let map = lock();
        map.get(session_key).map(|lane| Arc::clone(&lane.wake))
    };
    if let Some(wake) = wake {
        wake.notify_waiters();
    }
}

/// Report that the engine has admitted the run `run_id` on `session_key`: it is
/// executing now, so it is no longer *waiting* and must stop holding the lane.
///
/// Called from the engine's `SessionRunRegistry::try_claim` — the one place a
/// session's run slot is actually taken, exactly mirroring [`notify_slot_free`]
/// on the release side. A run that never came through a lane (loop tick, goal
/// continuation, delegated child) matches no ticket and this is a no-op.
///
/// # Why the lane must let go here
///
/// The lane is a **waiting room**, not a run registry. `deliver_with_ticket`
/// holds its ticket across the whole `attempt()`, and `attempt()` *is* the agent
/// run — so without this the front ticket sat at the head of its lane for the
/// run's entire lifetime and every follow-up parked behind the very run it
/// wanted to change. That starved the two busy-input modes which only mean
/// anything while a sibling runs: `Steer` (inject into the live loop) and
/// `Interrupt` (cancel it). Both silently degraded to `Queue` — a mid-task
/// correction became a *separate* run after the first finished, and the Panel's
/// queue auto-drain (which documents its reliance on Steer coalescing) turned
/// one burst into N sequential runs.
///
/// Two smaller lies had the same root: `/stop` counted the message it was
/// stopping among the "queued messages dropped" it reports, and
/// `gateway.metrics.busy_queue.total_waiting` counted messages that had already
/// become runs — the opposite of that gauge's documented meaning.
pub fn mark_admitted(session_key: &str, run_id: &str) {
    let wake = {
        let mut map = lock();
        let Some(lane) = map.get_mut(session_key) else {
            return;
        };
        let before = lane.tickets.len();
        lane.tickets.retain(|t| t.run_id != run_id);
        if lane.tickets.len() == before {
            return;
        }
        let wake = Arc::clone(&lane.wake);
        if lane.tickets.is_empty() {
            map.remove(session_key);
        }
        wake
    };
    // Promote whoever was behind the admitted run so it can attempt right away
    // instead of waiting out the fallback tick.
    wake.notify_waiters();
}

/// When the still-waiting message that would become `run_id` joined
/// `session_key`'s lane, or `None` if it holds no ticket.
///
/// `None` covers three genuinely different callers and all three want the same
/// treatment — "no constraint from the lane": a run that never queued (loop
/// tick, goal continuation, delegated child, the OpenAI-compat surface), a run
/// whose ticket [`mark_admitted`] already withdrew, and a lane that has since
/// been garbage-collected.
///
/// # Why the admission gate needs this
///
/// `BusyInputMode::Interrupt` means "supersede the task that was running when I
/// sent this". Once [`mark_admitted`] let followers reach the engine mid-run
/// (Round-6), a *burst* of interrupt-mode messages read that as "supersede
/// whatever is running when my turn to ask comes round" — and what is running by
/// then is the run the message immediately ahead of me just became. Each queued
/// message killed its own predecessor milliseconds after the lane admitted it,
/// so an N-message burst left only the last one alive and N-1 turns of work
/// destroyed with nothing to show for them.
///
/// The correct predicate is temporal, and this is the half of it the lane owns:
/// a message may only cancel a run that was **already admitted when the message
/// began waiting**. Cheaper predicates are wrong in the same direction — "is the
/// target the run my own lane admitted most recently?" would also suppress a
/// genuine interrupt that arrives while a healthy sibling runs, which is the
/// entire reason the mode exists.
#[must_use]
pub fn waiting_since(session_key: &str, run_id: &str) -> Option<Instant> {
    let map = lock();
    map.get(session_key)?
        .tickets
        .iter()
        .find(|t| t.run_id == run_id)
        .map(|t| t.enqueued_at)
}

/// Abandon every message waiting on `session_key` and return how many were
/// dropped.
///
/// The explicit-stop counterpart to [`notify_slot_free`]: `/stop` means "I do
/// not want this work", and a queued backlog firing one message at a time right
/// after the stop is the opposite of that (codex clears pending input on
/// `Op::Interrupt` for the same reason).
///
/// **Only call this for an explicit user stop.** The `Interrupt` busy-input
/// mode *depends* on the lane to restart the interrupting message after it
/// cancels the running sibling, so it must not purge.
///
/// Scoped to messages that are genuinely still waiting: the message that
/// already became the session's live run left the lane at
/// [`mark_admitted`], so the count reported back to the user ("N queued
/// messages dropped") no longer includes the run the stop is cancelling.
pub fn purge(session_key: &str) -> usize {
    let (count, wake) = {
        let mut map = lock();
        match map.get_mut(session_key) {
            Some(lane) => {
                let mut count = 0;
                for ticket in &mut lane.tickets {
                    if !ticket.cancelled {
                        ticket.cancelled = true;
                        count += 1;
                    }
                }
                (count, Some(Arc::clone(&lane.wake)))
            }
            None => (0, None),
        }
    };
    if let Some(wake) = wake {
        wake.notify_waiters();
    }
    count
}

/// Abandon the single queued message that would have become `run_id`.
/// Returns `true` when a matching waiting ticket was found.
///
/// A message still in the lane has a `run_id` the client already knows (both
/// RPC handlers return it up front) but no entry in the engine's `active_runs`,
/// so `ExecutionEngine::cancel` cannot reach it — without this, a Panel user
/// facing a queued bubble had no way to stop it. Run-scoped counterpart to the
/// session-scoped [`purge`].
pub fn cancel_queued_run(run_id: &str) -> bool {
    let (found, wake) = {
        let mut map = lock();
        let hit = map.iter_mut().find_map(|(_, lane)| {
            lane.tickets
                .iter_mut()
                .find(|t| t.run_id == run_id && !t.cancelled)
                .map(|t| {
                    t.cancelled = true;
                    Arc::clone(&lane.wake)
                })
        });
        (hit.is_some(), hit)
    };
    if let Some(wake) = wake {
        wake.notify_waiters();
    }
    found
}

/// Total messages waiting across all sessions, plus the per-session breakdown
/// (deepest first). Feeds `gateway.metrics.run_concurrency` so an operator can
/// see backlog the same way they see run-slot saturation.
/// A cancelled ticket is not counted. [`purge`] / [`cancel_queued_run`] mark
/// rather than remove — the waiter owns its guard and drops it when it next
/// wakes — so between the stop and that wake the lane still holds tickets whose
/// messages have already been abandoned. Counting them reports a backlog the
/// user just cancelled, which is the same class of lie `mark_admitted` fixed on
/// the other side (a message that had already become a run counted as
/// waiting). `total_waiting` means what it says: messages that may still run.
#[must_use]
pub fn snapshot() -> BusyQueueSnapshot {
    let map = lock();
    let mut per_session: Vec<(String, usize)> = map
        .iter()
        .map(|(key, lane)| {
            (
                key.clone(),
                lane.tickets.iter().filter(|t| !t.cancelled).count(),
            )
        })
        .filter(|(_, depth)| *depth > 0)
        .collect();
    per_session.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    BusyQueueSnapshot {
        total_waiting: per_session.iter().map(|(_, n)| *n).sum(),
        per_session,
    }
}

/// Observability view of the wait lanes. See [`snapshot`].
#[derive(Debug, Clone, Default)]
pub struct BusyQueueSnapshot {
    pub total_waiting: usize,
    /// `(session_key, depth)`, deepest first; empty lanes omitted.
    pub per_session: Vec<(String, usize)>,
}

/// RAII lane membership: joins on [`register`], leaves on `Drop`.
///
/// The `Drop` impl is load-bearing — a panic anywhere while the ticket is held
/// (e.g. inside the execution adapter the waiter awaits) unwinds the task and
/// would otherwise leave a corpse ticket in the lane; once that corpse reached
/// the front, every ticket behind it saw `is_front == false` forever (the
/// fail-open clause only rescues tickets NOT in the lane) and the session's
/// delivery lane was wedged until daemon restart. The other half of this
/// delivery pipeline makes the same move for the session claim
/// (`execution_engine::gate::RunSlot`).
pub struct TicketGuard {
    session_key: String,
    ticket: u64,
}

impl TicketGuard {
    /// The raw ticket number, for log correlation only.
    #[must_use]
    pub fn id(&self) -> u64 {
        self.ticket
    }

    /// The session key this ticket waits on.
    #[must_use]
    pub fn session_key(&self) -> &str {
        &self.session_key
    }

    /// Whether this ticket is at the front of its lane and may attempt
    /// delivery.
    ///
    /// A lane or ticket that no longer exists fails **open** (`true`): the
    /// engine's busy gate is the real authority, so the worst case of a stale
    /// ticket is one redundant delivery attempt, never a stuck message.
    #[must_use]
    pub fn is_front(&self) -> bool {
        let map = lock();
        match map.get(&self.session_key) {
            Some(lane) => match lane.tickets.front() {
                Some(front) => {
                    if front.id == self.ticket {
                        true
                    } else if !lane.tickets.iter().any(|t| t.id == self.ticket) {
                        tracing::debug!(
                            session = %self.session_key,
                            ticket = self.ticket,
                            "stale ticket not in lane; failing open"
                        );
                        true
                    } else {
                        false
                    }
                }
                None => true,
            },
            None => {
                tracing::debug!(
                    session = %self.session_key,
                    ticket = self.ticket,
                    "session lane not found; failing open"
                );
                true
            }
        }
    }

    /// Whether an explicit stop abandoned this message while it waited —
    /// either a session-wide [`purge`] or a run-scoped [`cancel_queued_run`].
    /// A ticket that is no longer in its lane reads `false` (fail open, same
    /// posture as [`Self::is_front`]).
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        lock().get(&self.session_key).is_some_and(|lane| {
            lane.tickets
                .iter()
                .any(|t| t.id == self.ticket && t.cancelled)
        })
    }

    /// The wake handle to park on. Grabbed as an `Arc` so the waiter can await
    /// without holding the lane lock.
    pub(super) fn wake_handle(&self) -> Arc<Notify> {
        let mut map = lock();
        Arc::clone(&map.entry(self.session_key.clone()).or_default().wake)
    }
}

impl Drop for TicketGuard {
    fn drop(&mut self) {
        // Leaving the lane promotes whoever is behind us, so wake them. A
        // ticket already withdrawn by `mark_admitted` simply finds nothing to
        // remove. Removes the lane entirely once empty, so idle sessions leak
        // nothing (and a later arrival gets a fresh, uncancelled ticket).
        let wake = {
            let mut map = lock();
            let Some(lane) = map.get_mut(&self.session_key) else {
                return;
            };
            lane.tickets.retain(|t| t.id != self.ticket);
            let wake = Arc::clone(&lane.wake);
            if lane.tickets.is_empty() {
                map.remove(&self.session_key);
            }
            wake
        };
        wake.notify_waiters();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CAP: usize = 32;

    // Tests share one process-global lane map, so each test uses a unique
    // session key to stay isolated under the parallel test runner.

    /// The lane's half of the interrupt-burst rule. What the gate needs is not
    /// "is this message queued" but "since when" — and the answer has to
    /// survive the predecessor being admitted out from under it.
    #[test]
    fn waiting_since_reports_arrival_and_stops_reporting_once_admitted() {
        let key = "agent:since";
        assert_eq!(
            waiting_since(key, "run-1"),
            None,
            "no lane, no ticket, no constraint"
        );

        let first = register(key, CAP, "run-1").expect("lane accepts");
        let t1 = waiting_since(key, "run-1").expect("a queued message names when it arrived");

        let second = register(key, CAP, "run-2").expect("lane accepts");
        let t2 = waiting_since(key, "run-2").expect("so does the follower");
        assert!(t2 >= t1, "arrival order is monotonic on the lane");

        assert_eq!(
            waiting_since(key, "run-absent"),
            None,
            "a run that never queued is unconstrained, not blocked"
        );

        // Admission withdraws the ticket, so the run that is now EXECUTING no
        // longer reports a wait — which is what makes it a legitimate
        // interrupt target for anything that arrives from here on.
        mark_admitted(key, "run-1");
        assert_eq!(waiting_since(key, "run-1"), None);
        assert_eq!(
            waiting_since(key, "run-2"),
            Some(t2),
            "the follower's own arrival instant is unchanged by its \
             predecessor's admission — that is the whole point"
        );

        drop(first);
        drop(second);
    }

    #[test]
    fn tickets_deliver_in_fifo_order() {
        let s = "bq-test-fifo";
        let first = register(s, CAP, "r").unwrap();
        let second = register(s, CAP, "r").unwrap();
        let third = register(s, CAP, "r").unwrap();

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
        let s = "bq-test-mid-removal";
        let first = register(s, CAP, "r").unwrap();
        let second = register(s, CAP, "r").unwrap();
        let third = register(s, CAP, "r").unwrap();

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
        let s = "bq-test-overflow";
        let mut guards: Vec<TicketGuard> =
            (0..CAP).map(|_| register(s, CAP, "r").unwrap()).collect();
        assert!(
            register(s, CAP, "r").is_none(),
            "lane at cap must reject newest"
        );

        // Draining one slot re-admits new arrivals.
        guards.remove(0); // dropped
        let _readmitted = register(s, CAP, "r").expect("freed slot re-admits");
    }

    #[test]
    fn cap_is_the_configured_value_not_a_constant() {
        let s = "bq-test-configured-cap";
        let _a = register(s, 2, "r").unwrap();
        let _b = register(s, 2, "r").unwrap();
        assert!(
            register(s, 2, "r").is_none(),
            "an operator-lowered cap must actually bind"
        );
    }

    #[test]
    fn unknown_lane_and_stale_ticket_fail_open() {
        // No lane at all → deliver.
        let orphan = TicketGuard {
            session_key: "bq-test-no-lane".to_string(),
            ticket: 999,
        };
        assert!(orphan.is_front());
        std::mem::forget(orphan); // no lane to clean up

        // Lane exists but the ticket is not in it → deliver; the engine gate is
        // authoritative, a stale ticket must never wedge.
        let s = "bq-test-stale";
        let _held = register(s, CAP, "r").unwrap();
        let stale = TicketGuard {
            session_key: s.to_string(),
            ticket: 12_345_678,
        };
        assert!(stale.is_front());
    }

    #[test]
    fn empty_lane_is_garbage_collected() {
        let s = "bq-test-gc";
        let t = register(s, CAP, "r").unwrap();
        drop(t);
        assert!(
            !lock().contains_key(s),
            "empty lane must be removed from the map"
        );
    }

    #[test]
    fn panic_while_holding_ticket_releases_the_lane() {
        // The RAII guard's whole reason to exist: a front waiter that panics
        // while holding its ticket must not leave a corpse at the head of the
        // lane (a leaked front ticket blocked everyone behind it forever).
        let s = "bq-test-panic";
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _front = register(s, CAP, "r").unwrap();
            panic!("simulated adapter panic while holding the front ticket");
        }));
        // Had the front ticket leaked, this later arrival would sit behind the
        // corpse (`is_front == false`) until daemon restart.
        let waiting = register(s, CAP, "r").unwrap();
        assert!(waiting.is_front(), "corpse ticket must not wedge the lane");
    }

    #[test]
    fn distinct_session_keys_do_not_block_each_other() {
        // Two different sessions (same agent in production) get independent
        // lanes → both are immediately front (true cross-session parallelism).
        let s1 = "bq-test-agentX|conv-1";
        let s2 = "bq-test-agentX|conv-2";
        let t1 = register(s1, CAP, "r").unwrap();
        let t2 = register(s2, CAP, "r").unwrap();
        assert!(t1.is_front(), "session-1 lane is its own front");
        assert!(
            t2.is_front(),
            "session-2 lane is its own front — not blocked by session-1"
        );
    }

    #[test]
    fn purge_marks_every_waiter_and_reports_the_count() {
        let s = "bq-test-purge";
        let a = register(s, CAP, "r").unwrap();
        let b = register(s, CAP, "r").unwrap();
        assert!(!a.is_cancelled());

        assert_eq!(purge(s), 2, "purge reports how many were waiting");
        assert!(a.is_cancelled());
        assert!(b.is_cancelled());

        // A purge already applied is not re-counted, and a later arrival on the
        // same lane starts clean (the flag is per ticket, not per lane).
        assert_eq!(purge(s), 0, "already-cancelled tickets are not re-counted");
        let fresh = register(s, CAP, "r").unwrap();
        assert!(
            !fresh.is_cancelled(),
            "a message that arrived after the stop must not inherit it"
        );
    }

    #[test]
    fn purge_on_an_idle_session_is_a_no_op() {
        assert_eq!(purge("bq-test-purge-idle"), 0);
    }

    /// `/stop` reports "N queued messages dropped" to the user. The message the
    /// user is actually stopping already became a run, so counting it made that
    /// number a lie — and would have marked a live run's ticket cancelled.
    #[test]
    fn purge_ignores_a_run_the_engine_already_admitted() {
        let s = "bq-test-purge-admitted";
        let running = register(s, CAP, "run-live").unwrap();
        let waiting = register(s, CAP, "run-queued").unwrap();
        mark_admitted(s, "run-live");

        assert_eq!(purge(s), 1, "only the still-waiting message is dropped");
        assert!(waiting.is_cancelled());
        assert!(
            !running.is_cancelled(),
            "a message that already became a run is not a queued message"
        );
    }

    /// `gateway.metrics.busy_queue.total_waiting` is documented as the backlog
    /// that has **not** become a run yet (its sibling gauge
    /// `run_concurrency.waiting` covers runs blocked on the semaphore). Counting
    /// the admitted run here inflated every busy session by one.
    #[test]
    fn snapshot_excludes_a_run_the_engine_already_admitted() {
        let s = "bq-test-snap-admitted";
        let _running = register(s, CAP, "run-live").unwrap();
        let _waiting = register(s, CAP, "run-queued").unwrap();
        mark_admitted(s, "run-live");

        let snap = snapshot();
        let depth = snap
            .per_session
            .iter()
            .find(|(k, _)| k == s)
            .map(|(_, d)| *d);
        assert_eq!(depth, Some(1), "only the waiting message counts as backlog");
    }

    /// The other half of that gauge's honesty. A stop marks tickets rather than
    /// removing them — the waiter owns its guard — so between the purge and the
    /// waiter's next wake the lane still holds them. Reporting a backlog the
    /// user just cancelled is the same lie in the other direction.
    #[test]
    fn snapshot_excludes_messages_an_explicit_stop_already_abandoned() {
        let s = "bq-test-snap-purged";
        let _a = register(s, CAP, "run-a").unwrap();
        let _b = register(s, CAP, "run-b").unwrap();
        assert_eq!(purge(s), 2);

        let depth = snapshot()
            .per_session
            .iter()
            .find(|(k, _)| k == s)
            .map(|(_, d)| *d);
        assert_eq!(
            depth, None,
            "a fully purged lane is empty backlog, even before its waiters wake"
        );
    }

    #[test]
    fn mark_admitted_is_a_no_op_for_runs_that_never_queued() {
        // Loop ticks, goal continuations and delegated children reach
        // `try_claim` without ever taking a ticket.
        mark_admitted("bq-test-admit-unknown-session", "run-x");

        let s = "bq-test-admit-unknown-run";
        let held = register(s, CAP, "run-a").unwrap();
        mark_admitted(s, "some-other-run");
        assert!(
            held.is_front(),
            "an unrelated admission must not disturb us"
        );
        assert_eq!(purge(s), 1, "and must not silently drop the ticket");
    }

    #[test]
    fn a_lane_emptied_by_admission_is_garbage_collected() {
        let s = "bq-test-admit-gc";
        let held = register(s, CAP, "run-a").unwrap();
        mark_admitted(s, "run-a");
        assert!(
            !lock().contains_key(s),
            "withdrawing the last ticket must remove the lane, like Drop does"
        );
        // The guard is still alive for the run's lifetime; dropping it later
        // must stay a clean no-op.
        drop(held);
        assert!(!lock().contains_key(s));
    }

    #[test]
    fn cancel_queued_run_targets_exactly_one_ticket() {
        // Run ids are a PROCESS-GLOBAL namespace here: `cancel_queued_run`
        // scans every lane, not just this session's, so a test that reaches
        // across lanes must not reuse an id a concurrent test holds. Two of
        // `deliver.rs`'s tests used to hold live tickets literally named
        // "run-b" while this one cancelled by that name — flaky, and only
        // under enough parallel load to line the schedules up.
        let s = "bq-test-cancel-run";
        let a = register(s, CAP, "bq-cancel-a").unwrap();
        let b = register(s, CAP, "bq-cancel-b").unwrap();

        assert!(cancel_queued_run("bq-cancel-b"));
        assert!(b.is_cancelled());
        assert!(!a.is_cancelled(), "siblings must be untouched");

        // A run id that is not queued (already running, or finished) is a miss,
        // so the caller can fall through to the engine's own cancel path.
        assert!(!cancel_queued_run("bq-cancel-not-queued"));
        // Re-cancelling the same ticket is also a miss (already abandoned).
        assert!(!cancel_queued_run("bq-cancel-b"));
    }

    #[test]
    fn snapshot_reports_depth_deepest_first() {
        let shallow = "bq-test-snap|a";
        let deep = "bq-test-snap|b";
        let _s1 = register(shallow, CAP, "r").unwrap();
        let _d1 = register(deep, CAP, "r").unwrap();
        let _d2 = register(deep, CAP, "r").unwrap();

        let snap = snapshot();
        let seen: Vec<_> = snap
            .per_session
            .iter()
            .filter(|(k, _)| k.starts_with("bq-test-snap|"))
            .cloned()
            .collect();
        assert_eq!(
            seen,
            vec![(deep.to_string(), 2), (shallow.to_string(), 1)],
            "deepest lane first"
        );
        assert!(snap.total_waiting >= 3);
    }

    #[tokio::test]
    async fn waiter_is_woken_by_slot_free_without_polling() {
        // The point of the Notify: a waiter parks and is released by the
        // engine's slot-free edge, not by a timer.
        let s = "bq-test-notify";
        let front = register(s, CAP, "r").unwrap();
        let behind = register(s, CAP, "r").unwrap();
        assert!(!behind.is_front());

        let wake = behind.wake_handle();
        let notified = wake.notified();
        tokio::pin!(notified);
        // Arm the waiter before signalling (Notify only releases armed waiters).
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(10), &mut notified)
                .await
                .is_err(),
            "nothing has happened yet"
        );

        drop(front); // promotes `behind` and wakes the lane
        tokio::time::timeout(std::time::Duration::from_millis(500), &mut notified)
            .await
            .expect("ticket departure must wake the lane");
        assert!(behind.is_front());
    }

    #[tokio::test]
    async fn notify_slot_free_wakes_the_front_waiter() {
        let s = "bq-test-notify-slot";
        let front = register(s, CAP, "r").unwrap();
        let wake = front.wake_handle();
        let notified = wake.notified();
        tokio::pin!(notified);
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(10), &mut notified)
                .await
                .is_err()
        );

        notify_slot_free(s);
        tokio::time::timeout(std::time::Duration::from_millis(500), &mut notified)
            .await
            .expect("slot-free signal must wake the lane");
    }

    #[test]
    fn notify_slot_free_on_an_unknown_session_is_a_no_op() {
        notify_slot_free("bq-test-notify-unknown");
    }
}
