//! Bounded, instance-scoped capability observations, never an authority or replay log.
//!
//! Records commit to memory before live notification and optional persistence.
//! Persistence failure is reported *after* that commit; retrying emission would
//! duplicate an observation. Persistence hooks run outside the buffer lock and
//! may complete out of order, so sinks must order by record sequence if needed.

use crate::capability::descriptor::CapabilityId;
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
    /// Live-only, with no historical replay; subscribe before starting producers.
    fn subscribe(&self, filter: EventFilter) -> EventSubscription;
}

pub struct EventSubscription {
    receiver: broadcast::Receiver<Arc<CapabilityEventRecord>>,
    filter: EventFilter,
}

impl EventSubscription {
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

#[derive(Default)]
struct Buffer {
    records: VecDeque<Arc<CapabilityEventRecord>>,
    sequence: u64,
    dropped: u64,
}

pub struct InMemoryEventStream {
    capacity: usize,
    buffer: Mutex<Buffer>,
    sender: broadcast::Sender<Arc<CapabilityEventRecord>>,
    persistence: Option<Arc<dyn EventPersistence>>,
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
        Ok(Self {
            capacity,
            buffer: Mutex::new(Buffer::default()),
            sender: broadcast::channel(capacity).0,
            persistence,
        })
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
        let record = {
            let mut buffer = self.buffer.lock().map_err(|_| EventError::Poisoned)?;
            let sequence = buffer
                .sequence
                .checked_add(1)
                .ok_or(EventError::SequenceExhausted)?;
            let record = Arc::new(CapabilityEventRecord {
                sequence,
                timestamp: Utc::now(),
                actor,
                event,
            });
            if buffer.records.len() == self.capacity {
                buffer.records.pop_front();
                buffer.dropped += 1;
            }
            buffer.records.push_back(Arc::clone(&record));
            buffer.sequence = sequence;
            // No listeners is normal. Memory collection does not depend on live subscribers.
            let _ = self.sender.send(Arc::clone(&record));
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
            receiver: self.sender.subscribe(),
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
