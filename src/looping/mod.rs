//! Loop subsystem: a per-session, timer-driven repeat managed by the LLM via
//! the `loop` tool (R8) and re-fired each turn by the execution engine's
//! continuation hook. The clock-gated sibling of `goal` (condition-gated).
//!
//! Storage is PROCESS MEMORY ONLY — never `tasks.db`. A daemon restart clears
//! every loop, which is the physical guarantee of the "dies with the session" semantics.

pub mod pursuit;
pub mod types;

pub use types::{Cadence, LoopState, LoopStatus};

use crate::sync_primitives::Arc;
use once_cell::sync::OnceCell;
use std::collections::HashMap;
use std::sync::Mutex;

/// How long past its scheduled wake an in-flight tick may stay unconfirmed
/// before the registry treats it as dead and lets a new tick be claimed. A
/// live tick clears its pending marker within milliseconds of waking; only a
/// panicked/aborted task leaves it behind, and without this grace the loop
/// would stall forever as an `Active` that never fires again.
const PENDING_TICK_STALE_GRACE_MS: u64 = 60_000;

/// Retry delay when a woken tick found the agent's run slot held by another
/// run (AgentBusy). Short enough that a watch loop resumes promptly once the
/// slot frees, long enough not to hammer a long-running collision. Must stay
/// below `PENDING_TICK_STALE_GRACE_MS` so a re-armed tick is never treated as
/// stale before it wakes.
const BUSY_RETRY_DELAY_MS: u64 = 30_000;

/// Outcome of [`LoopRegistry::try_claim_tick`] — the single atomic decision
/// the continuation hook acts on after a run completes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TickDecision {
    /// A tick was claimed: spawn it with `delay_ms`; at fire time it must
    /// [`LoopRegistry::confirm_fire`] with `wake_ms` before executing.
    Fire {
        delay_ms: u64,
        wake_ms: u64,
        prompt: String,
    },
    /// A safety cap tripped: the loop was stopped and `note` stored as its
    /// stop reason (returned so the caller can log/notify).
    Exhausted { note: String },
    /// Nothing to do: no active loop, or a tick is already in flight.
    Idle,
}

/// Outcome of [`LoopRegistry::rearm_after_busy`] — the loop sibling of
/// `goal::RearmDecision` (parallel, not shared, like `commit_field_update`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RearmDecision {
    /// Re-spawn the same tick after `delay_ms`, confirming `wake_ms`.
    Retry { delay_ms: u64, wake_ms: u64 },
    /// A cap tripped while the collision played out: the loop was stopped with
    /// `note` (returned so the caller clears the welded strategy and notifies
    /// the origin channel — R5), mirroring `stop_loop_on_failure`.
    Exhausted { note: String },
    /// Loop gone, stopped, or already re-claimed — drop this tick.
    Drop,
}

/// Outcome of [`LoopRegistry::transition`] — the single atomic lifecycle move
/// (`pause` / `resume` / `stop`) every caller goes through, so the legality
/// matrix and the pending-marker bookkeeping live in exactly one place.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransitionOutcome {
    /// The loop moved to the requested status; `from` is what it was.
    Applied { from: LoopStatus },
    /// The loop exists but the move is illegal or a no-op (already there, or
    /// resuming a terminal `Stopped`). `current` is what it actually is, so the
    /// caller can report the truth instead of a generic failure.
    Refused { current: LoopStatus },
    /// No loop registered for this session at all.
    Missing,
}

/// In-memory map of `session_key` → `LoopState`.
#[derive(Default)]
pub struct LoopRegistry {
    inner: Mutex<HashMap<String, LoopState>>,
}

impl LoopRegistry {
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, LoopState>> {
        // Poison-safe (project rule P7): recover the guard rather than panic.
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Insert or overwrite the loop for a session.
    pub fn put(&self, state: LoopState) {
        self.lock().insert(state.session_id.clone(), state);
    }

    /// Read the loop for a session regardless of status.
    #[must_use]
    pub fn get(&self, session_id: &str) -> Option<LoopState> {
        self.lock().get(session_id).cloned()
    }

    /// Read the loop only if it is `Active`.
    #[must_use]
    pub fn get_active(&self, session_id: &str) -> Option<LoopState> {
        self.lock()
            .get(session_id)
            .filter(|l| l.is_active())
            .cloned()
    }

    /// Snapshot every loop across ALL sessions, any status. Backs
    /// `loop(action='list')`: a loop started on one channel is invisible to
    /// `status`, which keys by the current session — the same cross-session gap
    /// `goal(action='list')` closes for goals (R6 one core, many channels / R8 chat IS the admin panel).
    /// The registry keeps at most one entry per session (`start` overwrites), so
    /// this is small — one row per session that started a loop this daemon
    /// lifetime. Process memory only, so it is exactly the loops alive now (no
    /// orphan rows to reconcile, matching the "dies with the session" contract). Ordering is
    /// the caller's to impose.
    #[must_use]
    pub fn list_all(&self) -> Vec<LoopState> {
        self.lock().values().cloned().collect()
    }

    /// The one atomic lifecycle move: `pause`, `resume` and every flavour of
    /// `stop` (tool, cap, failure, session retirement, agent deletion) go
    /// through here instead of the `get` → mutate → `put` shape they each used
    /// to spell out. Two things that used to be re-derived per call site are now
    /// invariants of the type:
    ///
    /// 1. **Legality.** `Stopped` is terminal — only `start` (a fresh `put`)
    ///    leaves it, so resuming one is `Refused`, not a silent resurrection.
    ///    `Paused` is reachable only from `Active`, `Active` only from `Paused`.
    ///    A move to the status a loop already has is `Refused` too, so callers
    ///    can report "already stopped" honestly.
    /// 2. **Pending-marker hygiene.** Leaving `Active` clears
    ///    `pending_tick_wake_ms`, which is what makes pause/resume actually
    ///    responsive: the sleeping tick's `confirm_fire` then mismatches (`None`
    ///    ≠ `Some(wake)`) and skips, and `resume` starts from a clean slate. Had
    ///    the marker survived, the next `try_claim_tick` would return `Idle`
    ///    until the 60 s stale grace elapsed *past the original wake* — for a
    ///    loop paused early in a 1 h cadence that is a ~56-minute dead zone
    ///    after resume.
    ///
    /// `reason` is stored as the loop's `stop_reason` (the "why it is not
    /// ticking" note surfaced by `status`/`list`) for the quiet states, and
    /// cleared on a resume — a running loop has no stop reason.
    pub fn transition(
        &self,
        session_id: &str,
        to: LoopStatus,
        reason: Option<String>,
    ) -> TransitionOutcome {
        let mut map = self.lock();
        let Some(live) = map.get(session_id) else {
            return TransitionOutcome::Missing;
        };
        let from = live.status;
        let legal = match (from, to) {
            (LoopStatus::Active, LoopStatus::Paused | LoopStatus::Stopped)
            | (LoopStatus::Paused, LoopStatus::Active | LoopStatus::Stopped) => true,
            // Stopped is terminal, and same→same is a no-op worth reporting.
            _ => false,
        };
        if !legal {
            return TransitionOutcome::Refused { current: from };
        }
        // Leaving Active retires a tick that is still asleep — it never ran, so
        // give its claim back (see `refund_iteration`). A tick already
        // executing has cleared the marker itself, so nothing is refunded then.
        let retires_unrun_tick = from == LoopStatus::Active && live.pending_tick_wake_ms.is_some();
        let base = if retires_unrun_tick {
            live.clone().refund_iteration()
        } else {
            live.clone()
        };
        let next = base
            .with_status(to)
            .with_stop_reason(if to == LoopStatus::Active {
                None
            } else {
                reason
            })
            // Leaving Active retires the in-flight tick; entering it starts with
            // no tick claimed (the resuming turn's completion hook claims one).
            .with_pending_tick(None);
        map.insert(session_id.to_string(), next);
        TransitionOutcome::Applied { from }
    }

    /// Kill switch: stop every loop that is still running or merely paused,
    /// across ALL sessions, under ONE lock guard. Returns the session keys
    /// actually stopped (already-stopped loops are left untouched and omitted),
    /// so the caller can clear each one's welded strategy and report a truthful
    /// count.
    ///
    /// The bulk sibling of [`Self::transition`] and the incident-response
    /// counterpart to `list_all`: before this, a loop started on one channel was
    /// *visible* from anywhere (`loop(action='list')`) but only stoppable from
    /// its own session, so "stop everything" was not expressible in
    /// conversation at all (R6 one core many channels / R8 chat IS the admin
    /// panel).
    pub fn stop_all(&self, reason: &str) -> Vec<String> {
        let mut map = self.lock();
        let targets: Vec<String> = map
            .values()
            .filter(|l| l.status != LoopStatus::Stopped)
            .map(|l| l.session_id.clone())
            .collect();
        for session in &targets {
            if let Some(live) = map.get(session) {
                // Leaving Active retires a tick that is still asleep — it
                // never ran, so give its claim back (see `refund_iteration`).
                // A tick already executing has cleared the marker itself,
                // so nothing is refunded then. Without this, a kill-switch
                // hit on a 1-cap loop could leave a Stopped row with
                // `iterations_used: 1/1` having executed nothing — exactly
                // the bug `transition` already defends against.
                let retires_unrun_tick =
                    live.status == LoopStatus::Active && live.pending_tick_wake_ms.is_some();
                let base = if retires_unrun_tick {
                    live.clone().refund_iteration()
                } else {
                    live.clone()
                };
                let next = base
                    .with_status(LoopStatus::Stopped)
                    .with_stop_reason(Some(reason.to_string()))
                    .with_pending_tick(None);
                map.insert(session.clone(), next);
            }
        }
        targets
    }

    /// Deactivation freeze (spec §10): pause every loop OWNED BY `user_id`
    /// that is still running or merely paused (mirrors [`Self::stop_all`]'s
    /// scan), going through [`Self::transition`] — THE single atomic
    /// lifecycle move (CLAUDE.md A4) — instead of hand-rolling a second one.
    /// Returns the count actually paused (a loop already `Paused` reports
    /// `Refused` from `transition` and is not counted, mirroring `stop_all`'s
    /// "already-stopped loops are left untouched and omitted").
    ///
    /// The predicate is exact equality against `Some(user_id)` — a legacy
    /// loop with `owner_user_id: None` belongs to the platform owner (spec
    /// §10: the owner account can never be deactivated), never to the user
    /// being deactivated here. One-way freeze: reactivating the user does not
    /// auto-resume its loops (spec is silent on auto-resume).
    pub fn pause_all_owned_by(&self, user_id: &str) -> usize {
        let reason = format!("Account '{user_id}' was deactivated — pursuit frozen.");
        let mut map = self.lock();
        // Snapshot + transition under a single lock guard, matching
        // `stop_all`'s atomic shape: no release between the scan and the
        // per-row transitions, so a concurrent `start` cannot insert a new
        // loop belonging to the same user that this snapshot silently misses.
        let targets: Vec<String> = map
            .values()
            .filter(|l| {
                l.status != LoopStatus::Stopped && l.owner_user_id.as_deref() == Some(user_id)
            })
            .map(|l| l.session_id.clone())
            .collect();
        let mut applied = 0usize;
        for session_id in &targets {
            let Some(live) = map.get(session_id) else {
                continue;
            };
            let from = live.status;
            // Pause is legal from Active or Paused (no-op counted below).
            let legal = matches!(from, LoopStatus::Active | LoopStatus::Paused);
            if !legal {
                continue;
            }
            let next = live
                .clone()
                .with_status(LoopStatus::Paused)
                .with_stop_reason(Some(reason.clone()))
                .with_pending_tick(None);
            map.insert(session_id.clone(), next);
            if from == LoopStatus::Active {
                applied += 1;
            }
        }
        applied
    }

    /// The continuation hook's whole post-run decision, under ONE lock guard:
    /// seed the token baseline, gate on an in-flight tick, then either claim
    /// the next tick (bump the counter, stamp the pending wake) or stop an
    /// exhausted loop with its reason. Read-modify-write races that plagued
    /// the get→await→put shape (a hook `put` resurrecting a loop the user
    /// stopped mid-await, or two completing runs each enqueuing a tick) are
    /// impossible here. `tokens_total` is the session's live cumulative token
    /// count (`None` → unavailable, budget unenforced this tick).
    #[must_use]
    pub fn try_claim_tick(
        &self,
        session_id: &str,
        tokens_total: Option<u64>,
        now_ms: u64,
    ) -> TickDecision {
        let mut map = self.lock();
        let Some(current) = map.get(session_id) else {
            return TickDecision::Idle;
        };
        if !current.is_active() {
            return TickDecision::Idle;
        }
        // Lazy token-baseline capture on the first claim that sees a budget:
        // just-captured → 0 spent, never a false over-budget.
        let state = match (
            current.token_budget,
            current.baseline_captured,
            tokens_total,
        ) {
            (Some(_), false, Some(total)) => current.clone().with_baseline(total),
            _ => current.clone(),
        };
        // Budget enforcement needs the live total; without one (or without a
        // budget at all) pass 0 so only the iteration/deadline caps apply.
        let tokens_now = if state.token_budget.is_some() {
            tokens_total.unwrap_or(0)
        } else {
            0
        };
        // A tick already in flight blocks another claim — this is the fan-out
        // gate. Past the stale grace the enqueued task is presumed dead and
        // the claim proceeds (its own confirm_fire will then mismatch).
        //
        // `now_ms == 0` is the documented "clock unavailable" sentinel — the
        // rest of the module treats it as fail-open (`exhausted`,
        // `fires_out_of_bounds`, `is_final_deadline_tick`, `live_status`).
        // Failing CLOSED here would strand a previously-healthy loop as a
        // dormant Active until the next user input, for a blip that may
        // last only one SystemTime call. Match the module-wide fail-open
        // discipline by skipping the in-flight gate when we cannot tell.
        if let Some(wake) = state.pending_tick_wake_ms {
            if now_ms != 0 && now_ms < wake.saturating_add(PENDING_TICK_STALE_GRACE_MS) {
                // Persist a just-seeded baseline even when skipping.
                map.insert(session_id.to_string(), state);
                return TickDecision::Idle;
            }
        }
        // Project the wake BEFORE stamping anything: the tick claimed here
        // executes one cadence from now, and a wall-clock deadline bounds when
        // the loop may still ACT, not merely when it may still be scheduled.
        let delay_ms = pursuit::tick_delay_ms(&state, now_ms);
        let wake_ms = now_ms.saturating_add(delay_ms);
        let out_of_bounds = pursuit::fires_out_of_bounds(&state, wake_ms, now_ms);
        // `is_active()` was established above, so `should_fire` here is exactly
        // `!exhausted` — these two arms are total and every exit persists
        // `state`, which is what keeps a just-seeded token baseline.
        if pursuit::exhausted(&state, tokens_now, now_ms) || out_of_bounds {
            // Ask the shared note-picker as of the moment that actually bound
            // us: at `now_ms` a projected overrun has not happened yet, so it
            // would fall through to the iteration-cap wording.
            let bound_at = if out_of_bounds { wake_ms } else { now_ms };
            let note = pursuit::stop_reason_note(&state, tokens_now, bound_at);
            map.insert(
                session_id.to_string(),
                state
                    .with_status(LoopStatus::Stopped)
                    .with_stop_reason(Some(note.clone())),
            );
            TickDecision::Exhausted { note }
        } else {
            let prompt = pursuit::tick_prompt(&state, tokens_now, now_ms);
            // Bump BEFORE the tick runs so caps hold even if it crashes; clear
            // the consumed model-paced wake so a model that forgot to re-set
            // `next_wake` falls back to the cadence default instead of
            // busy-looping on a stale past wake.
            map.insert(
                session_id.to_string(),
                state
                    .spent_iteration()
                    .with_next_wake_ms(None)
                    .with_pending_tick(Some(wake_ms)),
            );
            TickDecision::Fire {
                delay_ms,
                wake_ms,
                prompt,
            }
        }
    }

    /// Re-arm a tick after an AgentBusy collision: the woken tick already
    /// confirmed (pending cleared) but could not run because another run of
    /// this agent held the slot — possibly in a DIFFERENT session, whose
    /// completion re-enters the hook under ITS OWN key and therefore never
    /// re-arms this loop. Without this, a busy collision left the loop a
    /// silently dormant `Active` until the user next spoke in this exact
    /// session. Re-stamps the pending marker with a short retry delay so the
    /// SAME claimed tick fires again shortly — no iteration bump (the tick was
    /// already counted at claim). Returns [`RearmDecision::Retry`] to schedule,
    /// [`RearmDecision::Exhausted`] when a cap tripped during the collision (the
    /// loop is stopped with its reason; the caller clears the welded strategy and
    /// notifies — R5), or [`RearmDecision::Drop`] when the loop is gone/stopped or
    /// another tick was claimed meanwhile.
    #[must_use]
    pub fn rearm_after_busy(&self, session_id: &str, now_ms: u64) -> RearmDecision {
        self.rearm_after_interruption(session_id, now_ms, BUSY_RETRY_DELAY_MS)
    }

    /// [`rearm_after_busy`](Self::rearm_after_busy) with a caller-chosen delay:
    /// the same claimed tick, the same two bounds, re-armed `delay_ms` from now
    /// instead of the fixed busy-collision backoff.
    ///
    /// The second caller is the continuation hook's FAILURE arm: a tick whose
    /// run died on a rate-limit / unreachable provider is parked for as long as
    /// the provider's own `Retry-After` asks (clamped by the caller) rather
    /// than stopping the loop for good. Both bounds below therefore matter more
    /// than they did for a 30 s collision — a multi-hour outage can park the
    /// same tick many times over, and `fires_out_of_bounds` is what keeps those
    /// parks from re-arming a tick that would wake past `timeout_minutes`.
    ///
    /// The delay is a parameter rather than a second copy of this body because
    /// the *decision* — is this loop still claimable, and would waking then be
    /// out of bounds — is one question with one answer; only the wait differs.
    #[must_use]
    pub fn rearm_after_interruption(
        &self,
        session_id: &str,
        now_ms: u64,
        delay_ms: u64,
    ) -> RearmDecision {
        let mut map = self.lock();
        match map.get(session_id) {
            Some(s) if s.is_active() && s.pending_tick_wake_ms.is_none() => {
                // Deadline may have passed while the collision played out;
                // token budget is claim-side only (no live counter here). The
                // retry wake is projected for the same reason `try_claim_tick`
                // projects its own: re-arming into a wake past the deadline
                // would run a full turn out of bounds.
                let wake = now_ms.saturating_add(delay_ms);
                let out_of_bounds = pursuit::fires_out_of_bounds(s, wake, now_ms);
                if pursuit::exhausted(s, 0, now_ms) || out_of_bounds {
                    let bound_at = if out_of_bounds { wake } else { now_ms };
                    let note = pursuit::stop_reason_note(s, 0, bound_at);
                    let stopped = s
                        .clone()
                        .with_status(LoopStatus::Stopped)
                        .with_stop_reason(Some(note.clone()));
                    map.insert(session_id.to_string(), stopped);
                    return RearmDecision::Exhausted { note };
                }
                let rearmed = s.clone().with_pending_tick(Some(wake));
                map.insert(session_id.to_string(), rearmed);
                RearmDecision::Retry {
                    delay_ms,
                    wake_ms: wake,
                }
            }
            _ => RearmDecision::Drop,
        }
    }

    /// Atomically commit a tool-side field update (caps / prompt / cadence /
    /// `next_wake`) to an Active loop under ONE lock guard, so it can never
    /// clobber the `pending_tick_wake_ms` a concurrent `confirm_fire` /
    /// `rearm_after_busy` mutated between the tool's read and this write.
    ///
    /// The `loop` tool computes `next` from a `get` snapshot that may be stale
    /// by the time it writes back — during a user turn the loop's next tick is
    /// sleeping with a pending marker, and if that tick fires (or re-arms after
    /// an AgentBusy) in the tool's read→write gap, a plain `put` would restore
    /// the tool's stale pending and strand the loop: a resurrected past marker
    /// blocks the next `try_claim_tick` until the 60s stale grace elapses. This
    /// method re-reads the LIVE pending here and keeps it, so the tick pipeline
    /// stays the single owner of that field (the documented invariant: never
    /// `put` a hand-carried pending). `reschedule` = the update re-paced or
    /// re-targeted the loop, so the in-flight tick is superseded — clear the
    /// marker (its `confirm_fire` then mismatches and skips) and this run's
    /// completion re-claims a fresh tick from the updated state.
    ///
    /// `iterations_used` and every other field are taken from `next`: they are
    /// not concurrently mutated during a user turn (the counter only bumps in
    /// `try_claim_tick`, which the completion hook runs AFTER the turn, never
    /// interleaved with an in-turn tool call). Returns `false` — a no-op — if
    /// the loop vanished or was stopped since the tool's read (a concurrent
    /// stop won the race); the caller reports that honestly.
    ///
    /// `status` and `stop_reason` are taken from the LIVE row for the same
    /// reason `pending_tick_wake_ms` is: they belong to the lifecycle pipeline
    /// ([`Self::transition`] / [`Self::try_claim_tick`]), not to a field update.
    /// A paused loop IS updatable — re-pacing while quiet is what pause is for —
    /// so without this guard a snapshot read while the loop was `Active`, racing
    /// a concurrent `pause`, would write its stale `Active` back and silently
    /// un-pause the loop.
    #[must_use]
    pub fn commit_field_update(&self, next: LoopState, reschedule: bool) -> bool {
        let mut map = self.lock();
        match map.get(&next.session_id) {
            Some(live) if live.is_adjustable() => {
                let pending = if reschedule {
                    None
                } else {
                    live.pending_tick_wake_ms
                };
                let (status, stop_reason) = (live.status, live.stop_reason.clone());
                // Owner/scope attribution (P1 data isolation) is stamped once
                // at creation and never re-derived from a tool snapshot —
                // same ownership rule as `status`/`stop_reason` above.
                let (owner_user_id, scope_id) = (live.owner_user_id.clone(), live.scope_id.clone());
                let session = next.session_id.clone();
                // A reschedule discards a still-sleeping tick the same way
                // `transition` does, so it owes the same refund. `next` carries
                // the tool's snapshot of `iterations_used`, which is not
                // concurrently mutated during a user turn.
                let mut next = if reschedule && live.pending_tick_wake_ms.is_some() {
                    next.refund_iteration()
                } else {
                    next
                };
                next.owner_user_id = owner_user_id;
                next.scope_id = scope_id;
                map.insert(
                    session,
                    next.with_status(status)
                        .with_stop_reason(stop_reason)
                        .with_pending_tick(pending),
                );
                true
            }
            _ => false,
        }
    }

    /// Fire-time gate for a woken tick: proceed only if the loop is still
    /// Active and this tick is still the one on the books (`wake_ms` matches
    /// the pending marker), clearing the marker in the same lock guard. A
    /// `false` means the tick was superseded — the user stopped the loop or
    /// started a new one during the delay — and must NOT execute (the ghost
    /// tick this kills used to burn a full stale LLM turn).
    #[must_use]
    pub fn confirm_fire(&self, session_id: &str, wake_ms: u64) -> bool {
        let mut map = self.lock();
        match map.get(session_id) {
            Some(s) if s.is_active() && s.pending_tick_wake_ms == Some(wake_ms) => {
                let cleared = s.clone().with_pending_tick(None);
                map.insert(session_id.to_string(), cleared);
                true
            }
            _ => false,
        }
    }
}

/// Process-global registry. Initialized once at daemon boot
/// (`constructor.rs`); `None` until then so tests / early-boot read as "no
/// loop subsystem" and the continuation hook stays dormant.
static GLOBAL: OnceCell<Arc<LoopRegistry>> = OnceCell::new();

/// Install the global registry at boot. Idempotent.
pub fn init_global(registry: Arc<LoopRegistry>) {
    let _ = GLOBAL.set(registry);
}

/// Read the global registry, if initialized.
#[must_use]
pub fn global() -> Option<Arc<LoopRegistry>> {
    GLOBAL.get().cloned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::looping::types::{Cadence, LoopState, LoopStatus};

    fn st(sess: &str) -> LoopState {
        LoopState::new(sess, "p", Cadence::Fixed { interval_ms: 1000 }, 0)
    }

    #[test]
    fn put_then_get_returns_state() {
        let reg = LoopRegistry::default();
        reg.put(st("a"));
        assert_eq!(reg.get("a").unwrap().session_id, "a");
        assert!(reg.get("missing").is_none());
    }

    #[test]
    fn put_overwrites_same_session() {
        let reg = LoopRegistry::default();
        reg.put(st("a"));
        reg.put(st("a").spent_iteration());
        assert_eq!(reg.get("a").unwrap().iterations_used, 1);
    }

    #[test]
    fn claim_refuses_a_tick_whose_wake_would_land_past_the_deadline() {
        // `interval='2h', timeout_minutes=1`: no tick can run in-bounds, and
        // the deadline is the ONLY bound (start drops the default tick cap
        // when a deadline is present). Before the projection this claimed a
        // tick that executed 119 minutes past the user's limit.
        let reg = LoopRegistry::default();
        reg.put(
            LoopState::new(
                "a",
                "p",
                Cadence::Fixed {
                    interval_ms: 7_200_000,
                },
                0,
            )
            .with_deadline_ms(Some(60_000)),
        );
        match reg.try_claim_tick("a", None, 1_000) {
            TickDecision::Exhausted { note } => {
                assert!(note.contains("time limit"), "{note}");
            }
            other => panic!("expected Exhausted, got {other:?}"),
        }
        assert_eq!(reg.get("a").unwrap().status, LoopStatus::Stopped);
        assert!(
            reg.get("a").unwrap().pending_tick_wake_ms.is_none(),
            "a refused claim must not leave a pending marker"
        );
    }

    #[test]
    fn claim_still_fires_when_the_wake_lands_inside_the_deadline() {
        // Guard against over-refusal: 10m cadence, 25m window → the wake at
        // 10m is in-bounds and must fire.
        let reg = LoopRegistry::default();
        reg.put(
            LoopState::new(
                "a",
                "p",
                Cadence::Fixed {
                    interval_ms: 600_000,
                },
                0,
            )
            .with_deadline_ms(Some(1_500_000)),
        );
        assert!(matches!(
            reg.try_claim_tick("a", None, 0),
            TickDecision::Fire { .. }
        ));
        // …and the claim at 20m, whose wake would be 30m, refuses.
        let reg2 = LoopRegistry::default();
        reg2.put(
            LoopState::new(
                "a",
                "p",
                Cadence::Fixed {
                    interval_ms: 600_000,
                },
                0,
            )
            .with_deadline_ms(Some(1_500_000)),
        );
        assert!(matches!(
            reg2.try_claim_tick("a", None, 1_200_000),
            TickDecision::Exhausted { .. }
        ));
    }

    #[test]
    fn clock_unavailable_still_fails_open_on_the_projection() {
        let reg = LoopRegistry::default();
        reg.put(st("a").with_deadline_ms(Some(60_000)));
        assert!(
            matches!(reg.try_claim_tick("a", None, 0), TickDecision::Fire { .. }),
            "now_ms == 0 means clock unavailable — never trip on it"
        );
    }

    #[test]
    fn token_budget_still_outranks_the_deadline_projection_in_the_note() {
        // Both bind at once: the note must name the token budget, matching
        // `stop_reason_note`'s documented priority order.
        let reg = LoopRegistry::default();
        reg.put(
            LoopState::new(
                "a",
                "p",
                Cadence::Fixed {
                    interval_ms: 7_200_000,
                },
                0,
            )
            .with_deadline_ms(Some(60_000))
            .with_token_budget(Some(100))
            .with_baseline(1_000),
        );
        match reg.try_claim_tick("a", Some(5_000), 1_000) {
            TickDecision::Exhausted { note } => assert!(note.contains("token budget"), "{note}"),
            other => panic!("expected Exhausted, got {other:?}"),
        }
    }

    #[test]
    fn rearm_refuses_a_retry_whose_wake_crosses_the_deadline() {
        let reg = LoopRegistry::default();
        // Deadline 10s away; the busy retry is +30s, so it lands out of bounds.
        reg.put(st("a").with_deadline_ms(Some(11_000)));
        match reg.rearm_after_busy("a", 1_000) {
            RearmDecision::Exhausted { note } => assert!(note.contains("time limit"), "{note}"),
            other => panic!("expected Exhausted, got {other:?}"),
        }
        assert_eq!(reg.get("a").unwrap().status, LoopStatus::Stopped);
        // A deadline comfortably past the retry still re-arms.
        let reg2 = LoopRegistry::default();
        reg2.put(st("b").with_deadline_ms(Some(600_000)));
        assert!(matches!(
            reg2.rearm_after_busy("b", 1_000),
            RearmDecision::Retry { .. }
        ));
    }

    #[test]
    fn superseding_a_sleeping_tick_refunds_its_claim() {
        // Claim bumps to 1 and stamps a pending marker; pausing retires that
        // tick before it ever ran, so the count must go back to 0. Without the
        // refund, five re-paces of a max_iterations=5 loop reported "5/5" and
        // stopped at the cap having executed nothing.
        let reg = LoopRegistry::default();
        reg.put(st("a"));
        assert!(matches!(
            reg.try_claim_tick("a", None, 1_000),
            TickDecision::Fire { .. }
        ));
        assert_eq!(reg.get("a").unwrap().iterations_used, 1);
        reg.transition("a", LoopStatus::Paused, Some("held".into()));
        assert_eq!(
            reg.get("a").unwrap().iterations_used,
            0,
            "a tick that never ran must not consume the cap"
        );

        // A reschedule discards the sleeping tick the same way.
        let reg2 = LoopRegistry::default();
        reg2.put(st("b"));
        assert!(matches!(
            reg2.try_claim_tick("b", None, 1_000),
            TickDecision::Fire { .. }
        ));
        let snapshot = reg2.get("b").unwrap();
        assert!(reg2.commit_field_update(snapshot, true));
        assert_eq!(reg2.get("b").unwrap().iterations_used, 0);
    }

    #[test]
    fn a_tick_that_actually_fired_keeps_its_claim() {
        // Once `confirm_fire` clears the marker the tick IS running; stopping
        // mid-flight must not hand its budget back.
        let reg = LoopRegistry::default();
        reg.put(st("a"));
        let TickDecision::Fire { wake_ms, .. } = reg.try_claim_tick("a", None, 1_000) else {
            panic!("expected Fire");
        };
        assert!(reg.confirm_fire("a", wake_ms));
        reg.transition("a", LoopStatus::Stopped, Some("done".into()));
        assert_eq!(reg.get("a").unwrap().iterations_used, 1);
    }

    #[test]
    fn list_all_returns_every_session_regardless_of_status() {
        let reg = LoopRegistry::default();
        reg.put(st("a"));
        reg.put(st("b").with_status(LoopStatus::Stopped));
        reg.put(st("c"));
        let all = reg.list_all();
        assert_eq!(
            all.len(),
            3,
            "one row per session, active and stopped alike"
        );
        let ids: std::collections::HashSet<_> = all.iter().map(|l| l.session_id.as_str()).collect();
        assert!(ids.contains("a") && ids.contains("b") && ids.contains("c"));
        assert!(
            LoopRegistry::default().list_all().is_empty(),
            "empty registry"
        );
    }

    #[test]
    fn get_active_filters_stopped() {
        let reg = LoopRegistry::default();
        reg.put(st("a").with_status(LoopStatus::Stopped));
        assert!(reg.get("a").is_some(), "get returns regardless of status");
        assert!(reg.get_active("a").is_none(), "get_active skips Stopped");
    }

    #[test]
    fn claim_fires_and_stamps_pending_wake() {
        let reg = LoopRegistry::default();
        reg.put(st("a")); // Fixed 1000ms cadence
        let d = reg.try_claim_tick("a", None, 5_000);
        let TickDecision::Fire {
            delay_ms, wake_ms, ..
        } = d
        else {
            panic!("expected Fire, got {d:?}");
        };
        assert_eq!(delay_ms, 1_000);
        assert_eq!(wake_ms, 6_000);
        let s = reg.get("a").unwrap();
        assert_eq!(s.iterations_used, 1, "bumped before the tick runs");
        assert_eq!(s.pending_tick_wake_ms, Some(6_000));
    }

    #[test]
    fn claim_while_tick_in_flight_is_idle() {
        // The fan-out gate: a user turn completing while a tick sleeps must
        // NOT enqueue a second chain.
        let reg = LoopRegistry::default();
        reg.put(st("a"));
        assert!(matches!(
            reg.try_claim_tick("a", None, 5_000),
            TickDecision::Fire { .. }
        ));
        assert_eq!(reg.try_claim_tick("a", None, 5_500), TickDecision::Idle);
        assert_eq!(
            reg.get("a").unwrap().iterations_used,
            1,
            "skipped claim must not burn a tick"
        );
    }

    #[test]
    fn stale_pending_tick_is_reclaimed() {
        // A pending marker long past its wake (dead task) must not stall the
        // loop forever.
        let reg = LoopRegistry::default();
        reg.put(st("a").with_pending_tick(Some(6_000)));
        let past_grace = 6_000 + PENDING_TICK_STALE_GRACE_MS + 1;
        assert!(matches!(
            reg.try_claim_tick("a", None, past_grace),
            TickDecision::Fire { .. }
        ));
    }

    #[test]
    fn claim_on_exhausted_loop_stops_with_reason() {
        let reg = LoopRegistry::default();
        reg.put(st("a").with_max_iterations(Some(1)).spent_iteration());
        let d = reg.try_claim_tick("a", None, 5_000);
        assert!(matches!(&d, TickDecision::Exhausted { note } if note.contains("iteration")));
        let s = reg.get("a").unwrap();
        assert!(!s.is_active());
        assert!(s.stop_reason.is_some());
    }

    #[test]
    fn claim_seeds_token_baseline_once() {
        let reg = LoopRegistry::default();
        reg.put(st("a").with_token_budget(Some(500)));
        // First claim with a live total seeds the baseline (0 spent → fires).
        assert!(matches!(
            reg.try_claim_tick("a", Some(10_000), 5_000),
            TickDecision::Fire { .. }
        ));
        let s = reg.get("a").unwrap();
        assert!(s.baseline_captured);
        assert_eq!(s.tokens_at_start, 10_000);
        // Over budget on a later claim → Exhausted with the budget note.
        reg.put(s.with_pending_tick(None));
        let d = reg.try_claim_tick("a", Some(10_600), 6_000);
        assert!(matches!(&d, TickDecision::Exhausted { note } if note.contains("token budget")));
    }

    #[test]
    fn rearm_after_busy_restamps_pending_without_iteration_bump() {
        let reg = LoopRegistry::default();
        reg.put(st("a"));
        let TickDecision::Fire { wake_ms, .. } = reg.try_claim_tick("a", None, 5_000) else {
            panic!("expected Fire");
        };
        assert!(reg.confirm_fire("a", wake_ms), "tick fires");
        let ticks_before = reg.get("a").unwrap().iterations_used;
        // AgentBusy at fire time → re-arm the same tick with a retry delay.
        let RearmDecision::Retry {
            delay_ms: delay,
            wake_ms: wake,
        } = reg.rearm_after_busy("a", 7_000)
        else {
            panic!("expected re-arm");
        };
        assert_eq!(wake, 7_000 + delay);
        let s = reg.get("a").unwrap();
        assert_eq!(s.pending_tick_wake_ms, Some(wake));
        assert_eq!(
            s.iterations_used, ticks_before,
            "retry must not burn a tick"
        );
        // While the retry is pending, neither a second re-arm nor a fresh
        // claim may double-schedule.
        assert!(matches!(
            reg.rearm_after_busy("a", 7_100),
            RearmDecision::Drop
        ));
        assert_eq!(reg.try_claim_tick("a", None, 7_100), TickDecision::Idle);
        // The retry confirms and fires like any tick.
        assert!(reg.confirm_fire("a", wake));
    }

    #[test]
    fn rearm_after_busy_refuses_stopped_and_stops_past_deadline() {
        let reg = LoopRegistry::default();
        reg.put(st("stopped").with_status(LoopStatus::Stopped));
        assert!(matches!(
            reg.rearm_after_busy("stopped", 5_000),
            RearmDecision::Drop
        ));
        // Deadline passed during the collision → stop with the real reason
        // instead of re-arming a tick that could only fire out of bounds. The
        // cap-trip now surfaces as Exhausted so the caller notifies + cleans up.
        reg.put(st("late").with_deadline_ms(Some(4_000)));
        assert!(matches!(
            reg.rearm_after_busy("late", 5_000),
            RearmDecision::Exhausted { .. }
        ));
        let s = reg.get("late").unwrap();
        assert!(!s.is_active());
        assert!(s.stop_reason.as_deref().unwrap_or("").contains("time"));
    }

    #[test]
    fn commit_field_update_preserves_live_pending_not_stale() {
        // The race the atomic commit exists to close: the tool read a snapshot
        // carrying pending=100, computed a cap change, but a tick fired+re-armed
        // meanwhile so the LIVE pending is now 200. A plain put would restore
        // the stale 100 and stall the loop; commit must keep the live 200.
        let reg = LoopRegistry::default();
        reg.put(st("a").with_pending_tick(Some(100)));
        let stale_next = st("a")
            .with_max_iterations(Some(9))
            .with_pending_tick(Some(100));
        // Simulate the concurrent tick pipeline moving pending to 200.
        reg.put(reg.get("a").unwrap().with_pending_tick(Some(200)));
        assert!(
            reg.commit_field_update(stale_next, false),
            "active → commits"
        );
        let s = reg.get("a").unwrap();
        assert_eq!(s.max_iterations, Some(9), "cap change applied");
        assert_eq!(
            s.pending_tick_wake_ms,
            Some(200),
            "live pending preserved, stale clobber avoided"
        );
    }

    #[test]
    fn commit_field_update_reschedule_clears_pending() {
        // A re-pace / re-target supersedes the in-flight tick: pending cleared
        // regardless of what the live marker was, so confirm_fire mismatches.
        let reg = LoopRegistry::default();
        reg.put(st("a").with_pending_tick(Some(200)));
        let next = st("a").with_cadence(Cadence::Fixed {
            interval_ms: 120_000,
        });
        assert!(reg.commit_field_update(next, true));
        assert!(reg.get("a").unwrap().pending_tick_wake_ms.is_none());
    }

    #[test]
    fn commit_field_update_refuses_stopped_or_missing_loop() {
        // Lost the race to a concurrent stop / cap-exhaustion → no-op false,
        // so the tool reports the update did not land instead of lying.
        let reg = LoopRegistry::default();
        reg.put(st("a").with_status(LoopStatus::Stopped));
        assert!(!reg.commit_field_update(st("a").with_max_iterations(Some(5)), false));
        assert!(
            !reg.commit_field_update(st("gone").with_max_iterations(Some(5)), false),
            "no loop for this session → false"
        );
    }

    #[test]
    fn commit_field_update_never_clobbers_owner_scope() {
        // Mirrors `commit_field_update_preserves_live_pending_not_stale`: a
        // tool-side field update built from a bare `st()` snapshot (no owner
        // stamped) must not roll back the owner/scope recorded at creation.
        let reg = LoopRegistry::default();
        let attr = crate::scope::ScopeAttribution::personal("u-alice");
        reg.put(st("a").with_owner_scope(Some(&attr)));
        let next = st("a").with_max_iterations(Some(9)); // no owner_scope re-stamped
        assert!(reg.commit_field_update(next, false));
        let s = reg.get("a").unwrap();
        assert_eq!(s.max_iterations, Some(9), "cap change applied");
        assert_eq!(s.owner_user_id.as_deref(), Some("u-alice"));
        assert_eq!(s.scope_id.as_deref(), Some("personal:u-alice"));
    }

    #[test]
    fn pause_all_owned_by_pauses_exactly_that_users_loops() {
        let reg = LoopRegistry::default();
        let alice = crate::scope::ScopeAttribution::personal("u-alice");
        let bob = crate::scope::ScopeAttribution::personal("u-bob");
        reg.put(st("s-alice-1").with_owner_scope(Some(&alice)));
        reg.put(st("s-alice-2").with_owner_scope(Some(&alice)));
        reg.put(st("s-bob").with_owner_scope(Some(&bob)));
        // A legacy (pre-P1) loop: owner_user_id is None, never alice's.
        reg.put(st("s-legacy"));

        let count = reg.pause_all_owned_by("u-alice");
        assert_eq!(count, 2, "only alice's two loops are paused");

        assert_eq!(reg.get("s-alice-1").unwrap().status, LoopStatus::Paused);
        assert_eq!(reg.get("s-alice-2").unwrap().status, LoopStatus::Paused);
        assert_eq!(
            reg.get("s-bob").unwrap().status,
            LoopStatus::Active,
            "bob's loop must be untouched"
        );
        assert_eq!(
            reg.get("s-legacy").unwrap().status,
            LoopStatus::Active,
            "legacy None-owner rows belong to the platform owner, not alice — untouched"
        );
    }

    #[test]
    fn pause_all_owned_by_is_a_no_op_for_a_user_with_no_loops() {
        let reg = LoopRegistry::default();
        let bob = crate::scope::ScopeAttribution::personal("u-bob");
        reg.put(st("s-bob").with_owner_scope(Some(&bob)));
        assert_eq!(reg.pause_all_owned_by("u-alice"), 0);
        assert_eq!(reg.get("s-bob").unwrap().status, LoopStatus::Active);
    }

    #[test]
    fn transition_enforces_the_legality_matrix() {
        let reg = LoopRegistry::default();
        reg.put(st("a"));
        // Active → Paused → Active → Stopped is the legal round trip.
        assert_eq!(
            reg.transition("a", LoopStatus::Paused, Some("user".into())),
            TransitionOutcome::Applied {
                from: LoopStatus::Active
            }
        );
        assert_eq!(reg.get("a").unwrap().stop_reason.as_deref(), Some("user"));
        assert_eq!(
            reg.transition("a", LoopStatus::Active, None),
            TransitionOutcome::Applied {
                from: LoopStatus::Paused
            }
        );
        assert!(
            reg.get("a").unwrap().stop_reason.is_none(),
            "resuming clears the why-it-is-not-ticking note"
        );
        assert!(matches!(
            reg.transition("a", LoopStatus::Stopped, Some("done".into())),
            TransitionOutcome::Applied { .. }
        ));
        // Stopped is terminal: neither resume nor pause may resurrect it, and a
        // second stop is a reportable no-op rather than a silent success.
        for to in [LoopStatus::Active, LoopStatus::Paused, LoopStatus::Stopped] {
            assert_eq!(
                reg.transition("a", to, None),
                TransitionOutcome::Refused {
                    current: LoopStatus::Stopped
                },
                "{to:?} out of Stopped must be refused"
            );
        }
        assert_eq!(
            reg.get("a").unwrap().stop_reason.as_deref(),
            Some("done"),
            "a refused move must not overwrite the real stop reason"
        );
        assert_eq!(
            reg.transition("gone", LoopStatus::Stopped, None),
            TransitionOutcome::Missing
        );
    }

    #[test]
    fn pause_retires_the_in_flight_tick_so_resume_is_immediate() {
        // The dead-zone bug this guards: a loop paused early in a long cadence
        // keeps a far-future pending marker, and `try_claim_tick` would answer
        // Idle until 60s past THAT wake — ~56 minutes of silence after resume on
        // an hourly loop. Leaving Active must clear the marker.
        let reg = LoopRegistry::default();
        reg.put(LoopState::new(
            "a",
            "p",
            Cadence::Fixed {
                interval_ms: 3_600_000,
            },
            0,
        ));
        let TickDecision::Fire { wake_ms, .. } = reg.try_claim_tick("a", None, 1_000) else {
            panic!("expected Fire");
        };
        assert!(reg.get("a").unwrap().pending_tick_wake_ms.is_some());

        assert!(matches!(
            reg.transition("a", LoopStatus::Paused, None),
            TransitionOutcome::Applied { .. }
        ));
        assert!(reg.get("a").unwrap().pending_tick_wake_ms.is_none());
        // The tick still sleeping out its hour must not execute on wake.
        assert!(
            !reg.confirm_fire("a", wake_ms),
            "paused loop kills its tick"
        );
        // While paused, the hook sees nothing to do.
        assert_eq!(reg.try_claim_tick("a", None, 2_000), TickDecision::Idle);
        assert!(reg.get_active("a").is_none());

        // Resume, and the very next completed run claims a fresh tick — no wait
        // for the retired wake or the stale grace.
        assert!(matches!(
            reg.transition("a", LoopStatus::Active, None),
            TransitionOutcome::Applied { .. }
        ));
        assert!(matches!(
            reg.try_claim_tick("a", None, 2_000),
            TickDecision::Fire { .. }
        ));
    }

    #[test]
    fn stop_all_quiets_every_session_and_reports_only_real_stops() {
        let reg = LoopRegistry::default();
        reg.put(st("a"));
        reg.put(st("b").with_status(LoopStatus::Paused));
        reg.put(
            st("c")
                .with_status(LoopStatus::Stopped)
                .with_stop_reason(Some("earlier cap".into())),
        );
        let mut stopped = reg.stop_all("Stopped by kill switch.");
        stopped.sort();
        assert_eq!(
            stopped,
            vec!["a".to_string(), "b".to_string()],
            "already-stopped loops are not re-reported"
        );
        for s in ["a", "b"] {
            let l = reg.get(s).unwrap();
            assert!(!l.is_active() && !l.is_paused());
            assert_eq!(l.stop_reason.as_deref(), Some("Stopped by kill switch."));
            assert!(l.pending_tick_wake_ms.is_none());
        }
        assert_eq!(
            reg.get("c").unwrap().stop_reason.as_deref(),
            Some("earlier cap"),
            "an untouched loop keeps its original reason"
        );
        assert!(LoopRegistry::default().stop_all("x").is_empty());
    }

    /// `transition(_, Stopped, _)` already refunds an unrun tick when leaving
    /// Active with a claimed-but-unfired marker. `stop_all` used to write the
    /// status change directly and silently skip the refund, so a 1-cap loop
    /// hit by a kill switch would land `iterations_used: 1/1` having
    /// executed nothing. Pinned here so the two write paths cannot drift.
    #[test]
    fn stop_all_refunds_iteration_on_active_with_pending_tick() {
        let reg = LoopRegistry::default();
        // Claim a tick on a 1-cap loop — the bump to iterations_used=1
        // happens in `try_claim_tick` (spent_iteration), so the registry now
        // holds Active with pending_tick_wake_ms=Some(…) and iterations_used=1.
        let cap_one = st("a").with_max_iterations(Some(1));
        reg.put(cap_one.clone());
        let _ = reg.try_claim_tick("a", None, 1_000);
        let claimed = reg.get("a").unwrap();
        assert_eq!(claimed.iterations_used, 1);
        assert!(
            claimed.pending_tick_wake_ms.is_some(),
            "claim must stamp the pending marker"
        );

        let stopped = reg.stop_all("kill switch");
        assert_eq!(stopped, vec!["a".to_string()]);
        let after = reg.get("a").unwrap();
        assert_eq!(after.status, LoopStatus::Stopped);
        assert_eq!(
            after.iterations_used, 0,
            "the unrun tick's claim must be refunded, not carried into Stopped"
        );
        assert!(after.pending_tick_wake_ms.is_none());
    }

    /// `try_claim_tick` must match the module-wide fail-open discipline when
    /// the clock is unavailable (`now_ms == 0` sentinel). Today only this
    /// gate fails closed; the bug strands a healthy loop until the next user
    /// input. Pinned here so a future refactor cannot silently flip it back.
    #[test]
    fn try_claim_tick_proceeds_when_clock_unavailable_and_tick_in_flight() {
        let reg = LoopRegistry::default();
        reg.put(st("a").with_max_iterations(Some(3)));
        // Pre-seed an in-flight marker as if a previous claim had been made
        // and the wake is in the future.
        let pre = reg.get("a").unwrap().clone().with_pending_tick(Some(1_000_000));
        reg.put(pre);
        // now_ms == 0 means the clock is unavailable. The in-flight gate must
        // fall through, the claim must proceed, and the loop must Fire.
        let decision = reg.try_claim_tick("a", None, 0);
        assert!(
            matches!(decision, TickDecision::Fire { .. }),
            "clock-unavailable must not strand an Active loop: {decision:?}"
        );
    }

    #[test]
    fn commit_field_update_keeps_the_live_lifecycle_state() {
        // A paused loop is updatable (that is what pause is for), but the tool's
        // snapshot may have been taken while it was still Active. Committing the
        // snapshot's status would silently un-pause it.
        let reg = LoopRegistry::default();
        reg.put(st("a"));
        let stale_active_snapshot = reg.get("a").unwrap().with_max_iterations(Some(7));
        assert!(matches!(
            reg.transition("a", LoopStatus::Paused, Some("held".into())),
            TransitionOutcome::Applied { .. }
        ));
        assert!(
            reg.commit_field_update(stale_active_snapshot, false),
            "a paused loop accepts field updates"
        );
        let live = reg.get("a").unwrap();
        assert_eq!(live.max_iterations, Some(7), "the cap change landed");
        assert!(live.is_paused(), "the live pause survived a stale snapshot");
        assert_eq!(live.stop_reason.as_deref(), Some("held"));
    }

    #[test]
    fn confirm_fire_clears_pending_and_gates_supersede() {
        let reg = LoopRegistry::default();
        reg.put(st("a"));
        let TickDecision::Fire { wake_ms, .. } = reg.try_claim_tick("a", None, 5_000) else {
            panic!("expected Fire");
        };
        // Wrong wake (superseded by a stop/start cycle) → refuse.
        assert!(!reg.confirm_fire("a", wake_ms + 1));
        // Matching wake → proceed exactly once.
        assert!(reg.confirm_fire("a", wake_ms));
        assert!(reg.get("a").unwrap().pending_tick_wake_ms.is_none());
        assert!(!reg.confirm_fire("a", wake_ms), "second confirm refused");
        // Stopped during the delay → refuse.
        reg.put(st("b"));
        let TickDecision::Fire { wake_ms: wb, .. } = reg.try_claim_tick("b", None, 5_000) else {
            panic!("expected Fire");
        };
        reg.put(reg.get("b").unwrap().with_status(LoopStatus::Stopped));
        assert!(!reg.confirm_fire("b", wb), "ghost tick must not execute");
    }
}
