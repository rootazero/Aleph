//! What a chat client owes its user when the socket comes back.
//!
//! A reconnect is not a refresh. `seq` orders `RunningSetChanged` frames
//! **within one connection** and means nothing across one, frames sent while
//! the socket was down are simply gone, and a core that restarted numbers its
//! own frames from 0 again — so a client that keeps its old baseline discards
//! every frame the new process ever sends. On top of that the client is holding
//! run routes whose terminal frame may never arrive, and may be sitting in
//! front of a conversation the server is mid-turn on and it cannot route.
//!
//! All three are answered from **one** server snapshot, in one place, for every
//! form factor:
//!
//! 1. void the sequence baseline, so the seed below applies and the next live
//!    frame is accepted whatever its `seq`;
//! 2. settle every route the server does not confirm — the composer was locked
//!    on Stop and the dot lit until the user reloaded the page;
//! 3. re-join the run the server *does* confirm on the open conversation — the
//!    half that had no exit at all before.
//!
//! # Why this is not in `ChatSidebar`
//!
//! Steps 1 and 2 used to live in that component's reconnect `Effect`, and
//! `ChatSidebar` is mounted behind `not_phone`. So the phone — and the iOS
//! Panel shell, which is always in the phone band — never repaired anything
//! after a core restart. Mounted at the app root, both form factors inherit the
//! repair and neither has a copy of it.
//!
//! # Why one round trip
//!
//! The sidebar asked `gateway.metrics.run_concurrency` twice per reconnect
//! (once inside its data reload to seed the dots, once here to reconcile) and a
//! third time on every `run.session_updated` — where the seed is a no-op
//! anyway, because a live frame has already advanced the baseline. Two answers
//! to "what is running right now" taken a round trip apart can disagree, and
//! the settle pass is the one that acts on the answer.

use std::collections::HashSet;

use crate::api::system::SystemApi;
use crate::components::chat_sidebar::hydrate_and_follow;
use crate::context::DashboardState;
use crate::i18n::Locale;
use crate::state::layout::WorkspaceState;
use crate::state::sessions::SessionMap;
use crate::views::chat::ChatState;

/// Run the reconnect repair against one `run_concurrency` snapshot.
///
/// Call on every successful handshake (mount included — a cold load is a
/// connect whose baseline happens to be empty).
pub async fn reattach_after_connect(
    dash: DashboardState,
    chat: ChatState,
    sessions: SessionMap,
    workspace: Option<WorkspaceState>,
    locale: Locale,
) {
    // MUST precede the seed: `seed_server_running` is a no-op while a sequence
    // baseline survives, and the baseline from the previous connection is
    // exactly what has to go.
    //
    // It deliberately does not clear the set it re-bases. Blanking it would
    // extinguish every dot for the duration of the round trip below, which
    // reads as "all runs finished" — the opposite of the truth in the case that
    // matters, a long autonomous run that outlived the disconnect.
    sessions.reset_running_baseline();

    let Ok(metrics) = SystemApi::run_concurrency(&dash).await else {
        // Nothing is claimed on a failed probe. The stale set stands, which is
        // the same posture `reset_running_baseline` takes and for the same
        // reason: "I could not ask" must not render as "nothing is running".
        return;
    };
    let live: HashSet<String> = metrics.running_sessions.into_iter().collect();

    sessions.seed_server_running(live.clone());

    // Negative half. `settle_abandoned_run`, not `complete_run` / `fail_run`:
    // this turn may have finished, may have been resumed under a new id, may
    // have died with its process — all three are unknown, and the honest move
    // is to stop claiming it is in flight rather than to invent a verdict.
    for (run_id, conv) in sessions.settle_runs_absent_from(&live) {
        if let Some(target) = sessions.chat_for(conv, chat) {
            target.settle_abandoned_run(&run_id);
        }
    }

    // Positive half. `hydrate_and_follow` re-reads the transcript and binds
    // `chat.history`'s `active_run`, so the rest of the turn renders live from
    // the re-join point and `run_complete` finishes with the
    // history-authoritative answer.
    if let Some(key) = sessions.rejoin_target(&live) {
        hydrate_and_follow(dash, chat, workspace, sessions, key, locale).await;
    }
}
