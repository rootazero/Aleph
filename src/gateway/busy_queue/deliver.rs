//! The shared "wait for the session's run slot, then deliver" loop.
//!
//! Extracted from the inbound executor so the Panel / CLI RPC path
//! (`agent.run`, `chat.send`) uses the very same lane instead of hard-failing
//! on `AgentBusy`. Both surfaces then share arrival ordering, the overflow
//! policy, the wait bound, and the purge semantics — there is one queue, not
//! one per surface.

use std::future::Future;

use tokio::time::{timeout_at, Duration, Instant};

use super::{BusyQueueConfig, TicketGuard};
use crate::gateway::execution_engine::ExecutionError;

/// How a queued message finished.
///
/// The split between [`Executed`](DeliveryOutcome::Executed) and the three
/// never-ran outcomes is what stops **double error reporting**: once the
/// attempt runs, the run's own emitter has already delivered a `RunError` to
/// the user's surface, so the caller must not send a second receipt. Only the
/// queue-stage outcomes are the caller's to report.
#[derive(Debug)]
pub enum DeliveryOutcome {
    /// The attempt ran. Any error inside was already surfaced by the run's
    /// emitter (`StreamEvent::RunError`) — log it, do not re-report it.
    Executed(Result<(), ExecutionError>),
    /// Never ran: the lane was already at `max_per_session` (`REJECT_NEWEST`).
    Rejected,
    /// Never ran: waited out `max_wait_secs`.
    TimedOut,
    /// Never ran: an explicit stop abandoned it while it waited — `/stop`
    /// purging the session's lane, or a run-scoped abort reaching the queued
    /// message by its `run_id`.
    Purged,
}

impl DeliveryOutcome {
    /// The receipt the caller owes the user, if any.
    ///
    /// `None` for [`Executed`](Self::Executed) (already reported) and for
    /// [`Purged`](Self::Purged) (the stop handler reports the whole batch at
    /// once, rather than N separate "cancelled" messages).
    ///
    /// A surface that keeps per-run UI state — the Panel, whose bubble is keyed
    /// to the `run_id` and which nothing else will close — additionally emits a
    /// terminal `Cancelled` frame on [`Purged`](Self::Purged); see
    /// `server_init::spawn_queued_run`. Chat channels have no such state and
    /// stay silent.
    #[must_use]
    pub fn user_error(&self, agent_id: &str) -> Option<ExecutionError> {
        match self {
            Self::Rejected | Self::TimedOut => {
                Some(ExecutionError::AgentBusy(agent_id.to_string()))
            }
            Self::Executed(_) | Self::Purged => None,
        }
    }
}

/// Run `attempt` once the session's run slot is free (or once this ticket
/// reaches the front of its lane and the engine accepts it).
///
/// `attempt` is retried only while it reports [`ExecutionError::AgentBusy`] —
/// every other outcome is terminal. Between attempts the waiter parks on the
/// lane's wake signal (fired by the engine's slot release, by ticket
/// departures, and — for a steer refused because the running loop's burst is at
/// `max_pending_steering` — by that burst draining), with `wake_fallback_secs`
/// as a safety net; it never polls on a short timer. A drain that raced the
/// backpressure mark emits no signal to catch, so the waiter also compares the
/// lane's drain epoch across its attempt.
///
/// Takes an already-issued [`TicketGuard`] rather than registering internally,
/// because the ticket must be taken **synchronously on the arrival path**
/// (before the delivery task is spawned) for lane order to follow arrival order
/// rather than task-scheduling order. Callers handle
/// [`register`](super::register) returning `None` as
/// [`DeliveryOutcome::Rejected`].
///
/// The guard is RAII: dropped on every exit path — including a panic inside
/// `attempt` — so a dead waiter can never leave a corpse ticket wedging the
/// lane.
///
/// # Reporting position
///
/// `report` is called with `ahead` **only when it changes**, from the point
/// just before this loop parks. That point — not the "not front" branch — is
/// deliberate: a front ticket refused for steering backpressure is waiting
/// too, and it is the one wait that had no representation anywhere.
///
/// It is `async` and awaited inline rather than a sync callback that spawns:
/// `ahead` decreases monotonically, and a spawn abandons ordering, so two
/// rapid updates could land inverted and flicker the number a user is
/// reading. Awaiting inline is in-order by construction and mirrors how
/// `attempt` is already threaded through.
///
/// An idle session never reaches the report point: it is front, `attempt`
/// does not refuse, and the function returns first. Conversations that never
/// wait therefore gain no frames at all.
pub async fn deliver_with_ticket<F, Fut, R, RFut>(
    ticket: TicketGuard,
    cfg: BusyQueueConfig,
    attempt: &mut F,
    report: &mut R,
) -> DeliveryOutcome
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<(), ExecutionError>>,
    R: FnMut(u16) -> RFut,
    RFut: Future<Output = ()>,
{
    let deadline = Instant::now() + Duration::from_secs(cfg.max_wait_secs);
    let fallback = Duration::from_secs(cfg.wake_fallback_secs);
    let mut announced_busy = false;
    let mut last_reported: Option<u16> = None;

    loop {
        // Arm the wake BEFORE inspecting lane state. `Notified` registers on
        // `enable()`, so a slot-free signal that lands while we are checking
        // (or mid-`attempt`) is still delivered to this already-registered
        // waiter. Arming afterwards would drop such a signal into the gap and
        // leave the message parked until the fallback tick.
        let wake = ticket.wake_handle();
        let notified = wake.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();

        // Snapshot the burst-drain level for the same reason the wake is armed
        // here, but for the signal arming cannot cover: a drain that commits
        // between the steering check and its mark emits no wake at all (see
        // `mark_awaiting_burst_drain`), so there is no edge to have caught —
        // only this counter moving.
        let drain_epoch = ticket.drain_epoch();

        if ticket.is_cancelled() {
            return DeliveryOutcome::Purged;
        }

        // FIFO gate: only the front ticket attempts delivery; the rest park
        // behind it so arrival order is preserved when the slot frees.
        if ticket.is_front() {
            match attempt().await {
                Err(ExecutionError::AgentBusy(_)) => {
                    if !announced_busy {
                        announced_busy = true;
                        tracing::info!(
                            session = %ticket.session_key(),
                            ticket = ticket.id(),
                            "session busy; message queued for FIFO delivery"
                        );
                    }
                    // This attempt was refused for steering backpressure and a
                    // drain landed while it ran: the wake for it was suppressed
                    // (the lane was still unmarked when it fired), so parking
                    // would sleep through the exact edge being waited on. Retry
                    // instead. Only a deferred attempt asks — a `Queue`-mode
                    // waiter is never marked and keeps its slot-free cadence.
                    if ticket.awaits_burst_drain() && ticket.drain_epoch() != drain_epoch {
                        continue;
                    }
                }
                other => return DeliveryOutcome::Executed(other),
            }
        }

        // Still waiting — either behind someone, or front and refused for
        // backpressure. Both are waits; only the number changing is news.
        let ahead = ticket.ahead();
        if last_reported != Some(ahead) {
            last_reported = Some(ahead);
            report(ahead).await;
        }

        // Park until the lane says something changed. The fallback tick only
        // bounds the damage of a signal we could never have observed (e.g. a
        // release that raced a lane the guard had not yet created) — the lane
        // may lag, it can never wedge.
        let wake_by = deadline.min(Instant::now() + fallback);
        if timeout_at(wake_by, notified).await.is_err() && Instant::now() >= deadline {
            return DeliveryOutcome::TimedOut;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::busy_queue::{
        mark_admitted, mark_awaiting_burst_drain, notify_burst_drained, notify_slot_free, purge,
        register,
    };
    use crate::sync_primitives::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn cfg(max_per_session: usize) -> BusyQueueConfig {
        BusyQueueConfig {
            max_per_session,
            max_wait_secs: 30,
            // Long enough that any test that passes did so on a real wake
            // signal, not on the fallback tick.
            wake_fallback_secs: 3600,
        }
    }

    /// Mirrors the production shape exactly: take the ticket up front (that is
    /// what makes lane order arrival order), then hand it to the wait loop.
    async fn deliver<F, Fut>(
        session_key: &str,
        run_id: &str,
        cfg: BusyQueueConfig,
        mut attempt: F,
    ) -> DeliveryOutcome
    where
        F: FnMut() -> Fut,
        Fut: Future<Output = Result<(), ExecutionError>>,
    {
        match register(session_key, cfg.max_per_session, run_id) {
            None => DeliveryOutcome::Rejected,
            Some(ticket) => {
                let mut noop = |_: u16| async {};
                deliver_with_ticket(ticket, cfg, &mut attempt, &mut noop).await
            }
        }
    }

    #[tokio::test]
    async fn an_idle_session_delivers_on_the_first_attempt() {
        let outcome = deliver("bqd-idle", "run-idle", cfg(4), || async { Ok(()) }).await;
        assert!(matches!(outcome, DeliveryOutcome::Executed(Ok(()))));
    }

    /// The lane is a **waiting room**, not a run registry.
    ///
    /// `attempt()` is the whole agent run, so the front ticket is held for the
    /// run's entire lifetime. Until [`mark_admitted`] existed, that meant a
    /// follow-up message parked behind the very run it wanted to change and
    /// never reached the engine at all — starving the two busy-input modes that
    /// only make sense *while* a sibling runs (`Steer` injects into the live
    /// loop, `Interrupt` cancels it). Both silently degraded to `Queue`.
    ///
    /// Here the first attempt reports admission (exactly as
    /// `SessionRunRegistry::try_claim` does in production) and then blocks, and
    /// the follow-up must still get its own delivery attempt.
    #[tokio::test]
    async fn an_admitted_run_stops_blocking_its_lane() {
        let s = "bqd-admitted-releases-lane";
        let (release_a, hold_a) = tokio::sync::oneshot::channel::<()>();
        let a_admitted = Arc::new(AtomicUsize::new(0));

        let hold = Arc::new(tokio::sync::Mutex::new(Some(hold_a)));
        let admitted = Arc::clone(&a_admitted);
        let task_a = tokio::spawn(async move {
            deliver(s, "run-a", cfg(4), move || {
                let admitted = Arc::clone(&admitted);
                let hold = Arc::clone(&hold);
                async move {
                    mark_admitted(s, "run-a");
                    admitted.fetch_add(1, Ordering::SeqCst);
                    // Stand in for the run's execution: the ticket is still
                    // held here, which is the whole point.
                    if let Some(rx) = hold.lock().await.take() {
                        let _ = rx.await;
                    }
                    Ok(())
                }
            })
            .await
        });

        while a_admitted.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }

        // The follow-up arrives mid-run. It must reach its attempt (in
        // production: `admit_run`, which then steers or interrupts).
        let b_attempts = Arc::new(AtomicUsize::new(0));
        let ba = Arc::clone(&b_attempts);
        let task_b = tokio::spawn(async move {
            deliver(s, "run-b", cfg(4), move || {
                let ba = Arc::clone(&ba);
                async move {
                    ba.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                }
            })
            .await
        });

        let outcome_b = tokio::time::timeout(std::time::Duration::from_secs(5), task_b)
            .await
            .expect("the follow-up must reach the engine while the first run is still in flight")
            .expect("delivery task panicked");
        assert!(matches!(outcome_b, DeliveryOutcome::Executed(Ok(()))));
        assert_eq!(b_attempts.load(Ordering::SeqCst), 1);

        let _ = release_a.send(());
        let _ = task_a.await;
    }

    /// FIFO among *waiting* messages is what admission release must not cost.
    /// Two follow-ups arriving behind an already-admitted run still deliver in
    /// arrival order, one at a time.
    #[tokio::test]
    async fn waiting_followups_keep_arrival_order_after_admission() {
        let s = "bqd-admitted-fifo";
        let front = register(s, 8, "run-a").expect("lane accepts the first message");
        mark_admitted(s, "run-a"); // the engine took the slot for run-a

        let b = register(s, 8, "run-b").expect("lane accepts the follow-up");
        let c = register(s, 8, "run-c").expect("lane accepts the second follow-up");
        assert!(
            b.is_front(),
            "with the admitted run out of the way, the first follow-up leads"
        );
        assert!(!c.is_front(), "the second follow-up still waits its turn");

        drop(b);
        assert!(c.is_front(), "departure promotes the next in arrival order");
        drop(front); // the admitted run finishing must not disturb the lane
        assert!(c.is_front());
    }

    #[tokio::test]
    async fn a_full_lane_rejects_without_attempting() {
        let s = "bqd-full";
        let _held: Vec<_> = (0..2)
            .map(|i| register(s, 2, &format!("held-{i}")).unwrap())
            .collect();
        let attempts = Arc::new(AtomicUsize::new(0));
        let a = Arc::clone(&attempts);
        let outcome = deliver(s, "run-full", cfg(2), move || {
            let a = Arc::clone(&a);
            async move {
                a.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }
        })
        .await;
        assert!(matches!(outcome, DeliveryOutcome::Rejected));
        assert_eq!(attempts.load(Ordering::SeqCst), 0, "must not run at all");
    }

    /// The whole point of the wake model: a busy attempt parks and is released
    /// by the engine's slot-free signal, not by a timer. `wake_fallback_secs`
    /// is an hour here, so a pass proves the signal did the work.
    #[tokio::test]
    async fn a_busy_attempt_retries_when_the_slot_frees() {
        let s = "bqd-wake";
        let attempts = Arc::new(AtomicUsize::new(0));
        let a = Arc::clone(&attempts);
        let task = tokio::spawn(async move {
            deliver(s, "run-wake", cfg(4), move || {
                let a = Arc::clone(&a);
                async move {
                    // First attempt: busy. Second: succeeds.
                    if a.fetch_add(1, Ordering::SeqCst) == 0 {
                        Err(ExecutionError::AgentBusy("agent".into()))
                    } else {
                        Ok(())
                    }
                }
            })
            .await
        });

        // Let the waiter make its first (busy) attempt and park.
        while attempts.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
        notify_slot_free(s);

        let outcome = tokio::time::timeout(std::time::Duration::from_secs(5), task)
            .await
            .expect("the slot-free signal must wake the waiter")
            .expect("delivery task panicked");
        assert!(matches!(outcome, DeliveryOutcome::Executed(Ok(()))));
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
    }

    /// The check-then-mark window (`mark_awaiting_burst_drain`'s own caveat).
    /// `try_inject_steering` reads the burst, finds it at the cap, and only
    /// *then* marks the ticket — so a drain committing in between fires
    /// `notify_burst_drained` against a lane whose ticket is not yet marked,
    /// the lane filter suppresses it, and **no wake is emitted at all**.
    /// Arming the `Notify` early cannot rescue a signal that never fired,
    /// which is why the waiter also snapshots the lane's drain epoch.
    ///
    /// `wake_fallback_secs` is an hour and nothing else signals this lane, so
    /// a pass proves the deferred steer noticed the drain it raced rather than
    /// sleeping through it and degrading into a separate run.
    #[tokio::test]
    async fn a_drain_that_races_the_mark_does_not_park_the_deferred_steer() {
        let s = "bqd-drain-race";
        let attempts = Arc::new(AtomicUsize::new(0));
        let a = Arc::clone(&attempts);
        let outcome = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            deliver(s, "run-drain-race", cfg(4), move || {
                let a = Arc::clone(&a);
                async move {
                    if a.fetch_add(1, Ordering::SeqCst) == 0 {
                        // Production order, verbatim: the drain commits while
                        // the lane is still unmarked, the mark lands after it.
                        notify_burst_drained(s);
                        mark_awaiting_burst_drain(s, "run-drain-race");
                        Err(ExecutionError::AgentBusy("agent".into()))
                    } else {
                        Ok(())
                    }
                }
            }),
        )
        .await
        .expect("a deferred steer must not park on the drain it raced");
        assert!(matches!(outcome, DeliveryOutcome::Executed(Ok(()))));
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
    }

    /// A non-busy error is terminal: the lane must not retry a genuine failure.
    #[tokio::test]
    async fn a_real_failure_is_terminal() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let a = Arc::clone(&attempts);
        let outcome = deliver("bqd-fail", "run-fail", cfg(4), move || {
            let a = Arc::clone(&a);
            async move {
                a.fetch_add(1, Ordering::SeqCst);
                Err(ExecutionError::Failed("boom".into()))
            }
        })
        .await;
        assert!(matches!(
            outcome,
            DeliveryOutcome::Executed(Err(ExecutionError::Failed(_)))
        ));
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn a_purged_lane_abandons_the_waiter() {
        let s = "bqd-purge";
        let attempts = Arc::new(AtomicUsize::new(0));
        let a = Arc::clone(&attempts);
        let task = tokio::spawn(async move {
            deliver(s, "run-purge", cfg(4), move || {
                let a = Arc::clone(&a);
                async move {
                    a.fetch_add(1, Ordering::SeqCst);
                    Err(ExecutionError::AgentBusy("agent".into()))
                }
            })
            .await
        });

        while attempts.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
        assert_eq!(purge(s), 1);

        let outcome = tokio::time::timeout(std::time::Duration::from_secs(5), task)
            .await
            .expect("purge must wake the waiter")
            .expect("delivery task panicked");
        assert!(matches!(outcome, DeliveryOutcome::Purged));
    }

    /// A message still in the lane is not in the engine's `active_runs`, so a
    /// run-scoped abort can only reach it here. Without this the Panel showed a
    /// queued bubble it had no way to stop.
    #[tokio::test]
    async fn a_queued_run_can_be_aborted_by_its_run_id() {
        let s = "bqd-abort-by-run";
        let attempts = Arc::new(AtomicUsize::new(0));
        let a = Arc::clone(&attempts);
        let task = tokio::spawn(async move {
            deliver(s, "run-abort-me", cfg(4), move || {
                let a = Arc::clone(&a);
                async move {
                    a.fetch_add(1, Ordering::SeqCst);
                    Err(ExecutionError::AgentBusy("agent".into()))
                }
            })
            .await
        });

        while attempts.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
        assert!(crate::gateway::busy_queue::cancel_queued_run(
            "run-abort-me"
        ));

        let outcome = tokio::time::timeout(std::time::Duration::from_secs(5), task)
            .await
            .expect("the run-scoped abort must wake the waiter")
            .expect("delivery task panicked");
        assert!(matches!(outcome, DeliveryOutcome::Purged));
    }

    /// Only queue-stage outcomes are the caller's to report — an attempt that
    /// ran already surfaced its own `RunError`, so reporting again would show
    /// the user two messages for one failure.
    #[test]
    fn only_never_ran_outcomes_owe_the_user_a_receipt() {
        assert!(DeliveryOutcome::Rejected.user_error("a").is_some());
        assert!(DeliveryOutcome::TimedOut.user_error("a").is_some());
        assert!(DeliveryOutcome::Purged.user_error("a").is_none());
        assert!(DeliveryOutcome::Executed(Ok(())).user_error("a").is_none());
        assert!(
            DeliveryOutcome::Executed(Err(ExecutionError::Failed("x".into())))
                .user_error("a")
                .is_none(),
            "the engine already emitted RunError for this failure"
        );
    }

    #[tokio::test]
    async fn waiting_out_the_window_times_out() {
        let s = "bqd-timeout";
        let cfg = BusyQueueConfig {
            max_per_session: 4,
            max_wait_secs: 0,
            wake_fallback_secs: 1,
        };
        let outcome = deliver(s, "run-timeout", cfg, || async {
            Err(ExecutionError::AgentBusy("agent".into()))
        })
        .await;
        assert!(matches!(outcome, DeliveryOutcome::TimedOut));
    }

    /// Mirrors [`deliver`], but threads the position reporter the wait loop
    /// hands `RunQueued` through. A separate helper so the tests above stay
    /// exactly as they were.
    async fn deliver_reporting<F, Fut>(
        session_key: &str,
        run_id: &str,
        cfg: BusyQueueConfig,
        mut attempt: F,
        sink: Arc<tokio::sync::Mutex<Vec<u16>>>,
    ) -> DeliveryOutcome
    where
        F: FnMut() -> Fut,
        Fut: Future<Output = Result<(), ExecutionError>>,
    {
        let mut report = move |ahead: u16| {
            let sink = Arc::clone(&sink);
            async move {
                sink.lock().await.push(ahead);
            }
        };
        match register(session_key, cfg.max_per_session, run_id) {
            None => DeliveryOutcome::Rejected,
            Some(ticket) => deliver_with_ticket(ticket, cfg, &mut attempt, &mut report).await,
        }
    }

    /// The common path must stay silent: an idle session takes the front of
    /// the lane and runs, so a conversation that never waited gains no new
    /// frames at all. This is the ceiling on this feature's regression risk,
    /// so it is asserted directly rather than inferred.
    #[tokio::test]
    async fn an_idle_session_reports_no_position_at_all() {
        let sink = Arc::new(tokio::sync::Mutex::new(Vec::<u16>::new()));
        let outcome = deliver_reporting(
            "bqd-report-idle",
            "run-idle",
            cfg(4),
            || async { Ok(()) },
            Arc::clone(&sink),
        )
        .await;

        assert!(matches!(outcome, DeliveryOutcome::Executed(Ok(()))));
        assert!(
            sink.lock().await.is_empty(),
            "a conversation that never waited must gain no RunQueued frames"
        );
    }

    /// A front ticket refused for steering backpressure IS waiting, and it is
    /// the one wait with no representation anywhere before this. Reporting
    /// only from the "not front" branch would miss it entirely.
    #[tokio::test]
    async fn a_front_ticket_deferred_for_backpressure_still_reports() {
        let s = "bqd-report-deferred";
        let sink = Arc::new(tokio::sync::Mutex::new(Vec::<u16>::new()));
        let attempts = Arc::new(AtomicUsize::new(0));

        let waker = {
            let attempts = Arc::clone(&attempts);
            tokio::spawn(async move {
                while attempts.load(Ordering::SeqCst) == 0 {
                    tokio::task::yield_now().await;
                }
                // Let the loop reach its park, then wake it for the retry.
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                notify_slot_free(s);
            })
        };

        let seen = Arc::clone(&attempts);
        let outcome = deliver_reporting(
            s,
            "run-a",
            cfg(4),
            move || {
                let seen = Arc::clone(&seen);
                async move {
                    // Refuse the first attempt (backpressure), pass the retry.
                    if seen.fetch_add(1, Ordering::SeqCst) == 0 {
                        Err(ExecutionError::AgentBusy("a".to_string()))
                    } else {
                        Ok(())
                    }
                }
            },
            Arc::clone(&sink),
        )
        .await;
        waker.await.expect("waker task");

        assert!(matches!(outcome, DeliveryOutcome::Executed(Ok(()))));
        assert_eq!(
            *sink.lock().await,
            vec![0u16],
            "front-but-deferred reports ahead=0 — nobody ahead, but not started"
        );
    }

    /// Position is news only when it changes. The fallback tick re-runs this
    /// loop on a cadence, so re-sending an unchanged number would turn a
    /// bounded signal into a per-session heartbeat.
    #[tokio::test]
    async fn an_unchanged_position_is_not_reported_twice() {
        // Process-global run-id namespace — see the comment on
        // `cancel_queued_run_targets_exactly_one_ticket` in mod.rs.
        let s = "bqd-report-dedup";
        let front = register(s, 8, "bqd-dedup-front").expect("lane accepts the first message");
        let sink = Arc::new(tokio::sync::Mutex::new(Vec::<u16>::new()));

        let waker = tokio::spawn(async move {
            // Two spurious wakes with no lane movement between them.
            for _ in 0..2 {
                tokio::time::sleep(std::time::Duration::from_millis(40)).await;
                notify_slot_free(s);
            }
            tokio::time::sleep(std::time::Duration::from_millis(40)).await;
            // Now the position really changes — and the waiter becomes front,
            // so it runs instead of reporting 0.
            drop(front);
        });

        let outcome = deliver_reporting(
            s,
            "bqd-dedup-b",
            cfg(8),
            || async { Ok(()) },
            Arc::clone(&sink),
        )
        .await;
        waker.await.expect("waker task");

        assert!(matches!(outcome, DeliveryOutcome::Executed(Ok(()))));
        assert_eq!(
            *sink.lock().await,
            vec![1u16],
            "ahead=1 is news once; repeating it on every wake makes this a heartbeat"
        );
    }
}
