// This file is Aleph's own code and carries the crate's MIT licence. It is
// NOT a port: `crates/agent-detect` is the machine-readable Apache-2.0 zone
// for herdr-derived code (phase-1 spec), and no herdr-derived code lives
// outside it.
//
// What IS taken from herdr 0.8.2 (https://github.com/herdrdev/herdr) are two
// NUMBERS and the shape of an idea, cited where each is used:
//   * 500 ms, the fast end of the acquisition interval upstream picks in
//     `should_probe_foreground_job` (`src/pane.rs:491-497`, choosing between
//     `PROCESS_ACQUISITION_FAST_RECHECK` = 500 ms and
//     `PROCESS_ACQUISITION_SLOW_RECHECK` = 2 s, `src/pane.rs:297-298`);
//   * 6, upstream's `AGENT_MISS_CONFIRMATION_ATTEMPTS` (`src/pane.rs:291`).
// Aleph's 3 s silent recheck is NOT upstream's number and never was: the
// analogous constant there is `PROCESS_RECHECK_IDENTIFIED` = 5 s
// (`src/pane.rs:292`). An earlier version of this header cited
// `src/pane.rs:776-789` for a "500 ms / 3 s" pair, and neither half survives
// reading upstream — those lines are the CALL SITE, which computes no
// interval at all, and 3 s appears nowhere in herdr. A citation is a claim
// about someone else's file, so it is exactly the kind that rots unnoticed
// (判据 §1); re-read before trusting it.
// Upstream's probe machinery itself is not reproduced here. It reaches the
// process table through its own `crate::platform` FFI layer, which R1 puts
// out of reach; `should_probe_foreground_job` is a state machine folding in
// pane visibility, a content-sequence counter and an adaptive interval, none
// of which Aleph has producers for. The three-rule gate below, the sticky
// frame bit, the fact struct and the `portable-pty` + `sysinfo` probe are
// written for this codebase.

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
//! ONE rule: **nothing that reads the process table may run under a lock.**
//! Not the master lock, whose other holder is `PtySession::resize`, and least
//! of all the screen lock, which the PTY reader thread takes for every output
//! chunk.
//!
//! That is why a probe is THREE functions and not one — the brief's single
//! `probe(master, shell_pid)` cannot obey the rule, because a function
//! receiving `&dyn MasterPty` runs entirely inside whatever lock its caller
//! took to produce that reference:
//!
//! 1. [`leader_from_terminal`] — the only one that touches the master, one
//!    `tcgetpgrp` ioctl, under the lock;
//! 2. [`deepest_newest_descendant`] — the fallback when the terminal will not
//!    say, a FULL process-table refresh, outside every lock;
//! 3. [`fact_for_pid`] — one pid's facts, outside every lock.
//!
//! Splitting 1 from 2 is not cosmetic: they were briefly composed into one
//! `foreground_leader(master, shell_pid)`, which forced the caller to hold the
//! master lock across the full refresh — on Windows, on every probe — while
//! two doc comments asserted the opposite. `PtySession::maybe_probe_foreground`
//! is the single caller that runs all three in order, and
//! `no_process_table_read_happens_under_the_master_lock` pins the boundary.
//!
//! # Time
//!
//! Every timestamp here is unix epoch **milliseconds**, the same unit
//! `gateway::runtime` uses, so a fact and the tick that gated it are
//! comparable without a conversion (判据 §12).

/// Minimum gap between two probes of a session that is producing frames.
///
/// herdr's own floor for the same job: `should_probe_foreground_job`
/// (`src/pane.rs:491-497`) picks between `PROCESS_ACQUISITION_FAST_RECHECK`
/// (500 ms, while a pane is freshly acquiring) and
/// `PROCESS_ACQUISITION_SLOW_RECHECK` (2 s), and 500 ms is that fast end.
/// Aleph takes the number, not the adaptive choice — it has no acquisition
/// window to switch on. At the 16 ms flush cadence this caps a busy session
/// at ~2 probes per second instead of ~62.
pub const PROBE_MIN_INTERVAL_MS: i64 = 500;

/// Maximum gap between two probes of a session whose agent is already
/// identified, EVEN IF it produces no frames.
///
/// Without this rule the gate would be unreachable for the case that matters
/// most: an agent that exits leaves the shell in the foreground and paints
/// nothing, so a frame-gated probe would never look again and the panel would
/// show a finished agent forever.
///
/// 3 s is Aleph's own choice, not a borrowed number: herdr's analogue is
/// `PROCESS_RECHECK_IDENTIFIED` = 5 s (`src/pane.rs:292`). Said here because
/// the licence header used to attribute 3 s upstream, and an unattributed
/// number is easier to "correct" back than a contradicted one.
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

/// The pid of the process group the kernel says owns this terminal, asked of
/// the terminal itself.
///
/// Unix: `tcgetpgrp` through [`portable_pty::MasterPty::process_group_leader`]
/// — the authoritative answer, and a single cheap ioctl.
///
/// `None` means "the terminal would not say": no foreground group (the session
/// is on its way out, or a shell that never enabled job control), or a platform
/// where `portable-pty` has no such method at all. It is never "nothing is
/// running there" (判据 §8) — the caller answers that with
/// [`deepest_newest_descendant`].
///
/// # This is the ONLY thing that may run under the master lock
///
/// It is a separate function from the fallback for exactly that reason. An
/// earlier version had a `foreground_leader(master, shell_pid)` that composed
/// the two, so the caller had to hold the master lock across the composition —
/// and the fallback is a FULL `ProcessesToUpdate::All` refresh plus up to 64
/// passes over the table, which on Windows ran under the lock on every single
/// probe while `PtySession::resize` waited behind it. Two doc comments
/// asserted the opposite at the time. Splitting the function is what makes the
/// rule checkable rather than aspirational; `no_process_table_read_happens_under_the_master_lock`
/// pins it.
#[must_use]
pub fn leader_from_terminal(master: &dyn portable_pty::MasterPty) -> Option<u32> {
    leader_from_terminal_impl(master)
}

#[cfg(unix)]
fn leader_from_terminal_impl(master: &dyn portable_pty::MasterPty) -> Option<u32> {
    u32::try_from(master.process_group_leader()?).ok()
}

/// Windows has no `tcgetpgrp` and `portable-pty` exposes no equivalent, so
/// there is nothing to ask. `None` is "I could not look", which is exactly
/// what sends the caller to the descendant heuristic (判据 §8).
#[cfg(not(unix))]
fn leader_from_terminal_impl(_master: &dyn portable_pty::MasterPty) -> Option<u32> {
    None
}

/// Read one pid's facts out of the process table.
///
/// Takes no lock and must be called holding none — a `sysinfo` refresh is a
/// syscall-heavy operation and the screen lock is on the PTY reader thread's
/// hot path.
#[must_use]
pub fn fact_for_pid(pid: u32) -> Option<ForegroundFact> {
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
/// the ones that have it (see [`leader_from_terminal`]). It is deliberately NOT
/// `#[cfg(not(unix))]`: a branch that compiles on no machine any developer or
/// CI job runs is a branch nobody can falsify, and its only proof would have
/// been "it type-checks on Windows". Compiled and tested everywhere, it is
/// instead exercised by
/// `the_descendant_walk_finds_a_child_this_test_started`.
///
/// The cost is a FULL process-table refresh — a descendant walk needs every
/// process's parent — which is why it is second choice, why the rate gate
/// exists, and why it takes a bare `u32` and no terminal handle: a function
/// that cannot be handed the master cannot be called while the master lock is
/// held by accident.
#[must_use]
pub fn deepest_newest_descendant(shell_pid: u32) -> Option<u32> {
    use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};

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
    /// How many probes this session has performed — the cost guard's
    /// instrument (herdr's counting-architecture-test shape).
    ///
    /// PER SESSION, not a `static`. The first version was a process-global
    /// `AtomicU64`, which made the bound guard read whatever every other test
    /// in the same binary happened to be probing at the time: measured 60
    /// against a ceiling of 60, with five untagged tests driving
    /// `flush_session` concurrently, one of them polling for three seconds.
    /// That is the same defect already fixed once for
    /// `RuntimeAgents::visible_text_builds` and not carried to its twin.
    /// Counted here, the number is a function of this session's own ticks
    /// and nothing else.
    ///
    /// It counts PROBES, not process-table refreshes, because a probe is what
    /// the gate decides and what the ceiling arithmetic is written in. One
    /// probe is one refresh on Unix (the pgid read is an ioctl) and at most
    /// two on platforms that fall back to the descendant walk.
    probes: u64,
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
        self.probes = self.probes.saturating_add(1);
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

    /// How many probes this session has performed. See the field's doc.
    #[must_use]
    pub const fn probes(&self) -> u64 {
        self.probes
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
    ///
    /// No serial key, and that is the fix rather than an omission: the counter
    /// lives on the `ForegroundState` this test owns, so no other test in the
    /// binary can move it. The process-global version needed a key, did not
    /// have one on five of its drivers, and read 60 against a ceiling of 60.
    #[test]
    fn probe_count_can_reach_one() {
        let mut state = ForegroundState::default();
        assert_eq!(state.probes(), 0, "nothing probed yet");

        let me = std::process::id();
        let observed = fact_for_pid(me);
        state.observe(0, observed.clone());
        assert_eq!(state.probes(), 1, "one probe must count as exactly one");
        state.observe(1, None);
        assert_eq!(
            state.probes(),
            2,
            "a MISS is a probe too -- it cost the same process-table read"
        );

        let observed = observed.expect("this test's own process must be in the process table");
        assert_eq!(observed.pid, me);
        assert!(
            !observed.name.is_empty(),
            "a fact with no name identifies nothing"
        );
    }

    /// I1: nothing that reads the process table may run under the master lock.
    ///
    /// A behavioural test cannot see this — the lock is held for microseconds
    /// and the walk still returns the right answer — so the guard is
    /// structural, and it is written against the two facts that make the rule
    /// hold rather than against a promise in a comment:
    ///
    /// 1. `PtySession::maybe_probe_foreground` never takes the master lock
    ///    itself. The only `self.master.lock()` on the probe path is inside
    ///    `terminal_leader`.
    /// 2. `terminal_leader`'s whole body is that lock plus
    ///    `leader_from_terminal`. No walk, no single-pid read, no `sysinfo`.
    ///
    /// Falsify by moving `deepest_newest_descendant` back into
    /// `terminal_leader` (the shape this replaced) — assertion 2 goes red — or
    /// by inlining the lock into `maybe_probe_foreground` — assertion 1 does.
    #[test]
    fn no_process_table_read_happens_under_the_master_lock() {
        use crate::utils::source_scan::code_text;

        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src")
            .join("gateway")
            .join("pty")
            .join("session.rs");
        let src = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
        let code = code_text(&src);

        let body_of = |name: &str| -> String {
            let at = code.find(name).unwrap_or_else(|| {
                panic!(
                    "{name} not found in session.rs -- if it was renamed, re-point this \
                     guard; if it was deleted, the master-lock boundary is gone"
                )
            });
            let open = code[at..].find('{').expect("no body") + at;
            let mut depth = 0usize;
            for (i, ch) in code[open..].char_indices() {
                match ch {
                    '{' => depth += 1,
                    '}' => {
                        depth -= 1;
                        if depth == 0 {
                            return code[open..=open + i].to_string();
                        }
                    }
                    _ => {}
                }
            }
            panic!("{name}'s body is not brace-balanced");
        };

        let probe = body_of("fn maybe_probe_foreground");
        assert!(
            !probe.contains("master.lock()"),
            "maybe_probe_foreground must reach the master ONLY through \
             terminal_leader, so the lock's scope stays one ioctl wide. Body:\n{probe}"
        );
        assert!(
            probe.contains("terminal_leader()"),
            "maybe_probe_foreground must still call terminal_leader -- without it \
             this guard passes vacuously. Body:\n{probe}"
        );

        let locked = body_of("fn terminal_leader");
        assert!(
            locked.contains("master.lock()") && locked.contains("leader_from_terminal"),
            "terminal_leader must be the lock plus the ioctl. Body:\n{locked}"
        );
        for forbidden in [
            "deepest_newest_descendant",
            "fact_for_pid",
            "sysinfo",
            "refresh_processes",
        ] {
            assert!(
                !locked.contains(forbidden),
                "`{forbidden}` reads the process table and must not run under the \
                 master lock -- `PtySession::resize` is the other holder, and on a \
                 platform that falls back to the descendant walk this would scan \
                 every process on the machine on every probe. Body:\n{locked}"
            );
        }
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
