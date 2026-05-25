//! One-time bootstrap nonces for cookie-handoff to local browsers.
//!
//! Mirrors the shape of [`crate::gateway::challenge::ChallengeManager`]
//! (dashmap-backed pending + used sets, TTL-bounded), but with a far
//! simpler purpose: handing a same-machine browser an authenticated
//! session cookie without ever showing the user a token.
//!
//! Threat model: see `docs/superpowers/plans/2026-05-25-gateway-auth-ux-phase2-bootstrap-nonce.md`.

use std::time::{Duration, Instant};

use dashmap::DashMap;
use uuid::Uuid;

/// Default nonce lifetime: long enough to launch a browser and complete
/// the redirect, short enough that a leaked URL is useless soon after.
pub const DEFAULT_NONCE_TTL: Duration = Duration::from_secs(60);

/// Default replay-guard retention: how long a consumed nonce stays in
/// the `used` set before pruning.
pub const DEFAULT_USED_RETENTION: Duration = Duration::from_secs(300);

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum BootstrapError {
    #[error("bootstrap nonce not found")]
    NonceNotFound,
    #[error("bootstrap nonce already consumed (replay)")]
    NonceReplay,
    #[error("bootstrap nonce expired")]
    NonceExpired,
}

struct PendingNonce {
    issued_at: Instant,
}

/// Thread-safe issuer + consumer of one-shot bootstrap nonces.
pub struct BootstrapNonceManager {
    pending: DashMap<String, PendingNonce>,
    used: DashMap<String, Instant>,
    ttl: Duration,
    used_retention: Duration,
}

impl Default for BootstrapNonceManager {
    fn default() -> Self {
        Self::new(DEFAULT_NONCE_TTL, DEFAULT_USED_RETENTION)
    }
}

impl BootstrapNonceManager {
    pub fn new(ttl: Duration, used_retention: Duration) -> Self {
        Self {
            pending: DashMap::new(),
            used: DashMap::new(),
            ttl,
            used_retention,
        }
    }

    pub fn ttl(&self) -> Duration {
        self.ttl
    }

    /// Issue a new nonce. Returns `(nonce, expires_in_secs)`.
    pub fn issue(&self) -> (String, u64) {
        self.prune();
        let nonce = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
        self.pending.insert(
            nonce.clone(),
            PendingNonce {
                issued_at: Instant::now(),
            },
        );
        (nonce, self.ttl.as_secs())
    }

    /// Consume a nonce. Returns `Ok(())` on success and atomically moves
    /// the nonce from `pending` to `used`; further attempts fail with
    /// `NonceReplay`.
    ///
    /// Only the `used` map is pruned here. We deliberately do NOT prune
    /// `pending` on consume — doing so would silently turn an expired
    /// nonce into a `NonceNotFound` result and rob the caller of the
    /// more accurate `NonceExpired` diagnostic. Stale pending entries
    /// are reaped on `issue()` instead.
    pub fn consume(&self, nonce: &str) -> Result<(), BootstrapError> {
        self.used
            .retain(|_, used_at| used_at.elapsed() <= self.used_retention);
        if self.used.contains_key(nonce) {
            return Err(BootstrapError::NonceReplay);
        }
        let entry = self
            .pending
            .remove(nonce)
            .ok_or(BootstrapError::NonceNotFound)?;
        if entry.1.issued_at.elapsed() > self.ttl {
            return Err(BootstrapError::NonceExpired);
        }
        self.used.insert(nonce.to_string(), Instant::now());
        Ok(())
    }

    fn prune(&self) {
        self.pending
            .retain(|_, p| p.issued_at.elapsed() <= self.ttl);
        self.used
            .retain(|_, used_at| used_at.elapsed() <= self.used_retention);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn issued_nonce_consumes_once() {
        let mgr = BootstrapNonceManager::default();
        let (n, ttl) = mgr.issue();
        assert!(n.len() >= 32, "expected long nonce");
        assert_eq!(ttl, 60);
        mgr.consume(&n).unwrap();
    }

    #[test]
    fn second_consume_is_replay() {
        let mgr = BootstrapNonceManager::default();
        let (n, _) = mgr.issue();
        mgr.consume(&n).unwrap();
        assert_eq!(mgr.consume(&n), Err(BootstrapError::NonceReplay));
    }

    #[test]
    fn unknown_nonce_is_not_found() {
        let mgr = BootstrapNonceManager::default();
        assert_eq!(
            mgr.consume("never-issued"),
            Err(BootstrapError::NonceNotFound)
        );
    }

    #[test]
    fn expired_nonce_is_rejected() {
        let mgr = BootstrapNonceManager::new(Duration::from_millis(10), Duration::from_secs(60));
        let (n, _) = mgr.issue();
        std::thread::sleep(Duration::from_millis(30));
        assert_eq!(mgr.consume(&n), Err(BootstrapError::NonceExpired));
    }

    #[test]
    fn nonces_are_unique() {
        let mgr = BootstrapNonceManager::default();
        let (a, _) = mgr.issue();
        let (b, _) = mgr.issue();
        assert_ne!(a, b);
    }
}
