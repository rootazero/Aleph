//! Startup rules and live, source-owned facts extend the existing Policy gates.
//! This map owns restrictions only, never descriptors, handlers, or ownership.

use crate::capability::activation::CapabilityActivationController;
use crate::capability::constraints::{ConstraintContext, ConstraintSet, ConstraintViolation};
use crate::capability::descriptor::{CapabilityId, CapabilityRevision};
use crate::capability::facade::{BackendLease, BackendSourceEvidence};
use crate::capability::lifecycle::LifecycleState;
use crate::capability::ownership::{OwnerGeneration, OwnerRef, OwnershipTree, VisibilityScope};
use crate::capability::policy::{CapabilityLease, PolicyDenial, PolicyEngine};
use crate::capability::provider::{ProviderHandle, ProviderId};
use crate::config::types::policies::capability_constraints::{
    compile_constraints, CapabilityConstraintsConfig, CompiledConstraintRule, ResourceUnit,
};
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

/// Facts captured by an authenticated ingress, not deserializable client claims.
#[derive(Debug, Clone)]
pub struct CapabilityCallContext {
    actor: OwnerRef,
    principal: Option<String>,
    session_key: Option<String>,
    workspace: Option<String>,
    connection_id: Option<String>,
    call_id: String,
}
impl CapabilityCallContext {
    pub(crate) fn from_trusted_parts(
        actor: OwnerRef,
        principal: Option<String>,
        session_key: Option<String>,
        workspace: Option<String>,
        connection_id: Option<String>,
        call_id: String,
    ) -> Self {
        Self {
            actor,
            principal,
            session_key,
            workspace,
            connection_id,
            call_id,
        }
    }
    pub fn actor(&self) -> &OwnerRef {
        &self.actor
    }
    pub fn principal(&self) -> Option<&str> {
        self.principal.as_deref()
    }
    pub fn session_key(&self) -> Option<&str> {
        self.session_key.as_deref()
    }
    pub fn workspace(&self) -> Option<&str> {
        self.workspace.as_deref()
    }
    pub fn connection_id(&self) -> Option<&str> {
        self.connection_id.as_deref()
    }
    pub fn call_id(&self) -> &str {
        &self.call_id
    }
}

/// Selected by the typed operation owner, never by client input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourceUse {
    Allocate,
    Existing,
    Release,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PolicyFactSourceKey {
    provider: ProviderId,
    generation: u64,
    target: CapabilityId,
    incarnation: u64,
}
impl PolicyFactSourceKey {
    pub(crate) fn for_source(
        provider: &ProviderHandle,
        target: &CapabilityId,
        incarnation: u64,
    ) -> Self {
        provider.with_snapshot(|source| Self {
            provider: source.provider_id().clone(),
            generation: source.generation(),
            target: target.clone(),
            incarnation,
        })
    }
}

/// The owner supplies current facts after approval and before effect admission.
pub trait ConstraintFactsSource: Send + Sync {
    fn source_key(&self) -> PolicyFactSourceKey;
    fn supported_conditions(&self) -> &'static [&'static str];
    fn supported_units(&self) -> &'static [ResourceUnit];
    fn snapshot(
        &self,
        call: &CapabilityCallContext,
        unit: Option<ResourceUnit>,
        other_active_calls: usize,
    ) -> Result<ConstraintContext, ConstraintViolation>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AttachmentBinding {
    source: BackendSourceEvidence,
    revision: CapabilityRevision,
    generation: OwnerGeneration,
    owner: OwnerRef,
    visibility: VisibilityScope,
}
impl AttachmentBinding {
    fn from_lease(lease: &BackendLease) -> Self {
        Self {
            source: lease.source.clone(),
            revision: lease.descriptor.revision,
            generation: lease.owner_generation,
            owner: lease.owner.clone(),
            visibility: lease.descriptor.visibility.clone(),
        }
    }
}
struct AttachmentState {
    binding: AttachmentBinding,
    fact_key: PolicyFactSourceKey,
    facts: Arc<dyn ConstraintFactsSource>,
    controller: Mutex<CapabilityActivationController>,
    active_calls: Arc<AtomicUsize>,
}

impl AttachmentState {
    fn new(
        binding: AttachmentBinding,
        fact_key: PolicyFactSourceKey,
        facts: Arc<dyn ConstraintFactsSource>,
        active_calls: Arc<AtomicUsize>,
    ) -> Result<Self, PolicyDenial> {
        // Evaluate compiled rules once in Policy; the existing controller owns lifecycle.
        // Otherwise Existing/Release would accidentally re-run the allocation predicate.
        let mut controller =
            CapabilityActivationController::new(None, binding.generation, ConstraintSet::default());
        controller
            .attach()
            .map_err(|_| denied("attachment_lifecycle"))?;
        Ok(Self {
            binding,
            fact_key,
            facts,
            controller: Mutex::new(controller),
            active_calls,
        })
    }

    fn revoke(&self) {
        // Only an already-revoked controller can reject revoke; teardown is idempotent.
        let _ = self
            .controller
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .revoke();
    }

    fn admit(
        &self,
        rule: &CompiledConstraintRule,
        context: &ConstraintContext,
        resource_use: ResourceUse,
    ) -> Result<RunningCapabilityCall, PolicyDenial> {
        match resource_use {
            ResourceUse::Allocate => rule.constraints.evaluate(context),
            ResourceUse::Existing | ResourceUse::Release => {
                rule.constraints.evaluate_non_allocating(context)
            }
        }
        .map_err(|_| denied("runtime_constraints"))?;
        let mut controller = self
            .controller
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if controller.state() == LifecycleState::Attached {
            controller
                .activate(context, self.binding.generation)
                .map_err(|_| denied("activation_lifecycle"))?;
        }
        controller
            .validate_runtime(self.binding.generation, context)
            .map_err(|_| denied("runtime_lifecycle"))?;
        self.active_calls.fetch_add(1, Ordering::SeqCst);
        Ok(RunningCapabilityCall::new(
            Some(Arc::clone(&self.active_calls)),
            None,
        ))
    }
}

/// Opaque restriction attachment; it grants no capability authority on its own.
pub struct PolicyAttachment {
    id: CapabilityId,
    state: Option<Arc<AttachmentState>>,
}

/// Hold until the actual operation finishes, including asynchronous cancellation.
/// Dropping a caller while its operation continues must not release this guard.
#[must_use = "retain the guard until the actual capability operation finishes"]
pub struct RunningCapabilityCall {
    reservation: Arc<CallReservation>,
}
struct CallReservation {
    active_calls: Option<Arc<AtomicUsize>>,
    drain: Option<Arc<AdmissionTracker>>,
}
impl RunningCapabilityCall {
    fn new(active_calls: Option<Arc<AtomicUsize>>, drain: Option<Arc<AdmissionTracker>>) -> Self {
        Self {
            reservation: Arc::new(CallReservation {
                active_calls,
                drain,
            }),
        }
    }
    pub(crate) fn retained(&self) -> Self {
        Self {
            reservation: Arc::clone(&self.reservation),
        }
    }
}
impl Drop for CallReservation {
    fn drop(&mut self) {
        if let Some(active_calls) = &self.active_calls {
            active_calls.fetch_sub(1, Ordering::SeqCst);
        }
        if let Some(drain) = &self.drain {
            if drain.active.fetch_sub(1, Ordering::SeqCst) == 1 {
                drain.changed.notify_waiters();
            }
        }
    }
}
#[derive(Default)]
struct AdmissionTracker {
    stopped: Mutex<bool>,
    active: AtomicUsize,
    changed: tokio::sync::Notify,
}
impl AdmissionTracker {
    fn reserve(self: &Arc<Self>, id: &CapabilityId) -> Result<RunningCapabilityCall, PolicyDenial> {
        self.active
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |active| {
                active.checked_add(1)
            })
            .map_err(|_| unavailable(id))?;
        Ok(RunningCapabilityCall::new(None, Some(Arc::clone(self))))
    }
}

#[derive(Default)]
pub(super) struct PolicyRuntime {
    rules: HashMap<CapabilityId, CompiledConstraintRule>,
    attachments: Mutex<HashMap<CapabilityId, Vec<Arc<AttachmentState>>>>,
    admissions: Arc<AdmissionTracker>,
    events: std::sync::OnceLock<crate::capability::events::CapabilityEventEmitter>,
}
impl PolicyRuntime {
    fn attachments(
        &self,
    ) -> std::sync::MutexGuard<'_, HashMap<CapabilityId, Vec<Arc<AttachmentState>>>> {
        self.attachments
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn install(
        &self,
        lease: &BackendLease,
        facts: Arc<dyn ConstraintFactsSource>,
        fact_key: PolicyFactSourceKey,
    ) -> Result<PolicyAttachment, PolicyDenial> {
        let id = &lease.descriptor.id;
        let binding = AttachmentBinding::from_lease(lease);
        let mut entries = self.attachments();
        let sources = entries.entry(id.clone()).or_default();
        if let Some(current) = sources
            .iter()
            .find(|entry| entry.fact_key == fact_key && entry.binding == binding)
        {
            return Ok(PolicyAttachment {
                id: id.clone(),
                state: Some(Arc::clone(current)),
            });
        }
        if sources
            .iter()
            .any(|entry| entry.fact_key == fact_key && entry.binding.revision > binding.revision)
        {
            return Err(unavailable(id));
        }
        // Same live source, newer capture: old operations still count until done.
        let active_calls = sources
            .iter()
            .find(|entry| entry.fact_key == fact_key)
            .map(|entry| Arc::clone(&entry.active_calls))
            .unwrap_or_default();
        sources.retain(|entry| {
            if entry.fact_key != fact_key {
                return true;
            }
            entry.revoke();
            false
        });
        let state = Arc::new(AttachmentState::new(
            binding,
            fact_key,
            facts,
            active_calls,
        )?);
        sources.push(Arc::clone(&state));
        Ok(PolicyAttachment {
            id: id.clone(),
            state: Some(state),
        })
    }
}
fn unavailable(id: &CapabilityId) -> PolicyDenial {
    PolicyDenial::AttachmentUnavailable { id: id.clone() }
}
fn denied(reason: &'static str) -> PolicyDenial {
    PolicyDenial::ConstraintDenied {
        reason: reason.into(),
    }
}
fn invalid(reason: &'static str) -> PolicyDenial {
    PolicyDenial::InvalidConstraints {
        reason: reason.into(),
    }
}

impl PolicyEngine {
    pub fn with_shared_tree(
        tree: Arc<OwnershipTree>,
        rules: CapabilityConstraintsConfig,
    ) -> Result<Self, PolicyDenial> {
        let rules = compile_constraints(&rules).map_err(invalid)?;
        Ok(Self {
            tree,
            runtime: PolicyRuntime {
                rules,
                ..PolicyRuntime::default()
            },
        })
    }

    pub fn is_configured(&self, id: &CapabilityId) -> bool {
        self.runtime.rules.contains_key(id)
    }

    pub fn resource_unit(&self, id: &CapabilityId) -> Option<ResourceUnit> {
        self.runtime
            .rules
            .get(id)
            .and_then(|rule| rule.resource_unit)
    }

    /// The captured lease is still checked by the existing ownership authority.
    fn validate_captured_lease(&self, lease: &BackendLease) -> Result<(), PolicyDenial> {
        self.validate_lease(&CapabilityLease {
            capability: lease.descriptor.id.clone(),
            visibility: lease.descriptor.visibility.clone(),
            generation: lease.owner_generation,
            owner: lease.owner.clone(),
            lifetime: lease.descriptor.lifetime,
        })
    }

    pub fn prepare_attachment(
        &self,
        lease: &BackendLease,
        facts: Arc<dyn ConstraintFactsSource>,
    ) -> Result<PolicyAttachment, PolicyDenial> {
        let id = &lease.descriptor.id;
        let Some(rule) = self.runtime.rules.get(id) else {
            return Ok(PolicyAttachment {
                id: id.clone(),
                state: None,
            });
        };
        if rule
            .required_conditions
            .iter()
            .any(|name| !facts.supported_conditions().contains(&name.as_str()))
        {
            return Err(invalid("unsupported_condition"));
        }
        if rule
            .resource_unit
            .is_some_and(|unit| !facts.supported_units().contains(&unit))
        {
            return Err(invalid("unsupported_resource_unit"));
        }
        let fact_key = facts.source_key();
        self.validate_captured_lease(lease)?;
        self.runtime.install(lease, facts, fact_key)
    }

    pub fn admit_runtime(
        &self,
        lease: &BackendLease,
        attachment: Option<&PolicyAttachment>,
        call: &CapabilityCallContext,
        resource_use: ResourceUse,
    ) -> Result<RunningCapabilityCall, PolicyDenial> {
        let result = self.admit_runtime_inner(lease, attachment, call, resource_use);
        if let Err(denial) = &result {
            self.observe_refusal(lease, call, denial);
        }
        result
    }

    fn admit_runtime_inner(
        &self,
        lease: &BackendLease,
        attachment: Option<&PolicyAttachment>,
        call: &CapabilityCallContext,
        resource_use: ResourceUse,
    ) -> Result<RunningCapabilityCall, PolicyDenial> {
        let id = &lease.descriptor.id;
        let Some(rule) = self.runtime.rules.get(id) else {
            return self.track_call(id, RunningCapabilityCall::new(None, None));
        };
        let attachment = attachment
            .filter(|attachment| &attachment.id == id)
            .ok_or_else(|| unavailable(id))?;
        let state = attachment.state.as_ref().ok_or_else(|| unavailable(id))?;
        if state.binding != AttachmentBinding::from_lease(lease) {
            return Err(unavailable(id));
        }
        // Never hold ownership/registry/restriction locks while invoking a reader.
        let other_active_calls = state.active_calls.load(Ordering::SeqCst);
        let context = state
            .facts
            .snapshot(call, rule.resource_unit, other_active_calls)
            .map_err(|_| denied("facts_unavailable"))?;
        if state.facts.source_key() != state.fact_key {
            return Err(unavailable(id));
        }
        self.validate_captured_lease(lease)?;
        // Actor attribution is supplied by the trusted ingress. Its original
        // authorization gates remain there; a capability owner is not a caller.
        let entries = self.runtime.attachments();
        if !entries
            .get(id)
            .is_some_and(|sources| sources.iter().any(|current| Arc::ptr_eq(current, state)))
        {
            return Err(unavailable(id));
        }
        let running = state.admit(rule, &context, resource_use)?;
        drop(entries);
        self.track_call(id, running)
    }

    pub(crate) fn install_event_observer(
        &self,
        emitter: crate::capability::events::CapabilityEventEmitter,
    ) {
        let _ = self.runtime.events.set(emitter);
    }

    pub(crate) fn invocation_observation(
        &self,
        lease: &BackendLease,
        call: &CapabilityCallContext,
        running: &RunningCapabilityCall,
    ) -> Option<crate::capability::events::InvocationObservation> {
        crate::capability::events::InvocationObservation::new(
            lease,
            call,
            self.runtime.events.get()?.clone(),
            running.retained(),
        )
    }

    pub(crate) fn observe_refusal(
        &self,
        lease: &BackendLease,
        call: &CapabilityCallContext,
        denial: &PolicyDenial,
    ) {
        use crate::capability::events::CapabilityEvent;
        let Some(emitter) = self.runtime.events.get() else {
            return;
        };
        let private = lease.descriptor.visibility.principal.is_some()
            || matches!(
                lease.descriptor.kind,
                crate::capability::descriptor::CapabilityKind::Terminal
                    | crate::capability::descriptor::CapabilityKind::Ui
            );
        if private && call.principal().is_none() {
            return;
        }
        let audience = crate::capability::events::invocation_audience(lease, call);
        let reason = match denial {
            PolicyDenial::ConstraintDenied { .. } => "constraint_denied",
            PolicyDenial::InvalidConstraints { .. } => "invalid_constraints",
            PolicyDenial::AttachmentUnavailable { .. } => "attachment_unavailable",
            PolicyDenial::ApprovalRequired { .. } => "approval_required",
            PolicyDenial::Revoked | PolicyDenial::Disposed => "source_withdrawn",
            _ => "policy_denied",
        };
        if matches!(denial, PolicyDenial::ConstraintDenied { .. }) {
            emitter.emit_scoped(
                call.actor().clone(),
                audience.clone(),
                CapabilityEvent::ConstraintViolation {
                    id: lease.descriptor.id.clone(),
                    constraint_type: "constraint".into(),
                },
            );
        }
        emitter.emit_scoped(
            call.actor().clone(),
            audience,
            CapabilityEvent::PolicyViolation {
                id: lease.descriptor.id.clone(),
                reason: reason.into(),
            },
        );
    }

    fn track_call(
        &self,
        id: &CapabilityId,
        mut running: RunningCapabilityCall,
    ) -> Result<RunningCapabilityCall, PolicyDenial> {
        let stopped = self
            .runtime
            .admissions
            .stopped
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if *stopped {
            return Err(unavailable(id));
        }
        self.runtime
            .admissions
            .active
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |active| {
                active.checked_add(1)
            })
            .map_err(|_| unavailable(id))?;
        Arc::get_mut(&mut running.reservation)
            .expect("new admission is not shared")
            .drain = Some(Arc::clone(&self.runtime.admissions));
        Ok(running)
    }

    /// Seal every invocation and source publication, independently of configured quotas.
    pub fn stop_admission(&self) {
        *self
            .runtime
            .admissions
            .stopped
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = true;
    }

    pub async fn wait_for_idle(&self) {
        loop {
            let changed = self.runtime.admissions.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if self.runtime.admissions.active.load(Ordering::SeqCst) == 0 {
                return;
            }
            changed.await;
        }
    }

    /// The existing source commit runs inside the STOP gate, never an event callback.
    /// The returned reservation keeps post-commit observation ahead of final hub close.
    pub(crate) fn with_source_publication<R>(
        &self,
        id: &CapabilityId,
        publish: impl FnOnce() -> R,
    ) -> Result<(R, RunningCapabilityCall), PolicyDenial> {
        let stopped = self
            .runtime
            .admissions
            .stopped
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if *stopped {
            return Err(unavailable(id));
        }
        let running = self.runtime.admissions.reserve(id)?;
        let result = publish();
        drop(stopped);
        Ok((result, running))
    }

    pub fn withdraw_attachment(&self, attachment: &PolicyAttachment) {
        let Some(state) = &attachment.state else {
            return;
        };
        let mut entries = self.runtime.attachments();
        if let Some(sources) = entries.get_mut(&attachment.id) {
            sources.retain(|current| {
                if !Arc::ptr_eq(current, state) {
                    return true;
                }
                current.revoke();
                false
            });
            if sources.is_empty() {
                entries.remove(&attachment.id);
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn restriction_source_keys_for_test(
        &self,
        id: &CapabilityId,
    ) -> Vec<PolicyFactSourceKey> {
        self.runtime
            .attachments()
            .get(id)
            .map(|sources| sources.iter().map(|state| state.fact_key.clone()).collect())
            .unwrap_or_default()
    }

    pub fn withdraw_fact_source(&self, key: &PolicyFactSourceKey) {
        self.runtime.attachments().retain(|_, sources| {
            sources.retain(|state| {
                if &state.fact_key != key {
                    return true;
                }
                state.revoke();
                false
            });
            !sources.is_empty()
        });
    }
}
