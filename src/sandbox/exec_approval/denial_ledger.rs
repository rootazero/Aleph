//! Session-scoped denial ledger — the **negative twin** of
//! [`session_memory`](super::session_memory).
//!
//! `session_memory` remembers what the user *approved* for the rest of a
//! session so a confirm-gated tool is not re-prompted. This module remembers
//! what the user *denied*, for two reasons that the positive store cannot
//! cover:
//!
//! 1. **Blind-retry guard.** Once a user denies a specific action, an agent
//!    must not be able to re-request the *identical* intent and re-prompt the
//!    user. Re-prompting for something already refused is noise at best and a
//!    fatigue / coercion loop at worst. A denial is sticky for that intent for
//!    the rest of the session — the agent has to change its approach.
//!
//! 2. **Circuit breaker.** After enough distinct denials in one session, the
//!    autonomous escalation path is paused: further elevation prompts
//!    auto-deny without bothering the user. This bounds how hard a runaway or
//!    adversarial loop can push against the approval gate.
//!
//! Maps OpenSquilla's `DenialLedger` (`sandbox/governance.py`) — its
//! `action_fingerprint` (SHA over action+argv+cwd) + per-session counter +
//! sticky `autonomous_paused` flag + `DenialReason` taxonomy — onto Aleph,
//! reusing the same bounded-FIFO, process-wide, session-keyed shape as
//! [`session_memory::SessionApprovalMemory`] so the two stores are structural
//! mirrors and evict identically.

use std::collections::{HashMap, VecDeque};
use std::sync::LazyLock;
use crate::sync_primitives::Mutex;

/// Max distinct sessions retained before FIFO eviction kicks in. Matches
/// [`session_memory`](super::session_memory)'s bound so the two stores have an
/// identical memory ceiling.
const MAX_SESSIONS: usize = 1024;

/// Distinct denied intents in one session before the autonomous escalation
/// path is paused (sticky). Conservative: a paused session only auto-denies
/// *elevation / confirm* prompts — it never blocks already-approved or
/// auto-execute tools — so the cost of tripping it is at most a re-prompt the
/// user can resolve by acting deliberately, never silent data loss.
///
/// Set to 3 to match the product spec ("连续 3 次被拒绝 → AI 自动暂停执行,
/// 防止暴力穷举") and OpenSquilla's `DEFAULT_DENIAL_THRESHOLD = 3` — the
/// circuit breaker should trip the moment a brute-force pattern is
/// unmistakable, not give it two more free attempts. Tightening only (a
/// session that paused at 5 still pauses at 3); no caller hard-codes the
/// value, so the change is internal to the ledger.
const SESSION_PAUSE_THRESHOLD: u32 = 3;

/// Why an action is being auto-denied by the ledger (independent of the live
/// approval gate). Carries an agent-facing hint so the harness can tell the
/// model *why* a retry was refused and what to do instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DenialReason {
    /// The user explicitly rejected this action earlier in the session.
    UserRejected,
    /// The approval request timed out (treated as a soft denial).
    Timeout,
    /// The exact same intent was already denied — a blind retry.
    RepeatedSameIntent,
    /// The session crossed the denial threshold; escalation is paused.
    ThresholdExceeded,
}

impl DenialReason {
    /// Short, model-facing explanation appended to the refusal so the agent
    /// stops re-attempting and changes approach instead of looping.
    pub fn agent_hint(self) -> &'static str {
        match self {
            DenialReason::UserRejected => {
                "The user already declined this exact action this session; do not re-request it — try a different approach or ask the user directly."
            }
            DenialReason::Timeout => {
                "A prior approval request for this exact action timed out; do not silently retry it — surface the blocker to the user."
            }
            DenialReason::RepeatedSameIntent => {
                "This exact intent was denied earlier this session and is now auto-refused. Change the plan rather than repeating the request."
            }
            DenialReason::ThresholdExceeded => {
                "Too many actions were denied this session, so autonomous escalation is paused. Stop and let the user decide how to proceed."
            }
        }
    }
}

/// Per-session denial state: counts keyed by action fingerprint, a running
/// total, and a sticky pause flag once the threshold is crossed.
#[derive(Default)]
struct SessionDenials {
    counts: HashMap<String, u32>,
    total: u32,
    paused: bool,
}

#[derive(Default)]
struct Inner {
    by_session: HashMap<String, SessionDenials>,
    /// FIFO of session keys, for bounded eviction of the oldest session.
    order: VecDeque<String>,
}

/// Process-wide record of denied actions, keyed by session.
pub struct DenialLedger {
    inner: Mutex<Inner>,
}

/// Stable, dependency-free fingerprint grouping "the same denied intent".
///
/// FNV-1a over `tool\x1Fdetail`. A 64-bit hash is ample for intent-grouping
/// (a collision merely makes two distinct intents share a denial bucket — it
/// never grants anything), and avoids pulling a crypto-hash dependency into a
/// hot, purely-internal path. `detail` should be the same deterministic string
/// the approval prompt was built from (e.g. the capability-request text) so
/// the *same* request maps to the *same* fingerprint across attempts.
pub fn action_fingerprint(tool: &str, detail: &str) -> String {
    const FNV_OFFSET: u64 = 0xcbf29ce484222325;
    const FNV_PRIME: u64 = 0x100000001b3;
    let sep = [0x1fu8];
    let mut hash = FNV_OFFSET;
    for &byte in tool
        .as_bytes()
        .iter()
        .chain(sep.iter())
        .chain(detail.as_bytes().iter())
    {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    format!("{hash:016x}")
}

impl DenialLedger {
    fn new() -> Self {
        Self {
            inner: Mutex::new(Inner::default()),
        }
    }

    /// Single pre-prompt check: if this action should be auto-denied *without*
    /// re-prompting the user, return why. `None` means "go ahead and prompt".
    ///
    /// Ordering matters: a tripped session pause outranks a per-intent blind
    /// retry, because the circuit breaker is the stronger, session-wide signal.
    pub fn is_blocked(&self, session: &str, fingerprint: &str) -> Option<DenialReason> {
        let guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let denials = guard.by_session.get(session)?;
        if denials.paused {
            return Some(DenialReason::ThresholdExceeded);
        }
        if denials.counts.contains_key(fingerprint) {
            return Some(DenialReason::RepeatedSameIntent);
        }
        None
    }

    /// Record that `fingerprint` was denied in `session` for `reason`. Bumps
    /// the per-intent count and the session total, tripping the sticky pause
    /// flag once the total reaches [`SESSION_PAUSE_THRESHOLD`].
    ///
    /// Returns `true` iff this call *just* tripped the session pause (the
    /// circuit-breaker flipped false→true on this denial, never on later ones
    /// since the flag is sticky). Callers use that edge to fire one-shot side
    /// effects — e.g. purging the offloaded tool-result cache so a paused,
    /// adversarial session cannot mine previously-cached results
    /// (anti-reference-bypass). The return is advisory: existing callers that
    /// ignore it are unaffected.
    pub fn record_denial(&self, session: &str, fingerprint: &str, reason: DenialReason) -> bool {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if !guard.by_session.contains_key(session) {
            // New session: enforce the bound before inserting so the map and
            // the FIFO order stay in lockstep (mirrors session_memory).
            while guard.order.len() >= MAX_SESSIONS {
                match guard.order.pop_front() {
                    Some(evicted) => {
                        guard.by_session.remove(&evicted);
                    }
                    None => break,
                }
            }
            guard.order.push_back(session.to_string());
        }
        let denials = guard.by_session.entry(session.to_string()).or_default();
        *denials.counts.entry(fingerprint.to_string()).or_insert(0) += 1;
        denials.total += 1;
        // Only the false→true edge counts: a sticky pause must not re-fire the
        // one-shot purge on every subsequent denial in the same session.
        let just_tripped = !denials.paused && denials.total >= SESSION_PAUSE_THRESHOLD;
        if just_tripped {
            denials.paused = true;
        }
        tracing::info!(
            session = %session,
            fingerprint = %fingerprint,
            reason = ?reason,
            session_total = denials.total,
            paused = denials.paused,
            "denial ledger recorded a refused action"
        );
        just_tripped
    }

    /// Number of times `fingerprint` was denied in `session`.
    pub fn denial_count(&self, session: &str, fingerprint: &str) -> u32 {
        let guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        guard
            .by_session
            .get(session)
            .and_then(|d| d.counts.get(fingerprint).copied())
            .unwrap_or(0)
    }

    /// Total denials recorded for `session`.
    pub fn session_total(&self, session: &str) -> u32 {
        let guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        guard.by_session.get(session).map_or(0, |d| d.total)
    }

    /// Forget all denials for `session` (e.g. on session end).
    pub fn clear_session(&self, session: &str) {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if guard.by_session.remove(session).is_some() {
            guard.order.retain(|s| s != session);
        }
    }
}

static GLOBAL: LazyLock<DenialLedger> = LazyLock::new(DenialLedger::new);

/// Process-wide denial ledger shared by the confirm gate and the sandbox
/// elevation path — the negative counterpart to
/// [`session_memory::global`](super::session_memory::global).
pub fn global() -> &'static DenialLedger {
    &GLOBAL
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingerprint_is_stable_and_intent_sensitive() {
        let a = action_fingerprint("bash_exec", "rm -rf /tmp/x");
        let b = action_fingerprint("bash_exec", "rm -rf /tmp/x");
        let c = action_fingerprint("bash_exec", "ls -la");
        assert_eq!(a, b, "same intent → same fingerprint");
        assert_ne!(a, c, "different intent → different fingerprint");
        assert_eq!(a.len(), 16, "16 hex chars (64-bit)");
    }

    #[test]
    fn first_request_is_not_blocked_then_blind_retry_is() {
        let led = DenialLedger::new();
        let fp = action_fingerprint("code_exec", "allow_network");
        assert!(led.is_blocked("s1", &fp).is_none(), "first ask may prompt");
        led.record_denial("s1", &fp, DenialReason::UserRejected);
        assert_eq!(
            led.is_blocked("s1", &fp),
            Some(DenialReason::RepeatedSameIntent),
            "the identical intent now auto-denies"
        );
        assert_eq!(led.denial_count("s1", &fp), 1);
    }

    #[test]
    fn denials_do_not_leak_across_sessions() {
        let led = DenialLedger::new();
        let fp = action_fingerprint("code_exec", "allow_network");
        led.record_denial("s1", &fp, DenialReason::UserRejected);
        // A denial in s1 must never auto-deny s2 — the isolation invariant.
        assert!(led.is_blocked("s2", &fp).is_none());
    }

    #[test]
    fn distinct_intents_do_not_block_each_other() {
        let led = DenialLedger::new();
        let denied = action_fingerprint("bash_exec", "rm -rf /");
        let fresh = action_fingerprint("bash_exec", "echo hi");
        led.record_denial("s1", &denied, DenialReason::UserRejected);
        assert!(led.is_blocked("s1", &denied).is_some());
        assert!(
            led.is_blocked("s1", &fresh).is_none(),
            "a different intent under the same tool may still prompt"
        );
    }

    #[test]
    fn pause_threshold_matches_spec_of_three() {
        // Product spec + OpenSquilla parity: brute-force pause trips at 3.
        assert_eq!(SESSION_PAUSE_THRESHOLD, 3);
    }

    #[test]
    fn threshold_trips_sticky_session_pause() {
        let led = DenialLedger::new();
        // `SESSION_PAUSE_THRESHOLD` distinct denied intents trip the pause.
        for i in 0..SESSION_PAUSE_THRESHOLD {
            let fp = action_fingerprint("code_exec", &format!("intent-{i}"));
            led.record_denial("s1", &fp, DenialReason::UserRejected);
        }
        assert_eq!(led.session_total("s1"), SESSION_PAUSE_THRESHOLD);
        // Even a brand-new, never-seen intent is now auto-denied by the pause.
        let fresh = action_fingerprint("code_exec", "totally-new-intent");
        assert_eq!(
            led.is_blocked("s1", &fresh),
            Some(DenialReason::ThresholdExceeded),
            "pause outranks per-intent state"
        );
    }

    #[test]
    fn record_denial_signals_pause_trip_exactly_once() {
        let led = DenialLedger::new();
        // The denials below the threshold must not signal a trip.
        for i in 0..(SESSION_PAUSE_THRESHOLD - 1) {
            let fp = action_fingerprint("code_exec", &format!("intent-{i}"));
            assert!(
                !led.record_denial("s1", &fp, DenialReason::UserRejected),
                "denial {i} must not trip the pause yet"
            );
        }
        // The threshold-th distinct denial trips the circuit-breaker — once.
        let trip_fp = action_fingerprint("code_exec", "intent-trip");
        assert!(
            led.record_denial("s1", &trip_fp, DenialReason::UserRejected),
            "the threshold denial must signal the false→true pause trip"
        );
        // A sticky pause must not re-signal on later denials (one-shot edge).
        let after_fp = action_fingerprint("code_exec", "intent-after");
        assert!(
            !led.record_denial("s1", &after_fp, DenialReason::UserRejected),
            "an already-paused session must not re-fire the trip signal"
        );
    }

    #[test]
    fn clear_session_forgets_denials() {
        let led = DenialLedger::new();
        let fp = action_fingerprint("code_exec", "allow_network");
        led.record_denial("s1", &fp, DenialReason::UserRejected);
        led.clear_session("s1");
        assert!(led.is_blocked("s1", &fp).is_none());
        assert_eq!(led.session_total("s1"), 0);
    }

    #[test]
    fn bounded_eviction_drops_oldest_session() {
        let led = DenialLedger::new();
        let fp = action_fingerprint("t", "x");
        for i in 0..MAX_SESSIONS {
            led.record_denial(&format!("s{i}"), &fp, DenialReason::UserRejected);
        }
        assert!(led.is_blocked("s0", &fp).is_some());
        led.record_denial("overflow", &fp, DenialReason::UserRejected);
        // Oldest (s0) evicted; newest retained.
        assert!(led.is_blocked("s0", &fp).is_none());
        assert!(led.is_blocked("overflow", &fp).is_some());
        let guard = led.inner.lock().unwrap_or_else(|e| e.into_inner());
        assert_eq!(guard.order.len(), MAX_SESSIONS);
        assert_eq!(guard.by_session.len(), MAX_SESSIONS);
    }

    #[test]
    fn agent_hints_are_non_empty() {
        for r in [
            DenialReason::UserRejected,
            DenialReason::Timeout,
            DenialReason::RepeatedSameIntent,
            DenialReason::ThresholdExceeded,
        ] {
            assert!(!r.agent_hint().is_empty());
        }
    }
}
