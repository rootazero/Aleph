//! Source-private cold channel declarations; no executable provider admission.

use super::{ChannelHandle, ChannelRegistrationEntry};
use crate::acp::manager::SessionKey;
use crate::capability::descriptor::{
    CapabilityDescriptor, CapabilityFacet, CapabilityKind, ProjectionMetadata, SchemaRef,
    ServiceRelation,
};
use crate::capability::ownership::{
    BindingValidationError, LifetimeScope, OwnerGeneration, OwnerRef, OwnershipTree,
    VisibilityScope,
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
    pub(super) descriptor_revision: u64,
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
    #[error("channel descriptor publication revision exhausted")]
    DescriptorPublicationExhausted,
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
    pub(super) retiring: bool,
    pub(super) driver: Option<tokio::task::JoinHandle<bool>>,
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
    pub(super) fn validate_bound_capture(
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
            || bound.descriptor_revision != capture.descriptor_revision
            || bound.descriptor.revision.0 != capture.descriptor_revision
            || bound.descriptor_fingerprint != capture.descriptor_fingerprint
            || canonical_descriptor_fingerprint(&capture.descriptor) != bound.descriptor_fingerprint
        {
            return Err(ChannelBindingError::StaleDescriptor);
        }
        if current.runtime.lock().unwrap().retiring {
            return Err(ChannelBindingError::Retiring);
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
                            let operation = {
                                let channel = ticket.observation.capture.channel.read().await;
                                channel.send(message)
                            };
                            let result = operation.await;
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
        let mut handoff = Some((result, sender));
        let accepted = if !Arc::ptr_eq(&self.source, &observation.capture.source) {
            Err(ChannelBindingError::WrongSource)
        } else {
            let super::ChannelRegistration::Bound(bound) = &original.registration else {
                unreachable!()
            };
            let ownership_result = bound.ownership.with_current_binding(
                &bound.descriptor.id,
                &observation.visibility,
                observation.owner_generation,
                &bound.contract.owner,
                &bound.contract.lifetime,
                || {
                    bound
                        .provider
                        .with_admitted_completion(observation.capture.provider_generation, || {
                            let Some(current) = current.as_ref() else {
                                return Err(ChannelBindingError::NotFound);
                            };
                            if !Arc::ptr_eq(current, original) {
                                return Err(ChannelBindingError::StaleEntry);
                            }
                            let mut runtime = original
                                .runtime
                                .lock()
                                .expect("channel runtime mutex poisoned");
                            if runtime.retiring {
                                return Err(ChannelBindingError::Retiring);
                            }
                            runtime.active_work = runtime
                                .active_work
                                .checked_sub(1)
                                .expect("live original ticket");
                            let (result, sender) = handoff.take().expect("one completion handoff");
                            let _ = sender.send(BoundCompletion::Accepted(result));
                            Ok(())
                        })
                        .map_err(ChannelBindingError::from)
                        .and_then(|result| result)
                },
            );
            ownership_result
                .map_err(ChannelBindingError::from)
                .and_then(|result| result)
        };
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

    pub(crate) async fn replace_bound_channel(
        self: &Arc<Self>,
        channel_id: &super::ChannelId,
        channel: Box<dyn super::Channel>,
        contract: ChannelLifecycleContract,
        ownership: Arc<OwnershipTree>,
        initial: ChannelInitialState,
    ) -> Result<super::ChannelId, ChannelBindingError> {
        let candidate_id = channel.id().clone();
        if candidate_id != *channel_id {
            return Err(ChannelBindingError::InvalidContract);
        }
        let _source_publication = self.source_publication_sync.lock().await;
        let preflight_revision = self.next_bound_revision()?;
        let preflight_descriptor_revision = self.next_bound_descriptor_revision()?;
        let candidate = self.prepare_bound_entry(
            channel,
            contract,
            ownership,
            initial,
            preflight_revision,
            preflight_descriptor_revision,
        )?;
        let incumbent = self.begin_bound_retirement(channel_id).await?;
        let (receiver, marker) = self.spawn_bound_retirement(&incumbent).await?;
        self.await_bound_retirement(&incumbent, receiver).await?;
        self.commit_bound_replacement(channel_id, incumbent, marker, candidate)
            .await
    }

    pub(crate) async fn unregister_bound_channel(
        self: &Arc<Self>,
        channel_id: &super::ChannelId,
    ) -> Result<(), ChannelBindingError> {
        let _source_publication = self.source_publication_sync.lock().await;
        let _ = self.next_bound_revision()?;
        let _ = self.next_bound_descriptor_revision()?;
        let incumbent = self.begin_bound_retirement(channel_id).await?;
        let (receiver, marker) = self.spawn_bound_retirement(&incumbent).await?;
        self.await_bound_retirement(&incumbent, receiver).await?;
        self.commit_bound_unregistration(channel_id, incumbent, marker)
            .await
    }

    pub(super) fn next_bound_revision(&self) -> Result<u64, ChannelBindingError> {
        self.source_revision
            .load(std::sync::atomic::Ordering::Relaxed)
            .checked_add(1)
            .ok_or(ChannelBindingError::PublicationExhausted)
    }

    fn next_bound_descriptor_revision(&self) -> Result<u64, ChannelBindingError> {
        self.descriptor_revision
            .load(std::sync::atomic::Ordering::Relaxed)
            .checked_add(1)
            .ok_or(ChannelBindingError::DescriptorPublicationExhausted)
    }

    fn advance_bound_revision(&self) -> Result<u64, ChannelBindingError> {
        self.try_advance_source_revision()
    }

    fn advance_bound_descriptor_revision(&self) -> Result<u64, ChannelBindingError> {
        self.try_advance_descriptor_revision()
    }

    async fn begin_bound_retirement(
        &self,
        id: &super::ChannelId,
    ) -> Result<Arc<ChannelRegistrationEntry>, ChannelBindingError> {
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
        let _ = self.next_bound_revision()?;
        let mut desired = self.desired_state.write().await;
        {
            let mut runtime = entry
                .runtime
                .lock()
                .expect("channel runtime mutex poisoned");
            if let Some(driver) = &mut runtime.driver {
                match driver.now_or_never() {
                    Some(Ok(true)) => {
                        runtime.driver = None;
                        if runtime.retiring {
                            runtime.operation = None;
                            runtime.retiring = false;
                            runtime.residual = ChannelCleanupResidual::default();
                        }
                    }
                    Some(Ok(false)) => return Err(ChannelBindingError::Busy),
                    Some(Err(_)) => return Err(ChannelBindingError::Busy),
                    None => return Err(ChannelBindingError::Busy),
                }
            }
            if runtime.operation.is_some() {
                return Err(ChannelBindingError::Busy);
            }
            let state = bound.provider.state();
            if matches!(state, ProviderState::Ready | ProviderState::Starting) {
                bound.provider.transition(ProviderState::Stopping)?;
            }
            runtime.operation = Some(Arc::new(()));
            runtime.retiring = true;
            runtime.residual.transport_may_be_live = matches!(
                state,
                ProviderState::Ready | ProviderState::Starting | ProviderState::Stopping
            ) || runtime.forwarder.is_some();
        }
        desired.insert(id.clone(), super::DesiredChannelState::Stopped);
        Ok(entry)
    }

    async fn spawn_bound_retirement(
        self: &Arc<Self>,
        entry: &Arc<ChannelRegistrationEntry>,
    ) -> Result<
        (
            tokio::sync::oneshot::Receiver<Result<(), ChannelBindingError>>,
            Arc<()>,
        ),
        ChannelBindingError,
    > {
        let marker = entry
            .runtime
            .lock()
            .expect("channel runtime mutex poisoned")
            .operation
            .clone()
            .ok_or(ChannelBindingError::Busy)?;
        let (sender, receiver) = tokio::sync::oneshot::channel();
        let source = self.clone();
        let original = entry.clone();
        let driver_marker = marker.clone();
        let driver = tokio::spawn(async move {
            let result = source.drain_bound(&original, &driver_marker, true).await;
            let succeeded = result.is_ok();
            let _ = sender.send(result);
            succeeded
        });
        entry.runtime.lock().unwrap().driver = Some(driver);
        Ok((receiver, marker))
    }

    async fn await_bound_retirement(
        &self,
        entry: &Arc<ChannelRegistrationEntry>,
        receiver: tokio::sync::oneshot::Receiver<Result<(), ChannelBindingError>>,
    ) -> Result<(), ChannelBindingError> {
        let result = receiver.await.map_err(|_| ChannelBindingError::Runtime {
            phase: ChannelRuntimePhase::Stop,
            error: "retirement driver dropped before reporting cleanup".into(),
            residual: entry.runtime.lock().unwrap().residual.clone(),
        })?;
        let driver = entry.runtime.lock().unwrap().driver.take();
        if let Some(driver) = driver {
            if let Err(error) = driver.await {
                return Err(self.bound_cleanup_error(entry, error.to_string()));
            }
        }
        result
    }

    async fn commit_bound_replacement(
        &self,
        id: &super::ChannelId,
        incumbent: Arc<ChannelRegistrationEntry>,
        marker: Arc<()>,
        mut candidate: Arc<ChannelRegistrationEntry>,
    ) -> Result<super::ChannelId, ChannelBindingError> {
        let _source = self.lifecycle_sync.lock().await;
        let mut channels = self.channels.write().await;
        let current = channels
            .get(id)
            .cloned()
            .ok_or(ChannelBindingError::NotFound)?;
        if !Arc::ptr_eq(&current, &incumbent)
            || !incumbent
                .runtime
                .lock()
                .unwrap()
                .operation
                .as_ref()
                .is_some_and(|active| Arc::ptr_eq(active, &marker))
        {
            return Err(ChannelBindingError::StaleEntry);
        }
        if incumbent.runtime.lock().unwrap().active_work != 0 {
            return Err(ChannelBindingError::Busy);
        }
        let bound = match &mut Arc::get_mut(&mut candidate)
            .ok_or(ChannelBindingError::Busy)?
            .registration
        {
            super::ChannelRegistration::Bound(bound) => bound,
            _ => return Err(ChannelBindingError::InvalidContract),
        };
        let mut desired = self.desired_state.write().await;
        let _ = self.next_bound_revision()?;
        let _ = self.next_bound_descriptor_revision()?;
        let source_revision = self.advance_bound_revision()?;
        let descriptor_revision = self.advance_bound_descriptor_revision()?;
        bound.entry_revision = source_revision;
        bound.descriptor_revision = descriptor_revision;
        bound.descriptor.revision =
            crate::capability::descriptor::CapabilityRevision(descriptor_revision);
        bound.descriptor_fingerprint = canonical_descriptor_fingerprint(&bound.descriptor);
        channels.insert(id.clone(), candidate);
        desired.insert(id.clone(), super::DesiredChannelState::Stopped);
        Ok(id.clone())
    }

    async fn commit_bound_unregistration(
        &self,
        id: &super::ChannelId,
        incumbent: Arc<ChannelRegistrationEntry>,
        marker: Arc<()>,
    ) -> Result<(), ChannelBindingError> {
        let _source = self.lifecycle_sync.lock().await;
        let mut channels = self.channels.write().await;
        let current = channels
            .get(id)
            .cloned()
            .ok_or(ChannelBindingError::NotFound)?;
        if !Arc::ptr_eq(&current, &incumbent)
            || !incumbent
                .runtime
                .lock()
                .unwrap()
                .operation
                .as_ref()
                .is_some_and(|active| Arc::ptr_eq(active, &marker))
        {
            return Err(ChannelBindingError::StaleEntry);
        }
        if incumbent.runtime.lock().unwrap().active_work != 0 {
            return Err(ChannelBindingError::Busy);
        }
        let mut desired = self.desired_state.write().await;
        self.advance_bound_revision()?;
        self.advance_bound_descriptor_revision()?;
        channels.remove(id);
        desired.remove(id);
        Ok(())
    }

    async fn admit_bound_start(
        &self,
        id: &super::ChannelId,
    ) -> Result<(Arc<ChannelRegistrationEntry>, Arc<()>, u64), ChannelBindingError> {
        use futures::FutureExt;
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
                Some(Ok(true)) => runtime.driver = None,
                Some(Ok(false)) => return Err(ChannelBindingError::Busy),
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
        drop(runtime);
        Ok((entry, marker, generation))
    }

    pub(crate) async fn start_bound_channel_compat(
        &self,
        id: &super::ChannelId,
    ) -> Result<(), ChannelBindingError> {
        let (entry, marker, generation) = {
            let _source = self.lifecycle_sync.lock().await;
            self.admit_bound_start(id).await?
        };
        self.activate_bound(id, &entry, &marker, generation).await
    }

    pub(crate) async fn start_bound_channel(
        self: &Arc<Self>,
        id: &super::ChannelId,
    ) -> Result<LifecycleResult, ChannelBindingError> {
        let _source = self.lifecycle_sync.lock().await;
        let (entry, marker, generation) = self.admit_bound_start(id).await?;
        let (sender, receiver) = tokio::sync::oneshot::channel();
        let source = self.clone();
        let id = id.clone();
        let original = entry.clone();
        let driver = tokio::spawn(async move {
            let result = source
                .activate_bound(&id, &original, &marker, generation)
                .await;
            let succeeded = result.is_ok();
            let _ = sender.send(result);
            succeeded
        });
        entry.runtime.lock().unwrap().driver = Some(driver);
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

    async fn admit_bound_stop(
        &self,
        id: &super::ChannelId,
    ) -> Result<(Arc<ChannelRegistrationEntry>, Arc<()>), ChannelBindingError> {
        use crate::capability::provider::ProviderState;
        use futures::FutureExt;
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
                Some(Ok(true)) => {
                    runtime.driver = None;
                }
                Some(Ok(false)) => {
                    return Err(ChannelBindingError::Busy);
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
        drop(runtime);
        drop(desired);
        Ok((entry, marker))
    }

    pub(crate) async fn stop_bound_channel_compat(
        &self,
        id: &super::ChannelId,
    ) -> Result<(), ChannelBindingError> {
        let (entry, marker) = {
            let _source = self.lifecycle_sync.lock().await;
            self.admit_bound_stop(id).await?
        };
        self.drain_bound(&entry, &marker, false).await
    }

    pub(crate) async fn stop_bound_channel(
        self: &Arc<Self>,
        id: &super::ChannelId,
    ) -> Result<LifecycleResult, ChannelBindingError> {
        let _source = self.lifecycle_sync.lock().await;
        let (entry, marker) = self.admit_bound_stop(id).await?;
        let (sender, receiver) = tokio::sync::oneshot::channel();
        let source = self.clone();
        let original = entry.clone();
        let driver = tokio::spawn(async move {
            let result = source.drain_bound(&original, &marker, false).await;
            let succeeded = result.is_ok();
            let _ = sender.send(result);
            succeeded
        });
        // Admission and driver publication share the source gate.
        // The driver itself performs no channel I/O under that gate.
        entry.runtime.lock().unwrap().driver = Some(driver);
        Ok(receiver)
    }

    async fn drain_bound(
        &self,
        entry: &Arc<ChannelRegistrationEntry>,
        marker: &Arc<()>,
        retiring: bool,
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
        self.complete_bound_retirement(entry, marker, retiring)
    }

    fn complete_bound_retirement(
        &self,
        entry: &Arc<ChannelRegistrationEntry>,
        marker: &Arc<()>,
        retiring: bool,
    ) -> Result<(), ChannelBindingError> {
        use crate::capability::provider::ProviderState;
        let super::ChannelRegistration::Bound(bound) = &entry.registration else {
            unreachable!()
        };
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
        if !retiring {
            runtime.operation = None;
            runtime.retiring = false;
        }
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
