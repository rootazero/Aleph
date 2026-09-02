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

/// [`RuntimeAgentEntry::cwd`] has no producer this phase, for the same reason
/// [`OSC_PROGRESS_UNAVAILABLE`] does not: upstream sources a pane's working
/// directory by probing the child PID, and PID probing was deliberately not
/// ported (task-5 ruling R5-1). `PtySession` retains no cwd either — it hands
/// `SpawnOptions::cwd` to `CommandBuilder` and drops it.
///
/// The one weaker fact within reach — the directory requested at spawn — is
/// NOT used here on purpose: a shell that has since `cd`'d makes it a wrong
/// label rather than a missing one, and a wrong label is the more expensive
/// of the two (判据 §17). Empty means "I don't know", never "the root" and
/// never "same as the server" (判据 §8).
pub const CWD_UNAVAILABLE: &str = "";

/// The process-wide agent table.
///
/// A `LazyLock` singleton reached through [`agents()`], mirroring
/// `pty::manager()` in this same subsystem: the JSON-RPC handler (task 6) and
/// the tool face (task 11) both need it and neither has an `AppContext` to
/// thread it through. `Default` exists so a test can hold an isolated
/// instance instead of racing the global one.
#[derive(Default)]
pub struct RuntimeAgents {
    entries: Mutex<HashMap<String, RuntimeAgentEntry>>,
}

static GLOBAL: LazyLock<RuntimeAgents> = LazyLock::new(RuntimeAgents::default);

/// Access the process-global agent table.
#[must_use]
pub fn agents() -> &'static RuntimeAgents {
    &GLOBAL
}

impl RuntimeAgents {
    /// Fold one screen observation into the table.
    ///
    /// `shell` is [`crate::gateway::pty::PtySession::shell`] — the
    /// human-readable program label, which is what
    /// `agent_detect::identify_agent` matches against. `process_exited` is
    /// the session's `is_closed()`: a session killed but not yet reaped by
    /// its reader thread is still in the registry for up to one flush tick,
    /// and reporting it as still Working for that tick would be a stale
    /// answer rather than a missing one.
    ///
    /// One lock acquisition on the table; the caller holds the screen lock
    /// for the duration.
    pub fn sample(&self, session_id: &str, shell: &str, screen: &Screen, process_exited: bool) {
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

        let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        let state = match detection {
            Some(d) => wire_state(d.state),
            // `None` = a manifest rule with `skip_state_update` matched: the
            // screen is mid-repaint and carries no statement about the agent.
            // Keep what the last frame established; with nothing established
            // yet the answer is Unknown, not a guess (判据 §8).
            None => entries
                .get(session_id)
                .map_or(RuntimeAgentState::Unknown, |e| e.state),
        };
        entries.insert(
            session_id.to_owned(),
            RuntimeAgentEntry {
                session_id: session_id.to_owned(),
                label,
                cwd: CWD_UNAVAILABLE.to_owned(),
                agent: agent.map(|a| agent_detect::agent_label(a).to_owned()),
                state,
                updated_at: chrono::Utc::now().timestamp(),
            },
        );
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
            .cloned()
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

    /// 证伪守卫：剪断 osc_title 的接线，title 必须变空。
    /// 一条不会变红的守卫不是守卫（判据 §3）。
    ///
    /// `shell` is deliberately a name the OSC title does not equal, so
    /// cutting the title read makes `label` fall back to it and this
    /// assertion goes red. Falsified by hand on 2026-09-02 — see the task
    /// report.
    #[test]
    fn the_title_wire_is_actually_connected() {
        let mut s = crate::gateway::pty::screen::Screen::new(4, 40);
        s.feed(b"\x1b]0;my-agent\x07idle");
        let agents = RuntimeAgents::default();
        agents.sample("s1", "sh", &s, false);
        assert_eq!(agents.snapshot()[0].label, "my-agent");
    }

    /// osc_progress 本期没有生产者，必须是空串。
    /// 空串只有资格说「我不知道」，不许被读成「没有进度」（判据 §8）。
    #[test]
    fn osc_progress_has_no_producer_this_phase() {
        assert_eq!(OSC_PROGRESS_UNAVAILABLE, "");
    }

    /// Same shape, same reason: `cwd` is unavailable this phase because PID
    /// probing was not ported. Pinned so a later phase that wires a real
    /// producer has to delete this test rather than quietly leave the field
    /// empty forever.
    #[test]
    fn cwd_has_no_producer_this_phase() {
        let mut s = crate::gateway::pty::screen::Screen::new(4, 40);
        s.feed(b"hello");
        let agents = RuntimeAgents::default();
        agents.sample("s1", "sh", &s, false);
        assert_eq!(agents.snapshot()[0].cwd, CWD_UNAVAILABLE);
    }

    /// A program the bundled manifest does not know is `agent: None` — never
    /// a guessed name — and its state is Unknown, not Idle.
    #[test]
    fn an_unrecognised_program_is_none_and_unknown() {
        let mut s = crate::gateway::pty::screen::Screen::new(4, 40);
        s.feed(b"$ ");
        let agents = RuntimeAgents::default();
        agents.sample("s1", "sh", &s, false);
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
        let mut s = crate::gateway::pty::screen::Screen::new(4, 40);
        s.feed(b"$ ");
        let agents = RuntimeAgents::default();

        agents.sample("live", "sh", &s, false);
        agents.sample("dead", "sh", &s, true);

        let snap = agents.snapshot();
        let dead = snap.iter().find(|e| e.session_id == "dead").unwrap();
        let live = snap.iter().find(|e| e.session_id == "live").unwrap();
        assert_eq!(dead.state, RuntimeAgentState::Idle);
        assert_eq!(live.state, RuntimeAgentState::Unknown);
    }

    /// THE WIRE. The two tests above prove `sample()`; this one proves it has
    /// a caller. It spawns a real child, drives the same per-session body the
    /// flush ticker drives, and asserts the session landed in the PROCESS
    /// table — the one task 6 and task 11 read. A sampler with no caller is
    /// an empty table forever with every other test green (判据 §7).
    #[tokio::test(flavor = "multi_thread")]
    async fn a_published_frame_lands_the_session_in_the_table() {
        let id = "t-runtime-wire";
        agents().remove(id);

        let opts = SpawnOptions {
            command: Some(if cfg!(windows) { "cmd.exe" } else { "sh" }.to_string()),
            args: if cfg!(windows) {
                vec!["/C".into(), "echo ALEPH_RUNTIME_WIRE & pause".into()]
            } else {
                vec!["-c".into(), "printf 'ALEPH_RUNTIME_WIRE'; sleep 30".into()]
            },
            rows: 6,
            cols: 40,
            ..Default::default()
        };
        let session = PtySession::spawn(id.into(), &opts, None).expect("spawn");

        let mut framed = false;
        for _ in 0..100 {
            if crate::gateway::pty::manager::flush_session(&session).is_some() {
                framed = true;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        assert!(framed, "the child produced no frame in 2s");

        let present = agents().snapshot().iter().any(|e| e.session_id == id);
        session.kill();
        agents().remove(id);
        assert!(
            present,
            "a flushed frame must land the session in the process table"
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
            crate::gateway::pty::manager::flush_session(&session);
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
}
