//! Capability activation guarded by lifecycle and runtime constraints.
//!
//! `Attached -> Active` is the only transition guarded by constraints. Attach
//! remains a registration operation, and revocation remains an unconditional
//! safety path. Runtime validation re-checks both the lease and the current
//! constraints before an active capability is used.

use crate::capability::constraints::{ConstraintContext, ConstraintSet, ConstraintViolation};
use crate::capability::lifecycle::{CapabilityLifecycle, LifecycleError, LifecycleState};
use crate::capability::ownership::OwnerGeneration;
use std::time::Duration;

/// Errors returned while attaching, activating, validating, or revoking a capability.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ActivationError {
    #[error("lifecycle error: {0}")]
    Lifecycle(#[from] LifecycleError),
    #[error("activation constraint violated: {0}")]
    Constraint(#[from] ConstraintViolation),
    #[error("activation hook rejected the operation: {0}")]
    Hook(String),
}

/// Hooks around the activation lifecycle boundary.
///
/// A hook can reject activation before the lifecycle transition. The post-hook
/// is intentionally infallible because the capability is already active at
/// that point; callers should use runtime validation for subsequent failures.
pub trait ActivationHook {
    /// Run after lifecycle and constraint preconditions pass, before activation.
    fn before_activation(&self, _context: &ConstraintContext) -> Result<(), String> {
        Ok(())
    }

    /// Observe a successful `Attached -> Active` transition.
    fn after_activation(&self, _context: &ConstraintContext) {}

    /// Observe a successful revocation.
    fn after_revocation(&self) {}
}

struct NoopActivationHook;

impl ActivationHook for NoopActivationHook {}

/// Lifecycle owner for a capability's activation constraints.
#[derive(Debug, Clone)]
pub struct CapabilityActivation {
    lifecycle: CapabilityLifecycle,
    constraints: ConstraintSet,
}

impl CapabilityActivation {
    /// Create an activation controller in the lifecycle's `Created` state.
    pub fn new<C>(ttl: Option<Duration>, owner_generation: OwnerGeneration, constraints: C) -> Self
    where
        C: Into<ConstraintSet>,
    {
        Self {
            lifecycle: CapabilityLifecycle::new(ttl, owner_generation),
            constraints: constraints.into(),
        }
    }

    /// Wrap an existing lifecycle without changing its state or lease.
    #[must_use]
    pub fn from_lifecycle<C>(lifecycle: CapabilityLifecycle, constraints: C) -> Self
    where
        C: Into<ConstraintSet>,
    {
        Self {
            lifecycle,
            constraints: constraints.into(),
        }
    }

    #[must_use]
    pub const fn state(&self) -> LifecycleState {
        self.lifecycle.state()
    }

    #[must_use]
    pub const fn lifecycle(&self) -> &CapabilityLifecycle {
        &self.lifecycle
    }

    #[must_use]
    pub const fn constraints(&self) -> &ConstraintSet {
        &self.constraints
    }

    /// Attach the capability without evaluating activation constraints.
    pub fn attach(&mut self) -> Result<(), ActivationError> {
        self.lifecycle
            .transition_to(LifecycleState::Attached)
            .map_err(ActivationError::from)
    }

    /// Activate using the generation carried by this activation's lease.
    ///
    /// Call [`Self::activate_with_generation`] when the current owner
    /// generation comes from an external ownership registry.
    pub fn activate(&mut self, context: &ConstraintContext) -> Result<(), ActivationError> {
        self.activate_with_generation(context, self.lifecycle.owner_generation())
    }

    /// Activate after validating the attached lifecycle, lease, and constraints.
    pub fn activate_with_generation(
        &mut self,
        context: &ConstraintContext,
        current_generation: OwnerGeneration,
    ) -> Result<(), ActivationError> {
        self.activate_with_hook_and_generation(context, current_generation, &NoopActivationHook)
    }

    /// Activate with a lifecycle hook and an explicit current owner generation.
    pub fn activate_with_hook<H: ActivationHook>(
        &mut self,
        context: &ConstraintContext,
        hook: &H,
    ) -> Result<(), ActivationError> {
        self.activate_with_hook_and_generation(context, self.lifecycle.owner_generation(), hook)
    }

    fn activate_with_hook_and_generation<H: ActivationHook>(
        &mut self,
        context: &ConstraintContext,
        current_generation: OwnerGeneration,
        hook: &H,
    ) -> Result<(), ActivationError> {
        // Do not evaluate constraints for any state other than Attached. This
        // preserves the lifecycle's invalid-transition error and keeps Created
        // -> Attached and revocation independent from runtime restrictions.
        if self.lifecycle.state() != LifecycleState::Attached {
            self.lifecycle
                .transition_to(LifecycleState::Active)
                .map_err(ActivationError::from)?;
        }

        self.lifecycle
            .validate_activation(current_generation)
            .map_err(ActivationError::from)?;
        self.constraints
            .evaluate(context)
            .map_err(ActivationError::from)?;
        hook.before_activation(context)
            .map_err(ActivationError::Hook)?;
        self.lifecycle
            .transition_to(LifecycleState::Active)
            .map_err(ActivationError::from)?;
        hook.after_activation(context);
        Ok(())
    }

    /// Validate an active capability's lease and constraints before use.
    pub fn validate_runtime(
        &self,
        current_generation: OwnerGeneration,
        context: &ConstraintContext,
    ) -> Result<(), ActivationError> {
        self.lifecycle
            .validate_lease(current_generation)
            .map_err(ActivationError::from)?;
        self.constraints
            .evaluate(context)
            .map_err(ActivationError::from)
    }

    /// Alias for [`Self::validate_runtime`].
    pub fn enforce_runtime(
        &self,
        current_generation: OwnerGeneration,
        context: &ConstraintContext,
    ) -> Result<(), ActivationError> {
        self.validate_runtime(current_generation, context)
    }

    /// Begin detaching an active capability without evaluating constraints.
    pub fn detach(&mut self) -> Result<(), ActivationError> {
        self.lifecycle
            .transition_to(LifecycleState::Detaching)
            .map_err(ActivationError::from)
    }

    /// Revoke from any non-revoked lifecycle state.
    pub fn revoke(&mut self) -> Result<(), ActivationError> {
        self.lifecycle.revoke().map_err(ActivationError::from)
    }

    /// Revoke and notify a lifecycle hook. Constraints never block revocation.
    pub fn revoke_with_hook<H: ActivationHook>(&mut self, hook: &H) -> Result<(), ActivationError> {
        self.revoke()?;
        hook.after_revocation();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability::constraints::{Constraint, ConstraintContext, ConstraintViolation};
    use crate::capability::lifecycle::{LifecycleError, LifecycleState};
    use crate::capability::ownership::OwnerGeneration;
    use std::sync::Mutex;

    fn activation(constraints: Vec<Constraint>) -> CapabilityActivation {
        CapabilityActivation::new(None, OwnerGeneration(1), constraints)
    }

    fn attach(activation: &mut CapabilityActivation) {
        activation.attach().unwrap();
    }

    #[test]
    fn activation_with_valid_constraints_succeeds() {
        let mut activation = activation(vec![Constraint::time_window(9, 17).unwrap()]);
        attach(&mut activation);
        activation
            .activate(&ConstraintContext::at_hour(12))
            .unwrap();
        assert_eq!(activation.state(), LifecycleState::Active);
    }

    #[test]
    fn activation_with_violated_time_constraint_fails_without_transition() {
        let mut activation = activation(vec![Constraint::time_window(9, 17).unwrap()]);
        attach(&mut activation);
        let result = activation.activate(&ConstraintContext::at_hour(18));
        assert!(matches!(
            result,
            Err(ActivationError::Constraint(
                ConstraintViolation::TimeWindowClosed { .. }
            ))
        ));
        assert_eq!(activation.state(), LifecycleState::Attached);
    }

    #[test]
    fn activation_with_violated_resource_constraint_fails() {
        let mut activation = activation(vec![Constraint::resource_limit(2)]);
        attach(&mut activation);
        let context = ConstraintContext::at_hour(12).with_active_resources(2);
        assert!(matches!(
            activation.activate(&context),
            Err(ActivationError::Constraint(
                ConstraintViolation::ResourceLimitExceeded { .. }
            ))
        ));
        assert_eq!(activation.state(), LifecycleState::Attached);
    }

    #[test]
    fn activation_with_violated_conditional_constraint_fails() {
        let mut activation = activation(vec![Constraint::condition("network")]);
        attach(&mut activation);
        let context = ConstraintContext::at_hour(12);
        assert!(matches!(
            activation.activate(&context),
            Err(ActivationError::Constraint(
                ConstraintViolation::ConditionNotMet { .. }
            ))
        ));
    }

    #[test]
    fn constraints_are_evaluated_at_activation_time() {
        let mut activation = activation(vec![Constraint::time_window(9, 17).unwrap()]);
        attach(&mut activation);
        assert!(activation.activate(&ConstraintContext::at_hour(8)).is_err());
        activation.activate(&ConstraintContext::at_hour(9)).unwrap();
        assert_eq!(activation.state(), LifecycleState::Active);
    }

    #[test]
    fn activation_cannot_skip_created_to_attached() {
        let mut activation = activation(vec![Constraint::condition("ready")]);
        let result = activation.activate(&ConstraintContext::at_hour(12));
        assert!(matches!(
            result,
            Err(ActivationError::Lifecycle(
                LifecycleError::InvalidTransition {
                    from: LifecycleState::Created,
                    to: LifecycleState::Active
                }
            ))
        ));
        assert_eq!(activation.state(), LifecycleState::Created);
    }

    #[test]
    fn attachment_does_not_evaluate_constraints() {
        let mut activation = activation(vec![Constraint::condition("ready")]);
        activation.attach().unwrap();
        assert_eq!(activation.state(), LifecycleState::Attached);
    }

    #[test]
    fn revocation_does_not_evaluate_constraints() {
        let mut activation = activation(vec![Constraint::condition("ready")]);
        attach(&mut activation);
        activation.revoke().unwrap();
        assert_eq!(activation.state(), LifecycleState::Revoked);
    }

    #[test]
    fn revoked_activation_cannot_be_activated_even_with_valid_constraints() {
        let mut activation = activation(vec![]);
        activation.revoke().unwrap();
        let result = activation.activate(&ConstraintContext::at_hour(12));
        assert!(matches!(
            result,
            Err(ActivationError::Lifecycle(
                LifecycleError::InvalidTransition {
                    from: LifecycleState::Revoked,
                    to: LifecycleState::Active
                }
            ))
        ));
    }

    #[test]
    fn expired_lease_blocks_activation_before_constraints() {
        let mut activation = CapabilityActivation::new(
            Some(Duration::from_millis(1)),
            OwnerGeneration(1),
            vec![Constraint::condition("missing")],
        );
        attach(&mut activation);
        std::thread::sleep(Duration::from_millis(5));
        assert!(matches!(
            activation.activate(&ConstraintContext::at_hour(12)),
            Err(ActivationError::Lifecycle(LifecycleError::LeaseExpired))
        ));
        assert_eq!(activation.state(), LifecycleState::Attached);
    }

    #[test]
    fn runtime_validation_rechecks_constraints() {
        let mut activation = activation(vec![Constraint::resource_limit(2)]);
        attach(&mut activation);
        activation
            .activate(&ConstraintContext::at_hour(12))
            .unwrap();
        let context = ConstraintContext::at_hour(12).with_active_resources(2);
        assert!(matches!(
            activation.validate_runtime(OwnerGeneration(1), &context),
            Err(ActivationError::Constraint(
                ConstraintViolation::ResourceLimitExceeded { .. }
            ))
        ));
    }

    #[test]
    fn runtime_validation_checks_lease_before_constraints() {
        let mut activation = activation(vec![Constraint::condition("missing")]);
        attach(&mut activation);
        activation
            .activate(&ConstraintContext::at_hour(12).with_condition("missing", true))
            .unwrap();
        assert!(matches!(
            activation.validate_runtime(OwnerGeneration(2), &ConstraintContext::at_hour(12)),
            Err(ActivationError::Lifecycle(
                LifecycleError::GenerationMismatch { .. }
            ))
        ));
    }

    #[test]
    fn activation_hook_runs_around_lifecycle_transition() {
        let mut activation = activation(vec![]);
        attach(&mut activation);
        let hook = RecordingHook::default();
        activation
            .activate_with_hook(&ConstraintContext::at_hour(12), &hook)
            .unwrap();
        assert_eq!(hook.events(), vec!["before", "after"]);
    }

    #[test]
    fn failed_hook_keeps_activation_attached() {
        let mut activation = activation(vec![]);
        attach(&mut activation);
        let hook = RejectingHook;
        assert!(matches!(
            activation.activate_with_hook(&ConstraintContext::at_hour(12), &hook),
            Err(ActivationError::Hook(_))
        ));
        assert_eq!(activation.state(), LifecycleState::Attached);
    }

    #[derive(Default)]
    struct RecordingHook(Mutex<Vec<&'static str>>);

    impl ActivationHook for RecordingHook {
        fn before_activation(&self, _context: &ConstraintContext) -> Result<(), String> {
            self.0.lock().unwrap().push("before");
            Ok(())
        }

        fn after_activation(&self, _context: &ConstraintContext) {
            self.0.lock().unwrap().push("after");
        }
    }

    impl RecordingHook {
        fn events(&self) -> Vec<&'static str> {
            self.0.lock().unwrap().clone()
        }
    }

    struct RejectingHook;

    impl ActivationHook for RejectingHook {
        fn before_activation(&self, _context: &ConstraintContext) -> Result<(), String> {
            Err("hook rejected activation".to_string())
        }
    }
}
