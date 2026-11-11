//! Bounded, instance-scoped capability observations, never an authority or replay log.
//!
//! Records commit to memory before live notification and optional persistence.
//! Persistence failure is reported *after* that commit; retrying emission would
//! duplicate an observation. Persistence hooks run outside the buffer lock and
//! may complete out of order, so sinks must order by record sequence if needed.

use crate::capability::descriptor::CapabilityId;
use crate::capability::event_audience::{EventAudience, TrustedEventScope};
use crate::capability::ownership::{OwnerRef, VisibilityScope};
use crate::sync_primitives::Arc;
use chrono::{DateTime, Utc};
use serde::Serialize;
use std::collections::VecDeque;
use std::sync::Mutex;
use tokio::sync::broadcast;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityEventType {
    Created,
    Attached,
    Activated,
    Detaching,
    Invoked,
    Revoked,
    ConstraintViolation,
    PolicyViolation,
    ExecutionFailed,
}

/// Deliberately excludes handler payloads and free-form errors (which may contain secrets).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HandlerResult {
    Success,
    InvalidInput,
    ExecutionFailed,
    NotFound,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CapabilityEvent {
    CapabilityCreated {
        id: CapabilityId,
        owner: OwnerRef,
        visibility: Box<VisibilityScope>,
    },
    CapabilityAttached {
        id: CapabilityId,
    },
    /// `lease_id` is an audit correlation UUID, NOT an authorization token.
    /// `constraints_applied` is the number of successfully evaluated constraints.
    CapabilityActivated {
        id: CapabilityId,
        lease_id: String,
        constraints_applied: usize,
    },
    CapabilityDetaching {
        id: CapabilityId,
    },
    CapabilityInvoked {
        id: CapabilityId,
        operation: String,
        handler_result: HandlerResult,
    },
    CapabilityRevoked {
        id: CapabilityId,
        reason: String,
    },
    ConstraintViolation {
        id: CapabilityId,
        constraint_type: String,
    },
    PolicyViolation {
        id: CapabilityId,
        reason: String,
    },
    ExecutionFailed {
        id: CapabilityId,
        operation: String,
        reason: String,
    },
}

impl CapabilityEvent {
    pub fn id(&self) -> &CapabilityId {
        match self {
            Self::CapabilityCreated { id, .. }
            | Self::CapabilityAttached { id }
            | Self::CapabilityActivated { id, .. }
            | Self::CapabilityDetaching { id }
            | Self::CapabilityInvoked { id, .. }
            | Self::CapabilityRevoked { id, .. }
            | Self::ConstraintViolation { id, .. }
            | Self::PolicyViolation { id, .. }
            | Self::ExecutionFailed { id, .. } => id,
        }
    }

    pub const fn event_type(&self) -> CapabilityEventType {
        match self {
            Self::CapabilityCreated { .. } => CapabilityEventType::Created,
            Self::CapabilityAttached { .. } => CapabilityEventType::Attached,
            Self::CapabilityActivated { .. } => CapabilityEventType::Activated,
            Self::CapabilityDetaching { .. } => CapabilityEventType::Detaching,
            Self::CapabilityInvoked { .. } => CapabilityEventType::Invoked,
            Self::CapabilityRevoked { .. } => CapabilityEventType::Revoked,
            Self::ConstraintViolation { .. } => CapabilityEventType::ConstraintViolation,
            Self::PolicyViolation { .. } => CapabilityEventType::PolicyViolation,
            Self::ExecutionFailed { .. } => CapabilityEventType::ExecutionFailed,
        }
    }
}

/// Constructed only by the stream. No mutation, deserialization, or replacement API.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CapabilityEventRecord {
    sequence: u64,
    timestamp: DateTime<Utc>,
    actor: OwnerRef,
    audience: EventAudience,
    event: CapabilityEvent,
}

impl CapabilityEventRecord {
    pub const fn sequence(&self) -> u64 {
        self.sequence
    }
    pub const fn timestamp(&self) -> DateTime<Utc> {
        self.timestamp
    }
    pub fn actor(&self) -> &OwnerRef {
        &self.actor
    }
    pub fn event(&self) -> &CapabilityEvent {
        &self.event
    }
    pub fn audience(&self) -> &EventAudience {
        &self.audience
    }
}

#[derive(Debug, Clone, Default)]
pub struct EventFilter {
    pub event_type: Option<CapabilityEventType>,
    pub resource: Option<CapabilityId>,
    pub actor: Option<OwnerRef>,
}

impl EventFilter {
    pub(crate) fn matches(&self, record: &CapabilityEventRecord) -> bool {
        self.event_type
            .is_none_or(|kind| record.event.event_type() == kind)
            && self
                .resource
                .as_ref()
                .is_none_or(|id| record.event.id() == id)
            && self
                .actor
                .as_ref()
                .is_none_or(|actor| &record.actor == actor)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum EventError {
    #[error("scoped capability events are unsupported by this source")]
    Unsupported,
    #[error("event buffer capacity is zero or exceeds the broadcast channel limit")]
    InvalidCapacity,
    #[error("event buffer lock is poisoned")]
    Poisoned,
    #[error("event sequence exhausted")]
    SequenceExhausted,
    #[error("subscriber missed {0} live events; query the retained snapshot")]
    Lagged(u64),
    #[error("event stream closed")]
    Closed,
    #[error("event committed in memory but persistence failed: {0}")]
    Persistence(String),
}

pub trait EventPersistence: Send + Sync {
    fn persist(&self, record: &CapabilityEventRecord) -> Result<(), String>;
}

/// Emission can run under lifecycle synchronization. Implementations and their
/// persistence hooks must stay short and must not re-enter the emitting controller.
/// Offload slow I/O in the sink rather than blocking the capability owner.
pub trait CapabilityEventStream: Send + Sync {
    fn emit(&self, actor: OwnerRef, event: CapabilityEvent) -> Result<(), EventError>;
    fn emit_scoped(
        &self,
        _actor: OwnerRef,
        _audience: EventAudience,
        _event: CapabilityEvent,
    ) -> Result<(), EventError> {
        Err(EventError::Unsupported)
    }
    /// Live-only, with no historical replay; subscribe before starting producers.
    fn subscribe(&self, filter: EventFilter) -> EventSubscription;
}

pub struct EventSubscription {
    receiver: broadcast::Receiver<Arc<CapabilityEventRecord>>,
    filter: EventFilter,
}

impl EventSubscription {
    /// Live-only asynchronous receive. Closure drains queued matching records first.
    pub async fn recv(&mut self) -> Result<Arc<CapabilityEventRecord>, EventError> {
        loop {
            let record = self.receiver.recv().await.map_err(|error| match error {
                broadcast::error::RecvError::Closed => EventError::Closed,
                broadcast::error::RecvError::Lagged(count) => EventError::Lagged(count),
            })?;
            if self.filter.matches(&record) {
                return Ok(record);
            }
        }
    }

    /// `None` means currently empty, never "history complete".
    pub fn try_recv(&mut self) -> Result<Option<Arc<CapabilityEventRecord>>, EventError> {
        loop {
            match self.receiver.try_recv() {
                Ok(record) if self.filter.matches(&record) => return Ok(Some(record)),
                Ok(_) => {}
                Err(broadcast::error::TryRecvError::Empty) => return Ok(None),
                Err(broadcast::error::TryRecvError::Closed) => return Err(EventError::Closed),
                Err(broadcast::error::TryRecvError::Lagged(count)) => {
                    return Err(EventError::Lagged(count))
                }
            }
        }
    }
}

/// Owned live receiver, independent of the (intentionally non-Send) Zahir face.
pub struct ScopedEventSubscription {
    subscription: EventSubscription,
    scope: TrustedEventScope,
}
impl ScopedEventSubscription {
    pub async fn recv(&mut self) -> Result<Arc<CapabilityEventRecord>, EventError> {
        loop {
            let record = self.subscription.recv().await?;
            if self.scope.admits(&record).await {
                return Ok(record);
            }
        }
    }
}

/// One coherent retained-history cut, including an explicit loss counter.
#[derive(Debug, Clone)]
pub struct EventSnapshot {
    records: Vec<Arc<CapabilityEventRecord>>,
    dropped: u64,
}

impl EventSnapshot {
    pub fn records(&self) -> &[Arc<CapabilityEventRecord>] {
        &self.records
    }
    pub const fn dropped(&self) -> u64 {
        self.dropped
    }
}

struct Buffer {
    records: VecDeque<Arc<CapabilityEventRecord>>,
    sequence: u64,
    dropped: u64,
    // The sole sender is taken at close under the same commit mutex.
    sender: Option<broadcast::Sender<Arc<CapabilityEventRecord>>>,
    // Resubscription anchor for this SAME hub, including after explicit close.
    // It neither forwards records nor keeps a producer alive.
    receiver: broadcast::Receiver<Arc<CapabilityEventRecord>>,
}

pub const DEFAULT_EVENT_CAPACITY: usize = 1024;

pub struct InMemoryEventStream {
    capacity: usize,
    buffer: Mutex<Buffer>,
    persistence: Option<Arc<dyn EventPersistence>>,
}
impl Default for InMemoryEventStream {
    fn default() -> Self {
        Self::new(DEFAULT_EVENT_CAPACITY).expect("static event capacity is valid")
    }
}

impl InMemoryEventStream {
    pub fn new(capacity: usize) -> Result<Self, EventError> {
        Self::with_persistence(capacity, None)
    }

    pub fn with_persistence(
        capacity: usize,
        persistence: Option<Arc<dyn EventPersistence>>,
    ) -> Result<Self, EventError> {
        if capacity == 0 || capacity > usize::MAX / 2 {
            return Err(EventError::InvalidCapacity);
        }
        let (sender, receiver) = broadcast::channel(capacity);
        Ok(Self {
            capacity,
            buffer: Mutex::new(Buffer {
                records: VecDeque::new(),
                sequence: 0,
                dropped: 0,
                sender: Some(sender),
                receiver,
            }),
            persistence,
        })
    }

    /// Synchronously seals emission and wakes receivers, even while emitters keep Arcs.
    pub fn close(&self) -> Result<(), EventError> {
        let mut buffer = self.buffer.lock().map_err(|_| EventError::Poisoned)?;
        drop(buffer.sender.take());
        Ok(())
    }

    pub fn subscribe_scoped(
        &self,
        scope: TrustedEventScope,
        filter: EventFilter,
    ) -> ScopedEventSubscription {
        ScopedEventSubscription {
            subscription: self.subscribe(filter),
            scope,
        }
    }

    pub fn snapshot(&self) -> Result<EventSnapshot, EventError> {
        let buffer = self.buffer.lock().map_err(|_| EventError::Poisoned)?;
        Ok(EventSnapshot {
            records: buffer.records.iter().cloned().collect(),
            dropped: buffer.dropped,
        })
    }
}

impl CapabilityEventStream for InMemoryEventStream {
    fn emit(&self, actor: OwnerRef, event: CapabilityEvent) -> Result<(), EventError> {
        self.emit_scoped(actor, EventAudience::OperatorOnly, event)
    }

    fn emit_scoped(
        &self,
        actor: OwnerRef,
        audience: EventAudience,
        event: CapabilityEvent,
    ) -> Result<(), EventError> {
        let record = {
            let mut buffer = self.buffer.lock().map_err(|_| EventError::Poisoned)?;
            let Buffer {
                records,
                sequence,
                dropped,
                sender,
                ..
            } = &mut *buffer;
            let sender = sender.as_ref().ok_or(EventError::Closed)?;
            let next = sequence
                .checked_add(1)
                .ok_or(EventError::SequenceExhausted)?;
            let record = Arc::new(CapabilityEventRecord {
                sequence: next,
                timestamp: Utc::now(),
                actor,
                audience,
                event,
            });
            if records.len() == self.capacity {
                records.pop_front();
                *dropped += 1;
            }
            records.push_back(Arc::clone(&record));
            *sequence = next;
            // Commit and notification precede close under this SAME mutex.
            let _ = sender.send(Arc::clone(&record));
            record
        };
        if let Some(persistence) = &self.persistence {
            persistence
                .persist(&record)
                .map_err(EventError::Persistence)?;
        }
        Ok(())
    }

    fn subscribe(&self, filter: EventFilter) -> EventSubscription {
        EventSubscription {
            receiver: self
                .buffer
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .receiver
                .resubscribe(),
            filter,
        }
    }
}

/// Best-effort observation must never convert a denial into permission, undo an
/// already completed effect, or block unconditional revocation on a sink error.
#[derive(Clone)]
pub struct CapabilityEventEmitter(Arc<dyn CapabilityEventStream>);

impl std::fmt::Debug for CapabilityEventEmitter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CapabilityEventEmitter")
            .finish_non_exhaustive()
    }
}

impl CapabilityEventEmitter {
    pub fn new(stream: Arc<dyn CapabilityEventStream>) -> Self {
        Self(stream)
    }

    pub fn emit(&self, actor: OwnerRef, event: CapabilityEvent) {
        let kind = event.event_type();
        if let Err(error) = self.0.emit(actor, event) {
            tracing::warn!(event_type = ?kind, error = %error, "capability event emission failed");
        }
    }

    pub fn emit_scoped(&self, actor: OwnerRef, audience: EventAudience, event: CapabilityEvent) {
        let kind = event.event_type();
        if let Err(error) = self.0.emit_scoped(actor, audience, event) {
            tracing::warn!(event_type = ?kind, error = %error, "capability scoped event emission failed");
        }
    }
}

/// Observation metadata retained by the actual immutable source record.
/// Flags are not authority; source owners set them only inside their existing commit.
#[derive(Debug)]
pub(crate) struct SourceObservation {
    descriptor: crate::capability::descriptor::CapabilityDescriptor,
    audience: EventAudience,
    incarnation: String,
    emitter: CapabilityEventEmitter,
    state: std::sync::atomic::AtomicU8,
}

impl SourceObservation {
    pub(crate) fn new(
        descriptor: crate::capability::descriptor::CapabilityDescriptor,
        audience: EventAudience,
        incarnation: String,
        emitter: CapabilityEventEmitter,
    ) -> Arc<Self> {
        Arc::new(Self {
            descriptor,
            audience,
            incarnation,
            emitter,
            state: std::sync::atomic::AtomicU8::new(0),
        })
    }

    pub(crate) fn mark_committed(&self) {
        self.state.fetch_or(1, std::sync::atomic::Ordering::SeqCst);
    }
    pub(crate) fn mark_withdrawn(&self) {
        self.state.fetch_or(2, std::sync::atomic::Ordering::SeqCst);
    }
    pub(crate) fn incarnation(&self) -> &str {
        &self.incarnation
    }
    pub(crate) fn matches(&self, binding: &crate::capability::ownership::CapturedBinding) -> bool {
        self.descriptor.id == binding.capability
            && self.descriptor.visibility == binding.visibility
            && self.descriptor.owner == binding.owner
            && self.descriptor.lifetime == binding.lifetime
    }

    /// Drain one record's ordered observations without holding any source/tree mutex.
    /// A reentrant persistence callback only adds pending flags for this same drainer.
    pub(crate) fn flush(&self) {
        use std::sync::atomic::Ordering::SeqCst;
        loop {
            if self.state.fetch_or(32, SeqCst) & 32 != 0 {
                return;
            }
            loop {
                let state = self.state.load(SeqCst);
                let next = if state & 1 != 0 && state & 4 == 0 {
                    Some((
                        4,
                        CapabilityEvent::CapabilityCreated {
                            id: self.descriptor.id.clone(),
                            owner: self.descriptor.owner.clone(),
                            visibility: Box::new(self.descriptor.visibility.clone()),
                        },
                    ))
                } else if state & 1 != 0 && state & 8 == 0 {
                    Some((
                        8,
                        CapabilityEvent::CapabilityActivated {
                            id: self.descriptor.id.clone(),
                            lease_id: self.incarnation.clone(),
                            constraints_applied: 0,
                        },
                    ))
                } else if state & 3 == 3 && state & 16 == 0 {
                    Some((
                        16,
                        CapabilityEvent::CapabilityRevoked {
                            id: self.descriptor.id.clone(),
                            reason: "source_withdrawn".into(),
                        },
                    ))
                } else {
                    None
                };
                let Some((flag, event)) = next else { break };
                self.state.fetch_or(flag, SeqCst);
                self.emitter.emit_scoped(
                    self.descriptor.owner.clone(),
                    self.audience.clone(),
                    event,
                );
            }
            self.state.fetch_and(!32, SeqCst);
            let state = self.state.load(SeqCst);
            if !(state & 1 != 0 && state & 12 != 12 || state & 3 == 3 && state & 16 == 0) {
                return;
            }
        }
    }
}

/// Mechanical outbox item, never a source or authorization registry. Its
/// reservation keeps STOP/DRAIN open until post-lock observation completes.
pub(crate) struct SourceObservationReceipt {
    observation: Option<Arc<SourceObservation>>,
    running: Option<crate::capability::policy_runtime::RunningCapabilityCall>,
}

impl SourceObservationReceipt {
    pub(crate) fn new(
        observation: Option<Arc<SourceObservation>>,
        running: Option<crate::capability::policy_runtime::RunningCapabilityCall>,
    ) -> Self {
        Self {
            observation,
            running,
        }
    }
    pub(crate) fn flush(self) {
        if let Some(observation) = self.observation {
            observation.flush();
        }
        drop(self.running);
    }
}

/// Shared composition, not a source catalog or an authorization authority.
#[derive(Clone)]
pub(crate) struct SourceEventFactory {
    pub(crate) policy: Arc<crate::capability::policy::PolicyEngine>,
    pub(crate) emitter: CapabilityEventEmitter,
}

impl SourceEventFactory {
    pub(crate) fn tool(
        &self,
        descriptor: &crate::tools::descriptor::ToolCapabilityDescriptor,
    ) -> Arc<SourceObservation> {
        self.system(crate::capability::backend::to_descriptor(descriptor))
    }

    pub(crate) fn system(
        &self,
        descriptor: crate::capability::descriptor::CapabilityDescriptor,
    ) -> Arc<SourceObservation> {
        let incarnation = uuid::Uuid::new_v4().to_string();
        SourceObservation::new(
            descriptor,
            EventAudience::OperatorOnly,
            incarnation,
            self.emitter.clone(),
        )
    }

    pub(crate) fn principal(
        &self,
        descriptor: crate::capability::descriptor::CapabilityDescriptor,
        call: &crate::capability::policy_runtime::CapabilityCallContext,
    ) -> Option<Arc<SourceObservation>> {
        call.principal().filter(|principal| !principal.is_empty())?;
        let incarnation = uuid::Uuid::new_v4().to_string();
        Some(SourceObservation::new(
            descriptor,
            EventAudience::from_call(call, Some(incarnation.clone())),
            incarnation,
            self.emitter.clone(),
        ))
    }
}

/// One actual operation completion, retained by detached-from-caller source owners.
#[derive(Clone)]
pub(crate) struct InvocationObservation(Arc<InvocationObservationInner>);

struct InvocationObservationInner {
    id: CapabilityId,
    actor: OwnerRef,
    audience: EventAudience,
    emitter: CapabilityEventEmitter,
    completed: std::sync::atomic::AtomicBool,
    _running: Option<crate::capability::policy_runtime::RunningCapabilityCall>,
}

impl InvocationObservation {
    pub(crate) fn new(
        lease: &crate::capability::facade::BackendLease,
        call: &crate::capability::policy_runtime::CapabilityCallContext,
        emitter: CapabilityEventEmitter,
        running: crate::capability::policy_runtime::RunningCapabilityCall,
    ) -> Option<Self> {
        let private = lease.descriptor.visibility.principal.is_some()
            || matches!(
                lease.descriptor.kind,
                crate::capability::descriptor::CapabilityKind::Terminal
                    | crate::capability::descriptor::CapabilityKind::Ui
            );
        if private && call.principal().is_none() {
            return None;
        }
        Some(Self(Arc::new(InvocationObservationInner {
            id: lease.descriptor.id.clone(),
            actor: call.actor().clone(),
            audience: invocation_audience(lease, call),
            emitter,
            completed: std::sync::atomic::AtomicBool::new(false),
            _running: Some(running),
        })))
    }

    pub(crate) fn complete(&self, result: HandlerResult) {
        if self
            .0
            .completed
            .swap(true, std::sync::atomic::Ordering::SeqCst)
        {
            return;
        }
        let event = if result == HandlerResult::ExecutionFailed {
            CapabilityEvent::ExecutionFailed {
                id: self.0.id.clone(),
                operation: "invoke".into(),
                reason: "execution_failed".into(),
            }
        } else {
            CapabilityEvent::CapabilityInvoked {
                id: self.0.id.clone(),
                operation: "invoke".into(),
                handler_result: result,
            }
        };
        self.0
            .emitter
            .emit_scoped(self.0.actor.clone(), self.0.audience.clone(), event);
    }
}

pub(crate) fn invocation_audience(
    lease: &crate::capability::facade::BackendLease,
    call: &crate::capability::policy_runtime::CapabilityCallContext,
) -> EventAudience {
    use crate::capability::facade::BackendSourceEvidence;
    let incarnation = match &lease.source {
        BackendSourceEvidence::Tool {
            canonical_entry, ..
        } => canonical_entry
            .entry()
            .observation
            .as_ref()
            .map(|observation| observation.incarnation().to_owned()),
        BackendSourceEvidence::Special { capture, .. } => capture
            .source_observation()
            .map(|observation| observation.incarnation().to_owned()),
        BackendSourceEvidence::Provider {
            provider_id,
            provider_generation,
        } => Some(format!("provider/{provider_id:?}/{provider_generation}")),
        BackendSourceEvidence::MemoryResource(source) => source
            .upgrade()
            .and_then(|runtime| {
                runtime.source_observation_for_binding(
                    &crate::capability::ownership::CapturedBinding {
                        capability: lease.descriptor.id.clone(),
                        visibility: lease.descriptor.visibility.clone(),
                        generation: lease.owner_generation,
                        owner: lease.owner.clone(),
                        lifetime: lease.descriptor.lifetime,
                    },
                )
            })
            .map(|observation| observation.incarnation().to_owned()),
    };
    EventAudience::from_call(call, incarnation)
}

/// An observational decorator; policy decisions still belong exclusively to PolicyEngine.
/// Resource-free delegation/cross-session helpers remain on the underlying engine.
pub struct AuditedPolicyEngine {
    inner: crate::capability::policy::PolicyEngine,
    events: CapabilityEventEmitter,
}

#[expect(
    clippy::result_large_err,
    reason = "Observation preserves the existing PolicyEngine denial type without an API migration."
)]
impl AuditedPolicyEngine {
    pub(crate) fn new(
        inner: crate::capability::policy::PolicyEngine,
        events: CapabilityEventEmitter,
    ) -> Self {
        Self { inner, events }
    }

    pub fn resolve(
        &self,
        capability: &CapabilityId,
        principal: &OwnerRef,
        visibility: &VisibilityScope,
        descriptor: &crate::capability::descriptor::CapabilityDescriptor,
    ) -> Result<crate::capability::policy::CapabilityLease, crate::capability::policy::PolicyDenial>
    {
        self.inner
            .resolve(capability, principal, visibility, descriptor)
            .inspect_err(|reason| self.denied(capability, principal, reason))
    }

    pub fn validate_lease(
        &self,
        lease: &crate::capability::policy::CapabilityLease,
    ) -> Result<(), crate::capability::policy::PolicyDenial> {
        self.inner
            .validate_lease(lease)
            .inspect_err(|reason| self.denied(&lease.capability, &lease.owner, reason))
    }

    fn denied(
        &self,
        id: &CapabilityId,
        actor: &OwnerRef,
        reason: &crate::capability::policy::PolicyDenial,
    ) {
        self.events.emit(
            actor.clone(),
            CapabilityEvent::PolicyViolation {
                id: id.clone(),
                reason: reason.to_string(),
            },
        );
    }

    pub fn tree(&self) -> &crate::capability::ownership::OwnershipTree {
        self.inner.tree()
    }
}

/// Opt-in Zahir refusal observation, not a second registry or a new authority.
/// Provider activation effects, Tool execution, and ownership invalidation are
/// not reinterpreted as lifecycle events by this read-side decorator.
pub struct AuditedZahir {
    inner: Arc<dyn crate::capability::facade::Zahir>,
    actor: OwnerRef,
    events: CapabilityEventEmitter,
}

impl AuditedZahir {
    pub fn new(
        inner: Arc<dyn crate::capability::facade::Zahir>,
        actor: OwnerRef,
        events: CapabilityEventEmitter,
    ) -> Self {
        Self {
            inner,
            actor,
            events,
        }
    }

    fn denied(&self, id: &CapabilityId, reason: String) {
        self.events.emit(
            self.actor.clone(),
            CapabilityEvent::PolicyViolation {
                id: id.clone(),
                reason,
            },
        );
    }
}

impl crate::capability::facade::Zahir for AuditedZahir {
    fn describe(
        &self,
        scope: crate::capability::facade::Scope,
    ) -> crate::capability::facade::CapabilitySnapshot {
        self.inner.describe(scope)
    }

    fn descriptor_revision(
        &self,
        reference: &crate::capability::facade::Reference,
    ) -> Option<crate::capability::descriptor::CapabilityRevision> {
        self.inner.descriptor_revision(reference)
    }

    fn lookup_descriptor(
        &self,
        reference: &crate::capability::facade::Reference,
    ) -> Option<crate::capability::descriptor::CapabilityDescriptor> {
        self.inner.lookup_descriptor(reference)
    }

    fn lookup_descriptor_for_cache(
        &self,
        reference: &crate::capability::facade::Reference,
    ) -> Option<(crate::capability::descriptor::CapabilityDescriptor, bool)> {
        self.inner.lookup_descriptor_for_cache(reference)
    }

    fn resolve(
        &self,
        reference: crate::capability::facade::Reference,
    ) -> Result<crate::capability::facade::BackendLease, crate::capability::facade::ResolveError>
    {
        let id = reference.id.clone();
        self.inner
            .resolve(reference)
            .inspect_err(|reason| self.denied(&id, format!("{reason:?}")))
    }

    fn validate_lease(
        &self,
        lease: &crate::capability::facade::BackendLease,
    ) -> Result<(), crate::capability::facade::ResolveError> {
        self.inner
            .validate_lease(lease)
            .inspect_err(|reason| self.denied(&lease.descriptor.id, format!("{reason:?}")))
    }

    fn activate(
        &self,
        reference: crate::capability::facade::Reference,
    ) -> Result<
        crate::capability::facade::BackendLease,
        crate::capability::provider_registry::ActivationError,
    > {
        use crate::capability::provider_registry::ActivationError;
        let id = reference.id.clone();
        self.inner
            .activate(reference)
            .inspect_err(|reason| match reason {
                ActivationError::Failed => self.events.emit(
                    self.actor.clone(),
                    CapabilityEvent::ExecutionFailed {
                        id: id.clone(),
                        operation: "activate".into(),
                        reason: reason.to_string(),
                    },
                ),
                ActivationError::Rejected
                | ActivationError::NotVisible
                | ActivationError::Unknown => {
                    self.denied(&id, reason.to_string());
                }
            })
    }

    fn subscribe(
        &self,
        scope: crate::capability::facade::Scope,
        cursor: crate::capability::facade::Cursor,
    ) -> crate::capability::facade::CapabilityChangeStream {
        self.inner.subscribe(scope, cursor)
    }

    fn subscribe_events(
        &self,
        scope: TrustedEventScope,
        filter: EventFilter,
    ) -> Result<ScopedEventSubscription, EventError> {
        self.inner.subscribe_events(scope, filter)
    }

    fn project(
        &self,
        target: crate::capability::facade::TransportTarget,
        scope: crate::capability::facade::Scope,
    ) -> crate::capability::facade::Projection {
        self.inner.project(target, scope)
    }
}

/// Activation metadata is observational only and shared unchanged by controller clones.
#[derive(Debug, Clone)]
pub(crate) struct ActivationEventHook {
    pub id: CapabilityId,
    pub actor: OwnerRef,
    pub emitter: CapabilityEventEmitter,
    lease_id: String,
    constraints_applied: usize,
}

impl ActivationEventHook {
    pub fn new(
        descriptor: &crate::capability::descriptor::CapabilityDescriptor,
        constraints_applied: usize,
        emitter: CapabilityEventEmitter,
    ) -> Self {
        emitter.emit(
            descriptor.owner.clone(),
            CapabilityEvent::CapabilityCreated {
                id: descriptor.id.clone(),
                owner: descriptor.owner.clone(),
                visibility: Box::new(descriptor.visibility.clone()),
            },
        );
        Self {
            id: descriptor.id.clone(),
            actor: descriptor.owner.clone(),
            emitter,
            lease_id: uuid::Uuid::new_v4().to_string(),
            constraints_applied,
        }
    }

    pub fn violation(&self, violation: &crate::capability::constraints::ConstraintViolation) {
        use crate::capability::constraints::ConstraintViolation;
        let constraint_type = match violation {
            ConstraintViolation::InvalidTimeWindow { .. }
            | ConstraintViolation::TimeWindowClosed { .. } => "time",
            ConstraintViolation::ResourceLimitExceeded { .. } => "resource",
            ConstraintViolation::ConditionNotMet { .. } => "conditional",
            ConstraintViolation::InvalidContext { .. } | ConstraintViolation::ClockUnavailable => {
                "context"
            }
        };
        self.emitter.emit(
            self.actor.clone(),
            CapabilityEvent::ConstraintViolation {
                id: self.id.clone(),
                constraint_type: constraint_type.into(),
            },
        );
    }
}

impl crate::capability::activation::ActivationHook for ActivationEventHook {
    fn after_activation(&self, _: &crate::capability::constraints::ConstraintContext) {
        self.emitter.emit(
            self.actor.clone(),
            CapabilityEvent::CapabilityActivated {
                id: self.id.clone(),
                lease_id: self.lease_id.clone(),
                constraints_applied: self.constraints_applied,
            },
        );
    }

    fn after_revocation(&self) {
        self.emitter.emit(
            self.actor.clone(),
            CapabilityEvent::CapabilityRevoked {
                id: self.id.clone(),
                reason: "requested".into(),
            },
        );
    }
}

#[cfg(test)]
#[path = "events_tests.rs"]
mod integration_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability::ownership::OwnerRef;

    #[test]
    fn emit_collects_without_subscribers() {
        let stream = InMemoryEventStream::new(4).unwrap();
        stream
            .emit(
                OwnerRef::Runtime,
                CapabilityEvent::CapabilityRevoked {
                    id: CapabilityId {
                        namespace: "test".into(),
                        name: "one".into(),
                    },
                    reason: "requested".into(),
                },
            )
            .unwrap();
        assert_eq!(stream.snapshot().unwrap().records().len(), 1);
    }
}
