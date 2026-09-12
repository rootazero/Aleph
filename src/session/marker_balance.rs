//! Keep the run markers balanced when a *writer* shortens the log.
//!
//! [`crate::session::reduction`] reads the **live** log: a retired row is gone
//! from every read path, markers included. So a `chat.rewind` / `session.
//! truncate` that cuts away a `RunFinished` while leaving its `RunStarted`
//! behind does not produce a corrupt log — it produces a log that *says* a run
//! is still open. Nothing in the reducer can tell that apart from a crash, and
//! nothing should: the sentence is now true, and the writer is the only party
//! that knows it made it true.
//!
//! The cost of leaving it is not theoretical. The boot scan classifies that
//! session `Interrupted` on **every** later boot, appends a crash-boundary
//! repair, and re-triggers a run the user deleted — forever, because nothing
//! ever closes the marker.
//!
//! So the writer closes it, here, in one place shared by both verbs — and in
//! the SAME store transaction as the retire (§4.1). A closer appended as a
//! second step leaves a window in which a crash produces exactly the state
//! this module exists to prevent, and the reducer cannot tell that half-done
//! operation from a finished one.

use crate::session::events::{
    now_ms, EventSeq, Retire, RunOutcome, SessionEvent, SessionEventRecord,
};
use crate::session::reduction::reduce_run;
use crate::session::service::{SessionError, SessionId, SessionService};

/// What [`retire_from_and_close_run`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetireOutcome {
    /// Live events at or after `from_seq` when the call began — what the
    /// retire took out of the live log.
    pub retired: usize,
    /// The `run_id` closed in the same transaction, or `None` when there was
    /// nothing to close (the surviving tail already ends in a `RunFinished`,
    /// no `RunStarted` survives, or a run is in flight).
    pub closed_run: Option<String>,
}

/// The BUILDER: which run would `Retire::From(from_seq)` leave open?
///
/// Pure — it reads the log it is handed and decides; the caller commits.
/// Reduces the prefix that SURVIVES the retire (`seq < from_seq`), because
/// that is the log every later reader will see: a `RunStarted` at or after
/// the cut is gone with it and needs no closer.
///
/// # Errors
///
/// A log the reducer refuses is an `Err`, not `None`: "I cannot read this
/// slice" may not be read as "no run is open", because a closer chosen from
/// an unreadable slice would name the wrong `run_id` (criterion #8).
pub fn open_run_after_retire(
    events: &[SessionEventRecord],
    from_seq: EventSeq,
) -> Result<Option<String>, SessionError> {
    let surviving: Vec<SessionEventRecord> = events
        .iter()
        .filter(|r| r.seq < from_seq)
        .cloned()
        .collect();
    let reduction = reduce_run(&surviving).map_err(|c| SessionError::Other(c.to_string()))?;
    Ok(reduction.open_run.map(|o| o.run_id))
}

/// Retire every live event with `seq >= from_seq` and, in the SAME store
/// transaction, close the run that retire would leave open.
///
/// The closer is `RunFinished { outcome: Cancelled }`: the user cut this run
/// out of their own transcript deliberately. `Abandoned` is the resume
/// coordinator's word for "recovery gave up", and reusing it here would make a
/// user edit read as a failed recovery in every counter that separates the
/// two.
///
/// `is_running` answers "does this session have a turn in flight right now".
/// It is a parameter and not a lookup because the only authority on that is
/// the engine's per-session run registry, which lives above this layer — and
/// because the predicate is the half of this rule that a mutation can silence.
///
/// **`is_running` must fail closed.** A caller that cannot tell (no run
/// registry wired) has to answer `true`: "I do not know whether a run is live"
/// may not be read as "no run is live", because closing the marker of a run
/// that is still executing writes a `RunFinished` into the middle of it, and
/// the real finish then lands as a [`crate::session::reduction::
/// LogContradiction::FinishWithoutStart`] on a session that is now permanently
/// mis-read. A running session is therefore retired but NOT closed — the
/// batch is retire-only.
///
/// # Errors
///
/// The store could not be read, or the batch did not commit — in which case
/// nothing was retired and nothing was appended. A log the reducer refuses is
/// an `Err` BEFORE anything is retired.
pub async fn retire_from_and_close_run(
    service: &dyn SessionService,
    session: &SessionId,
    from_seq: EventSeq,
    is_running: impl Fn(&SessionId) -> bool,
) -> Result<RetireOutcome, SessionError> {
    let events = service.get_events(session, None, None).await?;
    let retired = events.iter().filter(|r| r.seq >= from_seq).count();
    let closed_run = if is_running(session) {
        None
    } else {
        open_run_after_retire(&events, from_seq)?
    };
    let closer: Vec<SessionEvent> = closed_run
        .iter()
        .map(|run_id| SessionEvent::RunFinished {
            run_id: run_id.clone(),
            outcome: RunOutcome::Cancelled,
            at: now_ms(),
        })
        .collect();
    // Retire and closer commit together or not at all. A retire-only batch
    // (no closer) is legal: it appends nothing and allocates no seq.
    service
        .emit_batch(session, closer, Some(Retire::From(from_seq)))
        .await?;
    Ok(RetireOutcome {
        retired,
        closed_run,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routing::session_key::SessionKey;
    use crate::session::in_process::InProcessActorSessionService;
    use crate::session::reduction::RunDisposition;
    use crate::session::store::test_support::CountingStore;
    use crate::session::store::SessionEventStore;
    use std::sync::atomic::Ordering;
    use std::sync::Arc;

    fn store() -> Arc<CountingStore> {
        CountingStore::in_memory()
    }

    fn run_started(at: i64) -> SessionEvent {
        SessionEvent::RunStarted {
            run_id: format!("run-{at}"),
            at,
            project_root: None,
            envelope: None,
        }
    }

    fn run_finished(at: i64) -> SessionEvent {
        SessionEvent::RunFinished {
            run_id: format!("run-{at}"),
            outcome: RunOutcome::Completed,
            at,
        }
    }

    fn rec(seq: EventSeq, event: SessionEvent) -> SessionEventRecord {
        SessionEventRecord {
            seq,
            event,
            created_at_ms: 10 * seq as i64,
        }
    }

    async fn seed(store: &Arc<CountingStore>, sid: &SessionId, evs: &[SessionEvent]) {
        for (i, ev) in evs.iter().enumerate() {
            store
                .append(sid, (i + 1) as EventSeq, ev, 10 * (i as i64 + 1))
                .await
                .expect("append");
        }
    }

    async fn disposition(store: &Arc<CountingStore>, sid: &SessionId) -> RunDisposition {
        let events = store.load_all_events(sid).await.expect("load");
        reduce_run(&events).expect("legal log").disposition
    }

    async fn live_seqs(store: &Arc<CountingStore>, sid: &SessionId) -> Vec<EventSeq> {
        store
            .load_all_events(sid)
            .await
            .expect("load")
            .into_iter()
            .map(|r| r.seq)
            .collect()
    }

    #[tokio::test]
    async fn a_rewind_that_cuts_the_finish_closes_the_run_in_the_same_call() {
        let store = store();
        let svc = InProcessActorSessionService::new(store.clone());
        let sid: SessionId = SessionKey::ephemeral("balance-rewind");
        seed(&store, &sid, &[run_started(10), run_finished(20)]).await;
        let n = store.append_batches.load(Ordering::SeqCst);

        let out = retire_from_and_close_run(&svc, &sid, 2, |_| false)
            .await
            .expect("rewind");

        assert_eq!(
            store.append_batches.load(Ordering::SeqCst) - n,
            1,
            "retire + closer = ONE call"
        );
        assert_eq!(
            store.retire_froms.load(Ordering::SeqCst),
            0,
            "no window between retire and closer"
        );
        assert_eq!(
            (out.retired, out.closed_run.as_deref()),
            (1, Some("run-10")),
            "closes THAT run's id"
        );
        assert_eq!(
            disposition(&store, &sid).await,
            RunDisposition::Clean,
            "the boot scan must no longer see an interrupted run here"
        );
        assert_eq!(
            live_seqs(&store, &sid).await,
            vec![1, 3],
            "the closer is live at seq 3; seq 2 is gone"
        );
    }

    #[tokio::test]
    async fn a_running_session_is_retired_but_not_closed() {
        let store = store();
        let svc = InProcessActorSessionService::new(store.clone());
        let sid: SessionId = SessionKey::ephemeral("balance-running");
        seed(&store, &sid, &[run_started(10), run_finished(20)]).await;
        let n = store.append_batches.load(Ordering::SeqCst);

        let out = retire_from_and_close_run(&svc, &sid, 2, |_| true)
            .await
            .expect("rewind");

        assert_eq!(
            out.closed_run, None,
            "a live run's marker must not be closed"
        );
        assert_eq!(out.retired, 1, "but the cut still happens");
        assert_eq!(
            store.append_batches.load(Ordering::SeqCst) - n,
            1,
            "still ONE call: a retire-only batch"
        );
        assert_eq!(store.retire_froms.load(Ordering::SeqCst), 0);
        assert_eq!(
            live_seqs(&store, &sid).await,
            vec![1],
            "nothing appended; the run reads as open because it IS"
        );
        assert_eq!(
            disposition(&store, &sid).await,
            RunDisposition::Interrupted { attempts: 0 }
        );
    }

    #[tokio::test]
    async fn a_balanced_cut_appends_no_closer() {
        let store = store();
        let svc = InProcessActorSessionService::new(store.clone());
        let sid: SessionId = SessionKey::ephemeral("balance-clean");
        seed(&store, &sid, &[run_started(10), run_finished(20)]).await;
        let n = store.append_batches.load(Ordering::SeqCst);

        // A cut past the finish leaves [RS, RF] intact — nothing to close.
        let out = retire_from_and_close_run(&svc, &sid, 3, |_| false)
            .await
            .expect("rewind");

        assert_eq!(
            out.closed_run, None,
            "no second closer for an already-closed run"
        );
        assert_eq!(out.retired, 0);
        assert_eq!(
            store.append_batches.load(Ordering::SeqCst) - n,
            1,
            "one (retire-only) call, even when it retires nothing"
        );
        assert_eq!(live_seqs(&store, &sid).await, vec![1, 2], "log len 2");
        assert_eq!(disposition(&store, &sid).await, RunDisposition::Clean);
    }

    #[test]
    fn open_run_after_retire_reduces_the_surviving_prefix() {
        let events = vec![rec(1, run_started(10)), rec(2, run_finished(20))];
        assert_eq!(
            open_run_after_retire(&events, 2).expect("legal"),
            Some("run-10".into()),
            "cutting the finish leaves the start open"
        );
        assert_eq!(
            open_run_after_retire(&events, 3).expect("legal"),
            None,
            "cutting past the finish leaves a balanced pair"
        );
        assert_eq!(
            open_run_after_retire(&events, 1).expect("legal"),
            None,
            "cutting the start too leaves nothing to close"
        );
    }

    /// Criterion #8: a slice the reducer refuses is an `Err`, never `None`.
    /// An out-of-order slice is the one REJECT kind a caller can hand in, and
    /// the surviving prefix must not be read as "no run open" just because it
    /// could not be read at all.
    #[test]
    fn an_unreadable_prefix_is_an_error_not_a_no_op() {
        let events = vec![rec(2, run_started(10)), rec(1, run_finished(20))];
        assert!(
            open_run_after_retire(&events, 3).is_err(),
            "a slice the reducer rejects is not a decision"
        );
    }
}
