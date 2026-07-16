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
//! This is the single seam every epoch-bumping surface must call BEFORE the
//! bump: the channel `/new` command (`inbound_router::command_handler`) and
//! the Panel `sessions.new` RPC (`handlers::session::db_handlers::create`).
//! Mechanical state termination only — no reasoning (R10-safe, lives outside
//! the harness).

use tracing::{info, warn};

/// Stop the retired session's active loop and block its active goal (both
/// best-effort), clearing their welded strategies so a stale plan can neither
/// steer later turns nor block a fresh start in the new epoch. `cause` names
/// the retiring surface for the stored stop reason / blocked note (e.g.
/// `"/new"`, `"sessions.new"`).
pub fn terminate_session_continuations(old_session: &str, cause: &str) {
    if let Some(reg) = crate::looping::global() {
        if let Some(state) = reg.get_active(old_session) {
            reg.put(
                state
                    .with_status(crate::looping::LoopStatus::Stopped)
                    .with_stop_reason(Some(format!("Session closed via {cause}"))),
            );
            if let Some(strat) = crate::strategy::global() {
                let _ = strat.delete(&crate::strategy::loop_key(old_session));
            }
            info!(session = %old_session, cause, "session retired: active loop stopped");
        }
    }
    if let Some(store) = crate::goal::global() {
        let note = format!(
            "Session was closed via {cause} — re-set the goal in the new session to \
             continue pursuit."
        );
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_millis() as u64);
        match store.block_if_active(old_session, &note, now_ms) {
            Ok(true) => {
                if let Some(strat) = crate::strategy::global() {
                    let _ = strat.delete(&crate::strategy::goal_key(old_session));
                }
                info!(session = %old_session, cause, "session retired: active goal blocked");
            }
            Ok(false) => {}
            Err(e) => {
                warn!(session = %old_session, cause, error = %e, "session retired: failed to block active goal");
            }
        }
    }
}
