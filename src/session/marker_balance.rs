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
//! So the writer closes it, here, in one place shared by both verbs.

use crate::session::events::{now_ms, RunOutcome, SessionEvent};
use crate::session::reduction::reduce_run;
use crate::session::service::{SessionError, SessionId};
use crate::session::store::SessionEventStore;

/// Close a run marker the retire left open, and say which run was closed.
///
/// `is_running` answers "does this session have a turn in flight right now".
/// It is a parameter and not a lookup because the only authority on that is the
/// engine's per-session run registry, which lives above this layer — and
/// because the predicate is the half of this rule that a mutation can silence.
///
/// **`is_running` must fail closed.** A caller that cannot tell (no run
/// registry wired) has to answer `true`: "I do not know whether a run is live"
/// may not be read as "no run is live", because closing the marker of a run
/// that is still executing writes a `RunFinished` into the middle of it, and
/// the real finish then lands as a [`crate::session::reduction::
/// LogContradiction::FinishWithoutStart`] on a session that is now permanently
/// mis-read.
///
/// Returns the `run_id` that was closed, or `None` when there was nothing to
/// close (the tail is already a `RunFinished`, the log has no `RunStarted`
/// left, or a run is in flight).
///
/// # Errors
///
/// The store could not be read or the append failed. A log the reducer refuses
/// is an `Err` too: appending a closer chosen from a slice nobody could read
/// would pick the wrong `run_id`.
pub async fn close_open_run_after_retire(
    store: &dyn SessionEventStore,
    session: &SessionId,
    is_running: impl Fn(&SessionId) -> bool,
) -> Result<Option<String>, SessionError> {
    if is_running(session) {
        return Ok(None);
    }
    let events = store.load_all_events(session).await?;
    let reduction = reduce_run(&events).map_err(|c| SessionError::Other(c.to_string()))?;
    let Some(open) = reduction.open_run else {
        return Ok(None);
    };
    // `Cancelled`, not `Abandoned`: the user cut this run out of their own
    // transcript deliberately. `Abandoned` is the resume coordinator's word for
    // "recovery gave up", and reusing it here would make a user edit read as a
    // failed recovery in every counter that separates the two.
    let ev = SessionEvent::RunFinished {
        run_id: open.run_id.clone(),
        outcome: RunOutcome::Cancelled,
        at: now_ms(),
    };
    let seq = store.load_head_seq(session).await? + 1;
    store.append(session, seq, &ev, now_ms()).await?;
    Ok(Some(open.run_id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routing::session_key::SessionKey;
    use crate::session::events::EventSeq;
    use crate::session::reduction::RunDisposition;
    use crate::session::store::{migrate_add_session_events, SqliteEventStore};
    use std::sync::Arc;

    fn store() -> Arc<dyn SessionEventStore> {
        let conn = rusqlite::Connection::open_in_memory().expect("sqlite");
        migrate_add_session_events(&conn).expect("migrate");
        Arc::new(SqliteEventStore::new(conn))
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

    async fn seed(store: &Arc<dyn SessionEventStore>, sid: &SessionId, evs: &[SessionEvent]) {
        for (i, ev) in evs.iter().enumerate() {
            store
                .append(sid, (i + 1) as EventSeq, ev, 10 * (i as i64 + 1))
                .await
                .expect("append");
        }
    }

    async fn disposition(store: &Arc<dyn SessionEventStore>, sid: &SessionId) -> RunDisposition {
        let events = store.load_all_events(sid).await.expect("load");
        reduce_run(&events).expect("legal log").disposition
    }

    #[tokio::test]
    async fn an_open_run_on_an_idle_session_is_closed() {
        let store = store();
        let sid: SessionId = SessionKey::ephemeral("balance-idle");
        seed(&store, &sid, &[run_started(10)]).await;

        let closed = close_open_run_after_retire(store.as_ref(), &sid, |_| false)
            .await
            .expect("balance");
        assert_eq!(closed.as_deref(), Some("run-10"), "closes THAT run's id");
        assert_eq!(
            disposition(&store, &sid).await,
            RunDisposition::Clean,
            "the boot scan must no longer see an interrupted run here"
        );
    }

    #[tokio::test]
    async fn an_open_run_on_a_running_session_is_left_alone() {
        let store = store();
        let sid: SessionId = SessionKey::ephemeral("balance-running");
        seed(&store, &sid, &[run_started(10)]).await;

        let closed = close_open_run_after_retire(store.as_ref(), &sid, |_| true)
            .await
            .expect("balance");
        assert_eq!(closed, None, "a live run's marker must not be closed");
        assert_eq!(
            disposition(&store, &sid).await,
            RunDisposition::Interrupted { trailing_starts: 1 },
            "and nothing was appended"
        );
    }

    #[tokio::test]
    async fn a_balanced_tail_appends_nothing() {
        let store = store();
        let sid: SessionId = SessionKey::ephemeral("balance-clean");
        seed(&store, &sid, &[run_started(10), run_finished(20)]).await;

        let closed = close_open_run_after_retire(store.as_ref(), &sid, |_| false)
            .await
            .expect("balance");
        assert_eq!(closed, None);
        assert_eq!(
            store.load_all_events(&sid).await.expect("load").len(),
            2,
            "no second closer for an already-closed run"
        );
    }

    #[tokio::test]
    async fn a_rewind_that_cut_the_finish_leaves_a_clean_log() {
        let store = store();
        let sid: SessionId = SessionKey::ephemeral("balance-rewind");
        seed(&store, &sid, &[run_started(10), run_finished(20)]).await;
        // What `chat.rewind` does: retire from the seq the user cut at.
        store.retire_from(&sid, 2).await.expect("retire");
        assert_eq!(
            disposition(&store, &sid).await,
            RunDisposition::Interrupted { trailing_starts: 1 },
            "precondition: the retire alone leaves the run reading as open"
        );

        let closed = close_open_run_after_retire(store.as_ref(), &sid, |_| false)
            .await
            .expect("balance");
        assert_eq!(closed.as_deref(), Some("run-10"));
        assert_eq!(disposition(&store, &sid).await, RunDisposition::Clean);
    }
}
