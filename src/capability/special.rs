//! Captured, asynchronous special-capability records.
//!
//! A registration owns the descriptor, canonical schema bytes, provider
//! generation, and handler as one immutable record. Captures retain that
//! record, but dispatch checks that the same record is still current; this is
//! the ABA/lifecycle boundary for special capabilities.

use crate::capability::descriptor::{CapabilityDescriptor, CapabilityId, CapabilityKind};
use crate::capability::facade::{CapabilitySnapshot, Cursor, Scope};
use crate::capability::policy_runtime::{CapabilityCallContext, RunningCapabilityCall};
use crate::capability::provider::ProviderHandle;
use crate::capability::ui_surface::UiSurface;
use crate::sync_primitives::Arc;
use async_trait::async_trait;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::RwLock;
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

/// Errors raised by a special capability handler.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CapabilityDispatchError {
    #[error("capability handler received invalid input: {0}")]
    InvalidInput(String),
    #[error("capability handler execution failed: {0}")]
    Execution(String),
    #[error("special capability source is unavailable")]
    Unavailable,
    #[error("capability is not registered: {0:?}")]
    NotFound(CapabilityId),
    #[error("capability handler call was cancelled")]
    Cancelled,
}

/// Trusted facts attached by the authenticated ingress and the policy runtime.
/// Fields are private so a wire payload cannot mint a call context.
pub struct CapabilityHandlerCall {
    context: CapabilityCallContext,
    running_calls: Vec<RunningCapabilityCall>,
    observation: Option<crate::capability::events::InvocationObservation>,
}

impl CapabilityHandlerCall {
    pub(crate) fn new(
        context: CapabilityCallContext,
        running_calls: Vec<RunningCapabilityCall>,
    ) -> Self {
        Self {
            context,
            running_calls,
            observation: None,
        }
    }

    pub(crate) fn with_observation(
        mut self,
        observation: Option<crate::capability::events::InvocationObservation>,
    ) -> Self {
        self.observation = observation;
        self
    }

    pub(crate) fn observation(&self) -> Option<crate::capability::events::InvocationObservation> {
        self.observation.clone()
    }

    #[must_use]
    pub fn context(&self) -> &CapabilityCallContext {
        &self.context
    }

    #[must_use]
    pub fn running_call_count(&self) -> usize {
        self.running_calls.len()
    }

    pub(crate) fn with_running_call(mut self, running: RunningCapabilityCall) -> Self {
        self.running_calls.push(running);
        self
    }
}

/// The asynchronous callable boundary for special capabilities.
#[async_trait]
pub trait CapabilityHandler: Send + Sync {
    async fn invoke(
        &self,
        input: Value,
        call: CapabilityHandlerCall,
        cancel: CancellationToken,
    ) -> Result<Value, CapabilityDispatchError>;
}

/// One late-binding weak handler slot used by pre-catalog adapters.
///
/// It carries no descriptor, owner, or session state. The registry remains the
/// source of truth; this slot only lets startup connect the already-published
/// canonical record to the real domain owner after the manager exists.
pub(crate) struct CapabilityHandlerSourceSlot {
    target: std::sync::OnceLock<std::sync::Weak<dyn CapabilityHandler>>,
}

impl CapabilityHandlerSourceSlot {
    fn new() -> Self {
        Self {
            target: std::sync::OnceLock::new(),
        }
    }

    pub(crate) fn bind(
        &self,
        target: std::sync::Weak<dyn CapabilityHandler>,
    ) -> Result<(), CapabilityDispatchError> {
        self.target
            .set(target)
            .map_err(|_| CapabilityDispatchError::Unavailable)
    }

    pub(crate) async fn invoke(
        &self,
        input: Value,
        call: CapabilityHandlerCall,
        cancel: CancellationToken,
    ) -> Result<Value, CapabilityDispatchError> {
        let target = self
            .target
            .get()
            .and_then(std::sync::Weak::upgrade)
            .ok_or(CapabilityDispatchError::Unavailable)?;
        target.invoke(input, call, cancel).await
    }
}

/// Immutable declaration and handler captured as a single record.
pub struct SpecialRegistration {
    descriptor: CapabilityDescriptor,
    schema: Value,
    handler: Arc<dyn CapabilityHandler>,
    provider: ProviderHandle,
    validate_schema: bool,
    ui_surface: Option<Arc<UiSurface>>,
    observation_owner: Option<CapabilityCallContext>,
    deferred_source_commit: bool,
}

impl SpecialRegistration {
    #[must_use]
    pub fn new(
        descriptor: CapabilityDescriptor,
        schema: Value,
        handler: Arc<dyn CapabilityHandler>,
        provider: ProviderHandle,
    ) -> Self {
        Self {
            descriptor,
            schema,
            handler,
            provider,
            validate_schema: true,
            ui_surface: None,
            observation_owner: None,
            deferred_source_commit: false,
        }
    }

    #[must_use]
    pub fn with_ui_surface(self, surface: Arc<UiSurface>) -> Self {
        Self {
            ui_surface: Some(surface),
            ..self
        }
    }

    pub(crate) fn with_deferred_source_commit(
        mut self,
        owner: Option<CapabilityCallContext>,
    ) -> Self {
        self.observation_owner = owner;
        self.deferred_source_commit = true;
        self
    }

    pub(crate) fn legacy(
        descriptor: CapabilityDescriptor,
        handler: Arc<dyn CapabilityHandler>,
        provider: ProviderHandle,
    ) -> Self {
        Self {
            descriptor,
            schema: Value::Null,
            handler,
            provider,
            validate_schema: false,
            ui_surface: None,
            observation_owner: None,
            deferred_source_commit: false,
        }
    }
}

/// Retirement belongs to this registration, not a newest-record lookup. The
/// mutex serializes installation with owner removal; it never owns a call guard.
#[derive(Default)]
struct TerminalPolicySource {
    retired: bool,
    binding: Option<(
        std::sync::Weak<super::policy::PolicyEngine>,
        super::policy_runtime::PolicyFactSourceKey,
    )>,
}

/// Immutable registry record retained by an admitted capture.
#[derive(Clone)]
pub struct CapturedSpecialEntry {
    descriptor: CapabilityDescriptor,
    schema: Value,
    canonical_schema: Arc<[u8]>,
    handler: Arc<dyn CapabilityHandler>,
    provider: ProviderHandle,
    provider_generation: u64,
    ui_surface: Option<Arc<UiSurface>>,
    observation: Option<Arc<crate::capability::events::SourceObservation>>,
    terminal_policy_source: Arc<std::sync::Mutex<TerminalPolicySource>>,
}

impl PartialEq for CapturedSpecialEntry {
    fn eq(&self, other: &Self) -> bool {
        self.descriptor == other.descriptor
            && self.canonical_schema == other.canonical_schema
            && self.provider_generation == other.provider_generation
            && self.provider.same_instance(&other.provider)
            && Arc::ptr_eq(&self.handler, &other.handler)
            && match (&self.ui_surface, &other.ui_surface) {
                (Some(left), Some(right)) => Arc::ptr_eq(left, right),
                (None, None) => true,
                _ => false,
            }
    }
}

impl Eq for CapturedSpecialEntry {}

impl std::fmt::Debug for CapturedSpecialEntry {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CapturedSpecialEntry")
            .field("descriptor", &self.descriptor)
            .field("provider_generation", &self.provider_generation)
            .finish_non_exhaustive()
    }
}

impl CapturedSpecialEntry {
    #[cfg(test)]
    pub(crate) fn terminal_policy_source_strong_count_for_test(&self) -> usize {
        Arc::strong_count(&self.terminal_policy_source)
    }

    pub(crate) fn with_terminal_policy_source<R>(
        &self,
        policy: &Arc<super::policy::PolicyEngine>,
        key: super::policy_runtime::PolicyFactSourceKey,
        install: impl FnOnce() -> Result<R, super::special_dispatch::CapabilityInvocationError>,
    ) -> Result<R, super::special_dispatch::CapabilityInvocationError> {
        use super::special_dispatch::CapabilityInvocationError;
        let mut source = self
            .terminal_policy_source
            .lock()
            .map_err(|_| CapabilityInvocationError::Unavailable)?;
        if source.retired {
            return Err(CapabilityInvocationError::Unavailable);
        }
        let weak = Arc::downgrade(policy);
        match &source.binding {
            Some((existing, existing_key)) if existing.ptr_eq(&weak) && existing_key == &key => {}
            Some(_) => return Err(CapabilityInvocationError::Unavailable),
            None => source.binding = Some((weak, key)),
        }
        // No registry/manager lookup or await while retirement is locked.
        // Both projected attachments must be installed before owner removal.
        install()
    }

    fn retire_terminal_policy_source(&self) {
        let mut source = self
            .terminal_policy_source
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        source.retired = true;
        if let Some((policy, key)) = source.binding.take() {
            if let Some(policy) = policy.upgrade() {
                policy.withdraw_fact_source(&key);
            }
        }
    }

    #[must_use]
    pub fn descriptor(&self) -> &CapabilityDescriptor {
        &self.descriptor
    }

    #[must_use]
    pub fn schema(&self) -> &Value {
        &self.schema
    }

    #[must_use]
    pub fn provider_generation(&self) -> u64 {
        self.provider_generation
    }

    pub(crate) fn validate_provider(&self) -> bool {
        self.provider.is_ready() && self.provider.generation() == self.provider_generation
    }

    pub(crate) fn provider_handle(&self) -> ProviderHandle {
        self.provider.clone()
    }

    pub(crate) fn ui_surface(&self) -> Option<Arc<super::ui_surface::UiSurface>> {
        self.ui_surface.clone()
    }

    pub(crate) fn source_observation(
        &self,
    ) -> Option<Arc<crate::capability::events::SourceObservation>> {
        self.observation.clone()
    }

    #[must_use]
    pub fn canonical_schema(&self) -> &[u8] {
        &self.canonical_schema
    }

    #[must_use]
    pub fn id(&self) -> &CapabilityId {
        &self.descriptor.id
    }

    fn matches_descriptor(&self, descriptor: &CapabilityDescriptor) -> bool {
        self.descriptor.id == descriptor.id
            && self.descriptor.revision == descriptor.revision
            && self.descriptor.schema == descriptor.schema
    }
}

struct RegistryEntry {
    capture: Arc<CapturedSpecialEntry>,
}

#[derive(Default)]
struct RegistryState {
    entries: HashMap<CapabilityId, RegistryEntry>,
    revision: u64,
    closed: bool,
}

/// Notification emitted after a special registry mutation commits. The host
/// treats this as a live invalidation and rebuilds its ordinary projection cut.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SpecialPublication {
    pub(crate) revision: u64,
}

/// An idempotent, incarnation-guarded special registration disposer.
#[derive(Clone)]
pub struct SpecialRegistrationHandle {
    state: std::sync::Weak<RwLock<RegistryState>>,
    changes: broadcast::Sender<SpecialPublication>,
    id: CapabilityId,
    revision: u64,
}

impl std::fmt::Debug for SpecialRegistrationHandle {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SpecialRegistrationHandle")
            .field("id", &self.id)
            .field("revision", &self.revision)
            .finish()
    }
}

impl SpecialRegistrationHandle {
    #[must_use]
    pub fn id(&self) -> &CapabilityId {
        &self.id
    }

    /// Remove only the exact special incarnation this handle created.
    #[must_use]
    pub fn dispose(&self) -> bool {
        let Some(state) = self.state.upgrade() else {
            return false;
        };
        let mut state = state
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(entry) = state.entries.get(&self.id) else {
            return false;
        };
        if entry.capture.descriptor.revision.0 != self.revision {
            return false;
        }
        let Some(revision) = state.revision.checked_add(1) else {
            return false;
        };
        let observation = state.entries.remove(&self.id).and_then(|entry| {
            entry.capture.retire_terminal_policy_source();
            entry.capture.observation.clone()
        });
        if let Some(observation) = &observation {
            observation.mark_withdrawn();
        }
        state.revision = revision;
        drop(state);
        let _ = self.changes.send(SpecialPublication { revision });
        if let Some(observation) = observation {
            observation.flush();
        }
        true
    }
}

/// One source of truth for special descriptors, schemas, provider generations,
/// and async handlers.
#[derive(Clone)]
pub struct CapabilityHandlerRegistry {
    state: Arc<RwLock<RegistryState>>,
    changes: broadcast::Sender<SpecialPublication>,
    terminal_source: Arc<CapabilityHandlerSourceSlot>,
    domain_facade: Arc<std::sync::OnceLock<std::sync::Weak<super::zahir_facade::ZahirFacade>>>,
    observations: Arc<std::sync::OnceLock<Arc<crate::capability::events::SourceEventFactory>>>,
}

impl Default for CapabilityHandlerRegistry {
    fn default() -> Self {
        let (changes, _) = broadcast::channel(16);
        Self {
            state: Arc::new(RwLock::new(RegistryState::default())),
            changes,
            terminal_source: Arc::new(CapabilityHandlerSourceSlot::new()),
            domain_facade: Arc::new(std::sync::OnceLock::new()),
            observations: Arc::new(std::sync::OnceLock::new()),
        }
    }
}

impl CapabilityHandlerRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub(crate) fn install_source_observer(
        &self,
        factory: Arc<crate::capability::events::SourceEventFactory>,
    ) {
        let _ = self.observations.set(factory);
    }

    pub(crate) fn with_source_observations<R>(
        &self,
        read: impl FnOnce(Vec<Arc<crate::capability::events::SourceObservation>>) -> R,
    ) -> R {
        let state = self
            .state
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        read(
            state
                .entries
                .values()
                .filter_map(|entry| entry.capture.observation.clone())
                .collect(),
        )
    }

    pub(crate) fn terminal_source_slot(&self) -> Arc<CapabilityHandlerSourceSlot> {
        Arc::clone(&self.terminal_source)
    }

    pub(crate) fn domain_facade_slot(
        &self,
    ) -> Arc<std::sync::OnceLock<std::sync::Weak<super::zahir_facade::ZahirFacade>>> {
        self.domain_facade.clone()
    }

    pub(crate) fn bind_domain_facade(
        &self,
        facade: &Arc<super::zahir_facade::ZahirFacade>,
    ) -> Result<(), CapabilityDispatchError> {
        let weak = Arc::downgrade(facade);
        if let Some(existing) = self.domain_facade.get() {
            return if existing.ptr_eq(&weak) {
                Ok(())
            } else {
                Err(CapabilityDispatchError::Unavailable)
            };
        }
        self.domain_facade
            .set(weak)
            .map_err(|_| CapabilityDispatchError::Unavailable)
    }

    pub fn register(
        &self,
        registration: SpecialRegistration,
    ) -> Result<CapabilityId, CapabilityRegistryError> {
        Ok(self.register_with_handle(registration)?.id)
    }

    /// Register a special record and retain an incarnation-safe disposer.
    pub fn register_with_handle(
        &self,
        registration: SpecialRegistration,
    ) -> Result<SpecialRegistrationHandle, CapabilityRegistryError> {
        let (result, running) = if let Some(factory) = self.observations.get() {
            factory
                .policy
                .with_source_publication(&registration.descriptor.id.clone(), || {
                    self.register_record(registration)
                })
                .map(|(result, running)| (result, Some(running)))
                .map_err(|_| CapabilityRegistryError::Closed)?
        } else {
            (self.register_record(registration), None)
        };
        let (handle, observation) = result?;
        if let Some(observation) = observation {
            observation.flush();
        }
        drop(running);
        Ok(handle)
    }

    fn register_record(
        &self,
        mut registration: SpecialRegistration,
    ) -> Result<
        (
            SpecialRegistrationHandle,
            Option<Arc<crate::capability::events::SourceObservation>>,
        ),
        CapabilityRegistryError,
    > {
        let canonical_schema = canonical_schema_bytes(&registration.schema);
        let mut fingerprint = [0_u8; 32];
        fingerprint.copy_from_slice(Sha256::digest(&canonical_schema).as_slice());
        if registration.validate_schema && registration.descriptor.schema.fingerprint != fingerprint
        {
            return Err(CapabilityRegistryError::InvalidSchema);
        }
        if registration.descriptor.id.namespace.trim().is_empty()
            || registration.descriptor.id.name.trim().is_empty()
        {
            return Err(CapabilityRegistryError::InvalidId);
        }
        let mut state = self
            .state
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.closed {
            return Err(CapabilityRegistryError::Closed);
        }
        if state.entries.contains_key(&registration.descriptor.id) {
            return Err(CapabilityRegistryError::Duplicate(
                registration.descriptor.id,
            ));
        }
        let revision = state
            .revision
            .checked_add(1)
            .ok_or(CapabilityRegistryError::RevisionExhausted)?;
        registration.descriptor.revision =
            crate::capability::descriptor::CapabilityRevision(revision);
        let id = registration.descriptor.id.clone();
        let provider_generation = registration
            .provider
            .with_snapshot(|provider| provider.generation());
        let observation = registration
            .validate_schema
            .then(|| self.observations.get())
            .flatten()
            .and_then(|factory| {
                let owner = registration.observation_owner.as_ref().or_else(|| {
                    registration
                        .ui_surface
                        .as_ref()
                        .map(|surface| surface.owner_context())
                });
                if let Some(owner) = owner {
                    factory.principal(registration.descriptor.clone(), owner)
                } else if matches!(
                    registration.descriptor.kind,
                    CapabilityKind::Terminal | CapabilityKind::Ui
                ) || registration.descriptor.visibility.principal.is_some()
                {
                    None
                } else {
                    Some(factory.system(registration.descriptor.clone()))
                }
            });
        if !registration.deferred_source_commit {
            if let Some(observation) = &observation {
                observation.mark_committed();
            }
        }
        let capture = CapturedSpecialEntry {
            descriptor: registration.descriptor,
            schema: registration.schema,
            canonical_schema: Arc::from(canonical_schema),
            handler: registration.handler,
            provider: registration.provider,
            provider_generation,
            ui_surface: registration.ui_surface,
            observation: observation.clone(),
            terminal_policy_source: Arc::new(
                std::sync::Mutex::new(TerminalPolicySource::default()),
            ),
        };
        state.entries.insert(
            id.clone(),
            RegistryEntry {
                capture: Arc::new(capture),
            },
        );
        state.revision = revision;
        drop(state);
        let _ = self.changes.send(SpecialPublication { revision });
        Ok((
            SpecialRegistrationHandle {
                state: Arc::downgrade(&self.state),
                changes: self.changes.clone(),
                id,
                revision,
            },
            observation,
        ))
    }

    /// Subscribe to live special-record publications. This stream is a
    /// notification only; callers rebuild from `snapshot_entries_in_scope`.
    pub(crate) fn subscribe(&self) -> broadcast::Receiver<SpecialPublication> {
        self.changes.subscribe()
    }

    #[must_use]
    pub fn capture(&self, id: &CapabilityId) -> Option<CapturedSpecialEntry> {
        let state = self
            .state
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.entries.get(id).map(|entry| CapturedSpecialEntry {
            descriptor: entry.capture.descriptor.clone(),
            schema: entry.capture.schema.clone(),
            canonical_schema: Arc::clone(&entry.capture.canonical_schema),
            handler: Arc::clone(&entry.capture.handler),
            provider: entry.capture.provider.clone(),
            provider_generation: entry.capture.provider_generation,
            ui_surface: entry.capture.ui_surface.clone(),
            observation: entry.capture.observation.clone(),
            terminal_policy_source: entry.capture.terminal_policy_source.clone(),
        })
    }

    /// Commit a physical source only while its immutable record and binding
    /// are current. Notifications remain deferred beyond the caller's locks.
    pub(crate) fn commit_source<R>(
        &self,
        id: &CapabilityId,
        tree: &crate::capability::ownership::OwnershipTree,
        commit: impl FnOnce() -> R,
    ) -> Result<
        (R, crate::capability::events::SourceObservationReceipt),
        crate::capability::facade::ResolveError,
    > {
        use crate::capability::facade::ResolveError;
        let publish = || {
            let state = self
                .state
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let capture = &state.entries.get(id).ok_or(ResolveError::Unknown)?.capture;
            let descriptor = &capture.descriptor;
            let generation = tree
                .generation(id, &descriptor.visibility)
                .ok_or(ResolveError::StaleOwner)?;
            tree.with_current_binding(
                id,
                &descriptor.visibility,
                generation,
                &descriptor.owner,
                &descriptor.lifetime,
                || {
                    let result = commit();
                    if let Some(observation) = &capture.observation {
                        observation.mark_committed();
                    }
                    (result, capture.observation.clone())
                },
            )
            .map_err(|_| ResolveError::StaleOwner)
        };
        let (result, running) = if let Some(factory) = self.observations.get() {
            let (result, running) = factory
                .policy
                .with_source_publication(id, publish)
                .map_err(|_| ResolveError::Revoked)?;
            (result, Some(running))
        } else {
            (publish(), None)
        };
        let (result, observation) = result?;
        Ok((
            result,
            crate::capability::events::SourceObservationReceipt::new(observation, running),
        ))
    }

    #[must_use]
    pub fn ui_surface(&self, id: &CapabilityId) -> Option<Arc<UiSurface>> {
        let state = self
            .state
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state
            .entries
            .get(id)
            .and_then(|entry| entry.capture.ui_surface.clone())
    }

    pub fn unregister(&self, id: &CapabilityId) -> Result<bool, CapabilityRegistryError> {
        let (removed, observation) = self.unregister_deferred(id)?;
        if let Some(observation) = observation {
            observation.flush();
        }
        Ok(removed)
    }

    pub(crate) fn unregister_deferred(
        &self,
        id: &CapabilityId,
    ) -> Result<
        (
            bool,
            Option<Arc<crate::capability::events::SourceObservation>>,
        ),
        CapabilityRegistryError,
    > {
        self.unregister_matching_deferred(id, None)
    }

    pub(crate) fn unregister_owned_deferred(
        &self,
        id: &CapabilityId,
        owner: &dyn CapabilityHandler,
    ) -> Result<
        (
            bool,
            Option<Arc<crate::capability::events::SourceObservation>>,
        ),
        CapabilityRegistryError,
    > {
        self.unregister_matching_deferred(id, Some(owner))
    }

    fn unregister_matching_deferred(
        &self,
        id: &CapabilityId,
        owner: Option<&dyn CapabilityHandler>,
    ) -> Result<
        (
            bool,
            Option<Arc<crate::capability::events::SourceObservation>>,
        ),
        CapabilityRegistryError,
    > {
        let mut state = self
            .state
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.closed {
            return Err(CapabilityRegistryError::Closed);
        }
        let Some(entry) = state.entries.get(id) else {
            return Ok((false, None));
        };
        if owner.is_some_and(|owner| {
            !std::ptr::addr_eq(
                Arc::as_ptr(&entry.capture.handler),
                owner as *const dyn CapabilityHandler,
            )
        }) {
            return Ok((false, None));
        }
        let revision = state
            .revision
            .checked_add(1)
            .ok_or(CapabilityRegistryError::RevisionExhausted)?;
        let observation = state.entries.remove(id).and_then(|entry| {
            entry.capture.retire_terminal_policy_source();
            entry.capture.observation.clone()
        });
        if let Some(observation) = &observation {
            observation.mark_withdrawn();
        }
        state.revision = revision;
        drop(state);
        let _ = self.changes.send(SpecialPublication { revision });
        Ok((true, observation))
    }

    /// Release all owned UI acknowledgement waits before global admission drain.
    /// Records remain current until the subsequent source close.
    pub async fn close_ui_surfaces(&self) -> Result<(), crate::capability::ui::UiError> {
        let surfaces: Vec<_> = self
            .state
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .entries
            .values()
            .filter_map(|entry| entry.capture.ui_surface.clone())
            .collect();
        let mut failure = None;
        for surface in surfaces {
            if let Err(error) = surface.close().await {
                if failure.is_none() {
                    failure = Some(error);
                }
            }
        }
        failure.map_or(Ok(()), Err)
    }

    pub fn close(&self) -> Result<(), CapabilityRegistryError> {
        let mut state = self
            .state
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.closed {
            return Ok(());
        }
        let next_revision = if state.entries.is_empty() {
            state.revision
        } else {
            state
                .revision
                .checked_add(1)
                .ok_or(CapabilityRegistryError::RevisionExhausted)?
        };
        state.closed = true;
        let observations: Vec<_> = state
            .entries
            .values()
            .filter_map(|entry| entry.capture.observation.clone())
            .collect();
        for observation in &observations {
            observation.mark_withdrawn();
        }
        for entry in state.entries.values() {
            entry.capture.retire_terminal_policy_source();
        }
        state.entries.clear();
        state.revision = next_revision;
        drop(state);
        let _ = self.changes.send(SpecialPublication {
            revision: next_revision,
        });
        for observation in observations {
            observation.flush();
        }
        Ok(())
    }

    #[must_use]
    pub fn revision(&self) -> u64 {
        self.state
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .revision
    }

    #[must_use]
    pub fn is_closed(&self) -> bool {
        self.state
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .closed
    }

    #[cfg(test)]
    pub(crate) fn set_revision_for_test(&self, revision: u64) {
        self.state
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .revision = revision;
    }

    #[must_use]
    pub fn describe(&self, scope: &Scope) -> Vec<CapturedSpecialEntry> {
        let state = self
            .state
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state
            .entries
            .values()
            .filter(|entry| {
                scope
                    .kind
                    .is_none_or(|kind| entry.capture.descriptor.kind == kind)
                    && scope
                        .namespace
                        .as_ref()
                        .is_none_or(|namespace| entry.capture.descriptor.id.namespace == *namespace)
            })
            .map(|entry| CapturedSpecialEntry {
                descriptor: entry.capture.descriptor.clone(),
                schema: entry.capture.schema.clone(),
                canonical_schema: Arc::clone(&entry.capture.canonical_schema),
                handler: Arc::clone(&entry.capture.handler),
                provider: entry.capture.provider.clone(),
                provider_generation: entry.capture.provider_generation,
                ui_surface: entry.capture.ui_surface.clone(),
                observation: entry.capture.observation.clone(),
                terminal_policy_source: entry.capture.terminal_policy_source.clone(),
            })
            .collect()
    }

    pub(crate) fn register_legacy(
        &self,
        descriptor: CapabilityDescriptor,
        handler: Arc<dyn CapabilityHandler>,
        provider: ProviderHandle,
    ) -> Result<CapabilityId, CapabilityRegistryError> {
        self.register(SpecialRegistration::legacy(descriptor, handler, provider))
    }

    /// Compatibility-only bridge for the old synchronous terminal/UI tests.
    /// It still resolves the handler from this registry; it does not own a
    /// second descriptor or handler map.
    pub(crate) fn dispatch_legacy(
        &self,
        id: &CapabilityId,
        input: Value,
    ) -> Result<Value, CapabilityDispatchError> {
        let capture = self
            .capture(id)
            .ok_or_else(|| CapabilityDispatchError::NotFound(id.clone()))?;
        let call = CapabilityHandlerCall::new(
            CapabilityCallContext::from_trusted_parts(
                crate::capability::ownership::OwnerRef::Runtime,
                None,
                None,
                None,
                None,
                "legacy-compat".into(),
            ),
            Vec::new(),
        );
        let registry = self.clone();
        let future = async move {
            registry
                .dispatch_admitted(AdmittedSpecialCall {
                    capture,
                    input,
                    call,
                    cancel: CancellationToken::new(),
                })
                .await
        };
        if tokio::runtime::Handle::try_current().is_ok() {
            std::thread::spawn(move || {
                tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .map_err(|error| CapabilityDispatchError::Execution(error.to_string()))
                    .and_then(|runtime| runtime.block_on(future))
            })
            .join()
            .map_err(|_| {
                CapabilityDispatchError::Execution("legacy dispatch thread panicked".into())
            })?
        } else {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|error| CapabilityDispatchError::Execution(error.to_string()))?
                .block_on(future)
        }
    }

    pub(crate) async fn dispatch_admitted(
        &self,
        call: AdmittedSpecialCall,
    ) -> Result<Value, CapabilityDispatchError> {
        let observation = call.call.observation();
        let result = self.dispatch_admitted_inner(call).await;
        if let Some(observation) = observation {
            use crate::capability::events::HandlerResult;
            observation.complete(match &result {
                Ok(_) => HandlerResult::Success,
                Err(CapabilityDispatchError::InvalidInput(_)) => HandlerResult::InvalidInput,
                Err(CapabilityDispatchError::NotFound(_)) => HandlerResult::NotFound,
                Err(_) => HandlerResult::ExecutionFailed,
            });
        }
        result
    }

    async fn dispatch_admitted_inner(
        &self,
        call: AdmittedSpecialCall,
    ) -> Result<Value, CapabilityDispatchError> {
        if call.cancel.is_cancelled() {
            return Err(CapabilityDispatchError::Cancelled);
        }
        let handler = {
            let state = self
                .state
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let current = state
                .entries
                .get(call.capture.id())
                .ok_or_else(|| CapabilityDispatchError::NotFound(call.capture.id().clone()))?;
            if !Arc::ptr_eq(&current.capture.handler, &call.capture.handler)
                || !current
                    .capture
                    .matches_descriptor(call.capture.descriptor())
            {
                return Err(CapabilityDispatchError::NotFound(call.capture.id().clone()));
            }
            Arc::clone(&current.capture.handler)
        };
        // Re-check immediately after releasing the registry lock and before the
        // user async handler is entered. No lock is held across user code.
        if call.cancel.is_cancelled() {
            return Err(CapabilityDispatchError::Cancelled);
        }
        handler.invoke(call.input, call.call, call.cancel).await
    }
}

impl crate::capability::backend::CapabilityBackend for CapabilityHandlerRegistry {
    fn lookup(&self, id: &CapabilityId) -> Option<CapabilityDescriptor> {
        self.capture(id).map(|capture| capture.descriptor.clone())
    }

    fn enumerate(&self, scope: &Scope) -> Vec<CapabilityDescriptor> {
        self.describe(scope)
            .into_iter()
            .map(|capture| capture.descriptor.clone())
            .collect()
    }

    fn snapshot(&self, scope: &Scope) -> CapabilitySnapshot {
        CapabilitySnapshot {
            capabilities: self.enumerate(scope),
            committed_cursor: Cursor(self.revision()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CapabilityRegistryError {
    #[error("capability is already registered: {0:?}")]
    Duplicate(CapabilityId),
    #[error("capability id must have a non-empty namespace and name")]
    InvalidId,
    #[error("capability revision is exhausted")]
    RevisionExhausted,
    #[error("special capability schema fingerprint does not match canonical schema")]
    InvalidSchema,
    #[error("special capability registry is closed")]
    Closed,
    #[error("canonical Tool registration failed: {0}")]
    ToolRegistration(String),
}

pub(crate) struct AdmittedSpecialCall {
    pub(crate) capture: CapturedSpecialEntry,
    pub(crate) input: Value,
    pub(crate) call: CapabilityHandlerCall,
    pub(crate) cancel: CancellationToken,
}

pub(crate) fn canonical_schema_bytes(schema: &Value) -> Vec<u8> {
    let mut bytes = Vec::new();
    crate::tools::descriptor::canonicalize_json(schema, &mut bytes);
    bytes
}
