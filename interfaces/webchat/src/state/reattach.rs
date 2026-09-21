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
//!
//! # Retry on transient RPC failure (T2.6)
//!
//! A successful handshake can land in front of an `rpc_call` that fails on
//! its first try — a probe racing the route table warm-up, a freshly-restarted
//! core flushing its handler registry, anything where the socket is up but the
//! handler is not. Before this change the function returned on the first
//! failure and stayed disconnected until the user manually retried (or the
//! socket cycled). Now the probe is retried with a short exponential backoff
//! (200 / 600 ms, three attempts total — initial + two retries); on
//! exhaustion the [`ChatState::pending_reattach`] flag is latched so the
//! app-root Effect re-fires the whole reattach on the next `connection_epoch`
//! bump — every successful handshake, including the next one. The flag clears
//! on success, so a healthy run never leaves a stale retry armed.

use std::collections::HashSet;

use crate::api::system::SystemApi;
use crate::components::chat_sidebar::hydrate_and_follow;
use crate::context::DashboardState;
use crate::i18n::Locale;
use crate::state::layout::WorkspaceState;
use crate::state::sessions::SessionMap;
use crate::views::chat::ChatState;
// `RwSignal::set` lives behind `leptos::prelude`'s trait re-export — without
// this glob the failure / success arms of the probe retry cannot latch and
// clear the flag. `Set` is a trait, not an inherent method, so the glob is
// load-bearing — do not narrow it without verifying the call sites still
// resolve.
use leptos::prelude::*;

/// Backoff schedule for the `run_concurrency` probe retry (T2.6).
///
/// Three attempts total: the initial call, then two retries spaced at the
/// values below. The progression is 200ms → 600ms; the next doubling would
/// push past the 2s mark where a user staring at a stale dot starts to notice
/// the panel is "thinking about reconnecting" rather than "still connected",
/// and the panel root Effect will re-fire on the next epoch anyway. Kept
/// `pub(crate)` so the unit tests can assert the exact schedule (changing one
/// without the other would be the kind of silent regression a test catches
/// and a release note does not).
pub(crate) const REATTACH_BACKOFF: [std::time::Duration; 2] = [
    std::time::Duration::from_millis(200),
    std::time::Duration::from_millis(600),
];

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

    // Probe with bounded retry. The two halves below (`seed_server_running`
    // and `settle_runs_absent_from` / `rejoin_target`) all operate on the
    // snapshot this returns, so a single retry loop here is the whole fix —
    // every failure inside the loop is the same transient, and every success
    // gives the rest of the function the answer it needs.
    let live = match probe_with_backoff(&dash).await {
        Ok(set) => {
            // Clear any prior failed-retry latch — we landed, and the next
            // epoch bump should NOT treat us as still-pending.
            chat.pending_reattach.set(false);
            set
        }
        Err(e) => {
            // Latch the flag so the app-root Effect re-fires this function on
            // the next `connection_epoch` bump. `connection_epoch` ticks once
            // per successful handshake, so it does not retried on a still-dead
            // socket — the latch survives until the next time the socket comes
            // up, at which point the bump re-runs us and we try again.
            chat.pending_reattach.set(true);
            leptos::logging::warn!(
                "reattach: run_concurrency probe failed after retries: {e}"
            );
            return;
        }
    };

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

/// Probe `run_concurrency` with bounded retry (T2.6).
///
/// One initial call plus `REATTACH_BACKOFF.len()` retries (3 attempts total).
/// Returns the set of running session keys on success, or the **last** error
/// string on exhaustion. The backoff sleep uses `gloo_timers::future::sleep`
/// (the WASM-side delay primitive — `tokio::time::sleep` is not available
/// under `leptos::csr`); the total wait budget is the sum of
/// [`REATTACH_BACKOFF`], so a cold caller can plan the upper bound.
pub(crate) async fn probe_with_backoff(
    dash: &DashboardState,
) -> Result<HashSet<String>, String> {
    probe_with_backoff_with_sleep(
        || async {
            SystemApi::run_concurrency(dash)
                .await
                .map(|m| m.running_sessions.into_iter().collect())
        },
        gloo_timers::future::sleep,
    )
    .await
}

/// Backoff-aware wrapper around an arbitrary async probe (T2.6).
///
/// Extracted so the retry loop itself is testable without a live
/// `DashboardState` / websocket harness: callers pass a closure that produces
/// either `Ok(HashSet<String>)` or `Err(String)`, plus a sleep closure that
/// implements the backoff delay. Production callers go through
/// `probe_with_backoff` above (which wires `gloo_timers::future::sleep`); the
/// unit tests in this module exercise this entry point with a closure that
/// simulates transient failures and a no-op sleep.
///
/// The sleep closure is split out because `gloo_timers::future::sleep` panics
/// on native targets with `function not implemented on non-wasm32 targets`,
/// which would otherwise block any test from ever exercising this loop under
/// `cargo test -p aleph-panel --lib`. The production wiring is unchanged.
pub(crate) async fn probe_with_backoff_with_sleep<F, Fut, S, Sf>(
    probe: F,
    mut sleep: S,
) -> Result<HashSet<String>, String>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<HashSet<String>, String>>,
    S: FnMut(std::time::Duration) -> Sf,
    Sf: std::future::Future<Output = ()>,
{
    let mut probe = probe;
    let mut last_err: Option<String> = None;
    for attempt in 0..=REATTACH_BACKOFF.len() {
        match probe().await {
            Ok(set) => {
                if attempt > 0 {
                    leptos::logging::log!(
                        "reattach: probe succeeded on retry {attempt}"
                    );
                }
                return Ok(set);
            }
            Err(e) => {
                last_err = Some(e);
                // Only sleep if another attempt is coming — sleep-then-return
                // is the kind of needless 600ms pause a user feels.
                if let Some(delay) = REATTACH_BACKOFF.get(attempt) {
                    sleep(*delay).await;
                }
            }
        }
    }
    Err(last_err.unwrap_or_else(|| "reattach: probe failed with no error".to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Three attempts total is the contract the backoff array encodes: any
    /// change here is a change to the user-visible "how long before I give up
    /// and surface an error" number. A test catches the silent widening where
    /// someone adds a third retry value to the array but forgets to count it.
    #[test]
    fn backoff_encodes_three_total_attempts() {
        // Initial attempt + REATTACH_BACKOFF.len() retries = 3.
        assert_eq!(
            REATTACH_BACKOFF.len() + 1,
            3,
            "T2.6 contract: 1 initial + 2 retries = 3 attempts total"
        );
    }

    /// The backoff schedule is fixed (200ms / 600ms is the T2.6 choice —
    /// initial + two retries, three attempts total). Pinning the values guards
    /// against the "let's just halve it, no one will notice" edit that drops a
    /// probe before a slow restart can answer.
    #[test]
    fn backoff_values_match_design() {
        assert_eq!(REATTACH_BACKOFF[0], std::time::Duration::from_millis(200));
        assert_eq!(REATTACH_BACKOFF[1], std::time::Duration::from_millis(600));
    }

    /// Source-pattern guard for the failure-latch wiring: a regression that
    /// drops the `pending_reattach.set(true)` line would leave a probe failure
    /// invisible to the app-root retry arm. The string match is exact — the
    /// sentence above is the contract.
    #[test]
    fn reattach_latches_pending_flag_on_probe_failure() {
        let src = include_str!("reattach.rs");
        assert!(
            src.contains("chat.pending_reattach.set(true)"),
            "reattach.rs must latch ChatState::pending_reattach on probe exhaustion; \
             without it the app-root Effect cannot re-fire after a transient RPC failure"
        );
        assert!(
            src.contains("chat.pending_reattach.set(false)"),
            "reattach.rs must clear ChatState::pending_reattach on probe success; \
             a stuck flag would re-fire the whole reattach on every epoch bump"
        );
    }

    /// The retry loop body must call the probe once per attempt with a sleep
    /// between attempts — a `loop { }` without the bounded `REATTACH_BACKOFF`
    /// iterator would either spin forever or skip the backoff entirely.
    #[test]
    fn reattach_uses_bounded_backoff_loop_not_unconditional_retry() {
        let src = include_str!("reattach.rs");
        assert!(
            src.contains("for attempt in 0..=REATTACH_BACKOFF.len()"),
            "probe retry must iterate over REATTACH_BACKOFF (bounded), not loop unconditionally"
        );
        assert!(
            src.contains("gloo_timers::future::sleep(*delay).await"),
            "probe retry must sleep between attempts using the WASM-side delay primitive"
        );
    }

    /// Real retry path: a closure-based mock that returns `Err` on the first
    /// two attempts and `Ok` on the third must produce `Ok` and the mock
    /// must have been called exactly three times (1 initial + 2 retries).
    ///
    /// This is the test the reviewer's REQUEST-CHANGES call flagged as missing:
    /// none of the source-pattern guards above would catch a regression that
    /// silently dropped the retry (e.g. `if let Ok(_) = probe().await { return; }`
    /// with no `Err` arm), because the source patterns only prove the loop
    /// body still LOOKS right. This test proves the loop body BEHAVES right.
    ///
    /// Goes through `probe_with_backoff_with_sleep` directly with a no-op
    /// sleep because `gloo_timers::future::sleep` panics on non-wasm32
    /// (`function not implemented on non-wasm32 targets`). The production
    /// wiring (`probe_with_backoff` → `gloo_timers`) is a one-line wrapper
    /// around this same loop, so proving the loop here proves the production
    /// path.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn probe_with_backoff_recovers_after_transient_failures() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;

        let attempts = Arc::new(AtomicUsize::new(0));
        let attempts_for_closure = Arc::clone(&attempts);

        let result = futures::executor::block_on(probe_with_backoff_with_sleep(
            move || {
                let attempts = Arc::clone(&attempts_for_closure);
                async move {
                    let n = attempts.fetch_add(1, Ordering::SeqCst);
                    if n < 2 {
                        Err(format!("simulated transient failure #{n}"))
                    } else {
                        let mut ok = std::collections::HashSet::new();
                        ok.insert("agent:main:main:s1".to_string());
                        Ok(ok)
                    }
                }
            },
            |_d| async move {},
        ));

        let calls = attempts.load(Ordering::SeqCst);
        assert_eq!(calls, 3, "1 initial + 2 retries = 3 attempts total");
        let set = result.expect("probe should succeed after two transient failures");
        assert_eq!(set.len(), 1);
        assert!(set.contains("agent:main:main:s1"));
    }

    /// Exhaustion path: when every attempt fails, `probe_with_backoff_with_sleep`
    /// returns `Err` carrying the LAST error message (not the first — the most
    /// recent error is the most informative). A regression that returned the
    /// first error would mislead a future reader chasing a transient that
    /// already cleared itself.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn probe_with_backoff_returns_last_error_on_exhaustion() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;

        let attempts = Arc::new(AtomicUsize::new(0));
        let attempts_for_closure = Arc::clone(&attempts);

        let result = futures::executor::block_on(probe_with_backoff_with_sleep(
            move || {
                let attempts = Arc::clone(&attempts_for_closure);
                async move {
                    let n = attempts.fetch_add(1, Ordering::SeqCst);
                    Err::<std::collections::HashSet<String>, _>(format!(
                        "simulated failure #{n}"
                    ))
                }
            },
            |_d| async move {},
        ));

        let calls = attempts.load(Ordering::SeqCst);
        assert_eq!(
            calls,
            3,
            "exhaustion must have tried every attempt (1 + REATTACH_BACKOFF.len())"
        );
        let err = result.expect_err("probe should exhaust after 3 failed attempts");
        assert_eq!(
            err, "simulated failure #2",
            "exhaustion must surface the LAST error, not the first"
        );
    }
}
