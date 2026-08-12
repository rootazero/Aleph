//! Per-session "a human just interjected" wake edge, for tools that **park**.
//!
//! # Why this exists (codex parity)
//!
//! Mid-loop steering appends the user's message to the live session log and
//! the running loop reads it *at its next turn boundary*
//! ([`crate::gateway::execution_engine::steering`]). That boundary is normally
//! milliseconds away — except when the turn's Act phase is sitting inside a
//! tool that is deliberately asleep:
//!
//! | park site | ceiling |
//! |---|---|
//! | `subagent{action:"wait"}` | `MAX_WAIT_TIMEOUT_SECS` = 600 s |
//! | `bash{process_action:"wait"}` | `WAIT_MAX_TIMEOUT_SECS` = 170 s |
//!
//! For that whole window the steer is durably written, the client was told
//! `HandledInline` (success), and **nothing happens** — no error, no red test,
//! just an agent that ignores its user for up to ten minutes. codex closes the
//! same hole from the other side: its `sleep` and `wait_agent` handlers both
//! `subscribe_activity` on the session input queue and report
//! `InputQueueActivity::Steer` / `WaitOutcome::Steered` as a first-class wake
//! reason. Aleph's `subagent` wait already cited that behaviour in its own doc
//! comment as the reason it listens to the cancel token — and then implemented
//! only the cancel half. This module is the other half.
//!
//! The rule it generalises is the one already written down for cancellation:
//! *the longest this `await` can sleep is the worst-case latency of the thing
//! it is not listening to.* A park owes an arm to **both** the cancel token
//! and this signal.
//!
//! # Why it lives in `session`, not in `gateway`
//!
//! Producer and consumer sit on opposite sides of the gateway boundary: the
//! steering injector is gateway code, the parked tools are `builtin_tools` /
//! `agents`. Hanging the registry off either one would point a dependency
//! backwards. The fact itself — "a user message landed on this session" — is a
//! session-log fact, so it belongs next to the log's own types.
//!
//! # Why it is a pure edge, with no pending flag
//!
//! A level flag ("this session has un-consumed user input") needs a *consume*
//! edge, and the only observable one is the next `AssistantMessage`. That is
//! the boundary [`count_pending_steering`] already uses, and it is an
//! approximation: a steer that lands while the provider call is in flight is
//! marked consumed by that call's assistant turn even though its prompt was
//! built before the steer existed. For the burst cap that costs one turn of
//! accounting. For a park it would be worse in both directions:
//!
//! * flag never cleared (run cancelled / failed before any assistant turn) ⇒
//!   the *next* run's first park returns instantly, forever — an early-return
//!   loop that burns a turn per lap;
//! * flag cleared too eagerly ⇒ the park sleeps through the steer anyway,
//!   i.e. the bug this module exists to fix, now with extra machinery.
//!
//! An edge has neither failure mode: there is no state to go stale, a watcher
//! that is not parked cannot be woken, and a woken watcher re-arms from
//! scratch. The price is precisely one window — a steer that lands *between*
//! the model emitting the tool call and the tool arming its watch does not cut
//! the park short. That window is one tool dispatch wide (microseconds), and
//! [`SteerWatch`] closes even that by using a `watch` channel rather than a
//! `Notify`: the receiver is created at watch time and remembers a send that
//! happened before the first `await`. The genuinely uncovered window is a
//! steer landing during the provider call that *produced* the tool call —
//! seconds, and the harness still reads that steer at the next Think, so the
//! cost is a park that runs its normal course, never a lost message.
//!
//! # Relationship to `AgentHarness::has_unanswered_user_message`
//!
//! Act already has a cooperative steer checkpoint at every tool-group boundary,
//! and it uses that method — a seq-ranged read of the session log against the
//! turn's `last_prompt_seq` watermark. The two are **complementary, not two
//! answers to one question**, and the difference is worth stating because at a
//! glance they look redundant:
//!
//! | | `has_unanswered_user_message` | this module |
//! |---|---|---|
//! | shape | level ("is there one *now*?") | edge ("tell me *when* one lands") |
//! | source | the session log + prompt watermark — authoritative | in-process, advisory |
//! | who can call it | the harness (owns the watermark) | anything inside a turn |
//! | when | at a boundary, between tool calls | *during* a tool call |
//!
//! A parked tool cannot use the level query: it is asleep, so it would have to
//! poll — a seq-ranged store read every tick for the whole 600 s — and it has
//! no access to the watermark that makes the query precise (reaching into
//! `AgentHarness` from a tool is exactly the coupling R10 forbids). An edge is
//! the only shape that fits, and it does not need to be authoritative because
//! it decides nothing: waking early only hands control back to the loop, and
//! the loop then asks the authoritative question at its next boundary.
//!
//! They agree on *what counts* — a non-synthetic `SessionEvent::UserMessage` —
//! and the two mechanisms compose end to end: the park lets go, its group
//! completes, the group boundary's level check defers the remaining calls, and
//! the next Think reads the message.
//!
//! # Why the injector is the producer, and not the append seam
//!
//! The lane's burst-drain edge
//! ([`crate::gateway::execution_engine::wake_lane_if_burst_drained`]) is fired
//! from the projector's "an event was appended" observer, because its question
//! — *did an assistant turn happen?* — has many producers (harness run, fast
//! path, simple engine) and naming them would be a list that rots.
//!
//! This question has exactly one, by construction. It asks *did a human
//! interject into a session that is **already executing**?*, and per-session
//! mutual exclusion (`SessionRunRegistry::try_claim`) means no second writer
//! can put a user message on such a session: every other arrival is either
//! folded here, cancelled ([`BusyInputMode::Interrupt`], which fires the cancel
//! token the same parks already listen to), or parked in the busy lane until
//! the run ends. Reading the append seam instead would additionally have to
//! exclude the run's own seed message and every `synthetic: true` scaffolding
//! event — two predicates whose only job would be to undo the extra coverage.
//!
//! The lane's parked messages are the interesting exclusion, and they are
//! excluded on purpose rather than by omission. A steer the injector *deferred*
//! (attachments, a slash command, a different model — see
//! `carries_more_than_text`) is redelivered later as a **fresh run**; it is not
//! in the running loop's log. Waking a park for it would hand the model a
//! report saying "the user sent new input, read it" about a message it cannot
//! see — a confident lie, and a worse failure than the sleep. The producer must
//! therefore be the point where the message becomes **visible to the model**,
//! not the point where it arrives.
//!
//! "Exactly one producer" is a claim, so it is pinned by a source-level census
//! ([`tests::note_steer_has_exactly_one_production_call_site`]) rather than by
//! this paragraph: a second one turns the sentence above from true into a
//! silent divergence, and the test goes red by name.
//!
//! [`count_pending_steering`]: crate::gateway::execution_engine::steering
//! [`BusyInputMode::Interrupt`]: crate::gateway::execution_engine::BusyInputMode

use std::collections::HashMap;
use std::sync::OnceLock;

use tokio::sync::watch;

use crate::routing::session_key::SessionKey;
use crate::sync_primitives::Mutex;

/// One session's wake channel, alive only while at least one tool is parked on
/// it.
struct Station {
    /// Monotonic steer counter. The value is never read — `watch` fires on
    /// *change*, and a counter is the cheapest thing that always changes.
    tx: watch::Sender<u64>,
    /// Live [`SteerWatch`] count. The station is dropped at zero so an idle
    /// process holds no per-session state (same posture as the busy lane's
    /// empty-lane GC).
    watchers: usize,
}

fn stations() -> &'static Mutex<HashMap<String, Station>> {
    static STATIONS: OnceLock<Mutex<HashMap<String, Station>>> = OnceLock::new();
    STATIONS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn lock() -> crate::sync_primitives::MutexGuard<'static, HashMap<String, Station>> {
    stations().lock().unwrap_or_else(|e| e.into_inner())
}

/// The one place either side spells the registry key.
///
/// Producer and consumer live in different modules and reach the session from
/// different carriers (a `RunRequest` on one side, `TURN_CONTEXT` on the
/// other). Neither is allowed to build the string: both hand over a
/// [`SessionKey`] and this derives it, so a change of spelling cannot connect
/// one side and silently orphan the other.
fn station_key(session: &SessionKey) -> String {
    session.to_key_string()
}

/// Announce that a mid-loop steering message just landed on `session`.
///
/// Called from the steering injector's success arm — see the module doc for
/// why that is the only producer. A session with no parked tool has no station
/// and this is a no-op (fail open, same posture as
/// `busy_queue::waiting_since`).
pub fn note_steer(session: &SessionKey) {
    let map = lock();
    if let Some(station) = map.get(&station_key(session)) {
        // `send_modify`, not `send`: the latter reports an error when the last
        // receiver has gone, and there is nothing to report — a station with
        // no receivers is garbage-collected, so this only ever runs with live
        // watchers.
        station.tx.send_modify(|n| *n = n.wrapping_add(1));
    }
}

/// A parked tool's subscription to its own session's steering edge.
///
/// Obtained from [`watch_current_turn`] and held across the park. Dropping it
/// unsubscribes, and the last drop releases the session's station.
///
/// An **inert** watch (no turn context: cron, internal runs, unit tests) never
/// fires. That is deliberate: it lets the park's `select!` carry the arm
/// unconditionally instead of duplicating itself under an `if let`, which is
/// how the second arm of a two-arm guard gets forgotten.
pub struct SteerWatch(Option<Subscription>);

struct Subscription {
    session_key: String,
    rx: watch::Receiver<u64>,
}

impl SteerWatch {
    /// Resolves the moment a steering message lands on this turn's session.
    ///
    /// Never resolves for an inert watch, nor after the station has gone (a
    /// closed channel is "no signal", never "steered" — reading a teardown as
    /// user input would cut a healthy wait short for nothing).
    ///
    /// Safe to poll repeatedly inside a `tokio::select!`.
    pub async fn steered(&mut self) {
        if let Some(sub) = self.0.as_mut() {
            if sub.rx.changed().await.is_ok() {
                return;
            }
        }
        // Inert, or the sender is gone: park forever so the caller's other arms
        // decide the outcome. A closed channel is "no signal", never "steered".
        std::future::pending::<()>().await
    }

    /// Whether this watch can ever fire — `false` outside a turn scope.
    /// Exposed for tests and for callers that want to say so in a log line.
    #[must_use]
    pub fn is_armed(&self) -> bool {
        self.0.is_some()
    }
}

impl Drop for SteerWatch {
    fn drop(&mut self) {
        let Some(sub) = self.0.take() else {
            return;
        };
        // Drop the receiver before the station could go, so the sender never
        // outlives its last receiver in a way an observer could catch.
        drop(sub.rx);
        let mut map = lock();
        if let Some(station) = map.get_mut(&sub.session_key) {
            station.watchers = station.watchers.saturating_sub(1);
            if station.watchers == 0 {
                map.remove(&sub.session_key);
            }
        }
    }
}

/// Subscribe to the steering edge of the session whose turn is executing this
/// tool call.
///
/// Reads `TURN_CONTEXT` — scoped by `ScopedToolService::execute`, the single
/// production tool-dispatch chokepoint — rather than taking a key from the
/// caller, so the consumer cannot spell the key differently from the producer.
/// Returns an inert watch outside a turn scope.
///
/// **Call this before the state check that decides whether to park.** The
/// subscription is a `watch::Receiver`, so a steer landing between this call
/// and the first `await` on [`SteerWatch::steered`] is remembered rather than
/// lost — but one that lands before this call is not.
#[must_use]
pub fn watch_current_turn() -> SteerWatch {
    let Some(session) = crate::tools::turn_context::current_turn_context() else {
        return SteerWatch(None);
    };
    watch_session(&session.session_key)
}

/// [`watch_current_turn`] against an explicit session — the seam tests use, and
/// the escape hatch for a future parking site that runs outside `TURN_CONTEXT`.
#[must_use]
pub fn watch_session(session: &SessionKey) -> SteerWatch {
    let key = station_key(session);
    let mut map = lock();
    let station = map.entry(key.clone()).or_insert_with(|| Station {
        tx: watch::Sender::new(0),
        watchers: 0,
    });
    station.watchers += 1;
    SteerWatch(Some(Subscription {
        session_key: key,
        rx: station.tx.subscribe(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sk(conv: &str) -> SessionKey {
        SessionKey::peer("main", conv)
    }

    /// The whole point: a watch armed *before* the steer fires on it.
    #[tokio::test]
    async fn a_steer_wakes_a_watcher_parked_on_that_session() {
        let s = sk("steer-wakes");
        let mut watch = watch_session(&s);
        note_steer(&s);
        tokio::time::timeout(std::time::Duration::from_secs(1), watch.steered())
            .await
            .expect("an armed watch must observe a steer on its own session");
    }

    /// A `watch` receiver remembers a send that happened before the first poll.
    /// This is the reason the module uses one instead of a bare `Notify`: with
    /// `notify_waiters` the send below would land with nobody registered and
    /// be dropped, and the park would run its full course.
    #[tokio::test]
    async fn a_steer_between_subscribing_and_awaiting_is_not_lost() {
        let s = sk("steer-no-lost-wakeup");
        let mut watch = watch_session(&s);
        // Not awaited yet — exactly the dispatch-to-`select!` window.
        note_steer(&s);
        tokio::task::yield_now().await;
        tokio::time::timeout(std::time::Duration::from_millis(200), watch.steered())
            .await
            .expect("a send before the first await must still be observed");
    }

    #[tokio::test]
    async fn a_steer_on_another_session_does_not_wake_this_one() {
        let mine = sk("steer-scope-mine");
        let theirs = sk("steer-scope-theirs");
        let mut watch = watch_session(&mine);
        note_steer(&theirs);
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(120), watch.steered())
                .await
                .is_err(),
            "a watch must only observe its own session"
        );
    }

    /// Every parked tool of a session is woken, not just one: an Act phase can
    /// run several waits in parallel and a steer supersedes all of them.
    #[tokio::test]
    async fn every_watcher_on_a_session_is_woken() {
        let s = sk("steer-broadcast");
        let mut a = watch_session(&s);
        let mut b = watch_session(&s);
        note_steer(&s);
        for (label, w) in [("first", &mut a), ("second", &mut b)] {
            tokio::time::timeout(std::time::Duration::from_millis(200), w.steered())
                .await
                .unwrap_or_else(|_| panic!("{label} watcher must be woken"));
        }
    }

    /// No state survives the last watcher, so a steer that arrives while
    /// nothing is parked cannot be replayed into the *next* park — the
    /// early-return loop the module doc rules out.
    #[tokio::test]
    async fn a_steer_with_nobody_parked_does_not_arm_the_next_park() {
        let s = sk("steer-no-replay");
        drop(watch_session(&s));
        note_steer(&s);
        let mut later = watch_session(&s);
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(120), later.steered())
                .await
                .is_err(),
            "an edge must not be replayed to a watcher that arrived after it"
        );
    }

    #[test]
    fn the_station_is_released_when_the_last_watcher_drops() {
        let s = sk("steer-gc");
        let a = watch_session(&s);
        let b = watch_session(&s);
        let key = station_key(&s);
        assert_eq!(lock().get(&key).map(|st| st.watchers), Some(2));
        drop(a);
        assert_eq!(lock().get(&key).map(|st| st.watchers), Some(1));
        drop(b);
        assert!(
            !lock().contains_key(&key),
            "an idle session must hold no station"
        );
    }

    #[test]
    fn a_watch_outside_a_turn_scope_is_inert() {
        assert!(
            !watch_current_turn().is_armed(),
            "no TURN_CONTEXT means nothing to subscribe to"
        );
    }

    /// The module doc's load-bearing claim — "this question has exactly one
    /// producer, by construction" — is only true while it stays true. A second
    /// `note_steer` call site means either a new mid-run user-message path
    /// exists (and the paragraph is now wrong) or someone is firing the edge
    /// for something that is not a human interjection (and every park on that
    /// session will cut short for it).
    ///
    /// Source-level, because at runtime "fired by the injector" and "fired by
    /// something else that happens to look the same" are indistinguishable.
    #[test]
    fn note_steer_has_exactly_one_production_call_site() {
        // Only files that could plausibly hold a producer; the census is over
        // call sites, so it must not count the definition or its own tests.
        let sources: &[(&str, &str)] = &[
            (
                "gateway/execution_engine/steering.rs",
                include_str!("../gateway/execution_engine/steering.rs"),
            ),
            (
                "gateway/execution_engine/gate.rs",
                include_str!("../gateway/execution_engine/gate.rs"),
            ),
            (
                "gateway/execution_engine/execute.rs",
                include_str!("../gateway/execution_engine/execute.rs"),
            ),
            (
                "gateway/session_projector.rs",
                include_str!("../gateway/session_projector.rs"),
            ),
        ];
        let mut sites: Vec<String> = Vec::new();
        for (name, src) in sources {
            // CRLF-safe: this repo checks out with CRLF on Windows, so a
            // separator anchored with a leading `\n` would match nothing and
            // the "production prefix" would silently become the whole file.
            let src = src.replace('\r', "");
            // Cut at the test **module**, not at every `#[cfg(test)]`: that
            // attribute also sits on individual test-only items, and
            // `steering.rs` has one 500 lines ABOVE the producer this census
            // exists to find. Splitting on the bare attribute made the
            // production prefix stop there and the census report zero sites —
            // the "a source-level guard only covers the block shape its
            // recogniser knows" trap, caught here only because the assertion
            // below is `== 1` rather than `<= 1`. Keep it that way: an
            // over-eager splitter must fail loudly, never quietly.
            let production = src.split("#[cfg(test)]\nmod ").next().unwrap_or(&src);
            for line in production.lines() {
                if line.contains("note_steer(") && !line.trim_start().starts_with("//") {
                    sites.push(format!("{name}: {}", line.trim()));
                }
            }
        }
        assert_eq!(
            sites.len(),
            1,
            "note_steer must have exactly one production producer (the steering \
             injector); found: {sites:#?}"
        );
        assert!(
            sites[0].starts_with("gateway/execution_engine/steering.rs"),
            "the sole producer must be the steering injector, found {}",
            sites[0]
        );
    }
}
