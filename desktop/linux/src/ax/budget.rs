//! A wall-clock budget for AT-SPI traversals.
//!
//! # Why a node count is not enough
//!
//! [`walk::MAX_NODES`](super::walk::MAX_NODES) bounds how *many* objects a walk
//! reads. It says nothing about how long each read takes — and every read here
//! is a D-Bus round trip **into another process**. When that process stops
//! pumping its main loop (a frozen Electron window, a GTK app blocked on a modal
//! it never drew) the reply never comes, and zbus waits: the accessibility bus
//! has no shorter deadline of its own than the D-Bus default.
//!
//! That is the same failure the Linux limb's shell-outs were capped against in
//! 2026-07: a wedged desktop service pins the whole agent turn against the
//! harness ceiling. It is worse here, because **a wedged application is the
//! single most common reason a user reaches for the agent in the first place** —
//! the tree walk is precisely the thing that gets pointed at it.
//!
//! Windows' UIA limb carries the same guard (`WALK_BUDGET`) for the same reason.
//! This is its AT-SPI counterpart.
//!
//! # Partial beats empty
//!
//! When the budget runs out the walk **stops and returns what it has**, exactly
//! as it does when the node budget runs out. A half-read tree of a hung
//! application still tells the model which application it is and what was
//! reachable; an error tells it nothing it can act on.

use std::time::{Duration, Instant};

/// How long one accessibility call may spend talking to a foreign process.
///
/// Sized to be invisible on a healthy desktop (a real window's tree comes back
/// in tens of milliseconds) and short enough that a wedged one costs the model a
/// retry rather than the turn.
pub const AX_BUDGET: Duration = Duration::from_secs(5);

/// A deadline shared by every step of one accessibility call.
///
/// Copied rather than borrowed so the recursive walk, the candidate scan and the
/// focus search can each hold one without threading a lifetime through the
/// boxed futures they are made of.
#[derive(Debug, Clone, Copy)]
pub struct Budget {
    deadline: Instant,
}

impl Budget {
    /// Start a budget of [`AX_BUDGET`] from now.
    #[must_use]
    pub fn start() -> Self {
        Self::lasting(AX_BUDGET)
    }

    /// Start a budget of an explicit duration — for tests, which must not wait
    /// five seconds to prove the guard fires.
    #[must_use]
    pub fn lasting(d: Duration) -> Self {
        Self {
            deadline: Instant::now() + d,
        }
    }

    /// Has the budget run out?
    #[must_use]
    pub fn spent(&self) -> bool {
        Instant::now() >= self.deadline
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fresh_budget_is_not_spent() {
        assert!(!Budget::start().spent());
    }

    #[test]
    fn an_elapsed_budget_is_spent() {
        // Zero-length: already past its deadline by the time it is asked.
        assert!(Budget::lasting(Duration::ZERO).spent());
    }

    #[test]
    fn the_default_budget_is_the_documented_one() {
        // Pinned because the number is a contract with the model: the tool
        // description promises a bounded answer, and 5s is what "bounded" means
        // here (the same figure the Windows UIA walk uses).
        assert_eq!(AX_BUDGET, Duration::from_secs(5));
    }
}
