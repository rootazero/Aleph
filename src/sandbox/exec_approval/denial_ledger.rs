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
//! Maps `OpenSquilla`'s `DenialLedger` (`sandbox/governance.py`) — its
//! `action_fingerprint` (SHA over action+argv+cwd) + per-session counter +
//! sticky `autonomous_paused` flag + `DenialReason` taxonomy — onto Aleph,
//! reusing the same bounded-FIFO, process-wide, session-keyed shape as
//! [`session_memory::SessionApprovalMemory`] so the two stores are structural
//! mirrors and evict identically.

use crate::routing::session_key::SessionKey;
use crate::sync_primitives::Mutex;
use std::collections::{HashMap, VecDeque};
use std::sync::LazyLock;

/// The one way to address a session in this ledger — and in its positive twin,
/// [`session_memory`](super::session_memory), which must share the bucket.
///
/// Two gates write here: the tool confirm gate
/// (`ScopedToolService::confirm_with_memory`) and the sandbox capability
/// elevation path (`sandbox::workspace`). They must derive the key identically,
/// or the same session addresses two disjoint buckets of one global map — and
/// that is exactly what happened. The sandbox path keyed the ledger with
/// `session_key_to_filename`, the SHA-256 *filename* encoder borrowed from the
/// workspace directory layout, while the confirm gate used the plain
/// [`SessionKey`] string. The two strings never collide, so:
///
/// * a refusal at one gate was invisible to the other, and
/// * the "3 denials pause the session" circuit breaker counted each path
///   separately — it was really 3 per *path*, not 3 per session, which is twice
///   the brute-force headroom the threshold was chosen to allow.
///
/// Deriving the key here, once, is what makes that class of drift impossible
/// rather than merely fixed. A filename encoder is not an identity.
#[must_use]
pub fn ledger_key(session: &SessionKey) -> String {
    session.to_string()
}

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
/// Set to 3 to match the product spec ("3 consecutive denials → AI auto-pauses
/// execution, preventing brute-force guessing") and `OpenSquilla`'s `DEFAULT_DENIAL_THRESHOLD = 3` — the
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
    /// The approval request expired with no answer. Refuses the call *this*
    /// turn, but is never written to the ledger — see
    /// [`DenialLedger::record_denial`] for why a non-answer must not harden
    /// into a refusal the user never made.
    Timeout,
    /// The exact same intent was already denied — a blind retry.
    RepeatedSameIntent,
    /// The session crossed the denial threshold; escalation is paused.
    ThresholdExceeded,
}

impl DenialReason {
    /// Short, model-facing explanation appended to the refusal so the agent
    /// stops re-attempting and changes approach instead of looping.
    #[must_use]
    pub const fn agent_hint(self) -> &'static str {
        match self {
            Self::UserRejected => {
                "The user already declined this exact action this session; do not re-request it — try a different approach or ask the user directly."
            }
            Self::Timeout => {
                "The approval request for this action expired with no answer — nobody appears to be at the keyboard. Do not silently retry it; surface the blocker to the user and stop."
            }
            Self::RepeatedSameIntent => {
                "This exact intent was denied earlier this session and is now auto-refused. Change the plan rather than repeating the request."
            }
            Self::ThresholdExceeded => {
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

/// Stable fingerprint grouping "the same intent" — for the denial ledger AND,
/// via [`grant_fingerprint`](super::action::grant_fingerprint), the positive
/// session-grant store.
///
/// Truncated SHA-256 over `tool\x1Fdetail`. It shares the store with the grant
/// path now, so a collision is no longer merely "two intents share a denial
/// bucket": it would let an "approve for session" on one action authorize a
/// *different* action that happens to collide — a privilege escalation. A
/// second-preimage-resistant hash removes that class of risk; `sha2` + `hex`
/// are already direct workspace deps (see `sandbox/workspace/path.rs`), so the
/// old "avoid a crypto-hash dependency" tradeoff no longer applies. 128 bits is
/// ample against both accidental and adversarial collision. The `0x1F`
/// separator keeps `tool` and `detail` unambiguous.
#[must_use]
pub fn action_fingerprint(tool: &str, detail: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(tool.as_bytes());
    hasher.update([0x1fu8]);
    hasher.update(detail.as_bytes());
    hex::encode(&hasher.finalize()[..16])
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
    ///
    /// # A timeout is not a denial
    ///
    /// [`DenialReason::Timeout`] is deliberately **not recorded**. Both stores
    /// this ledger keeps answer questions about a *decision the user made*:
    /// `counts` is "what did the user refuse?" (sticky, so the agent cannot
    /// re-prompt its way around a `no`), and `total` is "how hard is this
    /// session pushing against the gate?" (whose trip purges the offloaded
    /// tool-result cache as an **anti-adversarial** countermeasure). An expired
    /// approval answers neither: it is the *absence* of a decision.
    ///
    /// Recording it anyway produced two traps. A user who stepped away from the
    /// desk for one gated call came back to find that intent auto-refused for
    /// the rest of the session and never asked again — they reasonably read
    /// that as "it stopped asking me". Three such lapses tripped the breaker
    /// and **destroyed the session's cached tool output**, punishing inattention
    /// with the countermeasure built for an attacker.
    ///
    /// Nothing is lost by dropping it: an unanswered prompt grants the agent no
    /// capability, so there is no brute-force avenue to bound here, and an agent
    /// looping against an absent user is already bounded by the approval timeout
    /// itself plus the run-level turn timeout. The refusal still reaches the
    /// model this turn, carrying [`DenialReason::Timeout::agent_hint`], which
    /// tells it to surface the blocker rather than retry.
    ///
    /// The rule lives here, at the ledger, rather than at the two call sites
    /// (the confirm gate and the sandbox elevation path) so a third caller
    /// cannot reintroduce it by forgetting.
    ///
    /// [`DenialReason::Timeout::agent_hint`]: DenialReason::agent_hint
    pub fn record_denial(&self, session: &str, fingerprint: &str, reason: DenialReason) -> bool {
        if matches!(reason, DenialReason::Timeout) {
            tracing::info!(
                session = %session,
                fingerprint = %fingerprint,
                "approval expired with no answer — not recorded as a denial \
                 (a timeout is not a decision); the same intent stays askable"
            );
            return false;
        }
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

    /// Number of times `fingerprint` was denied in `session`. Test-only
    /// introspection: production reaches its decisions through
    /// [`is_blocked`](Self::is_blocked), never a raw count, so this accessor is
    /// gated out of the public API rather than implying a consumer that does not
    /// exist.
    #[cfg(test)]
    fn denial_count(&self, session: &str, fingerprint: &str) -> u32 {
        let guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        guard
            .by_session
            .get(session)
            .and_then(|d| d.counts.get(fingerprint).copied())
            .unwrap_or(0)
    }

    /// Total denials recorded for `session`. Test-only introspection: the
    /// breaker-trip path reads `denials.total` inline, not through this accessor.
    #[cfg(test)]
    fn session_total(&self, session: &str) -> u32 {
        let guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        guard.by_session.get(session).map_or(0, |d| d.total)
    }
}

static GLOBAL: LazyLock<DenialLedger> = LazyLock::new(DenialLedger::new);

/// Process-wide denial ledger shared by the confirm gate and the sandbox
/// elevation path — the negative counterpart to
/// [`session_memory::global`](super::session_memory::global).
#[must_use]
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
        assert_eq!(a.len(), 32, "128-bit truncated SHA-256, hex-encoded");
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

    /// The user walked away; the card expired. That is not a `no`. When they
    /// come back and ask for the same thing, they must be *asked*, not silently
    /// refused — the "it stopped asking me" trap.
    #[test]
    fn a_timed_out_approval_never_hardens_into_a_refusal() {
        let led = DenialLedger::new();
        let fp = action_fingerprint("bash_exec", "echo hi");

        led.record_denial("s1", &fp, DenialReason::Timeout);

        assert_eq!(
            led.is_blocked("s1", &fp),
            None,
            "an unanswered approval must leave the intent askable"
        );
        assert_eq!(led.denial_count("s1", &fp), 0);
    }

    /// The breaker's trip purges the offloaded tool-result cache as an
    /// anti-adversarial measure. Inattention is not an attack: no number of
    /// expired approvals may fire it.
    #[test]
    fn timeouts_never_trip_the_circuit_breaker() {
        let led = DenialLedger::new();
        for i in 0..(SESSION_PAUSE_THRESHOLD * 3) {
            let fp = action_fingerprint("code_exec", &format!("intent-{i}"));
            assert!(
                !led.record_denial("s1", &fp, DenialReason::Timeout),
                "a timeout must never report a breaker trip"
            );
        }
        assert_eq!(led.session_total("s1"), 0);
        let fresh = action_fingerprint("code_exec", "anything");
        assert_eq!(
            led.is_blocked("s1", &fresh),
            None,
            "walking away must not pause the session"
        );
    }

    /// The other half of the contract: an explicit `no` is still sticky, and a
    /// prior timeout on the same intent does not soften it.
    #[test]
    fn an_explicit_rejection_is_still_sticky_after_a_timeout() {
        let led = DenialLedger::new();
        let fp = action_fingerprint("bash_exec", "rm -rf /");

        led.record_denial("s1", &fp, DenialReason::Timeout);
        led.record_denial("s1", &fp, DenialReason::UserRejected);

        assert_eq!(
            led.is_blocked("s1", &fp),
            Some(DenialReason::RepeatedSameIntent),
            "a decided refusal must still block the blind retry"
        );
        assert_eq!(led.session_total("s1"), 1, "only the decision was counted");
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

    /// REGRESSION: the sandbox capability-elevation path keyed this ledger with
    /// `session_key_to_filename` — a SHA-256 *filename* encoder lifted from the
    /// workspace directory layout — while the tool confirm gate used the plain
    /// `SessionKey` string. Same session, two strings that never collide, so one
    /// global map held two disjoint ledgers: neither gate could see the other's
    /// refusals, and the "3 denials pause the session" breaker was really 3 per
    /// *path*. [`ledger_key`] is now the only derivation.
    #[test]
    fn cross_gate_denials_share_one_bucket_per_session() {
        let sk = SessionKey::Main {
            agent_id: "a1".into(),
            main_key: "main".into(),
            epoch: 0,
        };
        let led = DenialLedger::new();
        let fp = action_fingerprint("bash", "sudo rm -rf /");

        // The sandbox elevation gate refuses it...
        led.record_denial(&ledger_key(&sk), &fp, DenialReason::UserRejected);

        // ...and the tool confirm gate, keying the SAME session, must see it.
        assert_eq!(
            led.is_blocked(&ledger_key(&sk), &fp),
            Some(DenialReason::RepeatedSameIntent),
            "a refusal at one gate must be visible at the other"
        );

        // Proof the old key really did address a different bucket — this is the
        // bug, pinned: had either gate kept the filename encoding, the lookup
        // above would have missed and the user would have been re-prompted for
        // something they already refused.
        let filename_key = crate::sandbox::workspace::session_key_to_filename(&sk);
        assert_ne!(
            ledger_key(&sk),
            filename_key,
            "the filename encoding is a different string — that was the whole bug"
        );
        assert_eq!(
            led.is_blocked(&filename_key, &fp),
            None,
            "the filename-keyed bucket is empty: the two gates never met"
        );
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
