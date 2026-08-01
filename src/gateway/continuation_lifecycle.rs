//! Terminate a retired session's autonomous continuations (loop + goal).
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
//! Mechanical state termination only — no reasoning (R10-safe, lives outside
//! the harness).

use tracing::{info, warn};

/// Honestly terminate a session's active goal: block it with `note` and
/// delete its welded strategy so a stale plan can neither steer later turns
/// nor block a fresh start. Best-effort; returns whether an ACTIVE goal was
/// actually blocked (terminal goals are never clobbered —
/// `block_if_active`'s contract). Consumers: the epoch-retirement seam below
/// and the `ResumeCoordinator` abandon paths.
pub fn block_session_goal(session: &str, note: &str) -> bool {
    let Some(store) = crate::goal::global() else {
        return false;
    };
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64);
    match store.block_if_active(session, note, now_ms) {
        Ok(true) => {
            if let Some(strat) = crate::strategy::global() {
                let _ = strat.delete(&crate::strategy::goal_key(session));
            }
            info!(session = %session, "active goal blocked: {note}");
            true
        }
        Ok(false) => false,
        Err(e) => {
            warn!(session = %session, error = %e, "failed to block active goal");
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

/// Stop the retired session's active loop and block its active goal (both
/// best-effort), clearing their welded strategies so a stale plan can neither
/// steer later turns nor block a fresh start in the new epoch. `cause` names
/// the retiring surface for the stored stop reason / blocked note (e.g.
/// `"/new"`, `"sessions.new"`).
pub fn terminate_session_continuations(old_session: &str, cause: &str) {
    if let Some(reg) = crate::looping::global() {
        // Paused loops are retired too: the epoch bump makes the old session
        // unreachable, so a loop left quiet there could never be resumed —
        // leaving it `Paused` would only make `loop(action='list')` advertise a
        // loop nobody can ever restart.
        if matches!(
            reg.transition(
                old_session,
                crate::looping::LoopStatus::Stopped,
                Some(format!("Session closed via {cause}")),
            ),
            crate::looping::TransitionOutcome::Applied { .. }
        ) {
            if let Some(strat) = crate::strategy::global() {
                let _ = strat.delete(&crate::strategy::loop_key(old_session));
            }
            info!(session = %old_session, cause, "session retired: loop stopped");
        }
    }
    block_session_goal(
        old_session,
        &format!(
            "Session was closed via {cause} — re-set the goal in the new session to \
             continue pursuit."
        ),
    );
}
