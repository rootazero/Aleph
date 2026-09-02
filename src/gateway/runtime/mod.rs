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
    /// `agent_detect::identify_agent` matches against. `cwd` is
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
    }
}
