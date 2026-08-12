//! `PlanGate` — the live half of the read-only planning phase.
//!
//! The phase itself ([`crate::config::types::policies::PlanPhase`]) is a
//! per-session fact resolved once at run start. This is the thing that lets an
//! **approved handoff take effect inside the run that asked for it**, instead
//! of only from the next user message onward.
//!
//! ## Why a live latch at all
//!
//! Everything else about a turn's permissions is resolved once and frozen: the
//! tier, the merged policy, the tool surface. That is correct for knobs a human
//! turns between turns. It is wrong for this one, because the whole feature is
//! "the person approves the plan and work begins" — and a posture that only
//! lifts on the *next* message turns one gesture into two, which is exactly the
//! friction the handoff exists to remove. codex pays that cost (its TUI submits
//! a fresh `Implement the plan.` message in Default mode); Aleph does not have
//! to, because `ScopedToolService` already owns a generation-counted schema
//! cache built for precisely this shape of invalidation (health probes and
//! deferred-tool promotion both use it).
//!
//! ## What keeps it honest
//!
//! * It only ever moves in the safe direction. `engaged` starts `true` and can
//!   be set `false` once; there is no re-engage, so nothing can tighten and
//!   then loosen again inside a run based on state the model influenced.
//! * The only caller of [`PlanGate::release`] is the handoff arm of the gate
//!   chain, which runs **after** the person answered the approval card.
//! * Releasing does not raise the tier. The turn's `ExecTier` was resolved and
//!   clamped at run start and is untouched here; lifting the floor merely stops
//!   *subtracting* from it. A member who plans under `Ask` builds under `Ask`.
//! * The durable record is written **before** the latch moves. If the write
//!   fails the latch stays engaged and the model is told: a decision that did
//!   not survive to disk must not silently govern the rest of the run, because
//!   a restart would then snap back to planning with the writes already made
//!   (判据: 先记录意图、再做不可逆动作).

use std::sync::atomic::{AtomicBool, Ordering};

use async_trait::async_trait;

use crate::config::types::policies::PlanPhase;

/// Where an approved handoff is recorded so it outlives the process.
///
/// A trait rather than a session-store handle because `src/tools/` must not
/// depend on the gateway's storage types (P1/P4): the gateway implements this
/// over `SessionPatch`, tests implement it over a cell.
#[async_trait]
pub trait PlanPhaseSink: Send + Sync + std::fmt::Debug {
    /// Persist "this session has left the planning phase". Must be idempotent:
    /// the same approval can be recorded twice (a retried card, a racing
    /// sibling call) and the second write must not fail.
    async fn record_build_approved(&self) -> Result<(), String>;
}

/// The live read-only latch for one run.
#[derive(Debug)]
pub struct PlanGate {
    /// `true` while the read-only floor applies. Monotonic: set once at
    /// construction, cleared at most once by an approved handoff.
    engaged: AtomicBool,
    /// Durable record of the handoff. `None` in unit tests and on paths with
    /// no session store; the latch then releases on approval alone and the
    /// phase reverts on the next run — safe, and visibly so, because the
    /// release path says which of the two happened.
    sink: Option<std::sync::Arc<dyn PlanPhaseSink>>,
}

impl PlanGate {
    /// A gate for a run that starts in the read-only planning phase.
    #[must_use]
    pub fn planning(sink: Option<std::sync::Arc<dyn PlanPhaseSink>>) -> Self {
        Self {
            engaged: AtomicBool::new(true),
            sink,
        }
    }

    /// The phase this gate currently imposes.
    #[must_use]
    pub fn phase(&self) -> PlanPhase {
        if self.engaged.load(Ordering::Acquire) {
            PlanPhase::Planning
        } else {
            PlanPhase::Building
        }
    }

    /// Record the approval, then lift the floor for the rest of this run.
    ///
    /// `Ok(true)` — this call lifted it. `Ok(false)` — it was already lifted
    /// (a second handoff in the same run); the caller should say so rather than
    /// claim a fresh transition. `Err` — the durable write failed and the floor
    /// is **still engaged**.
    pub async fn release(&self) -> Result<bool, String> {
        if let Some(sink) = self.sink.as_ref() {
            sink.record_build_approved().await?;
        }
        Ok(self
            .engaged
            .compare_exchange(true, false, Ordering::AcqRel, Ordering::Acquire)
            .is_ok())
    }

    /// Whether an approved handoff would outlive this process.
    ///
    /// Read by the handoff arm so its answer to the model can be accurate about
    /// which of the two guarantees it just got. Saying "execution is unlocked"
    /// when only the in-memory latch moved would be a promise the next process
    /// does not keep.
    #[must_use]
    pub const fn is_durable(&self) -> bool {
        self.sink.is_some()
    }
}

impl super::ScopedToolService {
    /// The sentence this call is refused with while the session is planning, or
    /// `None` when the floor lets it through (which is always, in a session
    /// that never entered the phase).
    pub(super) fn plan_refusal(&self, name: &str, input: &serde_json::Value) -> Option<String> {
        (self.plan_admission(name, input) == crate::config::types::policies::PlanAdmission::Refused)
            .then(|| PlanPhase::refusal(name))
    }

    /// Lift the read-only floor after the person approved the plan, and return
    /// the line the model is told about it.
    ///
    /// The line distinguishes the three outcomes the model can actually act on
    /// differently — a fresh release, a repeat handoff in a run already
    /// released, and a release that lives only in this process — because a
    /// single "approved" would be a claim about durability this function is not
    /// always in a position to make.
    pub(super) async fn release_plan_gate(&self) -> Result<String, String> {
        let Some(gate) = self.plan_gate.as_ref() else {
            // Unreachable through the handoff arm (it only fires while the
            // phase is engaged, which requires a gate) but stated rather than
            // unwrapped: a future caller that gets here has a wiring bug, not a
            // panic.
            return Err(
                "This session is not in the read-only planning phase, so there is nothing \
                 to hand off. Continue with the work."
                    .to_string(),
            );
        };
        let first = gate.release().await.map_err(|e| {
            format!(
                "The user approved the plan, but recording that decision failed ({e}), so \
                 the session is still read-only and the approval would be lost on restart. \
                 Tell the user, and do not retry the handoff until they say the storage \
                 problem is fixed."
            )
        })?;
        if !first {
            return Ok(
                "This session already left the planning phase; execution was \
                       already unlocked."
                    .to_string(),
            );
        }
        Ok(if gate.is_durable() {
            "The user approved the plan. The read-only planning phase is over: from your \
             next tool call onward you may run the tools your approval mode allows. Work \
             the plan one step at a time and keep the scratchpad checklist current."
                .to_string()
        } else {
            "The user approved the plan. Execution is unlocked for the rest of this run, \
             but this session has no store to record the decision in, so a restart would \
             return it to planning. Prefer finishing the work in this run."
                .to_string()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[derive(Debug, Default)]
    struct CountingSink {
        writes: std::sync::atomic::AtomicUsize,
        fail: bool,
    }

    #[async_trait]
    impl PlanPhaseSink for CountingSink {
        async fn record_build_approved(&self) -> Result<(), String> {
            self.writes.fetch_add(1, Ordering::AcqRel);
            if self.fail {
                return Err("store unavailable".to_string());
            }
            Ok(())
        }
    }

    #[tokio::test]
    async fn release_lifts_the_floor_once() {
        let gate = PlanGate::planning(None);
        assert_eq!(gate.phase(), PlanPhase::Planning);
        assert!(gate.release().await.unwrap());
        assert_eq!(gate.phase(), PlanPhase::Building);
        // A second handoff in the same run is not a fresh transition.
        assert!(!gate.release().await.unwrap());
        assert_eq!(gate.phase(), PlanPhase::Building);
    }

    #[tokio::test]
    async fn a_failed_write_leaves_the_floor_engaged() {
        let sink = Arc::new(CountingSink {
            fail: true,
            ..Default::default()
        });
        let gate = PlanGate::planning(Some(sink.clone()));
        assert!(gate.release().await.is_err());
        assert_eq!(
            gate.phase(),
            PlanPhase::Planning,
            "a decision that did not reach disk must not govern the run"
        );
    }

    #[tokio::test]
    async fn the_record_is_written_before_the_latch_moves() {
        let sink = Arc::new(CountingSink::default());
        let gate = PlanGate::planning(Some(sink.clone()));
        gate.release().await.unwrap();
        assert_eq!(sink.writes.load(Ordering::Acquire), 1);
        assert_eq!(gate.phase(), PlanPhase::Building);
    }

    #[test]
    fn durability_is_reported_from_the_sink_not_assumed() {
        assert!(!PlanGate::planning(None).is_durable());
        assert!(PlanGate::planning(Some(Arc::new(CountingSink::default()))).is_durable());
    }
}
