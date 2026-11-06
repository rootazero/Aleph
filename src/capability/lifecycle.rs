//! Capability lifecycle management — state transitions, lease TTL, and
//! epoch-based revocation.
//!
//! This module implements the complete lifecycle from capability creation to
//! revocation. Every capability moves through defined states with validated
//! transitions, carries a lease with optional TTL, and can be revoked via
//! epoch advancement.

use crate::capability::ownership::OwnerGeneration;
use std::time::{Duration, Instant};

/// Lifecycle state of a capability binding.
///
/// State transitions are validated: not all transitions are legal. The state
/// machine enforces that capabilities move through the lifecycle in order,
/// preventing invalid state changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LifecycleState {
    /// Capability registered but not yet attached to an owner.
    Created,
    /// Capability attached to an owner, ready for activation.
    Attached,
    /// Capability is active and can be invoked/subscribed.
    Active,
    /// Capability is being detached from its owner.
    Detaching,
    /// Capability has been revoked and is no longer usable.
    Revoked,
}

impl LifecycleState {
    /// Whether this state allows the capability to be invoked.
    #[must_use]
    pub const fn is_invocable(self) -> bool {
        matches!(self, Self::Active)
    }

    /// Whether transition from `self` to `target` is valid.
    #[must_use]
    pub const fn can_transition_to(self, target: Self) -> bool {
        use LifecycleState::*;
        matches!(
            (self, target),
            (Created, Attached)
                | (Attached, Active)
                | (Active, Detaching)
                | (Detaching, Revoked)
                | (Active, Revoked) // direct revocation
                | (Attached, Revoked) // revoke before activation
                | (Created, Revoked) // revoke immediately after creation
        )
    }
}

/// A lease with optional time-to-live for capability access.
///
/// Leases track when they were issued and when they expire. An expired lease
/// cannot be used to invoke a capability. Leases without TTL never expire due
/// to time (but can still be revoked).
#[derive(Debug, Clone)]
pub struct Lease {
    /// When this lease was issued.
    issued_at: Instant,
    /// Time-to-live duration, if any.
    ttl: Option<Duration>,
    /// Owner generation this lease was minted for.
    owner_generation: OwnerGeneration,
}

impl Lease {
    /// Create a new lease with optional TTL.
    #[must_use]
    pub fn new(ttl: Option<Duration>, owner_generation: OwnerGeneration) -> Self {
        Self {
            issued_at: Instant::now(),
            ttl,
            owner_generation,
        }
    }

    /// Whether this lease has expired due to TTL.
    #[must_use]
    pub fn is_expired(&self) -> bool {
        self.ttl
            .map_or(false, |ttl| self.issued_at.elapsed() >= ttl)
    }

    /// Time remaining before expiration, if any.
    #[must_use]
    pub fn time_remaining(&self) -> Option<Duration> {
        self.ttl.and_then(|ttl| {
            let elapsed = self.issued_at.elapsed();
            ttl.checked_sub(elapsed)
        })
    }

    /// Owner generation this lease was minted for.
    #[must_use]
    pub const fn owner_generation(&self) -> OwnerGeneration {
        self.owner_generation
    }

    /// Renew this lease with a new TTL, keeping the same generation.
    ///
    /// Returns a new lease with the current time as `issued_at` and the
    /// provided TTL, preserving the owner generation.
    #[must_use]
    pub fn renew(&self, new_ttl: Option<Duration>) -> Self {
        Self {
            issued_at: Instant::now(),
            ttl: new_ttl,
            owner_generation: self.owner_generation,
        }
    }
}

/// Errors during lifecycle state transitions.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LifecycleError {
    #[error("invalid state transition from {from:?} to {to:?}")]
    InvalidTransition {
        from: LifecycleState,
        to: LifecycleState,
    },
    #[error("lease has expired")]
    LeaseExpired,
    #[error("capability is not in an invocable state: {0:?}")]
    NotInvocable(LifecycleState),
    #[error("owner generation mismatch: lease has {lease:?}, current is {current:?}")]
    GenerationMismatch {
        lease: OwnerGeneration,
        current: OwnerGeneration,
    },
}

/// Capability lifecycle tracker — state, lease, and epoch management.
///
/// Each capability binding has one lifecycle tracker. It enforces state
/// transition rules, manages lease expiration, and tracks owner generations
/// for epoch-based revocation.
#[derive(Debug, Clone)]
pub struct CapabilityLifecycle {
    state: LifecycleState,
    lease: Lease,
}

impl CapabilityLifecycle {
    /// Create a new lifecycle in the `Created` state.
    #[must_use]
    pub fn new(ttl: Option<Duration>, owner_generation: OwnerGeneration) -> Self {
        Self {
            state: LifecycleState::Created,
            lease: Lease::new(ttl, owner_generation),
        }
    }

    /// Current lifecycle state.
    #[must_use]
    pub const fn state(&self) -> LifecycleState {
        self.state
    }

    /// Transition to a new state, validating the transition.
    pub fn transition_to(&mut self, target: LifecycleState) -> Result<(), LifecycleError> {
        if !self.state.can_transition_to(target) {
            return Err(LifecycleError::InvalidTransition {
                from: self.state,
                to: target,
            });
        }
        self.state = target;
        Ok(())
    }

    /// Validate the lease before activating an attached capability.
    ///
    /// Activation is the one transition that needs lease validation while the
    /// lifecycle is not yet invocable. Keeping this check here prevents the
    /// activation layer from reaching into lease internals and makes the
    /// `Attached -> Active` boundary explicit.
    pub fn validate_activation(
        &self,
        current_generation: OwnerGeneration,
    ) -> Result<(), LifecycleError> {
        if self.state != LifecycleState::Attached {
            return Err(LifecycleError::NotInvocable(self.state));
        }
        if self.lease.is_expired() {
            return Err(LifecycleError::LeaseExpired);
        }
        if self.lease.owner_generation() != current_generation {
            return Err(LifecycleError::GenerationMismatch {
                lease: self.lease.owner_generation(),
                current: current_generation,
            });
        }
        Ok(())
    }

    /// Validate that this lifecycle's lease is still valid.
    ///
    /// Checks:
    /// 1. Lease has not expired due to TTL
    /// 2. Owner generation matches current generation
    /// 3. State is invocable
    pub fn validate_lease(
        &self,
        current_generation: OwnerGeneration,
    ) -> Result<(), LifecycleError> {
        if self.lease.is_expired() {
            return Err(LifecycleError::LeaseExpired);
        }
        if self.lease.owner_generation() != current_generation {
            return Err(LifecycleError::GenerationMismatch {
                lease: self.lease.owner_generation(),
                current: current_generation,
            });
        }
        if !self.state.is_invocable() {
            return Err(LifecycleError::NotInvocable(self.state));
        }
        Ok(())
    }

    /// Renew the lease with a new TTL.
    ///
    /// Only allowed in Active state. Keeps the same owner generation.
    pub fn renew_lease(&mut self, new_ttl: Option<Duration>) -> Result<(), LifecycleError> {
        if self.state != LifecycleState::Active {
            return Err(LifecycleError::NotInvocable(self.state));
        }
        self.lease = self.lease.renew(new_ttl);
        Ok(())
    }

    /// Revoke this capability by transitioning to Revoked state.
    ///
    /// Can revoke from any state except already Revoked.
    pub fn revoke(&mut self) -> Result<(), LifecycleError> {
        if self.state == LifecycleState::Revoked {
            return Err(LifecycleError::InvalidTransition {
                from: self.state,
                to: LifecycleState::Revoked,
            });
        }
        self.state = LifecycleState::Revoked;
        Ok(())
    }

    /// Time remaining on the lease, if any.
    #[must_use]
    pub fn time_remaining(&self) -> Option<Duration> {
        self.lease.time_remaining()
    }

    /// Owner generation of this lifecycle's lease.
    #[must_use]
    pub const fn owner_generation(&self) -> OwnerGeneration {
        self.lease.owner_generation()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // RED TEST 1: State transitions must be validated
    #[test]
    fn rejects_invalid_state_transitions() {
        let gen = OwnerGeneration(1);
        let mut lifecycle = CapabilityLifecycle::new(None, gen);

        // Cannot go directly from Created to Active (must go through Attached)
        let result = lifecycle.transition_to(LifecycleState::Active);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            LifecycleError::InvalidTransition {
                from: LifecycleState::Created,
                to: LifecycleState::Active
            }
        ));
    }

    // RED TEST 2: Valid transitions must succeed
    #[test]
    fn allows_valid_state_transitions() {
        let gen = OwnerGeneration(1);
        let mut lifecycle = CapabilityLifecycle::new(None, gen);

        // Created -> Attached -> Active
        assert!(lifecycle.transition_to(LifecycleState::Attached).is_ok());
        assert_eq!(lifecycle.state(), LifecycleState::Attached);

        assert!(lifecycle.transition_to(LifecycleState::Active).is_ok());
        assert_eq!(lifecycle.state(), LifecycleState::Active);
    }

    // RED TEST 3: Lease expiration must be detected
    #[test]
    fn detects_expired_leases() {
        let gen = OwnerGeneration(1);
        let ttl = Duration::from_millis(1);
        let mut lifecycle = CapabilityLifecycle::new(Some(ttl), gen);

        lifecycle.transition_to(LifecycleState::Attached).unwrap();
        lifecycle.transition_to(LifecycleState::Active).unwrap();

        // Wait for lease to expire
        std::thread::sleep(Duration::from_millis(5));

        let result = lifecycle.validate_lease(gen);
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), LifecycleError::LeaseExpired));
    }

    // RED TEST 4: Generation mismatch must be detected
    #[test]
    fn detects_generation_mismatch() {
        let gen1 = OwnerGeneration(1);
        let gen2 = OwnerGeneration(2);
        let mut lifecycle = CapabilityLifecycle::new(None, gen1);

        lifecycle.transition_to(LifecycleState::Attached).unwrap();
        lifecycle.transition_to(LifecycleState::Active).unwrap();

        // Validate with different generation
        let result = lifecycle.validate_lease(gen2);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            LifecycleError::GenerationMismatch { .. }
        ));
    }

    // RED TEST 5: Non-invocable states must fail validation
    #[test]
    fn rejects_validation_in_non_invocable_states() {
        let gen = OwnerGeneration(1);
        let lifecycle = CapabilityLifecycle::new(None, gen);

        // Created state is not invocable
        let result = lifecycle.validate_lease(gen);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            LifecycleError::NotInvocable(LifecycleState::Created)
        ));
    }

    // RED TEST 6: Lease renewal must preserve generation
    #[test]
    fn lease_renewal_preserves_generation() {
        let gen = OwnerGeneration(1);
        let mut lifecycle = CapabilityLifecycle::new(Some(Duration::from_secs(10)), gen);

        lifecycle.transition_to(LifecycleState::Attached).unwrap();
        lifecycle.transition_to(LifecycleState::Active).unwrap();

        let new_ttl = Some(Duration::from_secs(20));
        lifecycle.renew_lease(new_ttl).unwrap();

        assert_eq!(lifecycle.owner_generation(), gen);
        assert!(lifecycle.time_remaining().unwrap() > Duration::from_secs(15));
    }

    // RED TEST 7: Revocation must work from any state
    #[test]
    fn revocation_works_from_any_state() {
        let gen = OwnerGeneration(1);

        // Revoke from Created
        let mut lifecycle = CapabilityLifecycle::new(None, gen);
        assert!(lifecycle.revoke().is_ok());
        assert_eq!(lifecycle.state(), LifecycleState::Revoked);

        // Revoke from Attached
        let mut lifecycle = CapabilityLifecycle::new(None, gen);
        lifecycle.transition_to(LifecycleState::Attached).unwrap();
        assert!(lifecycle.revoke().is_ok());
        assert_eq!(lifecycle.state(), LifecycleState::Revoked);

        // Revoke from Active
        let mut lifecycle = CapabilityLifecycle::new(None, gen);
        lifecycle.transition_to(LifecycleState::Attached).unwrap();
        lifecycle.transition_to(LifecycleState::Active).unwrap();
        assert!(lifecycle.revoke().is_ok());
        assert_eq!(lifecycle.state(), LifecycleState::Revoked);
    }

    // RED TEST 8: Cannot revoke twice
    #[test]
    fn cannot_revoke_twice() {
        let gen = OwnerGeneration(1);
        let mut lifecycle = CapabilityLifecycle::new(None, gen);

        lifecycle.revoke().unwrap();
        let result = lifecycle.revoke();
        assert!(result.is_err());
    }

    // RED TEST 9: Lease without TTL never expires
    #[test]
    fn lease_without_ttl_never_expires() {
        let gen = OwnerGeneration(1);
        let lease = Lease::new(None, gen);

        std::thread::sleep(Duration::from_millis(5));

        assert!(!lease.is_expired());
        assert!(lease.time_remaining().is_none());
    }

    // RED TEST 10: Time remaining calculation is accurate
    #[test]
    fn time_remaining_is_accurate() {
        let gen = OwnerGeneration(1);
        let ttl = Duration::from_secs(10);
        let lease = Lease::new(Some(ttl), gen);

        let remaining = lease.time_remaining().unwrap();
        assert!(remaining <= ttl);
        assert!(remaining >= Duration::from_secs(9)); // Allow small drift
    }
}
