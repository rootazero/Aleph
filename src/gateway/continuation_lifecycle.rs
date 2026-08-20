//! Terminate a retired session's autonomous continuations (loop + goal) and
//! delete the `/btw` side session derived from it.
//!
//! Loops and goals are keyed by the FULL session string (epoch included) and
//! their continuation chains are self-sustaining — each tick's completion
//! re-enters the post-run hook under the OLD key. When a session is retired
//! by an epoch bump, every subsequent user message (including `/loop stop`
//! and `goal clear`) routes to the NEW epoch, where the tools honestly report
//! "no loop/goal in this session" — leaving an uncancellable background chain
//! posting to the channel for up to its full tick cap.
//!
//! This is the single seam every surface that retires a session must call
//! BEFORE the bump: the channel `/new` command
//! (`inbound_router::command_handler`), the Panel `sessions.new` RPC
//! (`handlers::session::db_handlers::create`), and the `sessions.delete` RPC
//! (`handlers::session::db_handlers::modify`) behind the Panel sidebar's
//! delete and the CLI's `session delete` — deletion retires the key just as
//! finally as an epoch bump, and the chains are keyed by the string, not by
//! the transcript's existence.
//!
//! The side session rides along here for the same reason: it is derived from
//! the retiring key (epoch included), so retiring that key is exactly the
//! moment it becomes unreachable, and nothing else in the tree ever deleted
//! one. Each of those three surfaces already holds a `SessionStore` handle, so
//! the handle is a parameter — see
//! [`terminate_session_continuations`] for why it is not fetched from a global.
//!
//! Mechanical state termination only — no reasoning (R10-safe, lives outside
//! the harness).

use tracing::{info, warn};

use crate::gateway::session_store::SessionStore;
use crate::routing::session_key::SessionKey;
use crate::sync_primitives::Arc;

/// Honestly terminate a session's goal: block it from either non-terminal
/// state and delete its welded strategy so a stale plan can neither steer
/// later turns nor block a fresh start. Best-effort; returns whether a goal
/// was actually retired (terminal goals are never clobbered —
/// [`crate::goal::GoalStore::block_if_not_terminal`]'s contract).
///
/// `Paused` is retired alongside `Active` for the same reason the loop side
/// retires paused loops: the epoch bump (or delete) makes the old session
/// unreachable, and a goal can only be re-armed from its own session, so a
/// `Paused` goal left behind is one `goal(action='list')` keeps advertising
/// and nobody can ever restart. Consumers: the epoch-retirement seam below.
/// The `ResumeCoordinator` abandon paths use the narrower
/// [`block_abandonable_session_goal`] instead.
pub fn block_session_goal(session: &str, note: &str) -> bool {
    let Some(store) = crate::goal::global() else {
        return false;
    };
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64);
    match store.block_if_not_terminal(session, note, now_ms) {
        Ok(true) => {
            if let Some(strat) = crate::strategy::global() {
                let _ = strat.delete(&crate::strategy::goal_key(session));
            }
            info!(session = %session, "goal retired: {note}");
            true
        }
        Ok(false) => false,
        Err(e) => {
            warn!(session = %session, error = %e, "failed to retire session goal");
            false
        }
    }
}

/// Block a session goal ONLY when its recovery genuinely hung on a crashed
/// autonomous run — used by `ResumeCoordinator::abandon`. Delegates to
/// [`crate::goal::GoalStore::block_if_abandonable`] (Active-pursuit, not
/// task-barrier-parked) so a passive goal, or one parked on a task barrier
/// (woken by `GoalWakeService`), is left untouched. Deletes the welded
/// strategy on a real block, like [`block_session_goal`]. Returns whether a
/// goal was blocked.
pub fn block_abandonable_session_goal(session: &str, note: &str) -> bool {
    let Some(store) = crate::goal::global() else {
        return false;
    };
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64);
    match store.block_if_abandonable(session, note, now_ms) {
        Ok(true) => {
            if let Some(strat) = crate::strategy::global() {
                let _ = strat.delete(&crate::strategy::goal_key(session));
            }
            info!(session = %session, "abandonable goal blocked: {note}");
            true
        }
        Ok(false) => false,
        Err(e) => {
            warn!(session = %session, error = %e, "failed to block abandonable goal");
            false
        }
    }
}

/// Delete the `/btw` side session derived from the retiring key.
///
/// The derived key includes the epoch, so an epoch bump already makes the old
/// side session unaddressable — nothing can reach it, seed it or answer into
/// it. Deleting it here is what keeps it from becoming permanent residue.
/// That is also why this is allowed to be best-effort: a missed delete costs
/// disk, never a crossed side thread.
///
/// Fire-and-forget because the caller is on a user-facing retirement path
/// (`/new`, `sessions.new`, `sessions.delete`) and must not inherit either the
/// latency or the failure of a store write for a session the user is done
/// with.
///
/// # Exactly one key, deliberately
///
/// This deletes `side_key_for(old_key)` and nothing else. It must not grow
/// into a sweep over `SessionKey::Ephemeral`: that variant is also where
/// background sub-agent children and the OpenAI-compatible completions face
/// live, so a variant-wide sweep would delete work that is currently running.
/// See the `Ephemeral` doc in `routing::session_key`.
fn retire_side_session(old_key: &SessionKey, cause: &str, store: Option<Arc<dyn SessionStore>>) {
    let Some(store) = store else {
        return;
    };
    // A key that is already derived has no side session of its own; asking for
    // one would mint a phantom key nothing ever ran on.
    let Some(side) = crate::gateway::btw::side_session_of(old_key) else {
        return;
    };
    let cause = cause.to_string();
    tokio::spawn(async move {
        let side_str = side.to_key_string();
        match store.delete_session(&side).await {
            // The overwhelmingly common case: this conversation never asked a
            // side question, so there was no row. Not an outcome worth a line.
            Ok(result) if !result.deleted => {}
            Ok(_) => info!(session = %side_str, cause, "btw: side session retired"),
            // The one outcome that leaves residue behind, and the only reason
            // anyone would ever go looking for this log. Distinct from "nothing
            // was there" on purpose — collapsing the two would make the leak
            // this function exists to close indistinguishable from success.
            Err(e) => warn!(
                session = %side_str,
                cause,
                error = %e,
                "btw: side session left behind — delete failed"
            ),
        }
    });
}

/// Stop the retired session's active loop, block its active goal, and delete
/// the `/btw` side session derived from it (all three best-effort), clearing
/// the loop's and goal's welded strategies so a stale plan can neither steer
/// later turns nor block a fresh start in the new epoch. `cause` names the
/// retiring surface for the stored stop reason / blocked note (e.g. `"/new"`,
/// `"sessions.new"`).
///
/// # Why the key is typed and the store is a parameter
///
/// `old_key` is a [`SessionKey`], not the key string the loop registry and
/// goal store are keyed by, because the side-session derivation
/// ([`crate::gateway::btw::side_key_for`]) hashes `to_key_string()` and must
/// hash *the same key the turn ran on*. Taking a string would put a re-parse
/// between the caller's key and the hash, and there are two spellings of that
/// parse (`SessionKey::parse` and `from_key_string`, which differ on legacy
/// input) — picking the wrong one derives a different side key, deletes
/// nothing, and reports success. Taking the key removes the class.
///
/// `store` is passed in rather than fetched because there is deliberately no
/// process-global `SessionStore`, and adding one for this single caller would
/// be a second answer to "where is the session store". All three callers
/// already hold a handle. `None` means the caller genuinely has no store (the
/// inbound router's is optional), and costs at most the disk residue this
/// function exists to prevent.
pub fn terminate_session_continuations(
    old_key: &SessionKey,
    cause: &str,
    store: Option<Arc<dyn SessionStore>>,
) {
    // Canonical spelling: the loop registry and the goal store are keyed by
    // `to_key_string()`.
    let old_session = old_key.to_key_string();
    retire_side_session(old_key, cause, store);
    if let Some(reg) = crate::looping::global() {
        // Paused loops are retired too: the epoch bump makes the old session
        // unreachable, so a loop left quiet there could never be resumed —
        // leaving it `Paused` would only make `loop(action='list')` advertise a
        // loop nobody can ever restart.
        if matches!(
            reg.transition(
                &old_session,
                crate::looping::LoopStatus::Stopped,
                Some(format!("Session closed via {cause}")),
            ),
            crate::looping::TransitionOutcome::Applied { .. }
        ) {
            if let Some(strat) = crate::strategy::global() {
                let _ = strat.delete(&crate::strategy::loop_key(&old_session));
            }
            info!(session = %old_session, cause, "session retired: loop stopped");
        }
    }
    block_session_goal(
        &old_session,
        &format!(
            "Session was closed via {cause} — re-set the goal in the new session to \
             continue pursuit."
        ),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builtin_tools::agent_manage::test_utils;
    use crate::gateway::btw::side_key_for;
    use crate::gateway::session_store::error::SessionStoreError;
    use crate::gateway::session_store::types::SessionMetadata;

    /// Has this key stopped resolving?
    ///
    /// The retirement is fire-and-forget, so a single `yield_now()` does not
    /// observe it — the spawned task suspends on the store write. This polls
    /// with a bound, which keeps the guard honest: remove the retirement and
    /// it goes red rather than hanging.
    ///
    /// Absence is `Ok(None)`, never `Err`. An `Err` means the store failed to
    /// answer, which is a different fact and must not be read as "it is gone" —
    /// that is the shape that would let these tests pass for the wrong reason,
    /// so it is returned rather than swallowed.
    async fn wait_until_absent(
        store: &dyn SessionStore,
        key: &SessionKey,
    ) -> Result<Option<SessionMetadata>, SessionStoreError> {
        let mut last = store.get_metadata(key).await;
        for _ in 0..200 {
            if matches!(last, Ok(None) | Err(_)) {
                return last;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            last = store.get_metadata(key).await;
        }
        last
    }

    /// The leak this module closes: nothing ever deleted a `/btw` side session.
    /// The epoch bump makes it unaddressable, so without a delete it is
    /// permanent disk residue no surface can list, reach or remove.
    #[tokio::test]
    async fn an_epoch_bump_retires_the_old_side_session() {
        let (store, _tmp) = test_utils::session_store();
        let main = SessionKey::main("assistant");
        let side = side_key_for(&main);

        store
            .get_or_create(&side)
            .await
            .expect("side session row created");
        // Anchor before negating: without this, a broken fixture would make
        // the assertion below pass while proving nothing.
        assert!(
            matches!(store.get_metadata(&side).await, Ok(Some(_))),
            "fixture: the side session must exist before the retirement runs"
        );

        terminate_session_continuations(&main, "/new", Some(store.clone()));

        match wait_until_absent(store.as_ref(), &side).await {
            Ok(None) => {}
            Ok(Some(_)) => {
                panic!("the old side session survived /new — it is now unreachable disk residue")
            }
            Err(e) => panic!("the store failed to answer, which is not the same as absence: {e}"),
        }
    }

    /// Retirement must delete exactly the derived key. `Ephemeral` is the
    /// catch-all for "a session that is not a conversation", and background
    /// sub-agent children live there too — a retirement that swept the variant
    /// would delete work that is currently running.
    #[tokio::test]
    async fn retirement_deletes_the_derived_key_and_no_other_ephemeral() {
        let (store, _tmp) = test_utils::session_store();
        let main = SessionKey::main("assistant");
        let side = side_key_for(&main);
        // Shaped like `subagent_spawner::ephemeral_for`: same agent, same
        // variant, different id.
        let bystander = SessionKey::Ephemeral {
            agent_id: "assistant".to_string(),
            ephemeral_id: "subagent-bg-live-work".to_string(),
        };

        store.get_or_create(&side).await.expect("side row");
        store
            .get_or_create(&bystander)
            .await
            .expect("bystander row");

        terminate_session_continuations(&main, "sessions.delete", Some(store.clone()));

        assert!(
            matches!(wait_until_absent(store.as_ref(), &side).await, Ok(None)),
            "the derived side session should have been retired"
        );
        assert!(
            matches!(store.get_metadata(&bystander).await, Ok(Some(_))),
            "retirement reached an ephemeral session it does not own — a sweep by \
             variant would delete live sub-agent work"
        );
    }

    /// A side key has no side session of its own, so retiring one must derive
    /// nothing and delete nothing.
    ///
    /// The observable is a row planted **at** the derived-of-derived key. A
    /// bare `side_key_for` here would derive that key and delete it; the
    /// production `side_session_of` returns `None` and never reaches the store.
    /// Without planting the row there is nothing to see — deleting a key that
    /// was never created is indistinguishable from not deriving it, which is
    /// how this guard would pass for the wrong reason.
    #[tokio::test]
    async fn retiring_a_side_key_derives_nothing_and_deletes_nothing() {
        let (store, _tmp) = test_utils::session_store();
        let main = SessionKey::main("assistant");
        let side = side_key_for(&main);
        let derived_of_derived = side_key_for(&side);

        store.get_or_create(&side).await.expect("side row");
        store
            .get_or_create(&derived_of_derived)
            .await
            .expect("probe row at the derived-of-derived key");

        terminate_session_continuations(&side, "sessions.delete", Some(store.clone()));

        // A window comparable to the one the other tests give their spawns, so
        // "nothing happened" is a finding rather than a race.
        for _ in 0..20 {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        assert!(
            matches!(store.get_metadata(&derived_of_derived).await, Ok(Some(_))),
            "retiring a side key derived a second one and deleted it — the derivation \
             must be idempotent, or a side question's own retirement mints keys \
             nothing ever ran on"
        );
        assert!(
            matches!(store.get_metadata(&side).await, Ok(Some(_))),
            "retiring a side key must not delete the side session itself — the delete \
             targets the key derived FROM the retiring key"
        );
    }
}
