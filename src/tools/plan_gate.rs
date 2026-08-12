//! The plan → build handoff, as one run-scoped cell.
//!
//! A conversation at [`ExecTier::Plan`] refuses every mutating call. The mode
//! ends when a **human** approves the plan, and the thing that makes the
//! handoff feel like one continuous piece of work rather than two is that the
//! refusal has to stop **immediately** — in the same turn, at the very next
//! tool call — not on the next message.
//!
//! Nothing in `src/harness/` learns any of this (R10). The loop still just
//! dispatches whatever the model asked for; what changes is the answer the one
//! enforcement chokepoint gives, and that answer is this `AtomicBool`.
//!
//! ## Why a cell and not a re-resolution
//!
//! [`crate::gateway::execution_engine::turn_permissions`] resolves the tier
//! **once per turn**, before the harness starts, and hands it to the
//! `ScopedToolService` the whole turn is dispatched through. Re-resolving
//! per call would mean a session-store read on the hot path and, worse, two
//! answers to "what tier is this turn at" — the resolved one and the stored
//! one — which drift the moment a concurrent surface patches the session. So
//! the tier stays resolved once, and the ONE thing that may move it mid-run
//! moves through this handle, which the resolver creates and the approval gate
//! flips.
//!
//! ## Who may flip it
//!
//! Only [`crate::builtin_tools::scratchpad`]'s `request_approval` action, and
//! only after [`crate::clarification::ask`] returned a human's `approved`.
//! That path already fails closed with no approval transport and on unattended
//! runs, so "the model releases its own gate" is never reachable without a
//! person on the other end. The release also writes through to the session
//! (see `scratchpad::release_plan_gate`) so turn 2+ and every attached client
//! see the same tier this run is already running at.
//!
//! ## Reaching it
//!
//! The `Arc` is owned by the turn's [`TurnContext`](super::turn_context::TurnContext),
//! which `ScopedToolService` both holds (so `permission_for` can read it) and
//! scopes as a task-local around every dispatch (so the tool can flip it).
//! One value, two readers, no second source.

use std::sync::atomic::{AtomicBool, Ordering};

use crate::config::types::policies::ExecTier;

/// The mid-run half of the plan → build handoff.
///
/// Created only for a turn that resolved to [`ExecTier::Plan`]; every other
/// turn carries `None` and is byte-identical to a build with no plan mode at
/// all.
#[derive(Debug)]
pub struct PlanGate {
    /// The tier this conversation returns to once the plan is approved.
    ///
    /// Resolved from the same three rungs every other turn uses, with the
    /// session's own `plan` override taken out of the running — i.e. "what
    /// would this turn have run at if the conversation had never said
    /// `plan`". Deliberately NOT a remembered pre-plan tier: that would be a
    /// second per-session key with two writers (`sessions.patch` and the
    /// request-carried stamp), and 判据 §0 has already collected the bill for
    /// convergences that forgot to count their writers.
    restore_to: ExecTier,
    released: AtomicBool,
}

impl PlanGate {
    /// A fresh, unreleased gate that will hand the turn back to `restore_to`.
    ///
    /// A `Plan` restore target is normalized to [`ExecTier::default`] here
    /// rather than at the call site. `[policies] exec_tier` is refused the
    /// `plan` id by config validation, but a hand-edited TOML deserializes it
    /// perfectly well, and the failure mode of trusting that is the worst
    /// shape a gate can have: a conversation that gets its plan approved and
    /// lands straight back in planning, with the approval spent. Normalizing
    /// in the constructor makes "the gate always hands back to something that
    /// can build" true by construction rather than by convention.
    #[must_use]
    pub const fn new(restore_to: ExecTier) -> Self {
        Self {
            restore_to: match restore_to {
                ExecTier::Plan => ExecTier::Auto,
                other => other,
            },
            released: AtomicBool::new(false),
        }
    }

    /// The tier the enforcement chokepoint should apply **right now**.
    #[must_use]
    pub fn tier(&self) -> ExecTier {
        if self.is_released() {
            self.restore_to
        } else {
            ExecTier::Plan
        }
    }

    /// The tier this gate hands back to. Reported to the model and written to
    /// the session by the release path, so all three say the same thing.
    #[must_use]
    pub const fn restore_to(&self) -> ExecTier {
        self.restore_to
    }

    #[must_use]
    pub fn is_released(&self) -> bool {
        self.released.load(Ordering::Acquire)
    }

    /// Lift the read-only gate. Returns `true` when THIS call did it.
    ///
    /// Idempotent on purpose, and the boolean says which call won: the
    /// session write-through and the "you may now build" message are worth
    /// emitting once, not once per approval the model asks for after the
    /// first. (`swap` rather than `store` for exactly that — 判据 §0's
    /// "一次性的动作，哪个面执行了哪个面就是唯一机会".)
    pub fn release(&self) -> bool {
        !self.released.swap(true, Ordering::AcqRel)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unreleased_gate_reports_plan() {
        let gate = PlanGate::new(ExecTier::Auto);
        assert_eq!(gate.tier(), ExecTier::Plan);
        assert!(!gate.is_released());
    }

    #[test]
    fn release_hands_back_the_restore_tier() {
        let gate = PlanGate::new(ExecTier::Ask);
        assert!(gate.release());
        assert_eq!(gate.tier(), ExecTier::Ask);
        assert_eq!(gate.restore_to(), ExecTier::Ask);
    }

    /// Only the first release is the release. The second call must still
    /// leave the gate open (idempotent) while reporting that it changed
    /// nothing — the write-through and the announcement ride on that answer.
    #[test]
    fn only_the_first_release_reports_true() {
        let gate = PlanGate::new(ExecTier::Auto);
        assert!(gate.release());
        assert!(!gate.release());
        assert_eq!(gate.tier(), ExecTier::Auto);
    }

    /// A gate can never hand back to `Plan` — that would be a conversation
    /// that gets its plan approved, lands back in planning, and has spent the
    /// approval getting there. Including when someone hand-writes
    /// `[policies] exec_tier = "plan"`, which deserializes fine.
    #[test]
    fn the_restore_tier_is_never_plan() {
        for restore in [
            ExecTier::Plan,
            ExecTier::Ask,
            ExecTier::Auto,
            ExecTier::Full,
        ] {
            let gate = PlanGate::new(restore);
            assert_ne!(gate.restore_to(), ExecTier::Plan);
            gate.release();
            assert_ne!(gate.tier(), ExecTier::Plan);
        }
        assert_eq!(PlanGate::new(ExecTier::Plan).restore_to(), ExecTier::Auto);
    }
}
