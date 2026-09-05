// This file is Aleph's own code and carries the crate's MIT licence. It is
// NOT a port: `crates/agent-detect` is the machine-readable Apache-2.0 zone
// for herdr-derived code (phase-1 spec), and no herdr-derived code lives
// outside it.
//
// What IS taken from herdr 0.8.2 (https://github.com/herdrdev/herdr) are two
// NUMBERS and the shape of an idea. Every upstream line number below was read
// at herdr 0.8.2 on 2026-09-04; a citation is a claim about someone else's
// file, which is exactly the kind that rots unnoticed (判据 §1), so re-read
// before trusting one:
//   * 500 ms, the fast end of the acquisition interval upstream picks in
//     `should_probe_foreground_job` (`src/pane.rs:478`; the fast/slow choice
//     is `src/pane.rs:491-495`), between `PROCESS_ACQUISITION_FAST_RECHECK` =
//     500 ms and `PROCESS_ACQUISITION_SLOW_RECHECK` = 2 s
//     (`src/pane.rs:297-298`). THIS is the one full statement of that
//     citation — `PROBE_MIN_INTERVAL_MS`'s doc below points here rather than
//     repeating it, because the previous version said it twice and only this
//     copy carried the version the numbers were read at;
//   * 6, upstream's `AGENT_MISS_CONFIRMATION_ATTEMPTS` (`src/pane.rs:291`).
// Aleph's 3 s silent recheck is NOT upstream's number: the constant doing that
// job there is `PROCESS_RECHECK_IDENTIFIED` = 5 s (`src/pane.rs:292`). An
// earlier version of this header cited `src/pane.rs:776-789` for a
// "500 ms / 3 s" pair; those lines are the CALL SITE and compute no interval
// at all. That version also said 3 s "appears nowhere in herdr", which is
// FALSE and was corrected by re-reading upstream: `AGENT_STARTUP_GRACE_WINDOW`
// = 3 s (`src/pane/agent_detection.rs:12-13`) is one of nine `from_secs(3)`
// sites under herdr's `src/`. It is a different job — a grace window after an
// agent starts, not a recheck interval — which is the whole point: an absolute
// negative is the easiest claim to falsify and the most load-bearing when
// someone later reaches for it to "correct" this number back.
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
//! 2. [`foreground_fact_for_shell`] — the fallback when the terminal will
//!    not say, one FULL process-table refresh plus a single-pid read per
//!    descendant, outside every lock;
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
/// Borrowed from herdr, which uses 500 ms as the FAST end of an adaptive
/// acquisition interval. The citation — upstream version, symbol and line
/// numbers — is stated once, in this file's licence header, and deliberately
/// not repeated here: it was written out in full in both places, and only the
/// header's copy said which release the lines were read at, so the two were
/// one edit away from disagreeing about a fact neither of them owns
/// (判据 §1).
///
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
/// `argv` is the whole vector and NOT a joined command line, because
/// `agent_detect::identify_agent_from_process` has to tokenise it and a join
/// is lossy exactly where Windows needs it not to be: `argv[0]` there is the
/// full image path, so `["C:\Program Files\nodejs\node.exe", …]` joined and
/// re-split starts with the token `C:\Program`. That token is not an agent,
/// not a launcher and not a runtime, so the launcher walk went dead for every
/// process installed under a path with a space, and the panel printed
/// `Program` as the program name. Measured on Windows 11, 2026-09-05 — see
/// `agent_detect::engine::normalized_program_name`. Carrying the vector means
/// there is nothing to reconstruct (判据 §1).
///
/// It is also ONE field where there were two: `argv0` was `cmdline`'s first
/// token, i.e. a second representation of the same fact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForegroundFact {
    /// The process id the terminal's foreground process group leads with.
    pub pid: u32,
    /// The kernel's name for the process (`sysinfo::Process::name`).
    pub name: String,
    /// The process's argv, as the process table reports it. Empty when it
    /// reports none — which is "the table would not say", not "no arguments".
    pub argv: Vec<String>,
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
            && self.argv == other.argv
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
/// [`foreground_fact_for_shell`].
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
        ForegroundFact {
            pid,
            name: p.name().to_string_lossy().into_owned(),
            argv: p
                .cmd()
                .iter()
                .map(|a| a.to_string_lossy().into_owned())
                .collect(),
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

/// One row of the process table, as the descendant walk needs it.
///
/// The walk is split into this plain data plus the two pure functions below
/// for one reason: BOTH of the rules they carry are things a live process
/// table may simply not contain on the day you look. Measured on Windows 11
/// on 2026-09-05, three consecutive reads: 10 of 174 processes pointed at a
/// parent pid no longer in the table, and **0** of them had already turned
/// into a start-time inversion. A rule whose input never occurs cannot be
/// shown to go red against the live table (判据 §2), and a synthetic table
/// is the only honest instrument for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProcRow {
    pub pid: u32,
    pub parent: Option<u32>,
    /// The table's own start-time reading. Only ever COMPARED with another
    /// row's, never interpreted as a date, so the unit is the platform's.
    pub start_time: u64,
}

/// `shell_pid`'s descendants as `(depth, start_time, pid)` — depth 1 is a
/// direct child. Empty when it has none.
///
/// ⚠️ A parent edge is DROPPED when the claimed parent started strictly AFTER
/// the child. Windows recycles pids aggressively, and a process whose real
/// parent exited keeps pointing at that pid; the 10 dangling pointers
/// measured above are each a false edge waiting for their pid to be reused,
/// and a false edge here grafts an unrelated process tree onto a terminal.
/// The gate is one-directional and therefore safe: a real child cannot
/// predate its own parent, so nothing legitimate is ever dropped.
///
/// Bounded at 64 levels so a parent-pointer CYCLE (a reparented pid, a table
/// read mid-teardown) cannot spin here — the monotonicity gate makes a cycle
/// far less likely but does not make it impossible, because two rows may
/// report the same start time.
#[must_use]
pub fn descendants_of(rows: &[ProcRow], shell_pid: u32) -> Vec<(usize, u64, u32)> {
    let start_of = |pid: u32| rows.iter().find(|r| r.pid == pid).map(|r| r.start_time);
    let mut found: Vec<(usize, u64, u32)> = Vec::new();
    let mut frontier = vec![shell_pid];
    let mut depth = 0usize;
    while !frontier.is_empty() && depth < 64 {
        depth += 1;
        let mut next = Vec::new();
        for row in rows {
            let Some(parent) = row.parent else { continue };
            if !frontier.contains(&parent) {
                continue;
            }
            if start_of(parent).is_some_and(|p| p > row.start_time) {
                // A recycled pid: this row's real parent is gone.
                continue;
            }
            // A pid is emitted at most once. Without this a self-parented row
            // (or any cycle the monotonicity gate cannot see, because two rows
            // may report the SAME start time) is re-emitted at every level and
            // its 64 copies then win the ranking on depth alone.
            if row.pid == shell_pid || found.iter().any(|&(_, _, pid)| pid == row.pid) {
                continue;
            }
            next.push(row.pid);
            found.push((depth, row.start_time, row.pid));
        }
        frontier = next;
    }
    found
}

/// The deepest, then newest candidate — the answer when nothing under the
/// shell identifies as an agent.
///
/// Maximised on `(depth, start_time, pid)` in that order; the pid term is
/// there only so the answer is a function of the table and not of its
/// iteration order (判据 §12).
#[must_use]
fn deepest_newest(candidates: &[(usize, u64, u32)]) -> Option<u32> {
    candidates.iter().max().map(|&(_, _, pid)| pid)
}

/// The foreground process of a shell that will not name one itself.
///
/// This is the whole answer on platforms with no `tcgetpgrp`, and the
/// fallback on the ones that have it (see [`leader_from_terminal`]). It is
/// deliberately NOT `#[cfg(not(unix))]`: a branch that compiles on no machine
/// any developer or CI job runs is a branch nobody can falsify, and its only
/// proof would have been "it type-checks on Windows".
///
/// ⚠️ That reasoning was right and the follow-through was not. An earlier doc
/// here said "compiled and tested everywhere" while its one exerciser,
/// `the_descendant_walk_finds_a_child_this_test_started`, carried
/// `#[cfg(unix)]` — so the sentence describing the guarantee and the guard
/// providing it disagreed, and the one that got read was the sentence
/// (判据 §1). That guard and `a_real_child_is_reported_as_the_foreground_program`
/// are now platform-independent and were run on Windows 11 on 2026-09-05.
/// Do not re-gate either one.
///
/// # Why not simply the deepest descendant
///
/// "Deepest, then newest" was the whole rule until 2026-09-05, and it answers
/// a DIFFERENT QUESTION from "which program owns this terminal". On Unix
/// `tcgetpgrp` names the process GROUP LEADER, and an agent's tool
/// subprocesses inherit its pgid — so `claude` running `rg` still answers
/// `claude`. Windows has no process groups to ask about, so the deepest
/// descendant of `claude` is `rg`, and `program` flipped to whatever tool the
/// agent had most recently spawned — exactly while it was working, which is
/// the only time anyone is looking.
///
/// So the rank is: the SHALLOWEST candidate that identifies as an agent
/// (shallowest, because `npx claude` puts the launcher above the agent and
/// the launcher's own command line already names its operand — see
/// `agent_detect::identify_agent_from_process`), and only failing that, the
/// deepest and newest. An agent's tools are always deeper than the agent.
///
/// # Cost
///
/// One FULL process-table refresh for the tree — a descendant walk needs
/// every process's parent — plus one single-pid read per DESCENDANT, which is
/// 0–2 in practice and never the table. That is why this is second choice,
/// why the rate gate exists, and why it takes a bare `u32` and no terminal
/// handle: a function that cannot be handed the master cannot be called while
/// the master lock is held by accident.
#[must_use]
pub fn foreground_fact_for_shell(shell_pid: u32) -> Option<ForegroundFact> {
    let candidates = descendants_of(&process_rows(), shell_pid);
    // A shell sitting at its prompt IS the foreground program.
    if candidates.is_empty() {
        return fact_for_pid(shell_pid);
    }
    let facts: Vec<(usize, u64, ForegroundFact)> = candidates
        .iter()
        .filter_map(|&(depth, start, pid)| fact_for_pid(pid).map(|f| (depth, start, f)))
        .collect();
    pick_foreground(&facts)
        .or_else(|| fact_for_pid(deepest_newest(&candidates).unwrap_or(shell_pid)))
        // Every descendant died between the walk and the fact read. The shell
        // is what is in the foreground now, and saying so is better than the
        // miss that "I could not look" would spend (判据 §8 cuts the other
        // way here: there IS an answer).
        .or_else(|| fact_for_pid(shell_pid))
}

/// Rank already-collected candidate facts. Pure, and it runs the REAL
/// identification rather than a predicate handed in by the caller, so the
/// production answer and the test's answer cannot come from two different
/// derivations of "is this an agent" (判据 §1 / §9).
#[must_use]
fn pick_foreground(facts: &[(usize, u64, ForegroundFact)]) -> Option<ForegroundFact> {
    let is_agent = |f: &ForegroundFact| {
        agent_detect::identify_agent_from_process(&f.name, &f.argv).is_some()
    };
    facts
        .iter()
        .filter(|(_, _, f)| is_agent(f))
        .min_by_key(|(depth, start, f)| (*depth, *start, f.pid))
        .or_else(|| facts.iter().max_by_key(|(depth, start, f)| (*depth, *start, f.pid)))
        .map(|(_, _, f)| f.clone())
}

/// The parent-pointer table, as one refresh. `ProcessRefreshKind::nothing()`
/// because the walk reads only pid / parent / start time; the per-candidate
/// command lines are fetched afterwards, for the handful of pids that
/// survived, by [`fact_for_pid`].
fn process_rows() -> Vec<ProcRow> {
    use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System};

    let mut sys = System::new();
    sys.refresh_processes_specifics(ProcessesToUpdate::All, true, ProcessRefreshKind::nothing());
    sys.processes()
        .iter()
        .map(|(pid, proc_)| ProcRow {
            pid: pid.as_u32(),
            parent: proc_.parent().map(sysinfo::Pid::as_u32),
            start_time: proc_.start_time(),
        })
        .collect()
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
            argv: Vec::new(),
            cwd: None,
            observed_at,
        }
    }

    fn row(pid: u32, parent: u32, start_time: u64) -> ProcRow {
        ProcRow {
            pid,
            parent: (parent != 0).then_some(parent),
            start_time,
        }
    }

    fn candidate(depth: usize, start: u64, pid: u32, name: &str, argv: &[&str]) -> (usize, u64, ForegroundFact) {
        (
            depth,
            start,
            ForegroundFact {
                pid,
                name: name.to_owned(),
                argv: argv.iter().map(|a| (*a).to_owned()).collect(),
                cwd: None,
                observed_at: 0,
            },
        )
    }

    /// A recycled pid makes a live process point at a parent that is NEWER
    /// than itself, and the walk must not graft that tree onto the terminal.
    ///
    /// ⚠️ This is a synthetic table on purpose, and that is the whole point of
    /// [`ProcRow`] existing. Measured on this machine on 2026-09-05, three
    /// consecutive reads of the live table: `dangling_ppid=10` of 174, and
    /// `inverted_start_time=0`. So the hazard is real and loaded (10 pointers
    /// into pids the OS is free to hand out again) while the FIRING CONDITION
    /// was absent — a rule that cannot be shown red against the machine you
    /// have is a rule you must be able to show red some other way (判据 §2).
    ///
    /// Falsify by deleting the `start_of(parent) > row.start_time` guard in
    /// [`descendants_of`]: pid 900 is adopted and `deepest_newest` names it.
    #[test]
    fn a_parent_pointer_that_predates_its_child_is_not_an_edge() {
        // 100 is the shell. 200 is its real child. 900 is an unrelated
        // process that started LONG ago and whose real parent has exited;
        // the OS then handed its pid, 200, to the shell's child.
        let rows = [
            row(100, 0, 1_000),
            row(200, 100, 1_100),
            row(900, 200, 5), // started at 5, "parent" started at 1_100
        ];
        let found = descendants_of(&rows, 100);

        assert_eq!(
            found,
            vec![(1, 1_100, 200)],
            "the shell's real child is the only descendant; 900 predates the \
             pid it points at, so that edge is a recycled pid and not a parent"
        );
        assert_eq!(deepest_newest(&found), Some(200));

        // The gate is one-directional: give 900 a start time AFTER 200 and it
        // is an ordinary grandchild again. Without this row the test would
        // pass just as well against a walk that dropped every depth-2 node.
        let rows = [row(100, 0, 1_000), row(200, 100, 1_100), row(900, 200, 1_200)];
        assert_eq!(
            deepest_newest(&descendants_of(&rows, 100)),
            Some(900),
            "a child that postdates its parent is a real edge and must survive"
        );
    }

    /// An agent outranks its own tool subprocess, which is the difference
    /// between `program: "claude"` and `program: "rg"` on every Windows
    /// terminal with an agent working in it.
    ///
    /// Runs the REAL `agent_detect` identification rather than a predicate
    /// this test hands in, so the ranking and production cannot disagree
    /// about what an agent is (判据 §9).
    ///
    /// Falsify by deleting the `filter(is_agent).min_by_key(...)` arm of
    /// [`pick_foreground`]: the first case then answers `rg`.
    #[test]
    fn an_agent_outranks_the_tool_it_spawned_and_only_an_agent_does() {
        // cmd.exe -> node (claude) -> rg. Deepest-and-newest says `rg`.
        let tree = [
            candidate(1, 10, 200, "node.exe", &["C:\\Program Files\\nodejs\\node.exe",
                "C:\\p\\node_modules\\@anthropic-ai\\claude-code\\cli.js"]),
            candidate(2, 20, 300, "rg.exe", &["rg", "--json", "TODO"]),
        ];
        assert_eq!(
            pick_foreground(&tree).map(|f| f.pid),
            Some(200),
            "the agent owns the terminal; the tool it spawned is deeper and newer"
        );

        // Two agents (`claude` launched under `npx`): the SHALLOWEST wins,
        // because the launcher's own command line already names its operand.
        let chain = [
            candidate(1, 10, 200, "node.exe", &["npm exec claude", "TERM_PROGRAM=x"]),
            candidate(2, 20, 300, "node.exe", &["claude"]),
        ];
        assert_eq!(pick_foreground(&chain).map(|f| f.pid), Some(200));

        // No agent anywhere: the fallback is unchanged, deepest then newest.
        // Without this the preference could be "always the shallowest" and
        // nothing here would notice.
        let plain = [
            candidate(1, 10, 200, "cmd.exe", &["cmd.exe", "/c", "ping"]),
            candidate(2, 20, 300, "PING.EXE", &["ping", "-n", "30", "127.0.0.1"]),
        ];
        assert_eq!(
            pick_foreground(&plain).map(|f| f.pid),
            Some(300),
            "with no agent in the tree the answer is still the deepest, newest"
        );
        assert_eq!(pick_foreground(&[]), None, "no candidates, no answer");
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
    /// Falsify by moving `foreground_fact_for_shell` back into
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
            "foreground_fact_for_shell",
            "descendants_of",
            "process_rows",
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

    /// A live TWO-LEVEL process tree: a parent that stays alive while a child
    /// of its own runs for ~30 s. Returns the parent handle; the caller kills
    /// the tree with [`kill_tree`].
    ///
    /// Unix: `sh -c "sleep 30; true"` — the trailing `true` is what stops the
    /// shell `exec`ing the sleep and vanishing, which would leave one level
    /// and nothing to descend to.
    ///
    /// Windows: `cmd /c ping -n 31 127.0.0.1`. Windows has no `exec`, so
    /// `cmd` is unconditionally a real parent of the external command it
    /// runs — the same two levels, reached by a different mechanism.
    /// `ping` and not `timeout` because `timeout` reads the console INPUT
    /// handle and aborts with "input redirection is not supported" the moment
    /// stdin is not one, which is exactly what a captured test child has.
    fn spawn_two_level_tree() -> std::process::Child {
        let mut cmd = if cfg!(windows) {
            let mut c = std::process::Command::new("cmd.exe");
            c.args(["/c", "ping", "-n", "31", "127.0.0.1"]);
            c
        } else {
            let mut c = std::process::Command::new("sh");
            c.args(["-c", "sleep 30; true"]);
            c
        };
        cmd.stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("spawn a two-level process tree")
    }

    /// Kill a root process AND the child it started.
    ///
    /// `Child::kill` alone is not enough on either platform: it kills the
    /// parent and leaves the grandchild running out its full 30 s. On Windows
    /// that orphan also keeps a ppid pointing at a pid the OS is now free to
    /// hand out again — a test that leaked one would be seeding the exact
    /// stale-edge hazard `descendants_of` has to survive.
    fn kill_tree(root: u32, inner: u32) {
        let mut cmd = if cfg!(windows) {
            // `/T` takes the tree, so `inner` is covered by naming the root.
            let mut c = std::process::Command::new("taskkill");
            c.args(["/PID", &root.to_string(), "/T", "/F"]);
            c
        } else {
            let mut c = std::process::Command::new("kill");
            c.args(["-TERM", &root.to_string(), &inner.to_string()]);
            c
        };
        let _ = cmd
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    }

    /// The Windows answer, exercised ON WINDOWS.
    ///
    /// `foreground_fact_for_shell` is the whole foreground answer on
    /// platforms without `tcgetpgrp`, and until 2026-09-05 this guard carried
    /// `#[cfg(unix)]` — so the one platform that depends on the walk for
    /// EVERYTHING was the one platform that never ran it. The walk was
    /// deliberately not `#[cfg(not(unix))]` precisely so that it could be
    /// exercised everywhere, and then its only exerciser was gated to the
    /// platform that does not need it (判据 §3: a guard's green covers only
    /// the shapes it recognises, and this one did not recognise Windows at
    /// all; 判据 §16: the porting round fixed the module for Windows and left
    /// its twin, the test, on Unix).
    ///
    /// The tree is rooted at a process this test OWNS, not at the test binary
    /// itself: sibling tests in this crate spawn PTY children of the test
    /// process, so a walk rooted there answers with whichever of THEIR
    /// children happens to be newest. That is not flakiness in the walk, it
    /// is a guard whose corpus was bigger than its subject.
    #[tokio::test(flavor = "multi_thread")]
    async fn the_descendant_walk_finds_a_child_this_test_started() {
        let mut child = spawn_two_level_tree();
        let shell_pid = child.id();

        // The process table needs a moment to see a just-forked grandchild.
        let mut answer = None;
        for _ in 0..40 {
            answer = foreground_fact_for_shell(shell_pid).map(|f| f.pid);
            if answer.is_some_and(|p| p != shell_pid) {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        let inner = answer.expect("the walk must never answer None for a live root");
        // Printed so a passing run carries the measurement and not just the
        // word "ok": on a platform where the walk is the ONLY answer, "which
        // process did it actually name" is the whole result (判据 §18).
        //
        // The elapsed time is printed for the same reason. This walk is a
        // FULL `ProcessesToUpdate::All` refresh and it runs on every probe of
        // every session on a platform without `tcgetpgrp`, so "how expensive
        // is second choice when it is the only choice" is a number the rate
        // gate's constants are chosen against — and it was never once read on
        // Windows before 2026-09-05. NOT asserted on: it is hardware, and a
        // ceiling nobody can reproduce is worse than no ceiling (判据 §13).
        let t0 = std::time::Instant::now();
        let repeat = foreground_fact_for_shell(shell_pid).map(|f| f.pid);
        let elapsed = t0.elapsed();
        // Read BEFORE the kill below. This entry answers with a FACT, and a
        // dead pid has none — `fact_for_pid` says "I could not look", which is
        // the right answer and is not the one this line is asking about. The
        // pid-only predecessor could not tell the two apart: it ended in
        // `.or(Some(shell_pid))` and so named a process that had already
        // exited (production then got `None` one call later anyway, from
        // `fact_for_pid`, so only this guard's subject moved).
        let leaf_answers_itself = foreground_fact_for_shell(inner).map(|f| f.pid);
        eprintln!(
            "walk: root={shell_pid} -> descendant={inner} (repeat={repeat:?}) \
             one_walk={elapsed:?} fact={:?}",
            fact_for_pid(inner)
        );
        kill_tree(shell_pid, inner);
        let _ = child.kill();
        let _ = child.wait();

        assert_ne!(
            inner, shell_pid,
            "the walk must descend past the shell ({shell_pid}) to the child it started"
        );
        assert_eq!(
            leaf_answers_itself,
            Some(inner),
            "a process with no descendants must answer with ITSELF, not None -- \
             a shell sitting at its prompt is the foreground program"
        );
    }

    /// A real PTY, a real child, and the production call order — on both
    /// platforms, because the two platforms reach the SAME answer down two
    /// different halves of this module.
    ///
    /// Unix: the child `portable-pty` spawns becomes the terminal's
    /// foreground process group (it `setsid`s and takes the pty as its
    /// controlling terminal), so `leader_from_terminal` names it directly and
    /// the walk is never reached.
    ///
    /// Windows: there is no `tcgetpgrp`, `leader_from_terminal` answers
    /// `None` on every probe, and the identical assertion below can only be
    /// satisfied by `foreground_fact_for_shell`. That makes this the ONLY
    /// place the Windows fallback is exercised through production's own call
    /// order — and it also pins the fall-through itself: an "optimisation"
    /// that returned early on a `None` leader would take Windows from
    /// working to blind, and until 2026-09-05 nothing would have gone red
    /// (判据 §8 — `None` is "I could not look", never "nothing is there").
    ///
    /// It drives the PRODUCTION path —
    /// `PtySession::maybe_probe_foreground` then `foreground_fact()` — and
    /// not a test-only twin, so the two halves are exercised in the order and
    /// under the locking discipline production uses (判据 §7: a guard on a
    /// function production does not call proves the function, not the wire).
    #[tokio::test(flavor = "multi_thread")]
    async fn a_real_child_is_reported_as_the_foreground_program() {
        // The same two-level shape as `spawn_two_level_tree`, spawned on a
        // PTY instead of a pipe. On Windows the subject of the assertion is
        // the GRANDCHILD (`ping`), because that is what the walk must reach;
        // on Unix `sleep` is both the child and the pgid leader.
        let (command, args, expect) = if cfg!(windows) {
            (
                "cmd.exe",
                vec!["/c", "ping", "-n", "31", "127.0.0.1"],
                "ping",
            )
        } else {
            ("sleep", vec!["30"], "sleep")
        };
        let opts = super::super::SpawnOptions {
            command: Some(command.to_string()),
            args: args.into_iter().map(String::from).collect(),
            rows: 6,
            cols: 40,
            ..Default::default()
        };
        let session = super::super::PtySession::spawn("t-foreground-probe".into(), &opts, None)
            .expect("spawn");

        // `sysinfo` reports `PING.EXE` on Windows and `sleep` on Unix; the
        // case is the platform's, not the program's.
        let names_the_child = |f: &ForegroundFact| f.name.to_ascii_lowercase().contains(expect);

        let mut seen: Option<ForegroundFact> = None;
        for i in 0..60 {
            // `frame_produced` is true and the min interval is respected by
            // spacing the calls, so rules 1 and 2 both authorise these.
            session.maybe_probe_foreground(i * PROBE_MIN_INTERVAL_MS, true, false);
            if let Some(f) = session.foreground_fact() {
                let hit = names_the_child(&f);
                seen = Some(f);
                if hit {
                    break;
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        session.kill();

        let seen = seen.expect("the probe returned nothing for a live child in 3s");
        // On Windows this line is the evidence that the FALLBACK ran: the
        // subject is a grandchild, and `leader_from_terminal` cannot name one.
        eprintln!("probe: expect={expect} seen={seen:?}");
        assert!(
            names_the_child(&seen),
            "the foreground program must be the child ({expect}), not the server \
             and not the shell that launched it: {seen:?}"
        );
        assert!(seen.pid > 0, "a fact with no pid names no process");
    }
}
