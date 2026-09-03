//! Agent-state sampling: screen text -> agent-detect -> RuntimeAgentEntry.
//!
//! Sampling rides the existing `pty.screen` diff-frame cadence. It does NOT
//! start its own timer — two clocks are two orderings (judgment §12). The
//! caller is `gateway::pty::manager::flush_session`, inside the arm that
//! already proved the screen changed.
//!
//! The table is keyed by PTY session id and is the single source for the
//! `runtime.*` surface. Entries appear when a session first produces a frame
//! and are REMOVED by the reader thread at child exit
//! (`gateway::pty::session::spawn_reader`) — one mechanism, not a
//! prune-on-read second opinion about whether a session is alive.
//!
//! # Time
//!
//! Every timestamp in this module — [`RuntimeAgentEntry::updated_at`] on the
//! wire and `pending_idle_since` internally — is **unix epoch milliseconds**,
//! and `now` is a parameter rather than a call to the clock. One unit, one
//! source: [`IDLE_HOLD_MS`] is 700 ms, which does not survive a round trip
//! through seconds, and a struct carrying two units is the shape judgment §12
//! names. Production passes `chrono::Utc::now().timestamp_millis()` from the
//! flush loop; tests pass literals and never sleep.

use std::collections::HashMap;
use std::sync::LazyLock;

use agent_detect::AgentState;
use aleph_protocol::runtime::{RuntimeAgentEntry, RuntimeAgentState};

use crate::gateway::pty::screen::Screen;
use crate::sync_primitives::Mutex;

/// Aleph's `osc_dispatch` handles OSC 0/2 (title) but NOT OSC 9;4
/// (ConEmu progress), so this phase has no producer for `osc_progress`.
/// The detection engine treats an empty string as "unavailable" and falls
/// back to its pre-OSC behaviour — correct, just weaker.
///
/// This is a DELIBERATE degradation, not an oversight. Wiring OSC 9;4 is
/// registered in the phase 0-A gap list. Do not read this as "no progress".
pub const OSC_PROGRESS_UNAVAILABLE: &str = "";

/// How long a working -> plain-idle observation is held before it is believed.
///
/// Not a new number: it is upstream's cap, herdr 0.8.2
/// `src/pane/agent_detection.rs:8-9` (`AGENT_PENDING_IDLE_CAP`,
/// `Duration::from_millis(700)`).
///
/// Upstream releases the hold on EITHER three confirmations or this cap, and
/// shortens its poll sleep while a hold is pending (`herdr src/pane.rs:727`)
/// so the confirmations can accumulate. Aleph has no such recheck — a frame
/// exists only when the screen changed, and an agent that finishes and goes
/// quiet emits no further frame. So the confirmation COUNT is dropped and
/// wall clock alone governs: counting re-observations that are not guaranteed
/// to arrive would strand a finished agent at Working forever, which is worse
/// than no damping at all. Release is [`RuntimeAgents::release_expired`],
/// driven by the flush ticker that already runs — not a second clock.
const IDLE_HOLD_MS: i64 = 700;

/// One row of the table: the wire entry plus the sampler's own bookkeeping.
///
/// `pending_idle_since` is deliberately NOT on [`RuntimeAgentEntry`]: it is
/// how this module reaches its answer, not part of the answer. A client that
/// could see it would have a second, earlier opinion about the same state.
#[derive(Debug, Clone)]
struct TableEntry {
    entry: RuntimeAgentEntry,
    /// Unix millis at which a working -> plain-idle transition was first
    /// observed, while it is being held. `None` = nothing held.
    pending_idle_since: Option<i64>,
}

/// The process-wide agent table.
///
/// A `LazyLock` singleton reached through [`agents()`], mirroring
/// `pty::manager()` in this same subsystem: the JSON-RPC handler (task 6) and
/// the tool face (task 11) both need it and neither has an `AppContext` to
/// thread it through. `Default` exists so a test can hold an isolated
/// instance instead of racing the global one.
#[derive(Default)]
pub struct RuntimeAgents {
    entries: Mutex<HashMap<String, TableEntry>>,
}

static GLOBAL: LazyLock<RuntimeAgents> = LazyLock::new(RuntimeAgents::default);

/// Access the process-global agent table.
#[must_use]
pub fn agents() -> &'static RuntimeAgents {
    &GLOBAL
}

impl RuntimeAgents {
    /// Fold one screen observation into the table. Returns whether anything
    /// OBSERVABLE changed — `state`, `agent`, `label` or `cwd`.
    ///
    /// The return value is what task 6's `runtime.agents.changed` must key on.
    /// `updated_at` advances only when this returns `true`, so "the entry
    /// differs from last time" and "something happened" are the same
    /// question: `RuntimeAgentEntry` derives `PartialEq`, and a timestamp
    /// rewritten on every frame would make the natural `old != new` predicate
    /// fire at the 16 ms flush cadence with nothing to report.
    ///
    /// `shell` is [`crate::gateway::pty::PtySession::shell`] — the
    /// human-readable program label, which is what
    /// `agent_detect::identify_agent` matches against.
    ///
    /// ⚠️ **KNOWN GAP, phase 1: in production this never identifies anything,
    /// so every row publishes `Unknown`.** `shell` is the SPAWN-TIME label:
    /// [`crate::gateway::pty::session::SpawnOptions`] with no `command` sets
    /// it to the platform default shell, and the only production `pty.spawn`
    /// client (the Panel's terminal view) sends `{rows, cols}` and nothing
    /// else. So `identify_agent` is handed `"zsh"`, returns `None`, and
    /// `detect_agent_with_osc` early-returns `Unknown` before any manifest
    /// rule is consulted. The 29 manifests, the rule engine and the idle-hold
    /// are all correct and all unreachable from here.
    ///
    /// The gap is not "no caller passes `command`" — it is that agents are
    /// started INTERACTIVELY, after the spawn. A user who opens a terminal and
    /// types `claude` leaves this label at `"zsh"` while the screen fills with
    /// Claude's UI, so passing `command` at spawn would fix only the case
    /// nobody uses. What identification actually needs is the PTY's current
    /// foreground process, which nothing in this codebase tracks yet.
    ///
    /// This ships deliberately: the panels render `Unknown` as its own `?`
    /// glyph, never Idle's, so the shipped surface says "I cannot tell" rather
    /// than claiming a state it does not have. Closing the gap is phase-2
    /// work (it needs per-platform foreground-process lookup, which R1 makes a
    /// decision rather than a detail) and is what step 0-A is for. Do not
    /// "fix" it by deleting the `agent: None` early return — that would turn
    /// "I do not know" into "it is idle", which is the one thing the panels
    /// are careful not to say.
    ///
    /// `cwd` is
    /// [`crate::gateway::pty::PtySession::cwd`], the SPAWN directory.
    /// `process_exited` is the session's `is_closed()`: a session killed but
    /// not yet reaped by its reader thread is still in the registry for up to
    /// one flush tick, and reporting it as still Working for that tick would
    /// be a stale answer rather than a missing one. `now` is unix millis —
    /// see the module doc on time.
    ///
    /// One lock acquisition on the table; the caller holds the screen lock
    /// for the duration.
    pub fn sample(
        &self,
        session_id: &str,
        shell: &str,
        cwd: &str,
        screen: &Screen,
        process_exited: bool,
        now: i64,
    ) -> bool {
        let text = screen.visible_text();
        let title = screen.title().unwrap_or_default();

        let agent = agent_detect::identify_agent(shell);
        let detection = agent_detect::screen_rules::detection_update_for_publish_with_osc(
            agent,
            &text,
            title,
            OSC_PROGRESS_UNAVAILABLE,
            process_exited,
        );

        // The OSC title is what an agent paints to say what it is working on;
        // it is strictly better than the program name when the program set
        // one. An OSC 0 carrying an empty payload is not a title.
        let label = if title.is_empty() {
            shell.to_owned()
        } else {
            title.to_owned()
        };
        let agent_name = agent.map(|a| agent_detect::agent_label(a).to_owned());

        let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        let previous = entries.get(session_id);
        let previous_state = previous.map_or(RuntimeAgentState::Unknown, |t| t.entry.state);
        let previous_updated_at = previous.map_or(now, |t| t.entry.updated_at);
        let mut pending_idle_since = previous.and_then(|t| t.pending_idle_since);

        let state = match detection {
            // `None` = a manifest rule with `skip_state_update` matched: the
            // screen is mid-repaint and carries no statement about the agent.
            // Keep what the last frame established, and keep any hold running
            // — a non-statement is not evidence either way (判据 §8).
            None => previous_state,
            Some(d) => {
                let observed = wire_state(d.state);
                // Upstream's hold: working -> idle with no VISIBLE idle
                // evidence is the engine's known-agent fallback guessing, not
                // the agent saying so. Hold Working until the cap.
                // `visible_idle` bypasses it (herdr
                // `visible_idle_bypasses_plain_idle_hold`), and so does an
                // exited process — `detection_update_for_publish_with_osc`
                // returns `visible_idle: true` for that case, which is why
                // upstream's separate `!process_exited` term needs no
                // restatement here.
                //
                // Upstream's other two terms are omitted because neither can
                // vary here. `!next.visible_blocker` (herdr
                // `agent_detection.rs:50`): `manifest.rs:470` computes
                // `visible_blocker: rule.visible_blocker && state == Blocked`,
                // so `state == Idle && visible_blocker` is unreachable.
                // `!agent_changed` (`:51`): a session's `shell` is fixed at
                // spawn, so the identified agent cannot change within one
                // session — this term comes back the day a phase identifies
                // the agent from something mutable (an OSC title, a PID
                // probe).
                if observed == RuntimeAgentState::Idle
                    && previous_state == RuntimeAgentState::Working
                    && !d.visible_idle
                {
                    pending_idle_since = Some(pending_idle_since.unwrap_or(now));
                    RuntimeAgentState::Working
                } else {
                    pending_idle_since = None;
                    observed
                }
            }
        };

        let changed = previous.is_none_or(|t| {
            t.entry.state != state
                || t.entry.agent != agent_name
                || t.entry.label != label
                || t.entry.cwd != cwd
        });

        entries.insert(
            session_id.to_owned(),
            TableEntry {
                entry: RuntimeAgentEntry {
                    session_id: session_id.to_owned(),
                    label,
                    cwd: cwd.to_owned(),
                    agent: agent_name,
                    state,
                    updated_at: if changed { now } else { previous_updated_at },
                },
                pending_idle_since,
            },
        );
        changed
    }

    /// Believe every held working -> plain-idle observation that is now at
    /// least [`IDLE_HOLD_MS`] old. Returns the session ids that flipped.
    ///
    /// Called once per flush tick from `pty::manager::start_flush_loop`,
    /// AFTER the per-session pass. That ticker already runs every
    /// `FLUSH_INTERVAL` regardless of whether any session produced a frame,
    /// so this is the same clock the sampler rides — not a second one. It has
    /// to be independent of frames: the whole point is the agent that went
    /// quiet, which by definition produces none.
    ///
    /// The returned ids are what task 6 will emit `runtime.agents.changed`
    /// for. Nothing in production consumes the `Vec` yet — the state flip is
    /// the effect that matters today.
    pub fn release_expired(&self, now: i64) -> Vec<String> {
        let mut flipped = Vec::new();
        let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        for (id, row) in entries.iter_mut() {
            let Some(since) = row.pending_idle_since else {
                continue;
            };
            if now.saturating_sub(since) >= IDLE_HOLD_MS {
                row.pending_idle_since = None;
                row.entry.state = RuntimeAgentState::Idle;
                row.entry.updated_at = now;
                flipped.push(id.clone());
            }
        }
        flipped.sort();
        flipped
    }

    /// The current table, ordered by session id.
    ///
    /// Ordered because a `HashMap` iteration order is not one: an unordered
    /// list would make task 6's change event fire on reshuffles that are not
    /// changes, and the order a client renders must be derived in one place
    /// (判据 §12).
    #[must_use]
    pub fn snapshot(&self) -> Vec<RuntimeAgentEntry> {
        let mut out: Vec<RuntimeAgentEntry> = self
            .entries
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .map(|t| t.entry.clone())
            .collect();
        out.sort_by(|a, b| a.session_id.cmp(&b.session_id));
        out
    }

    /// Drop a session's entry. Called from the PTY reader thread's exit path
    /// beside `pty::manager().remove`, so "the session is gone" has exactly
    /// one answer.
    pub fn remove(&self, session_id: &str) {
        self.entries
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(session_id);
    }
}

/// Exhaustive by construction — no `_ =>` arm. A wildcard here would fan a
/// newly added detection state into whatever the catch-all names, silently
/// (判据 §2); this way the compiler names the file to edit.
const fn wire_state(state: AgentState) -> RuntimeAgentState {
    match state {
        AgentState::Idle => RuntimeAgentState::Idle,
        AgentState::Working => RuntimeAgentState::Working,
        AgentState::Blocked => RuntimeAgentState::Blocked,
        AgentState::Unknown => RuntimeAgentState::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::pty::{PtySession, SpawnOptions};

    /// A screen carrying `bytes`, at the size the tests use throughout.
    fn screen(bytes: &[u8]) -> Screen {
        let mut s = Screen::new(4, 40);
        s.feed(bytes);
        s
    }

    /// 证伪守卫：剪断 osc_title 的接线，title 必须变空。
    /// 一条不会变红的守卫不是守卫（判据 §3）。
    ///
    /// `shell` is deliberately a name the OSC title does not equal, so
    /// cutting the title read makes `label` fall back to it and this
    /// assertion goes red. Falsified by hand on 2026-09-02 — see the task
    /// report.
    #[test]
    fn the_title_wire_is_actually_connected() {
        let s = screen(b"\x1b]0;my-agent\x07idle");
        let agents = RuntimeAgents::default();
        agents.sample("s1", "sh", "", &s, false, 0);
        assert_eq!(agents.snapshot()[0].label, "my-agent");
    }

    /// osc_progress 本期没有生产者，必须是空串。
    /// 空串只有资格说「我不知道」，不许被读成「没有进度」（判据 §8）。
    #[test]
    fn osc_progress_has_no_producer_this_phase() {
        assert_eq!(OSC_PROGRESS_UNAVAILABLE, "");
    }

    /// `cwd` is the SPAWN directory, and it has to be able to DIFFER from
    /// empty — a field that is empty for every session is a predicate that
    /// cannot vary (判据 §2), which is what pinning it to a constant was.
    /// Empty still means "the spawn inherited the server's directory", never
    /// the filesystem root.
    #[test]
    fn the_spawn_cwd_reaches_the_entry_and_empty_means_inherited() {
        let s = screen(b"$ ");
        let agents = RuntimeAgents::default();

        agents.sample("chosen", "sh", "/tmp/aleph-cwd-probe", &s, false, 0);
        agents.sample("inherited", "sh", "", &s, false, 0);

        let snap = agents.snapshot();
        let chosen = snap.iter().find(|e| e.session_id == "chosen").unwrap();
        let inherited = snap.iter().find(|e| e.session_id == "inherited").unwrap();
        assert_eq!(chosen.cwd, "/tmp/aleph-cwd-probe");
        assert_eq!(inherited.cwd, "");
    }

    /// A program the bundled manifest does not know is `agent: None` — never
    /// a guessed name — and its state is Unknown, not Idle.
    #[test]
    fn an_unrecognised_program_is_none_and_unknown() {
        let s = screen(b"$ ");
        let agents = RuntimeAgents::default();
        agents.sample("s1", "sh", "", &s, false, 0);
        let e = &agents.snapshot()[0];
        assert_eq!(e.agent, None);
        assert_eq!(e.state, RuntimeAgentState::Unknown);
    }

    /// The `process_exited` input is a real input, not a literal the caller
    /// always passes `false`: the same screen answers differently either
    /// side of it. Without this the exited arm of
    /// `detection_update_for_publish_with_osc` would be unreachable from
    /// here and the parameter would be decoration (判据 §2).
    #[test]
    fn an_exited_session_reads_idle_not_the_screens_answer() {
        let s = screen(b"$ ");
        let agents = RuntimeAgents::default();

        agents.sample("live", "sh", "", &s, false, 0);
        agents.sample("dead", "sh", "", &s, true, 0);

        let snap = agents.snapshot();
        let dead = snap.iter().find(|e| e.session_id == "dead").unwrap();
        let live = snap.iter().find(|e| e.session_id == "live").unwrap();
        assert_eq!(dead.state, RuntimeAgentState::Idle);
        assert_eq!(live.state, RuntimeAgentState::Unknown);
    }

    /// `updated_at` is the time of the last OBSERVABLE change, not of the
    /// last sample. `RuntimeAgentEntry` derives `PartialEq`, so a timestamp
    /// rewritten every frame turns task 6's natural `old != new` predicate
    /// into a ~60 Hz broadcast of an unchanged state.
    ///
    /// `now` is a parameter precisely so this test states the times instead
    /// of sleeping for them.
    #[test]
    fn updated_at_advances_only_when_something_observable_changed() {
        let s = screen(b"$ ");
        let agents = RuntimeAgents::default();

        assert!(
            agents.sample("s1", "sh", "", &s, false, 1_000),
            "a new session is a change"
        );
        assert_eq!(agents.snapshot()[0].updated_at, 1_000);

        assert!(
            !agents.sample("s1", "sh", "", &s, false, 9_999),
            "an identical observation is not a change"
        );
        assert_eq!(
            agents.snapshot()[0].updated_at,
            1_000,
            "updated_at must not follow the clock"
        );

        assert!(
            agents.sample("s1", "sh", "/elsewhere", &s, false, 12_000),
            "a different cwd is a change"
        );
        assert_eq!(agents.snapshot()[0].updated_at, 12_000);
    }

    /// Upstream damps the STATE, not the announcement: while the hold is
    /// active the pane's own `state` stays Working (herdr `src/pane.rs:266`
    /// is reached only from the `Publish` arm). Mirrors herdr's
    /// `pending_idle_holds_working_to_plain_idle_until_confirmed`, with the
    /// confirmation count replaced by wall clock — see [`IDLE_HOLD_MS`].
    ///
    /// `claude`'s `osc_title_working` rule drives Working; a title matching
    /// no rule leaves the engine on its known-agent idle fallback, which is
    /// Idle with `visible_idle: false` — exactly the "plain idle" upstream
    /// distrusts.
    #[test]
    fn a_plain_idle_after_working_is_held_until_the_cap() {
        let agents = RuntimeAgents::default();
        let working = screen("\x1b]0;⠋ building\x07".as_bytes());
        let plain_idle = screen("\x1b]0;claude\x07".as_bytes());

        agents.sample("s1", "claude", "", &working, false, 1_000);
        assert_eq!(
            agents.snapshot()[0].state,
            RuntimeAgentState::Working,
            "the working chrome must be detected, or this test proves nothing"
        );

        agents.sample("s1", "claude", "", &plain_idle, false, 2_000);
        assert_eq!(
            agents.snapshot()[0].state,
            RuntimeAgentState::Working,
            "plain idle is held, not written"
        );

        assert!(
            agents.release_expired(2_000 + IDLE_HOLD_MS - 1).is_empty(),
            "one millisecond before the cap nothing is released"
        );
        assert_eq!(agents.snapshot()[0].state, RuntimeAgentState::Working);

        let flipped = agents.release_expired(2_000 + IDLE_HOLD_MS);
        assert_eq!(flipped, vec!["s1".to_string()]);
        assert_eq!(agents.snapshot()[0].state, RuntimeAgentState::Idle);
        assert_eq!(
            agents.snapshot()[0].updated_at,
            2_000 + IDLE_HOLD_MS,
            "the flip is an observable change and carries its own time"
        );
    }

    /// Upstream's bypass: a screen carrying VISIBLE idle evidence is believed
    /// at once. Mirrors herdr's `visible_idle_bypasses_plain_idle_hold`.
    /// Without this arm the damper would delay every legitimate finish by
    /// 700 ms.
    #[test]
    fn visible_idle_bypasses_the_hold() {
        let agents = RuntimeAgents::default();
        let working = screen("\x1b]0;⠋ building\x07".as_bytes());
        let visible_idle = screen("\x1b]0;✳ Claude Code\x07".as_bytes());

        agents.sample("s1", "claude", "", &working, false, 1_000);
        assert_eq!(agents.snapshot()[0].state, RuntimeAgentState::Working);

        agents.sample("s1", "claude", "", &visible_idle, false, 2_000);
        assert_eq!(
            agents.snapshot()[0].state,
            RuntimeAgentState::Idle,
            "visible idle evidence is not a guess, so it is not held"
        );
        assert!(
            agents.release_expired(2_000 + IDLE_HOLD_MS).is_empty(),
            "nothing was pending, so nothing can be released"
        );
    }

    /// THE WIRE, half one. The `sample()` tests above prove the function;
    /// this one proves it has a caller. It spawns a real child, drives the
    /// same per-session body the flush ticker drives, and asserts the session
    /// landed in the PROCESS table — the one task 6 and task 11 read.
    ///
    /// It also carries the cwd across the seam: without that, the
    /// `PtySession.cwd` field and the argument at `manager.rs` would be
    /// proven only by a direct `sample()` call, one field short of the same
    /// 判据 §7 gap this test exists to close.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_published_frame_lands_the_session_in_the_table() {
        let id = "t-runtime-wire";
        agents().remove(id);

        let spawn_dir = std::env::temp_dir().to_string_lossy().into_owned();
        let opts = SpawnOptions {
            command: Some(if cfg!(windows) { "cmd.exe" } else { "sh" }.to_string()),
            args: if cfg!(windows) {
                vec!["/C".into(), "echo ALEPH_RUNTIME_WIRE & pause".into()]
            } else {
                vec!["-c".into(), "printf 'ALEPH_RUNTIME_WIRE'; sleep 30".into()]
            },
            cwd: Some(spawn_dir.clone()),
            rows: 6,
            cols: 40,
            ..Default::default()
        };
        let session = PtySession::spawn(id.into(), &opts, None).expect("spawn");

        let mut framed = false;
        for _ in 0..100 {
            let now = chrono::Utc::now().timestamp_millis();
            if crate::gateway::pty::manager::flush_session(&session, now).is_some() {
                framed = true;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        assert!(framed, "the child produced no frame in 2s");

        let entry = agents().snapshot().into_iter().find(|e| e.session_id == id);
        session.kill();
        agents().remove(id);

        let entry = entry.expect("a flushed frame must land the session in the process table");
        assert_eq!(
            entry.cwd, spawn_dir,
            "the spawn directory must cross the seam, not just reach sample()"
        );
    }

    /// Whether a raw bus frame is a `runtime.agents.changed` topic event —
    /// via the protocol constant, not a re-typed literal (fix round 1,
    /// review Minor 6's reasoning applied here too: a rename of the
    /// constant must redden every reader, not just the ones that remembered
    /// to update their copy of the string).
    fn is_agents_changed(raw: &str) -> bool {
        serde_json::from_str::<serde_json::Value>(raw)
            .ok()
            .and_then(|v| v.get("topic").and_then(|t| t.as_str().map(str::to_owned)))
            .as_deref()
            == Some(aleph_protocol::runtime::RUNTIME_AGENTS_CHANGED_TOPIC)
    }

    /// THE EVENT WIRE (task 6, fix round 1). `sample()` reporting a change
    /// must reach a real bus subscriber as `runtime.agents.changed`; a frame
    /// whose detected state/agent/label/cwd are unchanged must publish
    /// NOTHING — the "not on every frame" rule from R6-4 — and the exit path
    /// must publish once more.
    ///
    /// `start_flush_loop` cannot be driven from a test (see
    /// `the_flush_loop_body_calls_the_sampler_and_the_release`'s doc), so
    /// this drives the exact per-tick decision it makes instead: ONE call to
    /// `crate::gateway::pty::manager::publish_agents_changed_if(changed, &bus)`
    /// per frame — the `if` now lives INSIDE that function (fix round 1,
    /// review F4/Minor 8), not re-implemented here, so this test asserts
    /// what the helper actually does rather than a copy of its logic.
    ///
    /// Assertions are by ORDER, not by a final count: `bus.publish` is
    /// synchronous, so `try_recv` immediately after each call is
    /// deterministic (no wait needed for the first two steps) — this closes
    /// review Minor 4 (the old "seen >= 2" could not tell {frame1, frame2}
    /// from {frame1, exit}; this can, because each step drains and asserts
    /// before the next one runs).
    ///
    /// Three specific deletions redden three specific steps:
    /// - deleting the `if !changed { return; }` guard inside
    ///   `publish_agents_changed_if` reddens the FRAME 2 step (it would then
    ///   see an event instead of empty);
    /// - deleting the `bus.publish(...)` line inside that same function
    ///   reddens the FRAME 1 step (it would see empty instead of an event);
    /// - deleting the exit-site call in `session.rs` reddens the EXIT step.
    ///
    /// The source pin (`the_flush_loop_body_calls_the_sampler_and_the_release`)
    /// still covers the one thing this test cannot: that `start_flush_loop`'s
    /// own body still calls `publish_agents_changed_if` at all, rather than
    /// this test's own direct calls being the only production-shaped caller
    /// left standing.
    ///
    /// The command below writes "first", waits for the caller to unblock a
    /// `read`/`pause`, then writes "second" — demand-driven rather than a
    /// fixed sleep window (fix round 1, review Minor 5: a fixed-delay second
    /// write can land before the FIRST `flush_session` poll on a loaded
    /// runner, leaving no second frame to observe at all). What must NOT
    /// change between the two frames is the *detected* state: `shell` here
    /// is `"sh"`/`"cmd.exe"`, which `agent_detect::identify_agent` does not
    /// recognise, and an unrecognised agent is `Unknown` "regardless of
    /// screen content" (`agent_detect`'s own doc, judgment §8) — so
    /// differing visible text is exactly the case that must NOT count as a
    /// change here.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_changed_sample_reaches_the_bus_an_unchanged_one_does_not_and_exit_publishes_once() {
        let id = "t-runtime-event-wire";
        agents().remove(id);

        let bus =
            crate::sync_primitives::Arc::new(crate::gateway::event_bus::GatewayEventBus::new());
        let mut rx = bus.subscribe();

        let opts = SpawnOptions {
            command: Some(if cfg!(windows) { "cmd.exe" } else { "sh" }.to_string()),
            args: if cfg!(windows) {
                // `pause` waits for exactly one keystroke on stdin — the
                // Windows analogue of `read x` below.
                vec![
                    "/C".into(),
                    "echo first & pause & echo second & ping -n 30 127.0.0.1 >nul".into(),
                ]
            } else {
                vec![
                    "-c".into(),
                    "printf 'first'; read x; printf 'second'; sleep 30".into(),
                ]
            },
            rows: 6,
            cols: 40,
            ..Default::default()
        };
        let session = PtySession::spawn(id.into(), &opts, Some(bus.clone())).expect("spawn");

        // Frame 1: a brand-new session's first observation is unconditionally
        // a change (`sample`'s `previous.is_none_or(...)` — nothing to
        // compare against yet).
        let mut changed1 = None;
        for _ in 0..100 {
            let now = chrono::Utc::now().timestamp_millis();
            if let Some((_frame, changed)) =
                crate::gateway::pty::manager::flush_session(&session, now)
            {
                changed1 = Some(changed);
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        let changed1 = changed1.expect("the child produced no first frame in 2s");
        assert!(
            changed1,
            "a brand-new session's first frame must be reported as a change, \
             or this test proves nothing"
        );

        crate::gateway::pty::manager::publish_agents_changed_if(changed1, &bus);
        assert!(
            matches!(rx.try_recv(), Ok(raw) if is_agents_changed(&raw)),
            "publish_agents_changed_if(true, ..) must deliver runtime.agents.changed \
             to a real subscriber"
        );
        assert!(
            rx.try_recv().is_err(),
            "exactly one event for frame 1 — no more"
        );

        // Unblock the child's `read`/`pause` so it writes its second,
        // DIFFERENT visible text on demand rather than racing a fixed sleep.
        session.write_input(b"\r\n").expect("write to unblock read");

        // Frame 2: different visible text, same detected (Unknown) state —
        // must NOT be reported as a change.
        let mut changed2 = None;
        for _ in 0..100 {
            let now = chrono::Utc::now().timestamp_millis();
            if let Some((_frame, changed)) =
                crate::gateway::pty::manager::flush_session(&session, now)
            {
                changed2 = Some(changed);
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        let changed2 = changed2.expect("the child produced no second frame in 2s");
        assert!(
            !changed2,
            "a second frame with an unchanged detected state must not be \
             reported as a change"
        );

        crate::gateway::pty::manager::publish_agents_changed_if(changed2, &bus);
        assert!(
            rx.try_recv().is_err(),
            "publish_agents_changed_if(false, ..) must publish nothing"
        );

        // Exit path: kill the child; the reader thread's real EOF handling
        // must publish once more (`session.rs`, beside `agents().remove`) —
        // bounded poll, since this crosses a real thread boundary.
        session.kill();

        let mut seen_exit = false;
        for _ in 0..200 {
            if let Ok(raw) = rx.try_recv() {
                if is_agents_changed(&raw) {
                    seen_exit = true;
                    break;
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        agents().remove(id);
        assert!(
            seen_exit,
            "the exit path must publish one more runtime.agents.changed"
        );
        assert!(
            rx.try_recv().is_err(),
            "exactly one event from the exit path — no more"
        );
    }

    /// F2 (fix round 1, review Important 2): a row landing in the process
    /// table must actually reach the caller through `runtime.agents.list` —
    /// a handler that always returned `agents: vec![]`, or whose filter
    /// dropped every row, would pass every other test in this module.
    ///
    /// Ownership is REAL here, not the `Unknown`-admits-unscoped-caller
    /// fallback the other tests in this file lean on: the session is
    /// spawned through `pty::manager().spawn()` (not the bare
    /// `PtySession::spawn()` the flush-wire tests use), so
    /// `PtyManager::owner_of` has an actual `Known(Some("u-owner"))` record
    /// — the same mechanism `handlers::pty::require_owned` filters on. The
    /// table row itself is seeded directly via `sample()` on a synthetic
    /// screen (the `screen()` helper above): this test's subject is the RPC
    /// face's ownership filter, not the flush wire, which
    /// `a_published_frame_lands_the_session_in_the_table` above already
    /// proves.
    #[tokio::test(flavor = "multi_thread")]
    #[serial_test::parallel(pty_global_manager)]
    async fn a_row_reaches_the_caller_through_the_list_rpc_filtered_by_owner() {
        let opts = SpawnOptions {
            created_by: Some("u-owner".to_string()),
            ..Default::default()
        };
        let spawn = crate::gateway::pty::manager().spawn(&opts).expect("spawn");
        let id = spawn.session_id.clone();
        agents().remove(&id);

        let s = screen(b"$ ");
        agents().sample(&id, &spawn.shell, "", &s, false, 0);

        let list_req = || crate::gateway::protocol::JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "runtime.agents.list".to_string(),
            params: Some(serde_json::json!({})),
            id: Some(serde_json::json!(1)),
        };
        let agent_ids = |resp: &crate::gateway::protocol::JsonRpcResponse| -> Vec<String> {
            let parsed: aleph_protocol::runtime::RuntimeAgentsListResponse =
                serde_json::from_value(resp.result.clone().expect("list always succeeds"))
                    .expect("must be the protocol shape");
            parsed.agents.into_iter().map(|e| e.session_id).collect()
        };

        let owner_resp = crate::gateway::caller_identity::CALLER_USER
            .scope(
                Some("u-owner".to_string()),
                crate::gateway::handlers::runtime::handle_list(list_req()),
            )
            .await;
        let owner_ids = agent_ids(&owner_resp);
        assert!(
            owner_ids.contains(&id),
            "the owner must see their own row through runtime.agents.list: {owner_ids:?}"
        );

        let other_resp = crate::gateway::caller_identity::CALLER_USER
            .scope(
                Some("u-other".to_string()),
                crate::gateway::handlers::runtime::handle_list(list_req()),
            )
            .await;
        let other_ids = agent_ids(&other_resp);
        assert!(
            !other_ids.contains(&id),
            "a different actor must not see another owner's row: {other_ids:?}"
        );

        crate::gateway::pty::manager().close(&id).ok();
        agents().remove(&id);
    }

    /// Spec §5: PTY 会话消失 ⇒ 条目消失. Asserts presence FIRST — otherwise a
    /// child that exits before the first flush would let this test pass by
    /// observing an absence that was never a presence (判据 §2).
    #[tokio::test(flavor = "multi_thread")]
    async fn a_session_that_exits_leaves_the_table() {
        let id = "t-runtime-exit";
        agents().remove(id);

        let opts = SpawnOptions {
            command: Some(if cfg!(windows) { "cmd.exe" } else { "sh" }.to_string()),
            args: if cfg!(windows) {
                vec![
                    "/C".into(),
                    "echo ALEPH_RUNTIME_EXIT & ping -n 3 127.0.0.1 >nul".into(),
                ]
            } else {
                vec!["-c".into(), "printf 'ALEPH_RUNTIME_EXIT'; sleep 1".into()]
            },
            rows: 6,
            cols: 40,
            ..Default::default()
        };
        let session = PtySession::spawn(id.into(), &opts, None).expect("spawn");

        let mut present = false;
        for _ in 0..100 {
            let now = chrono::Utc::now().timestamp_millis();
            crate::gateway::pty::manager::flush_session(&session, now);
            if agents().snapshot().iter().any(|e| e.session_id == id) {
                present = true;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        assert!(
            present,
            "the session never landed in the table to begin with"
        );

        let mut gone = false;
        for _ in 0..200 {
            if !agents().snapshot().iter().any(|e| e.session_id == id) {
                gone = true;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        session.kill();
        agents().remove(id);
        assert!(gone, "the reader thread's exit path must drop the entry");
    }

    /// THE WIRE, half two — the frame the behavioural test cannot reach.
    ///
    /// `a_published_frame_lands_the_session_in_the_table` calls
    /// `flush_session` directly, so reverting `start_flush_loop`'s body to
    /// `session.feed_and_take_frame()` — a plausible merge resolution — leaves
    /// every test in this module and the whole `pty::` suite green while the
    /// table is empty forever in production (判据 §7). `start_flush_loop`
    /// takes `&'static self` and latches on a process-global `STARTED`, so it
    /// cannot be driven from a test; the repo's answer for exactly this class
    /// is a source-level pin (precedents:
    /// `execution_engine/run_loop/flow_scope_census.rs`,
    /// `orchestrator/dispatch.rs::the_harness_spawn_reestablishes_the_run_tree_originator`).
    ///
    /// `release_expired` is pinned by the same test and for the same reason:
    /// it is the only thing that ever releases a held idle, and it too has a
    /// single unwitnessed call site.
    ///
    /// Task 6 adds a third thing this same unwitnessed body must do: fold
    /// both triggers (`flush_session` reporting `changed` for any session
    /// touched this tick, and `release_expired` returning a non-empty `Vec`)
    /// into one bool and call `publish_agents_changed_if` with it — ONE call
    /// site (fix round 1, review F4/Minor 8 — coalesced per tick, so the
    /// two triggers share the one call rather than each getting their own).
    /// The behavioural half of this proof
    /// (`a_changed_sample_reaches_the_bus_an_unchanged_one_does_not_and_exit_publishes_once`)
    /// drives `publish_agents_changed_if` directly against a real bus and
    /// cannot see whether `start_flush_loop` itself still calls it — this
    /// pin covers the call site's existence, not whether the bool handed to
    /// it is folded correctly from both triggers.
    ///
    /// A missing file or a missing/renamed function FAILS — it does not skip.
    /// A guard that goes quiet when it cannot find its subject is not a guard
    /// (判据 §2).
    #[test]
    fn the_flush_loop_body_calls_the_sampler_and_the_release() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src")
            .join("gateway")
            .join("pty")
            .join("manager.rs");
        let src = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));

        // `code_text` rather than the bare `strip_comment_lines`: it also
        // drops string-literal PAYLOADS, so the brace walk below cannot be
        // thrown off by a `{` inside a message. Comments go either way.
        let code = crate::utils::source_scan::code_text(
            &crate::utils::source_scan::production_prefix(&src),
        );

        let at = code.find("fn start_flush_loop").expect(
            "start_flush_loop not found in manager.rs — if it was renamed, \
             re-point this pin; if it was deleted, the flush loop is gone",
        );
        let open = code[at..].find('{').expect("start_flush_loop has no body") + at;
        let mut depth = 0usize;
        let mut close = None;
        for (i, ch) in code[open..].char_indices() {
            match ch {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        close = Some(open + i);
                        break;
                    }
                }
                _ => {}
            }
        }
        let close = close.expect("start_flush_loop's body is not brace-balanced");
        let body = &code[open..=close];

        assert!(
            body.contains("flush_session("),
            "start_flush_loop must call flush_session — without it the sampler \
             has no production caller and the runtime table is empty forever \
             with every test green. Body was:\n{body}"
        );
        assert!(
            body.contains("release_expired("),
            "start_flush_loop must call release_expired — without it a held \
             working->idle observation is never believed and a finished agent \
             reads Working forever. Body was:\n{body}"
        );
        assert!(
            body.contains("publish_agents_changed_if("),
            "start_flush_loop must call publish_agents_changed_if — without it \
             neither trigger (flush_session's `changed`, or a non-empty \
             release_expired()) can ever reach a subscriber, and the RPC list \
             and the change event can disagree forever with every test green. \
             Body was:\n{body}"
        );
    }

    /// Every `agent_panel.rs` frontend file in the repo, EXCEPT the shared
    /// owner that legitimately sorts (`shared/ui_logic/src/state/agent_panel.rs`
    /// — the single source `sort_entries` lives in).
    ///
    /// Derived rather than hand-listed (Task 10, R10-5, 判据 §5): a fixed
    /// two-path list only covers the frontends that exist on the day it is
    /// written, so a THIRD frontend `agent_panel.rs` (a future mobile
    /// client, a desktop-native surface) would sit silently outside a
    /// hardcoded pair. Walking the tree instead means any file with that
    /// exact name is picked up automatically, wherever it lands.
    ///
    /// `target/`, `interfaces/webchat/node_modules/` and `graphify-out/` are
    /// skipped, not for correctness (none can contain a `.rs` file this
    /// guard cares about) but because `target/` alone is >100GB of build
    /// output and `graphify-out/` (Task 10 fix round 1, F6) is a 1.3 GB,
    /// `.gitignore`d, machine-generated tree that the reviewer measured at
    /// 59% of this walk's 15,689 visited entries — this repo's existing
    /// walker (`utils::source_scan::rust_sources_under`) has no such skip
    /// because every current call site points it at `src/`, never at the
    /// repo root, so this guard writes its own rather than pointing that
    /// one somewhere it was never meant to run.
    ///
    /// # Does not follow symlinks (Task 10 fix round 2, #9)
    ///
    /// Recursion is gated on `DirEntry::file_type()`, not `Path::is_dir()`
    /// — the latter follows symlinks, so a directory symlink would be
    /// descended into with no visited-set and no depth cap, and a symlink
    /// CYCLE would not merely run slow, it would stack-overflow the whole
    /// `cargo test -p alephcore --lib` binary (an infrastructure failure,
    /// not a guard result). No directory symlink is reachable here today —
    /// the only ones in the repo live under `node_modules/` and
    /// `desktop/macos/bridge/.build/`, both already skipped by name — so
    /// this was a latent hazard rather than a live bug, but `file_type()`
    /// removes it for the cost of one method call.
    ///
    /// # False positives this walk can produce (判据 §3 — the expensive
    /// direction; Task 10 fix round 2, #10)
    ///
    /// This walk is not scoped to `interfaces/`: `archive/` (72 `.rs` files
    /// measured at review time), `examples/`, `benches/`, `tests/` and
    /// `docs/` are walked too. A file legitimately named `agent_panel.rs`
    /// living in any of those trees — an archived copy, a doc example —
    /// would be picked up by this walk and scanned by the ordering guard
    /// below as if it were a live frontend, reddening on its own valid
    /// `.sort_by`. Exactly three `agent_panel.rs` files exist in the repo
    /// today (the two live frontends and the shared owner excluded below),
    /// so this is hypothetical, not live — but the false-positive
    /// direction is the one that gets a guard weakened by the next person
    /// it wrongly blocks, so it is worth knowing before someone trips it.
    fn agent_panel_frontend_files(root: &std::path::Path) -> Vec<std::path::PathBuf> {
        fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
            let Ok(entries) = std::fs::read_dir(dir) else {
                return;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                // `entry.file_type()` reports the entry itself and does NOT
                // follow symlinks (unlike `path.is_dir()`), so a symlinked
                // directory is neither recursed into nor treated as a
                // directory at all — see the symlink note above.
                let is_real_dir = entry.file_type().is_ok_and(|t| t.is_dir());
                if is_real_dir {
                    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                    if name == "target" || name == "node_modules" || name == "graphify-out" || name.starts_with('.') {
                        continue;
                    }
                    walk(&path, out);
                } else if path.file_name().and_then(|n| n.to_str()) == Some("agent_panel.rs") {
                    out.push(path);
                }
            }
        }
        let mut files = Vec::new();
        walk(root, &mut files);
        let owner = root.join("shared/ui_logic/src/state/agent_panel.rs");
        files.retain(|p| p != &owner);
        files
    }

    /// Substrings this guard treats as "this file performs its own
    /// ordering" (Task 10 fix round 1, F1+F2, reviewer-verified).
    ///
    /// `.sort` — not `.sort_by` — is the token, because `.sort_by` alone
    /// left `.sort_unstable_by` / `.sort_unstable_by_key` / `.sort_unstable()`
    /// GREEN: a real `.sort_unstable_by` inserted into the TUI panel's
    /// production code passed the old check, and `sort_unstable*` appears
    /// 74× in this repo's own first-party code — an established idiom
    /// here, not a theoretical dodge. `.sort` as a substring still catches
    /// `.sort_by`, `.sort_by_key`, `.sort()` and every `.sort_unstable*`
    /// spelling in one token. `.reverse()` is separate because it shares no
    /// substring with `.sort` at all, and it is the cheapest possible way
    /// to make the two frontends disagree without writing anything that
    /// reads like sorting.
    ///
    /// # What this guard cannot see (判据 §5 — name the gaps, don't
    /// pretend they are closed)
    ///
    /// Token-list gaps, specific to `BANNED_ORDERING_TOKENS`:
    ///
    /// - `.collect::<BTreeMap<_, _>>()` / `BTreeSet` — ordering with no
    ///   ordering CALL to grep for at all.
    /// - `.min_by` / `.max_by` / `.min_by_key` — picks one row rather than
    ///   reordering the rest, so it is a parity risk only for a "top
    ///   agent" affordance, not full-list ordering.
    /// - A `binary_search`-and-insert that maintains order incrementally.
    /// - An ordering call moved into a sibling file next to
    ///   `agent_panel.rs` (`agent_panel_rows.rs`, a `render_rows` in
    ///   `widgets/mod.rs`) — this guard is keyed on the file NAME, not the
    ///   widget. Left uncovered deliberately: no neighbour of either panel
    ///   (`btw_panel.rs`, `session_picker.rs`, `provider_picker.rs`, …)
    ///   currently sorts, and a directory-scoped scan trades this gap for
    ///   a false-positive one the day a legitimate sibling widget (a
    ///   picker) starts sorting its own rows — 判据 §3: the false-positive
    ///   direction is the expensive one, because the next person weakens
    ///   the guard to get past it.
    ///
    /// A gap inherited from `production_prefix`, not introduced here (Task
    /// 10 fix round 2, #11):
    ///
    /// - A line whose TEXT begins with `#[cfg(test)]` while actually being
    ///   string- or comment-literal payload is read by `production_prefix`
    ///   as a live attribute, which discards everything from there to the
    ///   end of that (mis-detected) item — the silent-approval direction
    ///   (`production_prefix`'s own doc comment, "Known gap (F2, review
    ///   round 4, unfixed)", has the full account). Zero reachable
    ///   instances in either frontend file as of this writing; noted here
    ///   so a reader of THIS guard does not have to go find that fact in
    ///   another module's doc comment to know it applies here too.
    const BANNED_ORDERING_TOKENS: [&str; 2] = [".sort", ".reverse()"];

    /// `code_text(production_prefix(src))`, extracted so the guard below
    /// and its true-negative fixture
    /// (`the_stripper_survives_sort_by_named_only_in_prose`) call the exact
    /// same stripping instead of each re-spelling it (Task 10 fix round 1,
    /// F5). Before this extraction the fixture wrote its own composition
    /// of the two calls, so weakening the guard to the weaker
    /// `strip_comment_lines` (the `live_apply.rs:477` precedent) would have
    /// left the fixture green while the guard started firing on string
    /// literals and doc comments — 判据 §1: two representations of one
    /// fact, and the weaker one is the one that ships.
    fn scrub(src: &str) -> String {
        crate::utils::source_scan::code_text(&crate::utils::source_scan::production_prefix(src))
    }

    /// The two frontends this guard is known to protect today, asserted by
    /// MEMBERSHIP in the derived walk's output rather than merely counted
    /// (Task 10 fix round 2, #8 — REPLACES the earlier `files.len() >= 2`
    /// floor rather than sitting beside it).
    ///
    /// A count floor passes as long as the walk finds ANY two files named
    /// `agent_panel.rs`, including two WRONG ones — if a real frontend were
    /// ever renamed out from under the walk on the same day an unrelated
    /// stray `agent_panel.rs` appeared under `archive/` or `examples/`
    /// (判据 §3's false positive, noted on the walk above), the count would
    /// still read 2 and the floor would stay silently green. Asserting
    /// these two specific paths are members of the walk's output does not
    /// have that hole: it can only pass if the walk actually reached the
    /// frontends it is supposed to guard, identity and all. The derived
    /// walk still catches a third, unlisted frontend automatically — this
    /// only pins that these two specific ones are never silently dropped.
    const KNOWN_FRONTENDS: [&str; 2] = [
        "interfaces/tui/src/tui/widgets/agent_panel.rs",
        "interfaces/webchat/src/components/sidebar/agent_panel.rs",
    ];

    /// R2: sorting lives ONLY in `shared_ui_logic::state::agent_panel::sort_entries`.
    /// Neither frontend's `agent_panel.rs` may perform its own ordering call.
    ///
    /// `code_text` (not the weaker `strip_comment_lines` the `live_apply.rs`
    /// precedent uses) is deliberate here (R10-8): it strips comments AND
    /// string-literal payloads over one lexer walk, and the property this
    /// guard checks is "this file performs no ordering call" — a `.sort_by`
    /// spelled inside a string literal is not a call either, any more than
    /// one spelled inside a doc comment is.
    ///
    /// A missing known frontend, or an ordering call in one that IS found,
    /// FAILS rather than vacuously passing (判据 §2 / §8): "I found nothing
    /// to check" is not the same fact as "I checked and it's clean".
    #[test]
    fn no_frontend_sorts_its_own_agent_panel_entries() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let files = agent_panel_frontend_files(root);

        for known in KNOWN_FRONTENDS {
            let expected = root.join(known);
            assert!(
                files.contains(&expected),
                "expected {} among the derived agent_panel.rs frontend files, \
                 but it was not found (found: {files:?}); a missing known \
                 frontend means this walk is not finding what it is supposed \
                 to guard — a silent pass, not a clean one.",
                expected.display()
            );
        }

        for path in &files {
            let src = std::fs::read_to_string(path).unwrap_or_else(|e| {
                panic!("{}: {e} — a missing frontend file is not a pass", path.display())
            });
            let code = scrub(&src);
            let hit = BANNED_ORDERING_TOKENS.iter().find(|token| code.contains(**token));
            assert!(
                hit.is_none(),
                "{} sorts its own agent-panel entries (found `{}`); sorting \
                 belongs to shared_ui_logic::state::agent_panel::sort_entries (R2)",
                path.display(),
                hit.unwrap_or(&"")
            );
        }
    }

    /// True-negative fixture for the guard above (R10-6, reversed by R10-8):
    /// a comment or string literal that merely NAMES `.sort_by`/`.sort()`/
    /// `.reverse()` — documenting the very rule this guard enforces — must
    /// not redden it. Kept here, next to the assertion it proves, rather
    /// than as a production doc comment in another crate that a future
    /// author with no idea this guard exists could reword out from under
    /// it. Calls the same `scrub` the guard above calls (F5) so weakening
    /// one cannot silently stop tracking the other.
    #[test]
    fn the_stripper_survives_sort_by_named_only_in_prose() {
        let synthetic = "\
//! module doc naming `.sort_by`, `.sort()` and `.reverse()` so nobody re-adds them\n\
/// doc comment: this widget must never call `.sort_by`, `.sort()` or `.reverse()`\n\
// plain comment, also just prose: .sort_by(...) .sort() .reverse()\n\
pub fn render() {\n\
    // still just a comment inside a function body: .sort_by .reverse()\n\
    let _ = \"a string literal mentioning .sort_by and .reverse() too\";\n\
}\n";
        let code = scrub(synthetic);
        let hit = BANNED_ORDERING_TOKENS.iter().find(|token| code.contains(**token));
        assert!(
            hit.is_none(),
            "code_text must strip `.sort`/`.reverse()` when they appear only \
             in `//`, `///` and `//!` comments or inside a string literal — \
             otherwise the guard above would redden on prose, and a guard \
             that fires on prose gets weakened by the next person who trips \
             it (判据 §3). Found `{}` in code after stripping:\n{code}",
            hit.unwrap_or(&"")
        );
    }
}
