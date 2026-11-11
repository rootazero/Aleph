//! Immutable audit projections of capability events.
//!
//! Collection shares the event buffer: there is no lossy drain, second log, or
//! thread-local state. Entries are read-only views, not cryptographic evidence.
//! Compliance queries fail closed after retention loss; persistence and recovery
//! require a separately supplied persistence sink and are not claimed here.

use crate::capability::descriptor::CapabilityId;
use crate::capability::event_audience::{EventAudience, TrustedEventScope};
use crate::capability::events::{
    CapabilityEvent, CapabilityEventRecord, CapabilityEventType, EventError, EventFilter,
    HandlerResult, InMemoryEventStream,
};
use crate::capability::ownership::OwnerRef;
use crate::sync_primitives::Arc;
use chrono::{DateTime, Utc};
use serde::{ser::SerializeStruct, Serialize, Serializer};

#[derive(Debug, thiserror::Error)]
pub enum AuditError {
    #[error(transparent)]
    Event(#[from] EventError),
    #[error("audit time range starts after it ends")]
    InvalidTimeRange,
    #[error("audit history is incomplete: {dropped} records evicted")]
    IncompleteHistory { dropped: u64 },
    #[error("audit JSON serialization failed: {0}")]
    Serialization(#[from] serde_json::Error),
}

/// No public fields, setters, Deserialize, or collector write API.
///
/// ```compile_fail
/// use alephcore::capability::audit::AuditLog;
/// use alephcore::capability::ownership::OwnerRef;
/// fn tamper(entry: &mut AuditLog) { entry.actor = OwnerRef::Runtime; }
/// ```
#[derive(Debug, Clone)]
pub struct AuditLog {
    record: Arc<CapabilityEventRecord>,
}

impl AuditLog {
    pub fn timestamp(&self) -> DateTime<Utc> {
        self.record.timestamp()
    }
    pub fn event_type(&self) -> CapabilityEventType {
        self.record.event().event_type()
    }
    pub fn actor(&self) -> &OwnerRef {
        self.record.actor()
    }
    pub fn audience(&self) -> &EventAudience {
        self.record.audience()
    }
    pub fn resource(&self) -> &CapabilityId {
        self.record.event().id()
    }
    pub fn event(&self) -> &CapabilityEvent {
        self.record.event()
    }
    pub fn sequence(&self) -> u64 {
        self.record.sequence()
    }

    pub fn action(&self) -> &str {
        match self.event() {
            CapabilityEvent::CapabilityCreated { .. } => "created",
            CapabilityEvent::CapabilityAttached { .. } => "attached",
            CapabilityEvent::CapabilityActivated { .. } => "activated",
            CapabilityEvent::CapabilityDetaching { .. } => "detaching",
            CapabilityEvent::CapabilityInvoked { operation, .. } => operation,
            CapabilityEvent::CapabilityRevoked { .. } => "revoked",
            CapabilityEvent::ConstraintViolation { .. } => "constraint_check",
            CapabilityEvent::PolicyViolation { .. } => "policy_check",
            CapabilityEvent::ExecutionFailed { operation, .. } => operation,
        }
    }

    pub fn result(&self) -> &str {
        match self.event() {
            CapabilityEvent::CapabilityInvoked { handler_result, .. } => match handler_result {
                HandlerResult::Success => "success",
                HandlerResult::InvalidInput => "invalid_input",
                HandlerResult::ExecutionFailed => "execution_failed",
                HandlerResult::NotFound => "not_found",
            },
            CapabilityEvent::ConstraintViolation { .. }
            | CapabilityEvent::PolicyViolation { .. } => "denied",
            CapabilityEvent::ExecutionFailed { .. } => "failed",
            CapabilityEvent::CapabilityCreated { .. }
            | CapabilityEvent::CapabilityAttached { .. }
            | CapabilityEvent::CapabilityActivated { .. }
            | CapabilityEvent::CapabilityDetaching { .. }
            | CapabilityEvent::CapabilityRevoked { .. } => "success",
        }
    }
}

impl Serialize for AuditLog {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut state = serializer.serialize_struct("AuditLog", 9)?;
        state.serialize_field("sequence", &self.sequence())?;
        state.serialize_field("timestamp", &self.timestamp())?;
        state.serialize_field("event_type", &self.event_type())?;
        state.serialize_field("actor", self.actor())?;
        state.serialize_field("audience", self.audience())?;
        state.serialize_field("resource", self.resource())?;
        state.serialize_field("action", self.action())?;
        state.serialize_field("result", self.result())?;
        state.serialize_field("event", self.event())?;
        state.end()
    }
}

/// Inclusive UTC time bounds. All specified filters are combined with AND.
#[derive(Debug, Clone, Default, Serialize)]
pub struct AuditQuery {
    pub from: Option<DateTime<Utc>>,
    pub through: Option<DateTime<Utc>>,
    pub event_type: Option<CapabilityEventType>,
    pub resource: Option<CapabilityId>,
    pub actor: Option<OwnerRef>,
}

#[derive(Debug, Serialize)]
pub struct ComplianceReport {
    generated_at: DateTime<Utc>,
    query: AuditQuery,
    scope: &'static str,
    entries: Vec<AuditLog>,
}

impl ComplianceReport {
    pub fn entries(&self) -> &[AuditLog] {
        &self.entries
    }
    pub fn to_json(&self) -> Result<String, AuditError> {
        Ok(serde_json::to_string_pretty(self)?)
    }
}

pub struct AuditCollector {
    stream: Arc<InMemoryEventStream>,
}

impl AuditCollector {
    pub fn new(stream: Arc<InMemoryEventStream>) -> Self {
        Self { stream }
    }

    pub fn query(&self, query: &AuditQuery) -> Result<Vec<AuditLog>, AuditError> {
        if matches!((query.from, query.through), (Some(from), Some(through)) if from > through) {
            return Err(AuditError::InvalidTimeRange);
        }
        let snapshot = self.stream.snapshot()?;
        if snapshot.dropped() > 0 {
            return Err(AuditError::IncompleteHistory {
                dropped: snapshot.dropped(),
            });
        }
        let filter = EventFilter {
            event_type: query.event_type,
            resource: query.resource.clone(),
            actor: query.actor.clone(),
        };
        Ok(snapshot
            .records()
            .iter()
            .filter(|record| {
                filter.matches(record)
                    && query.from.is_none_or(|from| record.timestamp() >= from)
                    && query
                        .through
                        .is_none_or(|through| record.timestamp() <= through)
            })
            .map(|record| AuditLog {
                record: Arc::clone(record),
            })
            .collect())
    }

    /// Query the same retained cut, then apply CURRENT request access outside the lock.
    pub async fn query_scoped(
        &self,
        scope: &TrustedEventScope,
        query: &AuditQuery,
    ) -> Result<Vec<AuditLog>, AuditError> {
        let mut visible = Vec::new();
        for entry in self.query(query)? {
            if scope.admits(&entry.record).await {
                visible.push(entry);
            }
        }
        Ok(visible)
    }

    pub fn report(&self, query: &AuditQuery) -> Result<ComplianceReport, AuditError> {
        Ok(ComplianceReport {
            generated_at: Utc::now(),
            query: query.clone(),
            scope: "observed events in this stream instance; not process-wide or durable",
            entries: self.query(query)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability::descriptor::CapabilityId;
    use crate::capability::events::{CapabilityEvent, CapabilityEventStream, InMemoryEventStream};
    use crate::capability::ownership::OwnerRef;
    use crate::sync_primitives::Arc;

    #[test]
    fn audit_collects_from_the_event_source_without_a_drain_worker() {
        let stream = Arc::new(InMemoryEventStream::new(4).unwrap());
        let audit = AuditCollector::new(Arc::clone(&stream));
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
        let entries = audit.query(&AuditQuery::default()).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].action(), "revoked");
    }

    fn resource(name: &str) -> CapabilityId {
        CapabilityId {
            namespace: "test/audit".into(),
            name: name.into(),
        }
    }

    fn activation(name: &str) -> CapabilityEvent {
        CapabilityEvent::CapabilityActivated {
            id: resource(name),
            lease_id: "audit-correlation".into(),
            constraints_applied: 2,
        }
    }

    #[test]
    fn empty_audit_produces_an_explicitly_instance_scoped_report() {
        let audit = AuditCollector::new(Arc::new(InMemoryEventStream::new(4).unwrap()));
        let report = audit.report(&AuditQuery::default()).unwrap();
        assert!(report.entries().is_empty());
        let json: serde_json::Value = serde_json::from_str(&report.to_json().unwrap()).unwrap();
        assert!(json["scope"]
            .as_str()
            .unwrap()
            .contains("not process-wide or durable"));
        assert!(json["generated_at"].is_string());
    }

    #[test]
    fn time_range_bounds_are_inclusive_and_do_not_leak_adjacent_times() {
        let stream = Arc::new(InMemoryEventStream::new(4).unwrap());
        stream.emit(OwnerRef::Runtime, activation("one")).unwrap();
        let timestamp = stream.snapshot().unwrap().records()[0].timestamp();
        let audit = AuditCollector::new(stream);
        assert_eq!(
            audit
                .query(&AuditQuery {
                    from: Some(timestamp),
                    through: Some(timestamp),
                    ..Default::default()
                })
                .unwrap()
                .len(),
            1
        );
        assert!(audit
            .query(&AuditQuery {
                from: Some(timestamp + chrono::Duration::nanoseconds(1)),
                ..Default::default()
            })
            .unwrap()
            .is_empty());
        assert!(audit
            .query(&AuditQuery {
                through: Some(timestamp - chrono::Duration::nanoseconds(1)),
                ..Default::default()
            })
            .unwrap()
            .is_empty());
    }

    #[test]
    fn reversed_time_range_returns_a_typed_error() {
        let audit = AuditCollector::new(Arc::new(InMemoryEventStream::new(4).unwrap()));
        let now = Utc::now();
        assert!(matches!(
            audit.report(&AuditQuery {
                from: Some(now),
                through: Some(now - chrono::Duration::seconds(1)),
                ..Default::default()
            }),
            Err(AuditError::InvalidTimeRange)
        ));
    }

    #[test]
    fn query_combines_event_type_actor_and_resource_filters() {
        use crate::capability::ownership::RunId;
        let stream = Arc::new(InMemoryEventStream::new(8).unwrap());
        stream.emit(OwnerRef::Runtime, activation("one")).unwrap();
        stream
            .emit(OwnerRef::Run(RunId("other".into())), activation("one"))
            .unwrap();
        stream.emit(OwnerRef::Runtime, activation("two")).unwrap();
        stream
            .emit(
                OwnerRef::Runtime,
                CapabilityEvent::PolicyViolation {
                    id: resource("one"),
                    reason: "denied".into(),
                },
            )
            .unwrap();
        let entries = AuditCollector::new(stream)
            .query(&AuditQuery {
                event_type: Some(CapabilityEventType::Activated),
                actor: Some(OwnerRef::Runtime),
                resource: Some(resource("one")),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].sequence(), 1);
    }

    #[test]
    fn report_serializes_filtered_entries_as_structured_json() {
        let stream = Arc::new(InMemoryEventStream::new(8).unwrap());
        stream.emit(OwnerRef::Runtime, activation("one")).unwrap();
        stream
            .emit(
                OwnerRef::Runtime,
                CapabilityEvent::PolicyViolation {
                    id: resource("one"),
                    reason: "denied".into(),
                },
            )
            .unwrap();
        let audit = AuditCollector::new(stream);
        let report = audit
            .report(&AuditQuery {
                event_type: Some(CapabilityEventType::Activated),
                ..Default::default()
            })
            .unwrap();
        let json: serde_json::Value = serde_json::from_str(&report.to_json().unwrap()).unwrap();
        assert_eq!(json["entries"].as_array().unwrap().len(), 1);
        let entry = &json["entries"][0];
        assert_eq!(entry["actor"], "Runtime");
        assert_eq!(entry["resource"]["namespace"], "test/audit");
        assert_eq!(entry["action"], "activated");
        assert_eq!(entry["event_type"], "activated");
        assert_eq!(entry["result"], "success");
        assert!(entry["timestamp"].is_string());
        assert_eq!(entry["event"]["constraints_applied"], 2);
    }

    #[test]
    fn reports_refuse_to_claim_completeness_after_retention_loss() {
        let stream = Arc::new(InMemoryEventStream::new(1).unwrap());
        stream.emit(OwnerRef::Runtime, activation("one")).unwrap();
        stream.emit(OwnerRef::Runtime, activation("two")).unwrap();
        let audit = AuditCollector::new(stream);
        assert!(matches!(
            audit.query(&AuditQuery::default()),
            Err(AuditError::IncompleteHistory { dropped: 1 })
        ));
        assert!(matches!(
            audit.report(&AuditQuery::default()),
            Err(AuditError::IncompleteHistory { dropped: 1 })
        ));
    }

    #[test]
    fn audit_entries_do_not_change_when_later_events_are_emitted() {
        let stream = Arc::new(InMemoryEventStream::new(4).unwrap());
        stream.emit(OwnerRef::Runtime, activation("one")).unwrap();
        let audit = AuditCollector::new(stream.clone());
        let original = audit.query(&AuditQuery::default()).unwrap();
        let serialized = serde_json::to_string(&original[0]).unwrap();
        stream.emit(OwnerRef::Runtime, activation("two")).unwrap();
        assert_eq!(serde_json::to_string(&original[0]).unwrap(), serialized);
        assert_eq!(original.len(), 1);
        assert_eq!(audit.query(&AuditQuery::default()).unwrap().len(), 2);
    }

    #[test]
    fn invocation_results_and_security_denials_have_distinct_audit_results() {
        let stream = Arc::new(InMemoryEventStream::new(8).unwrap());
        for result in [
            HandlerResult::Success,
            HandlerResult::InvalidInput,
            HandlerResult::ExecutionFailed,
            HandlerResult::NotFound,
        ] {
            stream
                .emit(
                    OwnerRef::Runtime,
                    CapabilityEvent::CapabilityInvoked {
                        id: resource("one"),
                        operation: "read".into(),
                        handler_result: result,
                    },
                )
                .unwrap();
        }
        stream
            .emit(
                OwnerRef::Runtime,
                CapabilityEvent::ConstraintViolation {
                    id: resource("one"),
                    constraint_type: "time".into(),
                },
            )
            .unwrap();
        stream
            .emit(
                OwnerRef::Runtime,
                CapabilityEvent::PolicyViolation {
                    id: resource("one"),
                    reason: "not_visible".into(),
                },
            )
            .unwrap();
        let entries = AuditCollector::new(stream)
            .query(&AuditQuery::default())
            .unwrap();
        assert_eq!(
            entries.iter().map(AuditLog::result).collect::<Vec<_>>(),
            vec![
                "success",
                "invalid_input",
                "execution_failed",
                "not_found",
                "denied",
                "denied"
            ]
        );
        assert_eq!(entries[4].action(), "constraint_check");
        assert_eq!(entries[5].action(), "policy_check");
    }

    #[test]
    fn named_session_identity_and_visibility_serialize_without_collapsing_names() {
        use crate::acp::manager::SessionKey;
        let session =
            SessionKey::with_name("harness", "/tmp/audit-workspace", Some("named-session"));
        let owner = OwnerRef::Session(session.clone());
        let stream = Arc::new(InMemoryEventStream::new(4).unwrap());
        stream
            .emit(
                owner.clone(),
                CapabilityEvent::CapabilityCreated {
                    id: resource("one"),
                    owner,
                    visibility: Box::new(crate::capability::ownership::VisibilityScope {
                        session: Some(session),
                        ..Default::default()
                    }),
                },
            )
            .unwrap();
        let report = AuditCollector::new(stream)
            .report(&AuditQuery::default())
            .unwrap();
        let json: serde_json::Value = serde_json::from_str(&report.to_json().unwrap()).unwrap();
        assert_eq!(
            json["entries"][0]["actor"]["Session"]["name"],
            "named-session"
        );
        assert_eq!(
            json["entries"][0]["event"]["visibility"]["session"]["harness_id"],
            "harness"
        );
    }

    #[test]
    fn concurrent_collection_reads_coherent_immutable_prefixes() {
        let stream = Arc::new(InMemoryEventStream::new(1_024).unwrap());
        let audit = AuditCollector::new(stream.clone());
        std::thread::scope(|scope| {
            scope.spawn(|| {
                for _ in 0..400 {
                    stream.emit(OwnerRef::Runtime, activation("one")).unwrap();
                }
            });
            for _ in 0..100 {
                let entries = audit.query(&AuditQuery::default()).unwrap();
                for (index, entry) in entries.iter().enumerate() {
                    assert_eq!(entry.sequence(), index as u64 + 1);
                }
            }
        });
        assert_eq!(audit.query(&AuditQuery::default()).unwrap().len(), 400);
    }
}
