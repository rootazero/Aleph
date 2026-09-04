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

/// How long a session must produce no frame before its silence is published
/// as [`RuntimeAgentEntry::quiet_since`].
///
/// This is NOT a state transition and must never become one (spec R2-3). An
/// agent thinking for five minutes emits nothing; code that let the clock turn
/// `Working` into `Idle` would be manufacturing evidence rather than reporting
/// it, which is the one thing the panels are careful not to do. Thirty seconds
/// is a human threshold — long enough that ordinary pauses between an agent's
/// tool calls do not trip it, short enough that "is this thing stuck" has an
/// answer before the person asks.
///
/// Upstream has no equivalent: herdr keeps a stalled agent visible by SORTING
/// rather than by publishing a duration. Aleph's panels are lists, so the
/// duration is the cheaper mechanism.
pub const QUIET_AFTER_MS: i64 = 30_000;

/// One row of the table: the wire entry plus the sampler's own bookkeeping.
///
/// `pending_idle_since` is deliberately NOT on [`RuntimeAgentEntry`]: it is
/// how this module reaches its answer, not part of the answer. A client that
/// could see it would have a second, earlier opinion about the same state.
#[derive(Debug, Clone)]
struct TableEntry {
    entry: RuntimeAgentEntry,
    /// Which agent [`RuntimeAgents::sample`] identified, as the detection
    /// engine's own type.
    ///
    /// The same fact as `entry.agent`, which is its LABEL — both are written
    /// in one expression from one `identify_agent` answer, so they cannot
    /// drift; what this field buys is that a consumer needing to run the
    /// engine again ([`RuntimeAgents::detected_agent`], for
    /// `terminal{explain}`) does not have to parse the label back into an
    /// `Agent`. A round trip through `agent_label` -> `identify_agent` would
    /// be a SECOND derivation of "which agent is this", and the day one of
    /// the 23 labels stopped round-tripping, `explain` would quietly explain
    /// a different agent than `status` reports (判据 §1).
    agent: Option<agent_detect::Agent>,
    /// Unix millis at which a working -> plain-idle transition was first
    /// observed, while it is being held. `None` = nothing held.
    pending_idle_since: Option<i64>,
    /// Unix millis of the last frame this session produced.
    ///
    /// Advanced ONLY by a sample that carries
    /// [`SampleInput::frame_produced`]. A sample is not evidence of a frame:
    /// `flush_session` also re-samples when the foreground program changed
    /// with the screen standing still, and a `cwd` move counts as a program
    /// change — so treating every sample as a frame restarted the quiet clock
    /// for an identified agent that `chdir`s while thinking silently, and
    /// republished it as not-quiet for another 30 s. That is a fact on the
    /// wire with no producer behind it.
    ///
    /// (This doc previously asserted the opposite, and that assertion was the
    /// whole argument for leaving `frame_produced` out of [`SampleInput`]. The
    /// predicate does vary at the only production call site.)
    last_frame_at: i64,
}

/// One screen observation, as the flush loop assembles it.
///
/// A struct rather than nine positional arguments because four of them are
/// `Option<&str>`/`&str` and adjacent — the shape where a swapped pair
/// compiles and then lies. The caller is `pty::manager::flush_session`.
pub struct SampleInput<'a> {
    /// The PTY session id; the table's key.
    pub session_id: &'a str,
    /// [`crate::gateway::pty::PtySession::shell`] — the SPAWN-TIME label.
    /// Used for `label` and as the identification fallback when the probe
    /// could not answer.
    pub shell: &'a str,
    /// The foreground process's name, from the probe. `None` = the probe
    /// could not look; it is never the shell standing in for a program.
    pub program: Option<&'a str>,
    /// The foreground process's `argv[0]`, from the probe.
    pub argv0: Option<&'a str>,
    /// The foreground process's whole command line, from the probe. This is
    /// what identifies `claude`, which runs as a Node script.
    pub cmdline: Option<&'a str>,
    /// The session's live working directory. Sourced by the caller in a fixed
    /// order — see `flush_session`.
    pub cwd: &'a str,
    /// The server-held screen, borrowed under its lock by the caller.
    pub screen: &'a Screen,
    /// The session's `is_closed()`.
    pub process_exited: bool,
    /// Whether the screen produced a frame on this tick.
    ///
    /// NOT constant-true, which an earlier version of this struct assumed
    /// while leaving the field out: `flush_session` also re-samples when the
    /// foreground program changed with the screen standing still. It is the
    /// only evidence [`RuntimeAgents::mark_quiet`] accepts, so a sample
    /// without it must leave the quiet bookkeeping exactly as it found it —
    /// otherwise a silent agent that `chdir`s is republished as not-quiet.
    pub frame_produced: bool,
    /// Unix millis, taken once per tick by the caller.
    pub now: i64,
}

/// The process-wide agent table.
///
/// A `LazyLock` singleton reached through [`agents()`], mirroring
/// `pty::manager()` in this same subsystem: the JSON-RPC handler (task 6) and
/// the tool face (task 11) both need it and neither has an `AppContext` to
/// thread it through. `Default` exists so a test can hold an isolated
/// instance instead of racing the global one.
pub struct RuntimeAgents {
    entries: Mutex<HashMap<String, TableEntry>>,
    /// Monotonic count of observable changes, for waiters.
    ///
    /// A `watch` channel rather than a broadcast: a waiter wants to know THAT
    /// the table changed, not which changes it missed, and `watch` collapses a
    /// burst into one wake-up by construction. The value is a generation
    /// number so a waiter that reads it before sleeping cannot miss a change
    /// that lands in between.
    ///
    /// `Sender` keeps working with zero receivers, so the bump costs the same
    /// whether or not anything is listening.
    generation: tokio::sync::watch::Sender<u64>,
    /// How many times [`Self::sample`] has built a screen's visible text.
    ///
    /// The cost guard's instrument for S3, and the twin of
    /// `pty::foreground::probe_count`. Building the text allocates the visible
    /// grid; when no agent is identified the detection engine discards it
    /// unread (`agent_detect::detect`'s permanent early return for
    /// `agent: None`), so the identification has to run FIRST. That is not
    /// observable from outside — the text is a pure value — which is why it is
    /// counted.
    ///
    /// PER INSTANCE, not a `static`. A process-global counter is a shared
    /// mutable fact between tests that otherwise have none, and the first
    /// version of this was one: `identify_runs_before_the_screen_text_is_built`
    /// read **9** where it expected 1, because sibling tests in the same
    /// binary were sampling concurrently. An isolated `RuntimeAgents` already
    /// isolates everything else this module owns; the counter belongs with it.
    visible_text_builds: crate::sync_primitives::AtomicU64,
}

impl Default for RuntimeAgents {
    fn default() -> Self {
        Self {
            entries: Mutex::default(),
            generation: tokio::sync::watch::channel(0).0,
            visible_text_builds: crate::sync_primitives::AtomicU64::new(0),
        }
    }
}

static GLOBAL: LazyLock<RuntimeAgents> = LazyLock::new(RuntimeAgents::default);

/// Access the process-global agent table.
#[must_use]
pub fn agents() -> &'static RuntimeAgents {
    &GLOBAL
}

impl RuntimeAgents {
    /// How many screen texts this table has built. See
    /// [`Self::visible_text_builds`]'s field doc for what it is for.
    #[must_use]
    pub fn visible_text_builds(&self) -> u64 {
        self.visible_text_builds
            .load(crate::sync_primitives::Ordering::Relaxed)
    }

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
    /// # What identifies the agent
    ///
    /// The FOREGROUND PROCESS ([`SampleInput::program`] / `argv0` / `cmdline`,
    /// from `pty::foreground`), falling back to
    /// [`crate::gateway::pty::PtySession::shell`] — the spawn-time label —
    /// only when the probe could not answer.
    ///
    /// That order is the whole point of round 2. Until the probe existed this
    /// function was handed the spawn label alone, and the spawn label is
    /// `"zsh"`: the Panel's terminal view sends `{rows, cols}` and no
    /// `command`, and agents are started INTERACTIVELY afterwards. So
    /// `identify_agent("zsh")` returned `None`, `detect_agent_with_osc`
    /// early-returned `Unknown` before any manifest rule was consulted, and
    /// the manifests, the rule engine and the idle hold were all correct and
    /// all unreachable in production. The end-to-end guard for the wire that
    /// closed it is
    /// `tests::a_real_agent_started_after_spawn_is_identified`, which names
    /// the agent nowhere and lets the probe find it.
    ///
    /// Do NOT "improve" the unidentified case by dropping `agent_detect`'s
    /// `agent: None` early return: that turns "I do not know" into "it is
    /// idle", which is the one thing the panels are careful not to say.
    ///
    /// # Cost
    ///
    /// The identification runs BEFORE `screen.visible_text()`, and the text is
    /// built only when an agent was identified — with no agent the engine
    /// discards it unread, so building it first was an allocation the size of
    /// the visible grid, per session, per frame, thrown away. Counted by
    /// [`visible_text_count`] and guarded by
    /// `tests::identify_runs_before_the_screen_text_is_built`.
    ///
    /// # The rest of the inputs
    ///
    /// `cwd` is the session's LIVE directory, sourced by the caller.
    /// `frame_produced` says whether the screen actually changed — reaching
    /// this function is NOT evidence of that, because `flush_session` also
    /// re-samples on a foreground-program change, and only a real frame may
    /// end a quiet mark ([`TableEntry::last_frame_at`]).
    /// `process_exited` is the session's
    /// `is_closed()`: a session killed but not yet reaped by its reader thread
    /// is still in the registry for up to one flush tick, and reporting it as
    /// still Working for that tick would be a stale answer rather than a
    /// missing one. `now` is unix millis — see the module doc on time.
    ///
    /// One lock acquisition on the table; the caller holds the screen lock
    /// for the duration.
    pub fn sample(&self, input: SampleInput<'_>) -> bool {
        let SampleInput {
            session_id,
            shell,
            program,
            argv0,
            cmdline,
            cwd,
            screen,
            process_exited,
            frame_produced,
            now,
        } = input;

        // FIRST, before any screen text is built (see the Cost section).
        //
        // One derivation for two fields: `program_name` is what to CALL what
        // is running and `agent` is which agent that is, and both come out of
        // the same `normalized_program_name` answer, so they cannot end up
        // describing different tokens (判据 §1). The kernel's raw name is not
        // publishable on its own — macOS reports a `#!/bin/sh` script called
        // `claude` as `bash`.
        let (program_name, agent) = match program {
            Some(name) => {
                let resolved = agent_detect::normalized_program_name(name, argv0, cmdline);
                let agent = agent_detect::identify_agent(&resolved);
                (Some(resolved), agent)
            }
            // The probe could not look. The spawn label is a weaker answer,
            // not a wrong one -- `pty.spawn` with an explicit `command` does
            // put the agent's name there. `program` stays `None`: "we could
            // not look" must not be spelled the same way as "the shell is
            // what is running" (`RuntimeAgentEntry::program`'s doc).
            None => (None, agent_detect::identify_agent(shell)),
        };

        let title = screen.title().unwrap_or_default();
        // `None` (this program never reported progress) and `""` (the engine's
        // spelling of "no data") mean the same thing, so the conversion is
        // faithful rather than a fail-open read of an absent answer.
        let osc_progress = screen.osc_progress().unwrap_or_default();
        let text = if agent.is_some() {
            self.visible_text_builds
                .fetch_add(1, crate::sync_primitives::Ordering::Relaxed);
            screen.visible_text()
        } else {
            String::new()
        };

        let detection = agent_detect::screen_rules::detection_update_for_publish_with_osc(
            agent,
            &text,
            title,
            osc_progress,
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
        let previously_quiet = previous.is_some_and(|t| t.entry.quiet_since.is_some());
        // The quiet bookkeeping moves ONLY on a real frame. A sample without
        // one (a foreground-program change while the screen stood still) must
        // carry both values through untouched, or a silent agent that
        // `chdir`s is republished as not-quiet for another QUIET_AFTER_MS.
        // A brand-new row with no frame has never been seen to paint, so its
        // clock starts now rather than at an instant nobody observed.
        let (last_frame_at, quiet_since) = if frame_produced {
            (now, None)
        } else {
            previous.map_or((now, None), |t| (t.last_frame_at, t.entry.quiet_since))
        };
        // A brand-new row has no previous agent to differ from, so its first
        // observation is not an agent CHANGE.
        let agent_changed = previous.is_some_and(|t| t.entry.agent != agent_name);
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
                // `!next.visible_blocker` (herdr `agent_detection.rs:50`) is
                // still omitted because it cannot vary: `manifest.rs:470`
                // computes `visible_blocker: rule.visible_blocker && state ==
                // Blocked`, so `state == Idle && visible_blocker` is
                // unreachable.
                //
                // `!agent_changed` (`:51`) is now CARRIED. This doc used to
                // say the term "comes back the day a phase identifies the
                // agent from something mutable (an OSC title, a PID probe)" —
                // the foreground probe is that day. A hold is an argument
                // about one agent's transition; when the program underneath
                // changes, the argument is about a different program and must
                // not survive.
                if observed == RuntimeAgentState::Idle
                    && previous_state == RuntimeAgentState::Working
                    && !d.visible_idle
                    && !agent_changed
                {
                    pending_idle_since = Some(pending_idle_since.unwrap_or(now));
                    RuntimeAgentState::Working
                } else {
                    pending_idle_since = None;
                    observed
                }
            }
        };

        // `quiet_since` enters this predicate as a FLIP, never as a value:
        // the only transition a sample can make is Some -> None, and only a
        // real frame makes it. A sample that carried no frame leaves the mark
        // where it was, so this term is false and the row is judged on its
        // other fields alone. An age that grows on its own is not news (R6-4).
        let changed = previous.is_none_or(|t| {
            t.entry.state != state
                || t.entry.agent != agent_name
                || t.entry.program != program_name
                || t.entry.label != label
                || t.entry.cwd != cwd
        }) || (previously_quiet && quiet_since.is_none());

        entries.insert(
            session_id.to_owned(),
            TableEntry {
                agent,
                entry: RuntimeAgentEntry {
                    session_id: session_id.to_owned(),
                    label,
                    cwd: cwd.to_owned(),
                    agent: agent_name,
                    program: program_name,
                    state,
                    updated_at: if changed { now } else { previous_updated_at },
                    quiet_since,
                },
                pending_idle_since,
                last_frame_at,
            },
        );
        drop(entries);
        if changed {
            self.bump();
        }
        changed
    }

    /// Publish the silence of every session that has produced no frame for
    /// [`QUIET_AFTER_MS`]. Returns the ids whose mark FLIPPED on this call.
    ///
    /// Called once per flush tick beside [`Self::release_expired`], and for
    /// the same reason: a session that went quiet produces no frame, so the
    /// per-session loop cannot reach it. Same tick, so no second clock
    /// (判据 §12).
    ///
    /// The mark is `Some(last_frame_at)` — the moment the silence STARTED,
    /// not the moment it was noticed — so a client renders an age that does
    /// not jump when the server was busy. It flips once and then stops
    /// reporting: staying quiet is not news, and an event per tick per quiet
    /// session is the noise clients learn to ignore (R6-4).
    ///
    /// `state` is untouched here, and that is the rule this function exists to
    /// obey: silence is a fact about output, not evidence about what the agent
    /// is doing (spec R2-3).
    pub fn mark_quiet(&self, now: i64) -> Vec<String> {
        let mut flipped = Vec::new();
        {
            let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
            for (id, row) in entries.iter_mut() {
                if row.entry.quiet_since.is_some() {
                    continue;
                }
                if now.saturating_sub(row.last_frame_at) >= QUIET_AFTER_MS {
                    row.entry.quiet_since = Some(row.last_frame_at);
                    flipped.push(id.clone());
                }
            }
            flipped.sort();
        }
        if !flipped.is_empty() {
            self.bump();
        }
        flipped
    }

    /// One session's published entry, or `None` when the table has no row for
    /// it.
    ///
    /// `None` means "nothing has ever been sampled here" — a session that has
    /// produced no frame, or one whose row was removed at child exit. It is
    /// NOT "this session is idle" and callers must not fold it into one
    /// (判据 §8): `terminal{wait}` reads it as "keep waiting" while the
    /// session is still registered, and as `gone` once it is not.
    ///
    /// A per-session lookup rather than filtering [`Self::snapshot`]: a waiter
    /// re-reads this on every wake-up, and cloning the whole table to answer
    /// about one row is a cost that grows with everyone else's sessions.
    #[must_use]
    pub fn entry(&self, session_id: &str) -> Option<RuntimeAgentEntry> {
        self.entries
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(session_id)
            .map(|t| t.entry.clone())
    }

    /// Which agent [`Self::sample`] identified in this session, as the
    /// detection engine's own type — the input `agent_detect`'s
    /// `explain_with_input` takes.
    ///
    /// Two absences are folded here on purpose, because the caller
    /// (`terminal{explain}`) must tell them apart and asks [`Self::entry`]
    /// first: no row at all, and a row whose foreground program is not an
    /// agent. Read alone, `None` says only "no agent to explain".
    #[must_use]
    pub fn detected_agent(&self, session_id: &str) -> Option<agent_detect::Agent> {
        self.entries
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(session_id)
            .and_then(|t| t.agent)
    }

    /// Whether this session currently has an identified agent.
    ///
    /// The foreground probe's recheck rule needs this
    /// (`pty::foreground::probe_due`'s rule 3), and the table is where the
    /// answer already lives — asking the session would mean re-running
    /// identification in a second place (判据 §1). An unknown session is
    /// `false`, which is the honest reading: nothing has identified an agent
    /// there.
    #[must_use]
    pub fn agent_known(&self, session_id: &str) -> bool {
        self.entries
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(session_id)
            .is_some_and(|t| t.entry.agent.is_some())
    }

    /// A receiver that is woken by every observable change to this table.
    ///
    /// The value is a generation number, so `terminal{wait}` (task W-D) can
    /// read it BEFORE it starts waiting and cannot miss a change that lands
    /// between the read and the sleep. `watch` rather than `broadcast` because
    /// a waiter wants to know that something changed, not to replay what it
    /// missed — a burst collapses into one wake-up by construction.
    #[must_use]
    pub fn subscribe(&self) -> tokio::sync::watch::Receiver<u64> {
        self.generation.subscribe()
    }

    /// The current generation number. Same counter [`Self::subscribe`]
    /// delivers; exposed so a caller with no receiver can still tell whether
    /// anything moved.
    #[must_use]
    pub fn generation(&self) -> u64 {
        *self.generation.borrow()
    }

    /// Advance the generation. Called from every producer of an observable
    /// change and nowhere else, so "what counts as a change" has one answer.
    fn bump(&self) {
        self.generation.send_modify(|g| *g = g.wrapping_add(1));
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
        {
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
        }
        if !flipped.is_empty() {
            self.bump();
        }
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
    ///
    /// Only an id that was actually there bumps the generation: removing
    /// something absent changed nothing, and a waiter woken by it would find
    /// the table exactly as it left it. (Tests call this to clean up the
    /// global table, so the no-op case is common, not hypothetical.)
    pub fn remove(&self, session_id: &str) {
        let existed = self
            .entries
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(session_id)
            .is_some();
        if existed {
            self.bump();
        }
    }
}

/// Exhaustive by construction — no `_ =>` arm. A wildcard here would fan a
/// newly added detection state into whatever the catch-all names, silently
/// (判据 §2); this way the compiler names the file to edit.
///
/// `pub(crate)` because `terminal{explain}` publishes a freshly-evaluated
/// detection state on the same wire the table's states go out on: two
/// spellings of one four-word vocabulary would be two derivations of the same
/// fact, and the one that drifts is the one nobody re-reads (判据 §1/§9).
pub(crate) const fn wire_state(state: AgentState) -> RuntimeAgentState {
    match state {
        AgentState::Idle => RuntimeAgentState::Idle,
        AgentState::Working => RuntimeAgentState::Working,
        AgentState::Blocked => RuntimeAgentState::Blocked,
        AgentState::Unknown => RuntimeAgentState::Unknown,
    }
}

/// The tests live in their own file: `mod.rs` was 1,237 lines, of which 906
/// were tests, and P2 puts the split at 500. `mod tests;` rather than an
/// inline module so the production half stays readable in one screenful.
#[cfg(test)]
mod tests;
