//! Unauthorized-request flood guard for WebSocket connections.
//!
//! A remote connection that has not passed the Gateway-token login wall may
//! only issue `connect`. A buggy or stale client stuck in an auto-retry loop
//! (e.g. a Panel still holding a rotated-out token) would otherwise hammer
//! the wall indefinitely — every rejected request costs a parse plus a reply
//! and keeps the socket open forever. Mirroring openclaw's
//! `unauthorized-flood-guard`, this counts login-wall rejections per
//! connection and asks the caller to close the socket once the budget is
//! spent. The guard never sees authorized traffic (the wall only fires for
//! non-operator roles), and a well-behaved unauthorized client that re-issues
//! `connect` with a fresh token authorizes before the budget runs out — so
//! only genuine retry floods are cut off.

/// Login-wall rejections tolerated per connection before it is closed.
/// Matches openclaw's default close threshold. The per-IP RPC rate limiter
/// still throttles the requests themselves; this bounds how long a
/// never-authorizing socket may keep being answered at all.
pub(super) const MAX_UNAUTHORIZED_STRIKES: u32 = 10;

/// Per-connection counter of login-wall rejections.
#[derive(Debug)]
pub(super) struct UnauthorizedFloodGuard {
    strikes: u32,
    max: u32,
}

impl UnauthorizedFloodGuard {
    /// Build a guard allowing `max` rejections before requesting a close.
    #[must_use]
    pub(super) const fn new(max: u32) -> Self {
        Self { strikes: 0, max }
    }

    /// Record one login-wall rejection. Returns `true` once the budget is
    /// exhausted and the connection should be closed.
    pub(super) fn record_rejection(&mut self) -> bool {
        self.strikes = self.strikes.saturating_add(1);
        self.strikes >= self.max
    }

    /// Rejections recorded so far (for the close log line).
    #[must_use]
    pub(super) const fn strikes(&self) -> u32 {
        self.strikes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stays_open_below_budget() {
        let mut g = UnauthorizedFloodGuard::new(3);
        assert!(!g.record_rejection());
        assert!(!g.record_rejection());
        assert_eq!(g.strikes(), 2);
    }

    #[test]
    fn requests_close_at_budget() {
        let mut g = UnauthorizedFloodGuard::new(3);
        g.record_rejection();
        g.record_rejection();
        assert!(g.record_rejection());
        assert_eq!(g.strikes(), 3);
    }

    #[test]
    fn keeps_requesting_close_past_budget_without_overflow() {
        let mut g = UnauthorizedFloodGuard::new(1);
        for _ in 0..1000 {
            assert!(g.record_rejection());
        }
        assert_eq!(g.strikes(), 1000);
    }

    #[test]
    fn default_budget_matches_openclaw() {
        assert_eq!(MAX_UNAUTHORIZED_STRIKES, 10);
    }
}
