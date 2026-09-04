// The gating constants and the miss hysteresis are ported from herdr 0.8.2
// (https://github.com/herdrdev/herdr), Copyright the herdr authors, licensed
// under the Apache License, Version 2.0 — see `crates/agent-detect/NOTICE`,
// which covers the rest of the herdr port. Specifically:
//
//   herdr src/pane.rs:455-700  `should_probe_foreground_job` and friends
//                              -> [`probe_due`] (three rules, not the state
//                                 machine — see that function's doc)
//   herdr src/pane/agent_detection.rs  `AGENT_MISS_CONFIRMATION_ATTEMPTS`
//                              -> [`PROBE_MISSES_TO_FORGET`]
//   herdr src/platform/mod.rs:6-19     `ForegroundProcess`
//                              -> [`ForegroundFact`]
//
// The rest of this file (the `portable-pty` + `sysinfo` probe itself) is
// Aleph's own: upstream reaches the process table through its own
// `crate::platform` FFI layer, which R1 puts out of reach here.

//! Which program is in the foreground of a PTY, probed behind a rate gate.
//!
//! This is the producer the agent sampler was missing. Before it,
//! `gateway::runtime` identified agents from `PtySession::shell` — the
//! SPAWN-TIME label, so a user who opens a terminal and types `claude` left
//! that label at `zsh` and nothing was ever identified in production. The
//! fact that answers the question is the terminal's foreground process, and
//! that is what this module reads.
//!
//! # What it is allowed to use
//!
//! `portable-pty` and `sysinfo`, both already direct dependencies, and
//! nothing else (spec R2-2). R1 forbids `src/` from reaching a platform API
//! crate directly; `MasterPty::process_group_leader` is `tcgetpgrp` behind
//! `portable-pty`'s abstraction, and the process table comes from
//! [`crate::utils::process_alive::with_process_specifics`], the one place in
//! this repo that owns the `sysinfo` single-pid idiom.
//!
//! # Locks
//!
//! The pgid read needs the session's master lock; the `sysinfo` refresh needs
//! no lock at all and must not be made to wait on one — least of all the
//! screen lock, which the PTY reader thread takes for every output chunk.
//!
//! That is why the probe is TWO functions, [`foreground_leader`] and
//! [`fact_for_pid`], rather than the one `probe(master, shell_pid)` the task
//! brief named: a single function receiving `&dyn MasterPty` runs entirely
//! inside whatever lock its caller holds to produce that reference, so it
//! cannot both take the master lock for the pgid and release it before the
//! process table read. Splitting is the only way to honour the rule; a
//! one-line composition kept beside them would have had no production caller
//! (R10). `PtySession::maybe_probe_foreground` is the single caller that runs
//! them in order, and the guards drive it rather than a private twin.
//!
//! # Time
//!
//! Every timestamp here is unix epoch **milliseconds**, the same unit
//! `gateway::runtime` uses, so a fact and the tick that gated it are
//! comparable without a conversion (判据 §12).

use crate::sync_primitives::{AtomicU64, Ordering};

/// Minimum gap between two probes of a session that is producing frames.
///
/// herdr's own floor for the same job (`src/pane.rs:776-789` computes a probe
/// interval from the pane's activity; 500 ms is its busy end). At the 16 ms
/// flush cadence this caps a busy session at ~2 probes per second instead of
/// ~62.
pub const PROBE_MIN_INTERVAL_MS: i64 = 500;

/// Maximum gap between two probes of a session whose agent is already
/// identified, EVEN IF it produces no frames.
///
/// Without this rule the gate would be unreachable for the case that matters
/// most: an agent that exits leaves the shell in the foreground and paints
/// nothing, so a frame-gated probe would never look again and the panel would
/// show a finished agent forever.
pub const PROBE_RECHECK_MS: i64 = 3_000;

/// How many consecutive probes must fail to see the program before it is
/// forgotten. A hit is believed immediately; a miss is not.
///
/// herdr `AGENT_MISS_CONFIRMATION_ATTEMPTS`. The asymmetry is the point: a
/// probe that cannot read the process table (a pgid mid-handoff between two
/// jobs, a permission blip) says "I could not look", which is not the same
/// answer as "nothing is running there" (判据 §8).
pub const PROBE_MISSES_TO_FORGET: u32 = 6;

/// One observation of a PTY's foreground process.
///
/// `argv0` and `cmdline` are separate because they answer different
/// questions and either can be absent: `argv0` is what the program was
/// invoked as, `cmdline` is the whole command. Both are needed —
/// `agent_detect::identify_agent_from_process` reads the command line
/// precisely because `claude` runs as a Node script whose process NAME is
/// `node`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForegroundFact {
    /// The process id the terminal's foreground process group leads with.
    pub pid: u32,
    /// The kernel's name for the process (`sysinfo::Process::name`).
    pub name: String,
    /// `argv[0]`, when the process table reports one.
    pub argv0: Option<String>,
    /// The whole command line, space-joined.
    pub cmdline: Option<String>,
    /// The process's current working directory, when readable. This is the
    /// LIVE cwd — it is what makes a shell that has `cd`'d visible, which the
    /// spawn directory cannot do.
    pub cwd: Option<String>,
    /// Unix epoch millis at which the process table was read for this fact.
    ///
    /// Read from the clock here rather than passed in, because it stamps the
    /// syscall and not the tick that authorised it: a fact retained across
    /// misses (see [`ForegroundState`]) keeps the instant it was actually
    /// observed, so "how old is this identification" has an answer. The rate
    /// gate uses the tick's `now`, never this — one clock per question.
    pub observed_at: i64,
}

impl ForegroundFact {
    /// Whether two observations describe the same process doing the same
    /// thing in the same place — everything except [`Self::observed_at`],
    /// which moves on every hit and so cannot be part of "did this change".
    #[must_use]
    pub fn same_subject(&self, other: &Self) -> bool {
        self.pid == other.pid
            && self.name == other.name
            && self.argv0 == other.argv0
            && self.cmdline == other.cmdline
            && self.cwd == other.cwd
    }
}

/// Whether this tick should probe.
///
/// Three rules, and they are the whole gate (spec §4.1) — deliberately not a
/// port of herdr's probe state machine (`src/pane.rs:455-700`), which folds
/// in pane visibility, a content-sequence counter and an adaptive interval
/// that Aleph has no producers for:
///
/// 1. Never probed ⇒ probe. The first observation of a session is what makes
///    the very first identification possible at all.
/// 2. A frame has arrived SINCE THE LAST PROBE and that probe is at least
///    [`PROBE_MIN_INTERVAL_MS`] old ⇒ probe. Frames are the cheap signal that
///    something happened.
/// 3. An agent is already identified here and the last probe is at least
///    [`PROBE_RECHECK_MS`] old ⇒ probe, frame or no frame. This is the only
///    rule that can fire for a silent session, and without it an agent's
///    EXIT is never noticed.
///
/// ⚠️ Rule 2 says "since the last probe", not "on this tick", and the
/// difference is the whole rule. The first version asked whether THIS 16 ms
/// tick produced a frame, and `a_real_agent_started_after_spawn_is_identified`
/// caught what that means: a program that starts, paints once and goes quiet
/// paints entirely inside the 500 ms shadow of the previous probe, so its one
/// frame is thrown away, no later frame ever arrives, and rule 3 cannot help
/// because nothing was identified. "Not identified" became an absorbing state
/// (判据 §2: the interesting question is not whether the rule is right but
/// when it can fire). The caller keeps the sticky bit — see
/// [`ForegroundState::note_frame`].
///
/// Pure, so the cost guard can drive it a thousand times without touching the
/// process table.
#[must_use]
pub fn probe_due(
    last_probe_at: Option<i64>,
    now: i64,
    frame_since_probe: bool,
    agent_known: bool,
) -> bool {
    let Some(last) = last_probe_at else {
        return true;
    };
    let age = now.saturating_sub(last);
    (frame_since_probe && age >= PROBE_MIN_INTERVAL_MS) || (agent_known && age >= PROBE_RECHECK_MS)
}

/// The pid of the process group the kernel says owns this terminal.
///
/// Unix: `tcgetpgrp` through [`portable_pty::MasterPty::process_group_leader`]
/// — the authoritative answer. Only when the kernel declines (no foreground
/// group: the session is on its way out, or a shell that never enabled job
/// control) does it fall through to the heuristic below, so the expensive
/// path is the exception rather than the rule.
///
/// Non-Unix: `portable-pty`'s trait does not have that method at all (it is
/// `#[cfg(unix)]` upstream), so the heuristic IS the answer there.
///
/// Either way this needs the master lock only for the `tcgetpgrp` call, and
/// the caller releases it before anything reads the process table.
#[must_use]
pub fn foreground_leader(
    master: &dyn portable_pty::MasterPty,
    shell_pid: Option<u32>,
) -> Option<u32> {
    leader_from_terminal(master).or_else(|| shell_pid.and_then(deepest_newest_descendant))
}

#[cfg(unix)]
fn leader_from_terminal(master: &dyn portable_pty::MasterPty) -> Option<u32> {
    u32::try_from(master.process_group_leader()?).ok()
}

/// Windows has no `tcgetpgrp` and `portable-pty` exposes no equivalent, so
/// there is nothing to ask. `None` is "I could not look", which is exactly
/// what sends [`foreground_leader`] to the descendant heuristic (判据 §8).
#[cfg(not(unix))]
fn leader_from_terminal(_master: &dyn portable_pty::MasterPty) -> Option<u32> {
    None
}

/// Read one pid's facts out of the process table.
///
/// Bumps [`probe_count`]. Takes no lock and must be called holding none — a
/// `sysinfo` refresh is a syscall-heavy operation and the screen lock is on
/// the PTY reader thread's hot path.
#[must_use]
pub fn fact_for_pid(pid: u32) -> Option<ForegroundFact> {
    PROBE_COUNT.fetch_add(1, Ordering::Relaxed);
    let observed_at = chrono::Utc::now().timestamp_millis();
    crate::utils::process_alive::with_process_specifics(pid, process_facts_refresh(), |p| {
        let cmd = p.cmd();
        ForegroundFact {
            pid,
            name: p.name().to_string_lossy().into_owned(),
            argv0: cmd.first().map(|a| a.to_string_lossy().into_owned()),
            cmdline: (!cmd.is_empty()).then(|| {
                cmd.iter()
                    .map(|a| a.to_string_lossy())
                    .collect::<Vec<_>>()
                    .join(" ")
            }),
            cwd: p.cwd().map(|c| c.to_string_lossy().into_owned()),
            observed_at,
        }
    })
}

/// Exactly the four fields [`ForegroundFact`] reads. `sysinfo` refreshes
/// nothing it is not asked for, and the three defaults this leaves off
/// (memory, cpu, disk usage) are the expensive ones.
fn process_facts_refresh() -> sysinfo::ProcessRefreshKind {
    use sysinfo::{ProcessRefreshKind, UpdateKind};
    ProcessRefreshKind::nothing()
        .with_cmd(UpdateKind::Always)
        .with_exe(UpdateKind::Always)
        .with_cwd(UpdateKind::Always)
}

/// The shell's deepest, then newest, descendant — or the shell itself when it
/// has none, because a shell sitting at its prompt IS the foreground program.
///
/// This is the answer on platforms with no `tcgetpgrp`, and the fallback on
/// the ones that have it (see [`foreground_leader`]). It is deliberately NOT
/// `#[cfg(not(unix))]`: a branch that compiles on no machine any developer or
/// CI job runs is a branch nobody can falsify, and its only proof would have
/// been "it type-checks on Windows". Compiled and tested everywhere, it is
/// instead exercised by
/// `the_descendant_walk_finds_a_child_this_test_started`.
///
/// The cost is a FULL process-table refresh — a descendant walk needs every
/// process's parent — which is why it is second choice and why the rate gate
/// exists. It counts toward [`probe_count`] like any other refresh.
fn deepest_newest_descendant(shell_pid: u32) -> Option<u32> {
    use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};

    PROBE_COUNT.fetch_add(1, Ordering::Relaxed);
    let mut sys = System::new();
    sys.refresh_processes_specifics(ProcessesToUpdate::All, true, ProcessRefreshKind::nothing());

    // (depth, start_time, pid), maximised in that order: deepest wins, then
    // newest, then the pid — the last term only so the answer is a function
    // of the table and not of its iteration order (判据 §12).
    let mut best: Option<(usize, u64, u32)> = None;
    let mut frontier = vec![Pid::from_u32(shell_pid)];
    let mut depth = 0usize;
    // Bounded so a parent-pointer cycle (a reparented pid, a table read
    // mid-teardown) cannot spin here. 64 is far past any real process tree.
    while !frontier.is_empty() && depth < 64 {
        depth += 1;
        let mut next = Vec::new();
        for (pid, proc_) in sys.processes() {
            if proc_.parent().is_some_and(|p| frontier.contains(&p)) {
                next.push(*pid);
                let candidate = (depth, proc_.start_time(), pid.as_u32());
                if best.is_none_or(|b| candidate > b) {
                    best = Some(candidate);
                }
            }
        }
        frontier = next;
    }
    best.map(|(_, _, pid)| pid).or(Some(shell_pid))
}

/// Process-wide count of process-table refreshes performed by this module.
///
/// The cost guard's instrument (herdr's counting-architecture-test shape).
/// It counts REFRESHES, not probe decisions: `probe_due` is pure and free,
/// and the number that matters is how often the kernel was actually asked.
static PROBE_COUNT: AtomicU64 = AtomicU64::new(0);

/// How many process-table refreshes this process has performed. See
/// [`PROBE_COUNT`].
#[must_use]
pub fn probe_count() -> u64 {
    PROBE_COUNT.load(Ordering::Relaxed)
}

/// Zero the refresh counter. Tests read a DELTA around their own work; this
/// is the cheaper spelling of that when the test holds the counter's serial
/// key.
pub fn reset_probe_count() {
    PROBE_COUNT.store(0, Ordering::Relaxed);
}

/// A session's probe bookkeeping: when it last looked, what it last saw, and
/// how many times in a row it has failed to see anything.
///
/// The hysteresis lives here rather than in the caller because "what is the
/// current foreground program" must have ONE answer, and a caller that could
/// see both the raw probe result and the retained one would have two.
#[derive(Debug, Default)]
pub struct ForegroundState {
    /// Unix millis of the last probe, hit or miss. `None` = never probed.
    /// Sourced from the flush tick's `now`, never from a fact's
    /// `observed_at` — one clock for the gate (判据 §12).
    last_probe_at: Option<i64>,
    /// The last fact actually observed, retained across up to
    /// [`PROBE_MISSES_TO_FORGET`] misses.
    fact: Option<ForegroundFact>,
    /// Consecutive misses since the last hit.
    misses: u32,
    /// Whether any frame has arrived since the last probe.
    ///
    /// Sticky, and that is the point: see the warning on [`probe_due`]. A
    /// per-tick answer loses every frame that lands inside the rate limit's
    /// shadow, which is where a program that paints once and goes quiet does
    /// all of its painting.
    frame_since_probe: bool,
}

impl ForegroundState {
    /// Record whether this tick produced a frame. Idempotent and sticky —
    /// once true it stays true until the next probe consumes it.
    pub fn note_frame(&mut self, produced: bool) {
        self.frame_since_probe |= produced;
    }

    /// Whether a frame has arrived since the last probe. Feeds [`probe_due`].
    #[must_use]
    pub const fn frame_since_probe(&self) -> bool {
        self.frame_since_probe
    }

    /// Fold one probe outcome in. `now` is the tick that authorised it.
    ///
    /// Returns whether the BELIEVED foreground process changed — a different
    /// program, a moved cwd, or the hysteresis finally forgetting one. The
    /// caller needs that answer because identification is an INPUT to the
    /// agent sampler, and the sampler is otherwise only reached when the
    /// screen produced a frame: a program that starts, paints once and goes
    /// quiet would be identified here and then never published.
    ///
    /// `observed_at` is excluded from the comparison. It moves on every hit by
    /// construction, so including it would make this return `true` forever and
    /// the answer would carry no information (判据 §2).
    pub fn observe(&mut self, now: i64, observed: Option<ForegroundFact>) -> bool {
        self.last_probe_at = Some(now);
        self.frame_since_probe = false;
        let before = self.fact.take();
        match observed {
            Some(fact) => {
                self.misses = 0;
                self.fact = Some(fact);
            }
            None => {
                self.misses = self.misses.saturating_add(1);
                self.fact = (self.misses < PROBE_MISSES_TO_FORGET)
                    .then(|| before.clone())
                    .flatten();
            }
        }
        match (&before, &self.fact) {
            (None, None) => false,
            (Some(a), Some(b)) => !a.same_subject(b),
            _ => true,
        }
    }

    /// The current believed foreground process, after hysteresis.
    #[must_use]
    pub fn current(&self) -> Option<&ForegroundFact> {
        self.fact.as_ref()
    }

    /// When this session was last probed. Feeds [`probe_due`].
    #[must_use]
    pub const fn last_probe_at(&self) -> Option<i64> {
        self.last_probe_at
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fact(name: &str, observed_at: i64) -> ForegroundFact {
        ForegroundFact {
            pid: 1,
            name: name.to_owned(),
            argv0: None,
            cmdline: None,
            cwd: None,
            observed_at,
        }
    }

    /// The gate's four interesting combinations, and the two ways it must
    /// say NO. A gate that cannot say no is not a gate (判据 §2), so the
    /// negative rows carry the weight here.
    #[test]
    fn probe_due_respects_min_interval_and_recheck() {
        // (last, now, frame, agent_known, expected, why)
        let cases: [(Option<i64>, i64, bool, bool, bool, &str); 8] = [
            (
                None,
                0,
                false,
                false,
                true,
                "never probed: look once, unconditionally",
            ),
            (
                Some(0),
                PROBE_MIN_INTERVAL_MS,
                true,
                false,
                true,
                "a frame after the min interval is the ordinary probe",
            ),
            (
                Some(0),
                PROBE_MIN_INTERVAL_MS - 1,
                true,
                false,
                false,
                "a frame BEFORE the min interval must not probe -- this is the rate limit",
            ),
            (
                Some(0),
                PROBE_RECHECK_MS,
                false,
                true,
                true,
                "an identified agent gone quiet must still be rechecked, or its exit is never seen",
            ),
            (
                Some(0),
                PROBE_RECHECK_MS - 1,
                false,
                true,
                false,
                "before the recheck interval, a silent session costs nothing",
            ),
            (
                Some(0),
                PROBE_RECHECK_MS * 10,
                false,
                false,
                false,
                "no frame and no agent: silence about an unidentified session is not news",
            ),
            (
                Some(0),
                PROBE_MIN_INTERVAL_MS,
                false,
                false,
                false,
                "the min interval alone does not authorise a probe -- rule 2 needs the frame",
            ),
            (
                Some(0),
                PROBE_RECHECK_MS,
                true,
                true,
                true,
                "both rules satisfied is still one probe",
            ),
        ];
        for (last, now, frame, known, expected, why) in cases {
            assert_eq!(
                probe_due(last, now, frame, known),
                expected,
                "probe_due({last:?}, {now}, frame={frame}, known={known}): {why}"
            );
        }
    }

    /// A frame that arrives while the gate is shut must still earn the next
    /// probe.
    ///
    /// This is the defect `a_real_agent_started_after_spawn_is_identified`
    /// found on its first real run, written down as a unit: the fake `claude`
    /// started, painted its chrome and slept, all within 500 ms of the
    /// previous probe. With a per-tick answer that frame was discarded, no
    /// later frame ever arrived (the program was asleep), and rule 3 could not
    /// help because nothing had been identified — so the session was
    /// unidentifiable for the rest of its life.
    #[test]
    fn a_frame_inside_the_rate_shadow_still_earns_the_next_probe() {
        let mut state = ForegroundState::default();
        state.observe(0, Some(fact("sh", 0)));

        // A frame at t=10ms: far too early to probe.
        state.note_frame(true);
        assert!(
            !probe_due(state.last_probe_at(), 10, state.frame_since_probe(), false),
            "the rate limit still applies"
        );

        // Nothing more happens: every later tick reports no frame of its own.
        for tick in [100, 200, 300, 400] {
            state.note_frame(false);
            assert!(!probe_due(
                state.last_probe_at(),
                tick,
                state.frame_since_probe(),
                false
            ));
        }

        state.note_frame(false);
        assert!(
            probe_due(
                state.last_probe_at(),
                PROBE_MIN_INTERVAL_MS,
                state.frame_since_probe(),
                false
            ),
            "the frame from t=10 must still be remembered at t={PROBE_MIN_INTERVAL_MS} -- \
             otherwise a program that paints once and sleeps is never looked at again"
        );

        // And the probe consumes it: no frame, no further probes.
        state.observe(
            PROBE_MIN_INTERVAL_MS,
            Some(fact("claude", PROBE_MIN_INTERVAL_MS)),
        );
        assert!(
            !state.frame_since_probe(),
            "a probe must consume the sticky bit, or the gate degenerates into \
             one probe per PROBE_MIN_INTERVAL_MS forever"
        );
        assert!(!probe_due(
            state.last_probe_at(),
            PROBE_MIN_INTERVAL_MS * 3,
            state.frame_since_probe(),
            false
        ));
    }

    /// A hit is believed at once; a miss is not believed until
    /// [`PROBE_MISSES_TO_FORGET`] of them agree.
    ///
    /// The retained fact must keep its ORIGINAL `observed_at` through the
    /// misses — a miss that refreshed the timestamp would make a stale
    /// identification look freshly confirmed, which is the "fail-closed
    /// answer consumed as a value" shape (判据 §8).
    #[test]
    fn misses_are_hysteretic_hits_are_immediate() {
        let mut state = ForegroundState::default();
        assert!(state.current().is_none(), "nothing observed yet");

        state.observe(10, Some(fact("claude", 10)));
        assert_eq!(
            state.current().map(|f| f.name.as_str()),
            Some("claude"),
            "a hit takes effect on the same observation"
        );

        for i in 1..PROBE_MISSES_TO_FORGET {
            state.observe(10 + i64::from(i), None);
            assert_eq!(
                state.current().map(|f| f.name.as_str()),
                Some("claude"),
                "miss {i} of {PROBE_MISSES_TO_FORGET} must not forget yet"
            );
            assert_eq!(
                state.current().map(|f| f.observed_at),
                Some(10),
                "a miss must not restamp the retained fact"
            );
        }

        state.observe(100, None);
        assert!(
            state.current().is_none(),
            "the {PROBE_MISSES_TO_FORGET}th consecutive miss forgets"
        );

        state.observe(200, Some(fact("codex", 200)));
        assert_eq!(
            state.current().map(|f| f.name.as_str()),
            Some("codex"),
            "the miss counter resets on a hit"
        );
        assert_eq!(
            state.last_probe_at(),
            Some(200),
            "the gate's clock advances on every observation, hit or miss"
        );
    }

    /// The counter is the cost guard's instrument, so it has to be able to
    /// MOVE. An instrument stuck at zero makes every bound assertion pass
    /// vacuously (判据 §18).
    #[test]
    #[serial_test::serial(foreground_probe_count)]
    fn probe_count_can_reach_one() {
        reset_probe_count();
        let me = std::process::id();
        let observed = fact_for_pid(me);
        assert_eq!(
            probe_count(),
            1,
            "one process-table read must count as exactly one"
        );
        let observed = observed.expect("this test's own process must be in the process table");
        assert_eq!(observed.pid, me);
        assert!(
            !observed.name.is_empty(),
            "a fact with no name identifies nothing"
        );
    }

    /// The Windows answer, exercised where it can actually run.
    ///
    /// `deepest_newest_descendant` is the whole foreground answer on
    /// platforms without `tcgetpgrp`, and on this machine there is no way to
    /// compile that platform, let alone run it. Keeping the walk
    /// platform-independent is what makes this guard possible at all: it
    /// starts a real two-level process tree and asserts the walk descends
    /// past the parent, which is the one thing the heuristic has to get
    /// right.
    ///
    /// The tree is rooted at a process this test OWNS, not at the test binary
    /// itself: sibling tests in this crate spawn PTY children of the test
    /// process, so a walk rooted there answers with whichever of THEIR
    /// children happens to be newest. That is not flakiness in the walk, it
    /// is a guard whose corpus was bigger than its subject.
    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread")]
    #[serial_test::serial(foreground_probe_count)]
    async fn the_descendant_walk_finds_a_child_this_test_started() {
        // `sleep 30; true` keeps the shell alive as a real parent instead of
        // letting it `exec` the sleep and vanish, so there are two levels to
        // descend.
        let mut child = std::process::Command::new("sh")
            .args(["-c", "sleep 30; true"])
            .spawn()
            .expect("spawn sh");
        let shell_pid = child.id();

        // The process table needs a moment to see a just-forked grandchild.
        let mut answer = None;
        for _ in 0..40 {
            answer = deepest_newest_descendant(shell_pid);
            if answer.is_some_and(|p| p != shell_pid) {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        let inner = answer.expect("the walk must never answer None for a live root");
        let killed = std::process::Command::new("kill")
            .args(["-TERM", &shell_pid.to_string(), &inner.to_string()])
            .status();
        let _ = child.kill();
        let _ = child.wait();
        let _ = killed;

        assert_ne!(
            inner, shell_pid,
            "the walk must descend past the shell ({shell_pid}) to the child it started"
        );
        assert_eq!(
            deepest_newest_descendant(inner),
            Some(inner),
            "a process with no descendants must answer with ITSELF, not None -- \
             a shell sitting at its prompt is the foreground program"
        );
    }

    /// A real PTY, a real child, and the pgid the kernel reports: the child
    /// `portable-pty` spawns becomes the terminal's foreground process group
    /// (it `setsid`s and takes the pty as its controlling terminal), so the
    /// probe must name it.
    ///
    /// It drives the PRODUCTION path —
    /// `PtySession::maybe_probe_foreground` then `foreground_fact()` — and
    /// not a test-only twin, so the two halves are exercised in the order and
    /// under the locking discipline production uses (判据 §7: a guard on a
    /// function production does not call proves the function, not the wire).
    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread")]
    #[serial_test::serial(foreground_probe_count)]
    async fn a_real_child_is_reported_as_the_foreground_program() {
        let opts = super::super::SpawnOptions {
            command: Some("sleep".to_string()),
            args: vec!["30".into()],
            rows: 6,
            cols: 40,
            ..Default::default()
        };
        let session = super::super::PtySession::spawn("t-foreground-probe".into(), &opts, None)
            .expect("spawn");

        let mut seen: Option<ForegroundFact> = None;
        for i in 0..60 {
            // `frame_produced` is true and the min interval is respected by
            // spacing the calls, so rules 1 and 2 both authorise these.
            session.maybe_probe_foreground(i * PROBE_MIN_INTERVAL_MS, true, false);
            if let Some(f) = session.foreground_fact() {
                seen = Some(f);
                if seen.as_ref().is_some_and(|f| f.name.contains("sleep")) {
                    break;
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        session.kill();

        let seen = seen.expect("the probe returned nothing for a live child in 3s");
        assert!(
            seen.name.contains("sleep"),
            "the foreground program must be the child, not the server: {seen:?}"
        );
        assert!(seen.pid > 0, "a fact with no pid names no process");
    }
}
