//! Recent-command ledger for the shell tools — advisory repeat detection.
//!
//! The behaviour this addresses: the model sometimes re-runs a byte-identical
//! shell command back-to-back in the same session (a stale `ls`, a `git
//! status` loop, a `cargo check` it just ran). Because bash sessions are
//! stateless — each call is a fresh process — the model can't "feel" that
//! nothing changed, so it burns a turn re-issuing the same command.
//!
//! This registry records the (canonicalized) command string per session with a
//! submission counter + timestamp, and on a recent repeat returns a small
//! [`RepeatAdvisory`] the tool attaches to the result's `advisory` field. It
//! **never suppresses execution** — the command always runs, because whether a
//! re-run is wasteful is the model's judgement, not ours (R7). The check is
//! pure mechanical string equality (no intent parsing, no semantic
//! normalization beyond peeling a redundant `bash -lc` wrapper via
//! [`canonicalize_shell_cmd`](super::command_canonicalize::canonicalize_shell_cmd)),
//! lives entirely in the tool-presentation layer (R10 budget: zero growth in
//! `src/harness/`), and only surfaces a fact for the model to act on.
//!
//! Isolation & bounds mirror [`process_registry`](super::process_registry):
//! every record is keyed by the caller's session label, and both the session
//! table and each session's command map are size-capped with least-recently-
//! used eviction so a long-lived daemon never grows the ledger without bound.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use once_cell::sync::Lazy;

use crate::sync_primitives::{Arc, Mutex, MutexGuard};

/// Only surface a repeat advisory when the identical command last ran within
/// this window. A re-run several minutes later is normal workflow (returning to
/// a task, re-checking after unrelated work) rather than a wasteful tight loop;
/// nudging on it would be noise.
const ADVISORY_WINDOW: Duration = Duration::from_secs(300);

/// Drop a session's log once it has been untouched for this long, so a
/// long-lived daemon doesn't accumulate per-session maps forever.
const SESSION_TTL: Duration = Duration::from_secs(3600);

/// Distinct commands remembered per session before least-recently-seen
/// eviction kicks in. Bounds memory for a session that runs a huge variety of
/// one-off commands while still comfortably covering any realistic re-run loop.
const MAX_COMMANDS_PER_SESSION: usize = 128;

/// Sessions tracked at once before least-recently-touched eviction. Mirrors
/// `process_registry`'s bounded-table discipline.
const MAX_SESSIONS: usize = 128;

/// Per-command bookkeeping within one session.
struct CmdRecord {
    last_seen: Instant,
    /// How many times this exact command has been submitted in this session,
    /// including the current submission.
    count: u32,
}

/// One session's command history plus a touch timestamp for session eviction.
struct SessionLog {
    entries: HashMap<String, CmdRecord>,
    last_touch: Instant,
}

impl SessionLog {
    fn new(now: Instant) -> Self {
        Self {
            entries: HashMap::new(),
            last_touch: now,
        }
    }
}

/// A detected re-run of a byte-identical command within the recency window.
pub struct RepeatAdvisory {
    /// Whole seconds since the previous identical submission.
    pub seconds_since_last: u64,
    /// Which submission this is (2 = first repeat, 3 = second repeat, …).
    pub occurrence: u32,
}

impl RepeatAdvisory {
    /// Model-facing note attached to the tool result's `advisory` field.
    /// Surfaces the fact and a gentle nudge, then explicitly defers to the
    /// model's judgement for intentional re-runs (R7 — advisory, not a gate).
    #[must_use]
    pub fn message(&self) -> String {
        format!(
            "You already ran this exact command {}s ago in this session (submission #{}); it was \
             re-executed and its output is below. Re-running an identical command rarely returns \
             new output — if you are waiting for something to change (a build finishing, a file \
             appearing), prefer BACKGROUND MODE's `wait`/`poll` over re-issuing it, or vary the \
             command. If the re-run was intentional (a write/append, or re-checking after an \
             edit), ignore this note.",
            self.seconds_since_last, self.occurrence
        )
    }
}

/// Process-global table of recent shell commands, keyed by session label.
pub struct CommandLedger {
    sessions: Mutex<HashMap<String, SessionLog>>,
}

impl CommandLedger {
    fn new() -> Self {
        Self {
            sessions: Mutex::new(HashMap::new()),
        }
    }

    fn lock(&self) -> MutexGuard<'_, HashMap<String, SessionLog>> {
        // P7: never propagate lock poisoning — a panicked caller must not wedge
        // the whole ledger for every later shell call.
        self.sessions.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Record a command submission for `session` and report whether it repeats
    /// a recent identical one. Always updates the record — the command is
    /// executed regardless of the return value; this is advisory only (R7).
    pub fn record(&self, session: &str, command: &str) -> Option<RepeatAdvisory> {
        let now = Instant::now();
        let mut sessions = self.lock();
        prune_stale_sessions(&mut sessions, now);
        // Only evict to make room when inserting a genuinely new session, so we
        // never drop the caller's own (freshly-touched) log.
        if !sessions.contains_key(session) {
            evict_sessions_to_fit(&mut sessions);
        }

        let log = sessions
            .entry(session.to_string())
            .or_insert_with(|| SessionLog::new(now));
        log.last_touch = now;

        match log.entries.get_mut(command) {
            Some(rec) => {
                let since = now.saturating_duration_since(rec.last_seen);
                rec.count = rec.count.saturating_add(1);
                rec.last_seen = now;
                let occurrence = rec.count;
                (since <= ADVISORY_WINDOW).then_some(RepeatAdvisory {
                    seconds_since_last: since.as_secs(),
                    occurrence,
                })
            }
            None => {
                evict_commands_to_fit(&mut log.entries);
                log.entries.insert(
                    command.to_string(),
                    CmdRecord {
                        last_seen: now,
                        count: 1,
                    },
                );
                None
            }
        }
    }
}

/// Drop sessions untouched for longer than [`SESSION_TTL`].
fn prune_stale_sessions(sessions: &mut HashMap<String, SessionLog>, now: Instant) {
    sessions.retain(|_, log| now.saturating_duration_since(log.last_touch) <= SESSION_TTL);
}

/// Evict least-recently-touched sessions until there is room for one more.
fn evict_sessions_to_fit(sessions: &mut HashMap<String, SessionLog>) {
    while sessions.len() >= MAX_SESSIONS {
        let Some(victim) = sessions
            .iter()
            .min_by_key(|(_, log)| log.last_touch)
            .map(|(k, _)| k.clone())
        else {
            break;
        };
        sessions.remove(&victim);
    }
}

/// Evict least-recently-seen commands until there is room for one more.
fn evict_commands_to_fit(entries: &mut HashMap<String, CmdRecord>) {
    while entries.len() >= MAX_COMMANDS_PER_SESSION {
        let Some(victim) = entries
            .iter()
            .min_by_key(|(_, rec)| rec.last_seen)
            .map(|(k, _)| k.clone())
        else {
            break;
        };
        entries.remove(&victim);
    }
}

/// Process-global ledger shared by every shell tool instance.
static LEDGER: Lazy<Arc<CommandLedger>> = Lazy::new(|| Arc::new(CommandLedger::new()));

/// Handle to the shared recent-command ledger.
pub fn command_ledger() -> Arc<CommandLedger> {
    LEDGER.clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_submission_is_not_flagged() {
        let ledger = CommandLedger::new();
        assert!(ledger.record("s1", "ls -la").is_none());
    }

    #[test]
    fn immediate_repeat_is_flagged_with_occurrence() {
        let ledger = CommandLedger::new();
        assert!(ledger.record("s1", "git status").is_none());
        let a = ledger.record("s1", "git status").expect("repeat flagged");
        assert_eq!(a.occurrence, 2, "second submission is occurrence #2");
        assert!(a.seconds_since_last < 5, "just-now repeat");
        let b = ledger.record("s1", "git status").expect("still flagged");
        assert_eq!(b.occurrence, 3, "occurrence keeps climbing");
    }

    #[test]
    fn distinct_commands_are_independent() {
        let ledger = CommandLedger::new();
        assert!(ledger.record("s1", "ls").is_none());
        assert!(
            ledger.record("s1", "pwd").is_none(),
            "a different command is not a repeat"
        );
    }

    #[test]
    fn sessions_are_isolated() {
        let ledger = CommandLedger::new();
        assert!(ledger.record("owner", "make").is_none());
        assert!(
            ledger.record("intruder", "make").is_none(),
            "another session running the same command is not a repeat"
        );
    }

    #[test]
    fn per_session_command_cap_evicts_oldest_but_bounds_table() {
        let ledger = CommandLedger::new();
        // Overflow one session's command map well past the cap.
        for i in 0..(MAX_COMMANDS_PER_SESSION + 50) {
            assert!(ledger.record("s", &format!("cmd-{i}")).is_none());
        }
        let sessions = ledger.lock();
        let log = sessions.get("s").expect("session present");
        assert!(
            log.entries.len() <= MAX_COMMANDS_PER_SESSION,
            "command map stays bounded (len {})",
            log.entries.len()
        );
    }

    #[test]
    fn session_table_stays_bounded() {
        let ledger = CommandLedger::new();
        for i in 0..(MAX_SESSIONS + 50) {
            assert!(ledger.record(&format!("sess-{i}"), "ls").is_none());
        }
        let sessions = ledger.lock();
        assert!(
            sessions.len() <= MAX_SESSIONS,
            "session table stays bounded (len {})",
            sessions.len()
        );
    }
}
