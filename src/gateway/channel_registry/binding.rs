//! Source-private cold channel declarations; no executable provider admission.

use super::{ChannelHandle, ChannelRegistrationEntry};
use crate::acp::manager::SessionKey;
use crate::capability::descriptor::{
    CapabilityDescriptor, CapabilityFacet, CapabilityKind, ProjectionMetadata, SchemaRef,
    ServiceRelation,
};
use crate::capability::ownership::{
    BindingValidationError, LifetimeScope, OwnerGeneration, OwnerRef, VisibilityScope,
};
use crate::capability::provider::{ProviderCallError, ProviderHandle, ProviderLifecycleError};
use crate::sync_primitives::Arc;
use crate::tools::descriptor::canonicalize_json;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

#[allow(dead_code)] // Cold declaration schema is not mounted in production yet.
pub(crate) struct ChannelLifecycleDeclarationV1;

impl ChannelLifecycleDeclarationV1 {
    #[allow(dead_code)] // Used by the contract constructor pending production opt-in.
    pub(crate) fn schema() -> Value {
        json!({
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "$id": "aleph:gateway:channel-lifecycle-declaration:v1",
            "type": "object",
            "additionalProperties": false,
            "required": ["declaration_kind", "channel_id", "provider_transport", "lifecycle_model_version"],
            "properties": {
                "declaration_kind": {"const": "channel-provider-lifecycle"},
                "channel_id": {"type": "string", "minLength": 1},
                "provider_transport": {"const": "in_process"},
                "lifecycle_model_version": {"const": 1}
            }
        })
    }
}

pub(crate) struct ChannelLifecycleContract {
    pub(super) owner: OwnerRef,
    pub(super) lifetime: LifetimeScope,
    pub(super) visibility: VisibilityScope,
    #[allow(dead_code)] // Retained for later source lifecycle validation.
    pub(super) lifecycle_schema: Value,
    pub(super) schema: SchemaRef,
    pub(super) metadata: ProjectionMetadata,
}

impl ChannelLifecycleContract {
    #[allow(dead_code)] // Validated cold registration is not mounted in production yet.
    pub(crate) fn new(
        owner: OwnerRef,
        lifetime: LifetimeScope,
        visibility: VisibilityScope,
        lifecycle_schema: Value,
        metadata: ProjectionMetadata,
    ) -> Result<Self, ChannelBindingError> {
        let mut canonical = Vec::new();
        canonicalize_json(&lifecycle_schema, &mut canonical);
        let mut expected = Vec::new();
        canonicalize_json(&ChannelLifecycleDeclarationV1::schema(), &mut expected);
        if canonical != expected {
            return Err(ChannelBindingError::InvalidContract);
        }
        Ok(Self {
            owner,
            lifetime,
            visibility,
            schema: SchemaRef {
                version: 1,
                fingerprint: canonical_schema_fingerprint(&lifecycle_schema),
            },
            lifecycle_schema,
            metadata,
        })
    }
}

#[derive(Clone, Copy)]
#[allow(dead_code)] // Cold registration callers arrive with later lifecycle tasks.
pub(crate) enum ChannelInitialState {
    Discovered,
    Cold,
    Disabled,
}

#[derive(Clone)]
#[allow(dead_code)] // Capture fields are consumed by later source admission validation.
pub(crate) struct BoundChannelCapture {
    pub(super) source: Arc<()>,
    pub(super) entry: Arc<ChannelRegistrationEntry>,
    pub(super) channel: ChannelHandle,
    pub(super) source_revision: u64,
    pub(super) entry_revision: u64,
    pub(super) descriptor: CapabilityDescriptor,
    pub(super) descriptor_fingerprint: [u8; 32],
    pub(super) provider: ProviderHandle,
    pub(super) provider_generation: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)] // Runtime start/stop error reporting arrives with lifecycle execution.
pub(crate) enum ChannelRuntimePhase {
    Start,
    Stop,
}

/// Diagnostic only, never authority to admit work or dispose a transport.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ChannelCleanupResidual {
    pub(super) active_work: usize,
    pub(super) unjoined_forwarder: bool,
    pub(super) transport_may_be_live: bool,
    pub(super) mount_retained: bool,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum ChannelBindingError {
    #[error("channel not found")]
    NotFound,
    #[error("legacy channel has no provider binding")]
    UnboundLegacy,
    #[error("unsupported channel lifecycle declaration")]
    InvalidContract,
    #[allow(dead_code)] // Emitted by later source provenance validation.
    #[error("wrong channel source")]
    WrongSource,
    #[allow(dead_code)] // Emitted by later provider identity validation.
    #[error("wrong channel provider")]
    WrongProvider,
    #[allow(dead_code)] // Emitted by later scope validation.
    #[error("wrong channel visibility")]
    WrongVisibility,
    #[allow(dead_code)] // Emitted by later entry identity validation.
    #[error("stale channel entry")]
    StaleEntry,
    #[allow(dead_code)] // Emitted by later descriptor validation.
    #[error("stale channel descriptor")]
    StaleDescriptor,
    #[allow(dead_code)] // Emitted by later lifecycle retirement admission.
    #[error("channel is retiring")]
    Retiring,
    #[error("channel publication is busy")]
    Busy,
    #[error("channel source publication revision exhausted")]
    PublicationExhausted,
    #[error(transparent)]
    Provider(#[from] ProviderCallError),
    #[error(transparent)]
    ProviderLifecycle(#[from] ProviderLifecycleError),
    #[error(transparent)]
    Owner(#[from] BindingValidationError),
    #[allow(dead_code)] // Emitted by later start/stop execution and cleanup.
    #[error("channel runtime {phase:?}: {error}; residual: {residual:?}")]
    Runtime {
        phase: ChannelRuntimePhase,
        error: String,
        residual: ChannelCleanupResidual,
    },
}

pub(crate) fn canonical_schema_fingerprint(schema: &Value) -> [u8; 32] {
    let mut bytes = Vec::new();
    canonicalize_json(schema, &mut bytes);
    Sha256::digest(bytes).into()
}

/// Explicit wire tags, arrays and nulls preserve all descriptor distinctions.
pub(crate) fn canonical_descriptor_fingerprint(descriptor: &CapabilityDescriptor) -> [u8; 32] {
    let kind = match descriptor.kind {
        CapabilityKind::Tool => "tool",
        CapabilityKind::Skill => "skill",
        CapabilityKind::Agent => "agent",
        CapabilityKind::Task => "task",
        CapabilityKind::Resource => "resource",
        CapabilityKind::EventSource => "event_source",
        CapabilityKind::Subscription => "subscription",
        CapabilityKind::Plugin => "plugin",
        CapabilityKind::Hook => "hook",
    };
    let lifetime = match descriptor.lifetime {
        LifetimeScope::Runtime => "runtime",
        LifetimeScope::Session => "session",
        LifetimeScope::Run => "run",
        LifetimeScope::Task => "task",
        LifetimeScope::External => "external",
    };
    let owner = match &descriptor.owner {
        OwnerRef::Runtime => json!(["runtime"]),
        OwnerRef::Session(session) => json!(["session", session_value(session)]),
        OwnerRef::Run(run) => json!(["run", run.0]),
        OwnerRef::Task(task) => json!(["task", task.0]),
    };
    let services: Vec<_> = descriptor.services.iter().map(|service| {
        let relation = match service.relation {
            ServiceRelation::Requires => "requires",
            ServiceRelation::Provides => "provides",
            ServiceRelation::Conflicts => "conflicts",
        };
        json!({"relation": relation, "target": {"namespace": service.target.namespace, "name": service.target.name}})
    }).collect();
    let facets: Vec<_> = [
        (CapabilityFacet::Callable, "callable"),
        (CapabilityFacet::EventSource, "event_source"),
        (CapabilityFacet::Subscription, "subscription"),
        (CapabilityFacet::ProviderLifecycle, "provider_lifecycle"),
        (CapabilityFacet::ExternalEffect, "external_effect"),
    ]
    .into_iter()
    .filter_map(|(facet, tag)| descriptor.supports(facet).then_some(tag))
    .collect();
    canonical_schema_fingerprint(&json!({
        "id": {"namespace": descriptor.id.namespace, "name": descriptor.id.name},
        "kind": kind,
        "schema": {"version": descriptor.schema.version, "fingerprint": descriptor.schema.fingerprint},
        "revision": descriptor.revision.0,
        "owner": owner,
        "lifetime": lifetime,
        "visibility": {
            "principal": descriptor.visibility.principal,
            "workspace": descriptor.visibility.workspace,
            "session": descriptor.visibility.session.as_ref().map(session_value),
            "allowed_kinds": descriptor.visibility.allowed_kinds,
            "allowed_ids": descriptor.visibility.allowed_ids,
            "approval": descriptor.visibility.approval,
        },
        "services": services,
        "metadata": {"surface": descriptor.metadata.surface},
        "facets": facets,
    }))
}

fn session_value(session: &SessionKey) -> Value {
    // Preserve path identity without lossy display strings or filesystem I/O.
    json!({"harness_id": session.harness_id(), "cwd": session.cwd().as_os_str().as_encoded_bytes(), "name": session.name()})
}

/// Source-owned, bounded in-process bookkeeping, not a discovery catalog.
#[derive(Default)]
pub(super) struct EntryRuntime {
    pub(super) operation: Option<Arc<()>>,
    pub(super) driver: Option<tokio::task::JoinHandle<()>>,
    pub(super) active_work: usize,
    pub(super) executions: Vec<tokio::task::JoinHandle<()>>,
    pub(super) forwarder: Option<OwnedForwarder>,
    pub(super) residual: ChannelCleanupResidual,
}

pub(super) struct OwnedForwarder {
    pub(super) cancel: tokio_util::sync::CancellationToken,
    pub(super) task: tokio::task::JoinHandle<()>,
}

#[derive(Clone)]
pub(crate) struct BoundObservation {
    pub(super) capture: BoundChannelCapture,
    pub(super) owner_generation: OwnerGeneration,
    pub(super) visibility: VisibilityScope,
}

/// Only admission constructs this; Drop deliberately cannot report drain.
struct AdmittedWork {
    observation: BoundObservation,
}

#[derive(Debug)]
#[allow(dead_code)] // Completion payloads are consumed by later Gateway callers and tests.
pub(crate) enum BoundCompletion {
    Accepted(crate::gateway::channel::ChannelResult<crate::gateway::channel::SendResult>),
    #[allow(dead_code)] // The stale reason is consumed by diagnostics/tests in later slices.
    DiscardedStale(ChannelBindingError),
}

type LifecycleResult = tokio::sync::oneshot::Receiver<Result<(), ChannelBindingError>>;

#[allow(dead_code)] // Source-private vertical slice; no production opt-in yet.
impl super::ChannelRegistry {
    fn validate_bound_capture(
        &self,
        capture: &BoundChannelCapture,
        current: &Arc<ChannelRegistrationEntry>,
    ) -> Result<(), ChannelBindingError> {
        let super::ChannelRegistration::Bound(bound) = &current.registration else {
            return Err(ChannelBindingError::UnboundLegacy);
        };
        if !Arc::ptr_eq(&self.source, &capture.source) {
            return Err(ChannelBindingError::WrongSource);
        }
        if !Arc::ptr_eq(current, &capture.entry) || !Arc::ptr_eq(&current.channel, &capture.channel)
        {
            return Err(ChannelBindingError::StaleEntry);
        }
        if !bound.provider.same_instance(&capture.provider) {
            return Err(ChannelBindingError::WrongProvider);
        }
        if bound.entry_revision != capture.entry_revision
            || bound.descriptor_fingerprint != capture.descriptor_fingerprint
            || canonical_descriptor_fingerprint(&capture.descriptor) != bound.descriptor_fingerprint
        {
            return Err(ChannelBindingError::StaleDescriptor);
        }
        Ok(())
    }

    pub(crate) async fn resolve_bound_channel(
        &self,
        capture: BoundChannelCapture,
        visibility: &VisibilityScope,
    ) -> Result<BoundObservation, ChannelBindingError> {
        let _source = self.lifecycle_sync.lock().await;
        let entry = self
            .channels
            .read()
            .await
            .get(&super::ChannelId::new(&capture.descriptor.id.name))
            .cloned()
            .ok_or(ChannelBindingError::NotFound)?;
        self.validate_bound_capture(&capture, &entry)?;
        let super::ChannelRegistration::Bound(bound) = &entry.registration else {
            unreachable!()
        };
        if visibility != &bound.descriptor.visibility {
            return Err(ChannelBindingError::WrongVisibility);
        }
        let owner_generation = bound
            .ownership
            .generation(&bound.descriptor.id, visibility)
            .ok_or(BindingValidationError::MissingBinding)?;
        bound.ownership.with_current_binding(
            &bound.descriptor.id,
            visibility,
            owner_generation,
            &bound.contract.owner,
            &bound.contract.lifetime,
            || {
                bound
                    .provider
                    .with_call_admission(capture.provider_generation, || ())
            },
        )??;
        Ok(BoundObservation {
            capture,
            owner_generation,
            visibility: visibility.clone(),
        })
    }

    /// The ticket and owned task are committed without an intervening await.
    pub(crate) async fn send_bound_once(
        self: &Arc<Self>,
        observation: BoundObservation,
        message: super::OutboundMessage,
    ) -> Result<tokio::sync::oneshot::Receiver<BoundCompletion>, ChannelBindingError> {
        use futures::FutureExt;
        let _source = self.lifecycle_sync.lock().await;
        let entry = self
            .channels
            .read()
            .await
            .get(&super::ChannelId::new(
                &observation.capture.descriptor.id.name,
            ))
            .cloned()
            .ok_or(ChannelBindingError::NotFound)?;
        self.validate_bound_capture(&observation.capture, &entry)?;
        let super::ChannelRegistration::Bound(bound) = &entry.registration else {
            unreachable!()
        };
        if observation.visibility != bound.descriptor.visibility {
            return Err(ChannelBindingError::WrongVisibility);
        }
        bound.ownership.with_current_binding(
            &bound.descriptor.id,
            &observation.visibility,
            observation.owner_generation,
            &bound.contract.owner,
            &bound.contract.lifetime,
            || {
                bound.provider.with_call_admission(
                    observation.capture.provider_generation,
                    || {
                        let mut runtime = entry
                            .runtime
                            .lock()
                            .expect("channel runtime mutex poisoned");
                        // Join completed tasks before reusing bounded execution slots.
                        runtime
                            .executions
                            .retain_mut(|task| match (&mut *task).now_or_never() {
                                Some(Ok(())) => false,
                                Some(Err(_)) => true,
                                None => true,
                            });
                        if runtime.executions.len() >= 32 {
                            return Err(ChannelBindingError::Busy);
                        }
                        let (sender, receiver) = tokio::sync::oneshot::channel();
                        let ticket = AdmittedWork {
                            observation: observation.clone(),
                        };
                        runtime.active_work += 1;
                        let source = self.clone();
                        runtime.executions.push(tokio::spawn(async move {
                            let result = {
                                let channel = ticket.observation.capture.channel.read().await;
                                channel.send(message).await
                            };
                            source.complete_bound_work(ticket, result, sender).await;
                        }));
                        Ok(receiver)
                    },
                )?
            },
        )?
    }

    async fn complete_bound_work(
        &self,
        ticket: AdmittedWork,
        result: super::ChannelResult<super::SendResult>,
        sender: tokio::sync::oneshot::Sender<BoundCompletion>,
    ) {
        let _source = self.lifecycle_sync.lock().await;
        let observation = &ticket.observation;
        let original = &observation.capture.entry;
        let current = self
            .channels
            .read()
            .await
            .get(&super::ChannelId::new(
                &observation.capture.descriptor.id.name,
            ))
            .cloned();
        let validation = current
            .as_ref()
            .ok_or(ChannelBindingError::NotFound)
            .and_then(|current| self.validate_bound_capture(&observation.capture, current));
        let mut handoff = Some((result, sender));
        let accepted = validation.and_then(|()| {
            let super::ChannelRegistration::Bound(bound) = &original.registration else {
                unreachable!()
            };
            bound
                .ownership
                .with_current_binding(
                    &bound.descriptor.id,
                    &observation.visibility,
                    observation.owner_generation,
                    &bound.contract.owner,
                    &bound.contract.lifetime,
                    || {
                        bound.provider.with_admitted_completion(
                            observation.capture.provider_generation,
                            || {
                                let mut runtime = original
                                    .runtime
                                    .lock()
                                    .expect("channel runtime mutex poisoned");
                                runtime.active_work = runtime
                                    .active_work
                                    .checked_sub(1)
                                    .expect("live original ticket");
                                let (result, sender) =
                                    handoff.take().expect("one completion handoff");
                                let _ = sender.send(BoundCompletion::Accepted(result));
                            },
                        )
                    },
                )?
                .map_err(ChannelBindingError::from)
        });
        if let Err(error) = accepted {
            let mut runtime = original
                .runtime
                .lock()
                .expect("channel runtime mutex poisoned");
            runtime.active_work = runtime
                .active_work
                .checked_sub(1)
                .expect("live original ticket");
            let (_, sender) = handoff
                .take()
                .expect("stale completion has not been accepted");
            let _ = sender.send(BoundCompletion::DiscardedStale(error));
        }
        original.drained.notify_waiters();
    }

    pub(crate) async fn start_bound_channel(
        self: &Arc<Self>,
        id: &super::ChannelId,
    ) -> Result<LifecycleResult, ChannelBindingError> {
        use futures::FutureExt;
        let _source = self.lifecycle_sync.lock().await;
        let entry = self
            .channels
            .read()
            .await
            .get(id)
            .cloned()
            .ok_or(ChannelBindingError::NotFound)?;
        let super::ChannelRegistration::Bound(bound) = &entry.registration else {
            return Err(ChannelBindingError::UnboundLegacy);
        };
        let mut runtime = entry
            .runtime
            .lock()
            .expect("channel runtime mutex poisoned");
        if let Some(driver) = &mut runtime.driver {
            match driver.now_or_never() {
                Some(Ok(())) => runtime.driver = None,
                Some(Err(_)) => return Err(ChannelBindingError::Busy),
                None => return Err(ChannelBindingError::Busy),
            }
        }
        if runtime.operation.is_some()
            || runtime.driver.is_some()
            || runtime.active_work != 0
            || runtime.forwarder.is_some()
        {
            return Err(ChannelBindingError::Busy);
        }
        let generation = bound
            .provider
            .transition(crate::capability::provider::ProviderState::Starting)?
            .generation;
        let marker = Arc::new(());
        runtime.operation = Some(marker.clone());
        runtime.residual.transport_may_be_live = true;
        let (sender, receiver) = tokio::sync::oneshot::channel();
        let source = self.clone();
        let original = entry.clone();
        let id = id.clone();
        runtime.driver = Some(tokio::spawn(async move {
            let result = source
                .activate_bound(&id, &original, &marker, generation)
                .await;
            let _ = sender.send(result);
        }));
        Ok(receiver)
    }

    async fn activate_bound(
        &self,
        id: &super::ChannelId,
        entry: &Arc<ChannelRegistrationEntry>,
        marker: &Arc<()>,
        generation: u64,
    ) -> Result<(), ChannelBindingError> {
        use crate::capability::provider::ProviderState;
        let setup = {
            let mut channel = entry.channel.write().await;
            match channel.start().await {
                Ok(()) => Ok((
                    channel.inbound_subscribe(),
                    channel
                        .webhook_handler()
                        .map(|handler| handler.path().to_owned()),
                )),
                Err(error) => Err(error.to_string()),
            }
        };
        let super::ChannelRegistration::Bound(bound) = &entry.registration else {
            unreachable!()
        };
        let (mut inbound, path) = match setup {
            Ok(setup) => setup,
            Err(error) => {
                let _source = self.lifecycle_sync.lock().await;
                bound.provider.transition(ProviderState::Failed)?;
                return Err(ChannelBindingError::Runtime {
                    phase: ChannelRuntimePhase::Start,
                    error,
                    residual: entry.runtime.lock().unwrap().residual.clone(),
                });
            }
        };
        // No Bound ingress is mounted in Stage A. Invalid paths fail setup;
        // raw handlers and inbound events never reach Legacy consumers.
        if path
            .as_ref()
            .is_some_and(|path| !path.starts_with("/webhook/"))
        {
            let _source = self.lifecycle_sync.lock().await;
            bound.provider.transition(ProviderState::Failed)?;
            return Err(ChannelBindingError::Runtime {
                phase: ChannelRuntimePhase::Start,
                error: "invalid webhook mount path".into(),
                residual: entry.runtime.lock().unwrap().residual.clone(),
            });
        }
        let cancel = tokio_util::sync::CancellationToken::new();
        let cancelled = cancel.clone();
        let task = tokio::spawn(async move {
            loop {
                tokio::select! {
                    biased;
                    _ = cancelled.cancelled() => break,
                    result = inbound.recv() => if matches!(result, Err(tokio::sync::broadcast::error::RecvError::Closed)) { break; },
                }
            }
        });
        let _source = self.lifecycle_sync.lock().await;
        let current = self
            .channels
            .read()
            .await
            .get(id)
            .cloned()
            .ok_or(ChannelBindingError::NotFound)?;
        let mut desired = self.desired_state.write().await;
        bound.provider.with_ready_commit(generation, |_| {
            let mut runtime = entry
                .runtime
                .lock()
                .expect("channel runtime mutex poisoned");
            if !Arc::ptr_eq(&current, entry)
                || !runtime
                    .operation
                    .as_ref()
                    .is_some_and(|active| Arc::ptr_eq(active, marker))
            {
                return Err(ChannelBindingError::StaleEntry);
            }
            runtime.forwarder = Some(OwnedForwarder { cancel, task });
            runtime.residual.unjoined_forwarder = true;
            runtime.operation = None;
            // The current driver ends without any more I/O; retain its handle
            // until stop joins it (or a later management call observes it).
            desired.insert(id.clone(), super::DesiredChannelState::Running);
            Ok(())
        })
    }

    pub(crate) async fn stop_bound_channel(
        self: &Arc<Self>,
        id: &super::ChannelId,
    ) -> Result<LifecycleResult, ChannelBindingError> {
        use crate::capability::provider::ProviderState;
        use futures::FutureExt;
        let _source = self.lifecycle_sync.lock().await;
        let entry = self
            .channels
            .read()
            .await
            .get(id)
            .cloned()
            .ok_or(ChannelBindingError::NotFound)?;
        let super::ChannelRegistration::Bound(bound) = &entry.registration else {
            return Err(ChannelBindingError::UnboundLegacy);
        };
        let mut desired = self.desired_state.write().await;
        let mut runtime = entry
            .runtime
            .lock()
            .expect("channel runtime mutex poisoned");
        if let Some(driver) = &mut runtime.driver {
            match driver.now_or_never() {
                Some(Ok(())) => {
                    runtime.driver = None;
                }
                Some(Err(_)) => {
                    return Err(ChannelBindingError::Busy);
                }
                None => return Err(ChannelBindingError::Busy),
            }
        }
        // A failed, joined setup driver may be cleaned explicitly. A live
        // driver is never overlapped or inferred ended from observer Drop.
        if runtime.operation.is_some() && bound.provider.state() != ProviderState::Failed {
            return Err(ChannelBindingError::Busy);
        }
        let state = bound.provider.state();
        if matches!(state, ProviderState::Ready | ProviderState::Starting) {
            bound.provider.transition(ProviderState::Stopping)?;
        }
        let marker = Arc::new(());
        runtime.operation = Some(marker.clone());
        desired.insert(id.clone(), super::DesiredChannelState::Stopped);
        let (sender, receiver) = tokio::sync::oneshot::channel();
        let source = self.clone();
        let original = entry.clone();
        runtime.driver = Some(tokio::spawn(async move {
            let result = source.drain_bound(&original, &marker).await;
            let _ = sender.send(result);
        }));
        Ok(receiver)
    }

    async fn drain_bound(
        &self,
        entry: &Arc<ChannelRegistrationEntry>,
        marker: &Arc<()>,
    ) -> Result<(), ChannelBindingError> {
        use crate::capability::provider::ProviderState;
        loop {
            let notified = entry.drained.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if entry
                .runtime
                .lock()
                .expect("channel runtime mutex poisoned")
                .active_work
                == 0
            {
                break;
            }
            notified.await;
        }
        let (executions, forwarder) = {
            let mut runtime = entry
                .runtime
                .lock()
                .expect("channel runtime mutex poisoned");
            (
                std::mem::take(&mut runtime.executions),
                runtime.forwarder.take(),
            )
        };
        for task in executions {
            if let Err(error) = task.await {
                return Err(self.bound_cleanup_error(entry, error.to_string()));
            }
        }
        if let Some(forwarder) = forwarder {
            forwarder.cancel.cancel();
            if let Err(error) = forwarder.task.await {
                return Err(self.bound_cleanup_error(entry, error.to_string()));
            }
            entry.runtime.lock().unwrap().residual.unjoined_forwarder = false;
        }
        let super::ChannelRegistration::Bound(bound) = &entry.registration else {
            unreachable!()
        };
        if !matches!(
            bound.provider.state(),
            ProviderState::Cold | ProviderState::Discovered | ProviderState::Disabled
        ) {
            if let Err(error) = entry.channel.write().await.stop().await {
                return Err(self.bound_cleanup_error(entry, error.to_string()));
            }
        }
        let _source = self.lifecycle_sync.lock().await;
        let mut runtime = entry
            .runtime
            .lock()
            .expect("channel runtime mutex poisoned");
        if !runtime
            .operation
            .as_ref()
            .is_some_and(|active| Arc::ptr_eq(active, marker))
        {
            return Err(ChannelBindingError::Busy);
        }
        match bound.provider.state() {
            ProviderState::Stopping | ProviderState::Failed => {
                bound.provider.transition(ProviderState::Stopped)?;
            }
            _ => {}
        }
        runtime.residual = ChannelCleanupResidual::default();
        runtime.operation = None;
        Ok(())
    }

    fn bound_cleanup_error(
        &self,
        entry: &ChannelRegistrationEntry,
        error: String,
    ) -> ChannelBindingError {
        ChannelBindingError::Runtime {
            phase: ChannelRuntimePhase::Stop,
            error,
            residual: entry
                .runtime
                .lock()
                .expect("channel runtime mutex poisoned")
                .residual
                .clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::acp::manager::SessionKey;
    use crate::capability::descriptor::{
        CapabilityDescriptor, CapabilityFacet, CapabilityFacets, CapabilityId, CapabilityKind,
        CapabilityRevision, ProjectionMetadata, SchemaRef, Service, ServiceRelation,
    };
    use crate::capability::ownership::{LifetimeScope, OwnerRef, RunId, TaskId, VisibilityScope};
    use serde_json::json;

    fn contract(
        schema: serde_json::Value,
    ) -> Result<ChannelLifecycleContract, ChannelBindingError> {
        ChannelLifecycleContract::new(
            OwnerRef::Runtime,
            LifetimeScope::Runtime,
            VisibilityScope::default(),
            schema,
            ProjectionMetadata::default(),
        )
    }

    #[test]
    fn lifecycle_contract_rejects_placeholder_and_operation_schema() {
        for schema in [
            json!({}),
            json!(true),
            json!({"type":"object"}),
            json!({"type":"object", "properties":{"message":{"type":"string"}}, "required":["message"]}),
            json!({"input":{"type":"object"},"output":{"type":"string"}}),
        ] {
            assert!(matches!(
                contract(schema),
                Err(ChannelBindingError::InvalidContract)
            ));
        }
        let valid = ChannelLifecycleDeclarationV1::schema();
        for field in [
            "$schema",
            "$id",
            "additionalProperties",
            "required",
            "properties",
        ] {
            let mut missing = valid.clone();
            missing.as_object_mut().unwrap().remove(field);
            assert!(matches!(
                contract(missing),
                Err(ChannelBindingError::InvalidContract)
            ));
        }
        for field in [
            "declaration_kind",
            "channel_id",
            "provider_transport",
            "lifecycle_model_version",
        ] {
            let mut missing = valid.clone();
            missing["properties"].as_object_mut().unwrap().remove(field);
            assert!(matches!(
                contract(missing),
                Err(ChannelBindingError::InvalidContract)
            ));
        }
        let mut empty = valid.clone();
        empty["required"] = json!([]);
        assert!(matches!(
            contract(empty),
            Err(ChannelBindingError::InvalidContract)
        ));
        assert!(contract(valid).is_ok());
    }

    #[test]
    fn lifecycle_schema_fingerprint_is_canonical() {
        let ordered_a: serde_json::Value =
            serde_json::from_str(r#"{"z":{"b":2,"a":1},"a":["x","y"]}"#).unwrap();
        let ordered_b: serde_json::Value =
            serde_json::from_str(r#"{"a":["x","y"],"z":{"a":1,"b":2}}"#).unwrap();
        assert_eq!(
            canonical_schema_fingerprint(&ordered_a),
            canonical_schema_fingerprint(&ordered_b)
        );
        let mut changed = ordered_a.clone();
        changed["a"] = json!(["y", "x"]);
        assert_ne!(
            canonical_schema_fingerprint(&ordered_a),
            canonical_schema_fingerprint(&changed)
        );
        changed["z"]["a"] = json!(3);
        assert_ne!(
            canonical_schema_fingerprint(&ordered_a),
            canonical_schema_fingerprint(&changed)
        );
        assert_ne!(
            canonical_schema_fingerprint(&ChannelLifecycleDeclarationV1::schema()),
            [0; 32]
        );
        let reordered = serde_json::from_str(r#"{
            "properties": {
                "lifecycle_model_version":{"const":1}, "provider_transport":{"const":"in_process"},
                "channel_id":{"minLength":1,"type":"string"}, "declaration_kind":{"const":"channel-provider-lifecycle"}
            },
            "required":["declaration_kind","channel_id","provider_transport","lifecycle_model_version"],
            "additionalProperties":false,"type":"object",
            "$id":"aleph:gateway:channel-lifecycle-declaration:v1",
            "$schema":"https://json-schema.org/draft/2020-12/schema"
        }"#).unwrap();
        assert!(contract(reordered).is_ok());
    }

    fn descriptor() -> CapabilityDescriptor {
        CapabilityDescriptor {
            id: CapabilityId {
                namespace: "aleph/gateway/channels".into(),
                name: "team/a/b".into(),
            },
            kind: CapabilityKind::Resource,
            schema: SchemaRef {
                version: 1,
                fingerprint: canonical_schema_fingerprint(&ChannelLifecycleDeclarationV1::schema()),
            },
            revision: CapabilityRevision(1),
            lifetime: LifetimeScope::Runtime,
            visibility: VisibilityScope::default(),
            owner: OwnerRef::Runtime,
            services: vec![],
            metadata: ProjectionMetadata::default(),
            facets: CapabilityFacets::new().with(CapabilityFacet::ProviderLifecycle),
        }
    }

    #[test]
    fn descriptor_only_change_changes_full_fingerprint() {
        let before = descriptor();
        let session = SessionKey::with_name("test", "/tmp/channel-binding", Some("named"));
        let mutations: Vec<Box<dyn Fn(&mut CapabilityDescriptor)>> = vec![
            Box::new(|d| d.id.namespace.push('x')),
            Box::new(|d| d.id.name.push('x')),
            Box::new(|d| d.kind = CapabilityKind::Tool),
            Box::new(|d| d.revision.0 += 1),
            Box::new(|d| d.owner = OwnerRef::Run(RunId("run".into()))),
            Box::new(|d| d.owner = OwnerRef::Task(TaskId("task".into()))),
            Box::new({
                let session = session.clone();
                move |d| d.owner = OwnerRef::Session(session.clone())
            }),
            Box::new(|d| d.lifetime = LifetimeScope::External),
            Box::new(|d| d.visibility.principal = Some("principal".into())),
            Box::new(|d| d.visibility.workspace = Some("workspace".into())),
            Box::new(move |d| d.visibility.session = Some(session.clone())),
            Box::new(|d| d.visibility.allowed_kinds = Some(vec!["Resource".into()])),
            Box::new(|d| d.visibility.allowed_ids = Some(vec!["team/a/b".into()])),
            Box::new(|d| d.visibility.approval = Some("approval".into())),
            Box::new(|d| d.metadata.surface = Some("internal".into())),
            Box::new(|d| d.facets = d.facets.with(CapabilityFacet::Callable)),
            Box::new(|d| {
                d.services.push(Service {
                    relation: ServiceRelation::Requires,
                    target: CapabilityId {
                        namespace: "dep".into(),
                        name: "one".into(),
                    },
                })
            }),
        ];
        for mutate in mutations {
            let mut changed = before.clone();
            mutate(&mut changed);
            assert_ne!(
                canonical_descriptor_fingerprint(&before),
                canonical_descriptor_fingerprint(&changed)
            );
            assert_eq!(before.schema.fingerprint, changed.schema.fingerprint);
        }
        for change_version in [false, true] {
            let mut changed = before.clone();
            if change_version {
                changed.schema.version += 1;
            } else {
                changed.schema.fingerprint[0] ^= 1;
            }
            assert_ne!(
                canonical_descriptor_fingerprint(&before),
                canonical_descriptor_fingerprint(&changed)
            );
        }
        let mut services = before.clone();
        services.services = vec![
            Service {
                relation: ServiceRelation::Requires,
                target: CapabilityId {
                    namespace: "dep".into(),
                    name: "one".into(),
                },
            },
            Service {
                relation: ServiceRelation::Provides,
                target: CapabilityId {
                    namespace: "dep".into(),
                    name: "two".into(),
                },
            },
        ];
        let mut changed = services.clone();
        changed.services.reverse();
        assert_ne!(
            canonical_descriptor_fingerprint(&services),
            canonical_descriptor_fingerprint(&changed)
        );
        changed = services.clone();
        changed.services[0].relation = ServiceRelation::Conflicts;
        assert_ne!(
            canonical_descriptor_fingerprint(&services),
            canonical_descriptor_fingerprint(&changed)
        );
        changed = services.clone();
        changed.services[0].target.name.push('x');
        assert_ne!(
            canonical_descriptor_fingerprint(&services),
            canonical_descriptor_fingerprint(&changed)
        );
        changed = services.clone();
        changed.services[0].target.namespace.push('x');
        assert_ne!(
            canonical_descriptor_fingerprint(&services),
            canonical_descriptor_fingerprint(&changed)
        );
        let mut scope = before.clone();
        scope.visibility.allowed_ids = Some(vec!["a".into(), "b".into()]);
        changed = scope.clone();
        changed.visibility.allowed_ids.as_mut().unwrap().reverse();
        assert_ne!(
            canonical_descriptor_fingerprint(&scope),
            canonical_descriptor_fingerprint(&changed)
        );
        changed = before.clone();
        changed.visibility.allowed_ids = Some(vec![]);
        assert_ne!(
            canonical_descriptor_fingerprint(&before),
            canonical_descriptor_fingerprint(&changed)
        );
    }
}
