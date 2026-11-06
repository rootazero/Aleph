//! Capability activation guarded by lifecycle and runtime constraints.
//!
//! `Attached -> Active` is the only transition guarded by constraints. Attach
//! remains a registration operation, and revocation remains an unconditional
//! safety path. Runtime validation re-checks both the lease and the current
//! constraints before an active capability is used.

use crate::capability::constraints::{ConstraintContext, ConstraintSet, ConstraintViolation};
use crate::capability::descriptor::CapabilityDescriptor;
use crate::capability::events::{ActivationEventHook, CapabilityEvent, CapabilityEventEmitter};
use crate::capability::lifecycle::{CapabilityLifecycle, LifecycleError, LifecycleState};
use crate::capability::ownership::OwnerGeneration;
use crate::sync_primitives::Arc;
use std::sync::Mutex;
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

/// Runtime activation contract used by dispatchers and capability owners.
pub trait CapabilityActivation: Send + Sync {
    /// Current lifecycle state.
    fn state(&self) -> LifecycleState;

    /// Attach without evaluating runtime restrictions.
    fn attach(&mut self) -> Result<(), ActivationError>;

    /// Activate with an explicit current owner generation.
    fn activate(
        &mut self,
        context: &ConstraintContext,
        current_generation: OwnerGeneration,
    ) -> Result<(), ActivationError>;

    /// Validate the lease and current runtime restrictions immediately before use.
    fn validate_runtime(
        &self,
        current_generation: OwnerGeneration,
        context: &ConstraintContext,
    ) -> Result<(), ActivationError>;

    /// Detach without evaluating runtime restrictions.
    fn detach(&mut self) -> Result<(), ActivationError>;

    /// Revoke without evaluating runtime restrictions.
    fn revoke(&mut self) -> Result<(), ActivationError>;
}

/// Lifecycle owner for a capability's activation constraints.
#[derive(Debug, Clone)]
pub struct CapabilityActivationController {
    lifecycle: Arc<Mutex<CapabilityLifecycle>>,
    constraints: ConstraintSet,
    events: Option<ActivationEventHook>,
}

impl CapabilityActivationController {
    /// Create an activation controller in the lifecycle's `Created` state.
    pub fn new<C>(ttl: Option<Duration>, owner_generation: OwnerGeneration, constraints: C) -> Self
    where
        C: Into<ConstraintSet>,
    {
        Self {
            lifecycle: Arc::new(Mutex::new(CapabilityLifecycle::new(ttl, owner_generation))),
            constraints: constraints.into(),
            events: None,
        }
    }

    /// Create a new, observed lifecycle. Existing constructors remain unobserved.
    /// The descriptor supplies audit labels, never authorization or generation authority.
    pub fn new_observed<C: Into<ConstraintSet>>(
        ttl: Option<Duration>,
        owner_generation: OwnerGeneration,
        constraints: C,
        descriptor: &CapabilityDescriptor,
        emitter: CapabilityEventEmitter,
    ) -> Self {
        let controller = Self::new(ttl, owner_generation, constraints);
        let events = ActivationEventHook::new(descriptor, controller.constraints.len(), emitter);
        Self {
            events: Some(events),
            ..controller
        }
    }

    /// Wrap an existing lifecycle without changing its state or lease.
    #[must_use]
    pub fn from_lifecycle<C>(lifecycle: CapabilityLifecycle, constraints: C) -> Self
    where
        C: Into<ConstraintSet>,
    {
        Self {
            lifecycle: Arc::new(Mutex::new(lifecycle)),
            constraints: constraints.into(),
            events: None,
        }
    }

    fn lifecycle_guard(&self) -> std::sync::MutexGuard<'_, CapabilityLifecycle> {
        self.lifecycle
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    #[must_use]
    pub fn state(&self) -> LifecycleState {
        self.lifecycle_guard().state()
    }

    #[must_use]
    pub fn lifecycle(&self) -> CapabilityLifecycle {
        self.lifecycle_guard().clone()
    }

    #[must_use]
    pub const fn constraints(&self) -> &ConstraintSet {
        &self.constraints
    }

    /// Attach the capability without evaluating activation constraints.
    pub fn attach(&mut self) -> Result<(), ActivationError> {
        let mut lifecycle = self.lifecycle_guard();
        lifecycle.transition_to(LifecycleState::Attached)?;
        if let Some(events) = &self.events {
            events.emitter.emit(
                events.actor.clone(),
                CapabilityEvent::CapabilityAttached {
                    id: events.id.clone(),
                },
            );
        }
        Ok(())
    }

    /// Activate after validating the attached lifecycle, lease, and constraints.
    ///
    /// The current generation is supplied by the ownership authority. It must
    /// not be derived from this activation's captured lease, otherwise an
    /// owner rotation could be mistaken for a still-valid activation.
    pub fn activate(
        &mut self,
        context: &ConstraintContext,
        current_generation: OwnerGeneration,
    ) -> Result<(), ActivationError> {
        self.activate_with_hook(context, current_generation, &NoopActivationHook)
    }

    /// Activate with a lifecycle hook and an explicit current owner generation.
    pub fn activate_with_hook<H: ActivationHook>(
        &mut self,
        context: &ConstraintContext,
        current_generation: OwnerGeneration,
        hook: &H,
    ) -> Result<(), ActivationError> {
        self.activate_with_hook_and_generation(context, current_generation, hook)
            .inspect_err(|error| self.observe_failure(error))
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
        let mut lifecycle = self.lifecycle_guard();
        if lifecycle.state() != LifecycleState::Attached {
            lifecycle
                .transition_to(LifecycleState::Active)
                .map_err(ActivationError::from)?;
        }

        lifecycle
            .validate_activation(current_generation)
            .map_err(ActivationError::from)?;
        self.constraints
            .evaluate(context)
            .map_err(ActivationError::from)?;
        hook.before_activation(context)
            .map_err(ActivationError::Hook)?;
        lifecycle
            .transition_to(LifecycleState::Active)
            .map_err(ActivationError::from)?;
        if let Some(events) = &self.events {
            events.after_activation(context);
        }
        hook.after_activation(context);
        Ok(())
    }

    /// Validate an active capability's lease and constraints before use.
    pub fn validate_runtime(
        &self,
        current_generation: OwnerGeneration,
        context: &ConstraintContext,
    ) -> Result<(), ActivationError> {
        self.lifecycle_guard()
            .validate_lease(current_generation)
            .map_err(ActivationError::from)
            .and_then(|()| {
                self.constraints
                    .evaluate(context)
                    .map_err(ActivationError::from)
            })
            .inspect_err(|error| self.observe_failure(error))
    }

    fn observe_failure(&self, error: &ActivationError) {
        let Some(events) = &self.events else {
            return;
        };
        let reason = match error {
            ActivationError::Constraint(violation) => {
                events.violation(violation);
                return;
            }
            ActivationError::Hook(_) => "activation_hook_rejected",
            ActivationError::Lifecycle(error) => match error {
                LifecycleError::LeaseExpired => "lease_expired",
                LifecycleError::GenerationMismatch { .. } => "generation_mismatch",
                LifecycleError::NotInvocable(_) => "not_invocable",
                LifecycleError::InvalidTransition { .. } => "invalid_transition",
            },
        };
        events.emitter.emit(
            events.actor.clone(),
            CapabilityEvent::PolicyViolation {
                id: events.id.clone(),
                reason: reason.into(),
            },
        );
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
        let mut lifecycle = self.lifecycle_guard();
        lifecycle.transition_to(LifecycleState::Detaching)?;
        if let Some(events) = &self.events {
            events.emitter.emit(
                events.actor.clone(),
                CapabilityEvent::CapabilityDetaching {
                    id: events.id.clone(),
                },
            );
        }
        Ok(())
    }

    /// Revoke from any non-revoked lifecycle state.
    pub fn revoke(&mut self) -> Result<(), ActivationError> {
        let mut lifecycle = self.lifecycle_guard();
        lifecycle.revoke()?;
        if let Some(events) = &self.events {
            events.after_revocation();
        }
        Ok(())
    }

    /// Revoke and notify a lifecycle hook. Constraints never block revocation.
    pub fn revoke_with_hook<H: ActivationHook>(&mut self, hook: &H) -> Result<(), ActivationError> {
        self.revoke()?;
        hook.after_revocation();
        Ok(())
    }
}

impl CapabilityActivation for CapabilityActivationController {
    fn state(&self) -> LifecycleState {
        CapabilityActivationController::state(self)
    }

    fn attach(&mut self) -> Result<(), ActivationError> {
        CapabilityActivationController::attach(self)
    }

    fn activate(
        &mut self,
        context: &ConstraintContext,
        current_generation: OwnerGeneration,
    ) -> Result<(), ActivationError> {
        CapabilityActivationController::activate(self, context, current_generation)
    }

    fn validate_runtime(
        &self,
        current_generation: OwnerGeneration,
        context: &ConstraintContext,
    ) -> Result<(), ActivationError> {
        CapabilityActivationController::validate_runtime(self, current_generation, context)
    }

    fn detach(&mut self) -> Result<(), ActivationError> {
        CapabilityActivationController::detach(self)
    }

    fn revoke(&mut self) -> Result<(), ActivationError> {
        CapabilityActivationController::revoke(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability::constraints::{Constraint, ConstraintContext, ConstraintViolation};
    use crate::capability::lifecycle::{LifecycleError, LifecycleState};
    use crate::capability::ownership::OwnerGeneration;
    use std::sync::Mutex;

    fn activation(constraints: Vec<Constraint>) -> CapabilityActivationController {
        CapabilityActivationController::new(None, OwnerGeneration(1), constraints)
    }

    fn attach(activation: &mut CapabilityActivationController) {
        activation.attach().unwrap();
    }

    #[test]
    fn cloned_controller_shares_lifecycle_revocation() {
        let mut original = activation(vec![]);
        original.attach().unwrap();
        let clone = original.clone();
        original.revoke().unwrap();
        assert_eq!(clone.state(), LifecycleState::Revoked);
    }

    #[test]
    fn activation_with_valid_constraints_succeeds() {
        let mut activation = activation(vec![Constraint::time_window(9, 17).unwrap()]);
        attach(&mut activation);
        activation
            .activate(&ConstraintContext::at_hour(12).unwrap(), OwnerGeneration(1))
            .unwrap();
        assert_eq!(activation.state(), LifecycleState::Active);
    }

    #[test]
    fn activation_with_violated_time_constraint_fails_without_transition() {
        let mut activation = activation(vec![Constraint::time_window(9, 17).unwrap()]);
        attach(&mut activation);
        let result =
            activation.activate(&ConstraintContext::at_hour(18).unwrap(), OwnerGeneration(1));
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
        let context = ConstraintContext::at_hour(12)
            .unwrap()
            .with_active_resources(2);
        assert!(matches!(
            activation.activate(&context, OwnerGeneration(1)),
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
        let context = ConstraintContext::at_hour(12).unwrap();
        assert!(matches!(
            activation.activate(&context, OwnerGeneration(1)),
            Err(ActivationError::Constraint(
                ConstraintViolation::ConditionNotMet { .. }
            ))
        ));
    }

    #[test]
    fn constraints_are_evaluated_at_activation_time() {
        let mut activation = activation(vec![Constraint::time_window(9, 17).unwrap()]);
        attach(&mut activation);
        assert!(activation
            .activate(&ConstraintContext::at_hour(8).unwrap(), OwnerGeneration(1))
            .is_err());
        activation
            .activate(&ConstraintContext::at_hour(9).unwrap(), OwnerGeneration(1))
            .unwrap();
        assert_eq!(activation.state(), LifecycleState::Active);
    }

    #[test]
    fn activation_cannot_skip_created_to_attached() {
        let mut activation = activation(vec![Constraint::condition("ready")]);
        let result =
            activation.activate(&ConstraintContext::at_hour(12).unwrap(), OwnerGeneration(1));
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
        let result =
            activation.activate(&ConstraintContext::at_hour(12).unwrap(), OwnerGeneration(1));
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
        let mut activation = CapabilityActivationController::new(
            Some(Duration::from_millis(1)),
            OwnerGeneration(1),
            vec![Constraint::condition("missing")],
        );
        attach(&mut activation);
        std::thread::sleep(Duration::from_millis(5));
        assert!(matches!(
            activation.activate(&ConstraintContext::at_hour(12).unwrap(), OwnerGeneration(1)),
            Err(ActivationError::Lifecycle(LifecycleError::LeaseExpired))
        ));
        assert_eq!(activation.state(), LifecycleState::Attached);
    }

    #[test]
    fn activation_rejects_stale_current_generation() {
        let mut activation = activation(vec![]);
        attach(&mut activation);
        assert!(matches!(
            activation.activate(&ConstraintContext::at_hour(12).unwrap(), OwnerGeneration(2)),
            Err(ActivationError::Lifecycle(
                LifecycleError::GenerationMismatch { .. }
            ))
        ));
        assert_eq!(activation.state(), LifecycleState::Attached);
    }

    #[test]
    fn detach_moves_active_capability_to_detaching() {
        let mut activation = activation(vec![]);
        attach(&mut activation);
        activation
            .activate(&ConstraintContext::at_hour(12).unwrap(), OwnerGeneration(1))
            .unwrap();
        activation.detach().unwrap();
        assert_eq!(activation.state(), LifecycleState::Detaching);
    }

    #[test]
    fn revoke_with_hook_notifies_without_constraint_evaluation() {
        let mut activation = activation(vec![Constraint::condition("never")]);
        attach(&mut activation);
        let hook = RecordingHook::default();
        activation.revoke_with_hook(&hook).unwrap();
        assert_eq!(activation.state(), LifecycleState::Revoked);
        assert_eq!(hook.events(), vec!["revoked"]);
    }

    #[test]
    fn runtime_validation_rechecks_constraints() {
        let mut activation = activation(vec![Constraint::resource_limit(2)]);
        attach(&mut activation);
        activation
            .activate(&ConstraintContext::at_hour(12).unwrap(), OwnerGeneration(1))
            .unwrap();
        let context = ConstraintContext::at_hour(12)
            .unwrap()
            .with_active_resources(2);
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
            .activate(
                &ConstraintContext::at_hour(12)
                    .unwrap()
                    .with_condition("missing", true),
                OwnerGeneration(1),
            )
            .unwrap();
        assert!(matches!(
            activation
                .validate_runtime(OwnerGeneration(2), &ConstraintContext::at_hour(12).unwrap()),
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
            .activate_with_hook(
                &ConstraintContext::at_hour(12).unwrap(),
                OwnerGeneration(1),
                &hook,
            )
            .unwrap();
        assert_eq!(hook.events(), vec!["before", "after"]);
    }

    #[test]
    fn failed_hook_keeps_activation_attached() {
        let mut activation = activation(vec![]);
        attach(&mut activation);
        let hook = RejectingHook;
        assert!(matches!(
            activation.activate_with_hook(
                &ConstraintContext::at_hour(12).unwrap(),
                OwnerGeneration(1),
                &hook
            ),
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

        fn after_revocation(&self) {
            self.0.lock().unwrap().push("revoked");
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
