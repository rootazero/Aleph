use super::*;
use crate::capability::activation::{ActivationError, CapabilityActivationController};
use crate::capability::backend::{
    CapabilityBackend, CapabilityDispatchError, CapabilityHandler, CapabilityHandlerRegistry,
};
use crate::capability::constraints::{Constraint, ConstraintContext};
use crate::capability::descriptor::{
    CapabilityDescriptor, CapabilityFacet, CapabilityFacets, CapabilityKind, CapabilityRevision,
    ProjectionMetadata, SchemaRef,
};
use crate::capability::ownership::{LifetimeScope, OwnerGeneration, OwnershipTree, RunId};
use crate::capability::policy::PolicyEngine;
use serde_json::{json, Value};

fn id(name: &str) -> CapabilityId {
    CapabilityId {
        namespace: "test/events".into(),
        name: name.into(),
    }
}

fn revoked(name: &str) -> CapabilityEvent {
    CapabilityEvent::CapabilityRevoked {
        id: id(name),
        reason: "requested".into(),
    }
}

fn descriptor() -> CapabilityDescriptor {
    CapabilityDescriptor {
        id: id("one"),
        kind: CapabilityKind::Ambient,
        facets: CapabilityFacets::new().with(CapabilityFacet::Callable),
        owner: OwnerRef::Runtime,
        lifetime: LifetimeScope::Runtime,
        visibility: VisibilityScope::default(),
        revision: CapabilityRevision(1),
        services: vec![],
        schema: SchemaRef {
            version: 1,
            fingerprint: [0; 32],
        },
        metadata: ProjectionMetadata::default(),
    }
}

fn emitter(stream: &Arc<InMemoryEventStream>) -> CapabilityEventEmitter {
    CapabilityEventEmitter::new(Arc::clone(stream) as Arc<dyn CapabilityEventStream>)
}

#[test]
fn legacy_event_envelope_has_operator_only_audience() {
    let stream = InMemoryEventStream::new(1).unwrap();
    stream.emit(OwnerRef::Runtime, revoked("legacy")).unwrap();
    let snapshot = stream.snapshot().unwrap();
    let record = serde_json::to_value(&snapshot.records()[0]).unwrap();
    assert_eq!(record["audience"], json!({ "kind": "operator_only" }));
}

#[test]
fn legacy_faces_fail_closed_for_unsupported_scoped_events() {
    use crate::capability::facade::{
        BackendLease, CapabilityChangeStream, CapabilitySnapshot, Cursor, Projection, Reference,
        ResolveError, Scope, TransportTarget, Zahir,
    };
    struct LegacyFace;
    impl Zahir for LegacyFace {
        fn describe(&self, _: Scope) -> CapabilitySnapshot {
            unreachable!("not used by scoped default")
        }
        fn resolve(&self, _: Reference) -> Result<BackendLease, ResolveError> {
            unreachable!("not used by scoped default")
        }
        fn validate_lease(&self, _: &BackendLease) -> Result<(), ResolveError> {
            unreachable!("not used by scoped default")
        }
        fn activate(
            &self,
            _: Reference,
        ) -> Result<BackendLease, crate::capability::provider_registry::ActivationError> {
            unreachable!("not used by scoped default")
        }
        fn subscribe(&self, _: Scope, _: Cursor) -> CapabilityChangeStream {
            unreachable!("not used by scoped default")
        }
        fn project(&self, _: TransportTarget, _: Scope) -> Projection {
            unreachable!("not used by scoped default")
        }
    }
    struct MustNotRun;
    #[async_trait::async_trait]
    impl crate::capability::event_audience::CapabilityEventAccess for MustNotRun {
        async fn admits(&self, _: &CapabilityEventRecord) -> bool {
            panic!("unsupported face must not invoke access")
        }
    }
    struct LegacyStream(InMemoryEventStream);
    impl CapabilityEventStream for LegacyStream {
        fn emit(&self, actor: OwnerRef, event: CapabilityEvent) -> Result<(), EventError> {
            self.0.emit(actor, event)
        }
        fn subscribe(&self, filter: EventFilter) -> EventSubscription {
            self.0.subscribe(filter)
        }
    }
    let scope = TrustedEventScope::from_access(Arc::new(MustNotRun));
    assert!(matches!(
        LegacyFace.subscribe_events(scope.clone(), EventFilter::default()),
        Err(EventError::Unsupported)
    ));
    let stream = Arc::new(LegacyStream(InMemoryEventStream::new(2).unwrap()));
    assert!(matches!(
        stream.emit_scoped(
            OwnerRef::Runtime,
            EventAudience::OperatorOnly,
            revoked("unsupported")
        ),
        Err(EventError::Unsupported)
    ));
    CapabilityEventEmitter::new(stream.clone()).emit_scoped(
        OwnerRef::Runtime,
        EventAudience::OperatorOnly,
        revoked("best-effort"),
    );
    assert!(stream.0.snapshot().unwrap().records().is_empty());
    let audited = AuditedZahir::new(
        Arc::new(LegacyFace),
        OwnerRef::Runtime,
        CapabilityEventEmitter::new(stream),
    );
    assert!(matches!(
        audited.subscribe_events(scope, EventFilter::default()),
        Err(EventError::Unsupported)
    ));
}

#[test]
fn rejects_zero_capacity_without_panicking() {
    assert!(matches!(
        InMemoryEventStream::new(0),
        Err(EventError::InvalidCapacity)
    ));
    assert!(matches!(
        InMemoryEventStream::new(usize::MAX),
        Err(EventError::InvalidCapacity)
    ));
}

#[test]
fn subscribers_receive_future_events_in_sequence_order() {
    let stream = InMemoryEventStream::new(8).unwrap();
    stream.emit(OwnerRef::Runtime, revoked("old")).unwrap();
    let mut sub = stream.subscribe(EventFilter::default());
    assert!(sub.try_recv().unwrap().is_none());
    stream.emit(OwnerRef::Runtime, revoked("new")).unwrap();
    let record = sub.try_recv().unwrap().unwrap();
    assert_eq!(record.sequence(), 2);
    assert_eq!(record.event().id(), &id("new"));
}

#[test]
fn subscription_filters_combine_actor_resource_and_type() {
    let stream = InMemoryEventStream::new(8).unwrap();
    let mut sub = stream.subscribe(EventFilter {
        actor: Some(OwnerRef::Runtime),
        resource: Some(id("one")),
        event_type: Some(CapabilityEventType::Revoked),
    });
    stream
        .emit(OwnerRef::Run(RunId("other".into())), revoked("one"))
        .unwrap();
    stream.emit(OwnerRef::Runtime, revoked("two")).unwrap();
    stream
        .emit(
            OwnerRef::Runtime,
            CapabilityEvent::PolicyViolation {
                id: id("one"),
                reason: "denied".into(),
            },
        )
        .unwrap();
    stream.emit(OwnerRef::Runtime, revoked("one")).unwrap();
    assert_eq!(sub.try_recv().unwrap().unwrap().sequence(), 4);
    assert!(sub.try_recv().unwrap().is_none());
}

#[test]
fn lag_is_reported_instead_of_silently_skipped() {
    let stream = InMemoryEventStream::new(2).unwrap();
    let mut sub = stream.subscribe(EventFilter::default());
    for _ in 0..6 {
        stream.emit(OwnerRef::Runtime, revoked("one")).unwrap();
    }
    assert!(matches!(sub.try_recv(), Err(EventError::Lagged(4))));
    assert_eq!(sub.try_recv().unwrap().unwrap().sequence(), 5);
}

#[test]
fn closed_subscription_returns_typed_error() {
    let stream = InMemoryEventStream::new(2).unwrap();
    let mut sub = stream.subscribe(EventFilter::default());
    drop(stream);
    assert!(matches!(sub.try_recv(), Err(EventError::Closed)));
}

#[test]
fn high_volume_retention_is_bounded_and_loss_is_counted() {
    let stream = InMemoryEventStream::new(16).unwrap();
    for _ in 0..10_000 {
        stream.emit(OwnerRef::Runtime, revoked("one")).unwrap();
    }
    let snapshot = stream.snapshot().unwrap();
    assert_eq!(snapshot.records().len(), 16);
    assert_eq!(snapshot.dropped(), 9_984);
    assert_eq!(snapshot.records()[0].sequence(), 9_985);
}

#[test]
fn concurrent_emitters_and_subscriptions_observe_the_same_total_order() {
    let stream = Arc::new(InMemoryEventStream::new(1_024).unwrap());
    let mut a = stream.subscribe(EventFilter::default());
    let mut b = stream.subscribe(EventFilter::default());
    std::thread::scope(|scope| {
        for _ in 0..4 {
            let stream = Arc::clone(&stream);
            scope.spawn(move || {
                for _ in 0..100 {
                    stream.emit(OwnerRef::Runtime, revoked("one")).unwrap();
                }
            });
        }
    });
    for sequence in 1..=400 {
        assert_eq!(a.try_recv().unwrap().unwrap().sequence(), sequence);
        assert_eq!(b.try_recv().unwrap().unwrap().sequence(), sequence);
    }
    assert_eq!(stream.snapshot().unwrap().dropped(), 0);
}

struct Persistence(std::sync::Mutex<Vec<u64>>, bool);
impl EventPersistence for Persistence {
    fn persist(&self, record: &CapabilityEventRecord) -> Result<(), String> {
        self.0.lock().unwrap().push(record.sequence());
        if self.1 {
            Err("disk unavailable".into())
        } else {
            Ok(())
        }
    }
}

#[test]
fn persistence_hook_receives_the_committed_record() {
    let persistence = Arc::new(Persistence(std::sync::Mutex::new(vec![]), false));
    let stream = InMemoryEventStream::with_persistence(4, Some(persistence.clone())).unwrap();
    stream.emit(OwnerRef::Runtime, revoked("one")).unwrap();
    assert_eq!(*persistence.0.lock().unwrap(), vec![1]);
}

#[test]
fn persistence_failure_keeps_committed_memory_and_notification() {
    let stream = InMemoryEventStream::with_persistence(
        4,
        Some(Arc::new(Persistence(std::sync::Mutex::new(vec![]), true))),
    )
    .unwrap();
    let mut sub = stream.subscribe(EventFilter::default());
    assert!(matches!(
        stream.emit(OwnerRef::Runtime, revoked("one")),
        Err(EventError::Persistence(_))
    ));
    assert_eq!(stream.snapshot().unwrap().records().len(), 1);
    assert_eq!(sub.try_recv().unwrap().unwrap().sequence(), 1);
}

#[test]
fn snapshots_cannot_change_when_the_live_buffer_changes() {
    let stream = InMemoryEventStream::new(1).unwrap();
    stream.emit(OwnerRef::Runtime, revoked("one")).unwrap();
    let old = stream.snapshot().unwrap();
    stream.emit(OwnerRef::Runtime, revoked("two")).unwrap();
    assert_eq!(old.records()[0].event().id(), &id("one"));
    assert_eq!(old.dropped(), 0);
}

fn observed(
    stream: &Arc<InMemoryEventStream>,
    constraints: Vec<Constraint>,
) -> CapabilityActivationController {
    CapabilityActivationController::new_observed(
        None,
        OwnerGeneration(1),
        constraints,
        &descriptor(),
        emitter(stream),
    )
}

#[test]
fn activation_emits_every_committed_lifecycle_transition() {
    let stream = Arc::new(InMemoryEventStream::new(8).unwrap());
    let mut activation = observed(&stream, vec![Constraint::resource_limit(2)]);
    activation.attach().unwrap();
    activation
        .activate(&ConstraintContext::at_hour(12).unwrap(), OwnerGeneration(1))
        .unwrap();
    activation.detach().unwrap();
    activation.revoke().unwrap();
    assert!(activation.revoke().is_err());
    let snapshot = stream.snapshot().unwrap();
    let kinds: Vec<_> = snapshot
        .records()
        .iter()
        .map(|record| record.event().event_type())
        .collect();
    assert_eq!(
        kinds,
        vec![
            CapabilityEventType::Created,
            CapabilityEventType::Attached,
            CapabilityEventType::Activated,
            CapabilityEventType::Detaching,
            CapabilityEventType::Revoked
        ]
    );
    match snapshot.records()[2].event() {
        CapabilityEvent::CapabilityActivated {
            lease_id,
            constraints_applied,
            ..
        } => {
            assert!(uuid::Uuid::parse_str(lease_id).is_ok());
            assert_eq!(*constraints_applied, 1);
        }
        other => panic!("wrong event: {other:?}"),
    }
}

#[test]
fn rejected_activation_emits_constraint_violation_not_activation() {
    let stream = Arc::new(InMemoryEventStream::new(8).unwrap());
    let mut activation = observed(&stream, vec![Constraint::time_window(9, 17).unwrap()]);
    activation.attach().unwrap();
    assert!(matches!(
        activation.activate(&ConstraintContext::at_hour(18).unwrap(), OwnerGeneration(1)),
        Err(ActivationError::Constraint(_))
    ));
    let snapshot = stream.snapshot().unwrap();
    assert!(
        matches!(snapshot.records().last().unwrap().event(), CapabilityEvent::ConstraintViolation { constraint_type, .. } if constraint_type == "time")
    );
    assert!(!snapshot
        .records()
        .iter()
        .any(|record| record.event().event_type() == CapabilityEventType::Activated));
}

#[test]
fn runtime_constraint_violation_is_audited() {
    let stream = Arc::new(InMemoryEventStream::new(8).unwrap());
    let mut activation = observed(&stream, vec![Constraint::resource_limit(1)]);
    activation.attach().unwrap();
    activation
        .activate(&ConstraintContext::at_hour(12).unwrap(), OwnerGeneration(1))
        .unwrap();
    assert!(activation
        .validate_runtime(
            OwnerGeneration(1),
            &ConstraintContext::at_hour(12)
                .unwrap()
                .with_active_resources(1)
        )
        .is_err());
    assert!(
        matches!(stream.snapshot().unwrap().records().last().unwrap().event(), CapabilityEvent::ConstraintViolation { constraint_type, .. } if constraint_type == "resource")
    );
}

#[test]
fn cloned_controller_does_not_duplicate_creation_or_revocation_events() {
    let stream = Arc::new(InMemoryEventStream::new(8).unwrap());
    let mut activation = observed(&stream, vec![]);
    let mut clone = activation.clone();
    activation.revoke().unwrap();
    assert!(clone.revoke().is_err());
    assert_eq!(stream.snapshot().unwrap().records().len(), 2);
}

#[test]
fn policy_denial_records_requesting_actor_and_resource() {
    let stream = Arc::new(InMemoryEventStream::new(8).unwrap());
    let desc = descriptor();
    let tree = OwnershipTree::new();
    tree.register(
        desc.id.clone(),
        desc.owner.clone(),
        desc.lifetime,
        desc.visibility.clone(),
    )
    .unwrap();
    let policy = PolicyEngine::new(tree).with_events(emitter(&stream));
    let actor = OwnerRef::Run(RunId("intruder".into()));
    assert!(policy
        .resolve(&desc.id, &actor, &desc.visibility, &desc)
        .is_err());
    let snapshot = stream.snapshot().unwrap();
    assert_eq!(snapshot.records()[0].actor(), &actor);
    assert_eq!(snapshot.records()[0].event().id(), &desc.id);
    assert!(matches!(
        snapshot.records()[0].event(),
        CapabilityEvent::PolicyViolation { .. }
    ));
}

#[test]
fn lease_validation_denial_is_audited() {
    let stream = Arc::new(InMemoryEventStream::new(8).unwrap());
    let desc = descriptor();
    let tree = OwnershipTree::new();
    tree.register(
        desc.id.clone(),
        desc.owner.clone(),
        desc.lifetime,
        desc.visibility.clone(),
    )
    .unwrap();
    let policy = PolicyEngine::new(tree).with_events(emitter(&stream));
    let lease = policy
        .resolve(&desc.id, &desc.owner, &desc.visibility, &desc)
        .unwrap();
    policy.tree().revoke(&desc.id);
    assert!(policy.validate_lease(&lease).is_err());
    assert_eq!(stream.snapshot().unwrap().records().len(), 1);
}

struct Echo;
impl CapabilityHandler for Echo {
    fn invoke(&self, input: Value) -> Result<Value, CapabilityDispatchError> {
        if input["fail"] == true {
            Err(CapabilityDispatchError::Execution(
                "SECRET should never be logged".into(),
            ))
        } else {
            Ok(input)
        }
    }
}

#[test]
fn handler_invocation_success_and_failure_are_audited_without_payloads() {
    let stream = Arc::new(InMemoryEventStream::new(8).unwrap());
    let registry = CapabilityHandlerRegistry::new().with_events(emitter(&stream));
    let id = registry.register(descriptor(), Arc::new(Echo)).unwrap();
    assert!(registry.lookup(&id).is_some());
    assert!(registry
        .dispatch(&id, json!({"operation":"read", "secret":"SECRET"}))
        .is_ok());
    assert!(registry
        .dispatch(&id, json!({"operation":"write", "fail":true}))
        .is_err());
    let snapshot = stream.snapshot().unwrap();
    let events: Vec<_> = snapshot
        .records()
        .iter()
        .map(|record| record.event())
        .collect();
    assert_eq!(events.len(), 2);
    assert!(
        matches!(events[0], CapabilityEvent::CapabilityInvoked { operation, handler_result: HandlerResult::Success, .. } if operation == "read")
    );
    assert!(matches!(
        events[1],
        CapabilityEvent::CapabilityInvoked {
            handler_result: HandlerResult::ExecutionFailed,
            ..
        }
    ));
    assert!(!serde_json::to_string(&events).unwrap().contains("SECRET"));
}

#[test]
fn visibility_mismatch_is_audited_without_changing_the_denial() {
    let stream = Arc::new(InMemoryEventStream::new(8).unwrap());
    let mut desc = descriptor();
    desc.visibility.workspace = Some("private".into());
    let tree = OwnershipTree::new();
    let requested = VisibilityScope::default();
    tree.register(
        desc.id.clone(),
        desc.owner.clone(),
        desc.lifetime,
        requested.clone(),
    )
    .unwrap();
    let policy = PolicyEngine::new(tree).with_events(emitter(&stream));
    assert_eq!(
        policy.resolve(&desc.id, &desc.owner, &requested, &desc),
        Err(crate::capability::policy::PolicyDenial::VisibilityMismatch)
    );
    assert_eq!(stream.snapshot().unwrap().records().len(), 1);
}

#[test]
fn failing_sink_does_not_block_activation_revocation_or_handler_result() {
    let stream = Arc::new(
        InMemoryEventStream::with_persistence(
            16,
            Some(Arc::new(Persistence(std::sync::Mutex::new(vec![]), true))),
        )
        .unwrap(),
    );
    let mut activation = observed(&stream, vec![]);
    activation.attach().unwrap();
    activation
        .activate(&ConstraintContext::at_hour(12).unwrap(), OwnerGeneration(1))
        .unwrap();
    activation.revoke().unwrap();
    let registry = CapabilityHandlerRegistry::new().with_events(emitter(&stream));
    let id = registry.register(descriptor(), Arc::new(Echo)).unwrap();
    assert_eq!(
        registry.dispatch(&id, json!({"value": 42})).unwrap(),
        json!({"value": 42})
    );
    assert!(registry.dispatch(&id, json!({"fail":true})).is_err());
    assert_eq!(stream.snapshot().unwrap().records().len(), 6);
}

#[test]
fn external_activation_hooks_and_event_hooks_both_observe_success() {
    struct Hook(std::cell::Cell<usize>);
    impl crate::capability::activation::ActivationHook for Hook {
        fn after_activation(&self, _: &ConstraintContext) {
            self.0.set(self.0.get() + 1);
        }
        fn after_revocation(&self) {
            self.0.set(self.0.get() + 1);
        }
    }
    let stream = Arc::new(InMemoryEventStream::new(8).unwrap());
    let mut activation = observed(&stream, vec![]);
    let hook = Hook(std::cell::Cell::new(0));
    activation.attach().unwrap();
    activation
        .activate_with_hook(
            &ConstraintContext::at_hour(12).unwrap(),
            OwnerGeneration(1),
            &hook,
        )
        .unwrap();
    activation.revoke_with_hook(&hook).unwrap();
    assert_eq!(hook.0.get(), 2);
    assert_eq!(stream.snapshot().unwrap().records().len(), 4);
}

#[test]
fn conditional_and_owner_generation_failures_are_observed() {
    let stream = Arc::new(InMemoryEventStream::new(8).unwrap());
    let mut activation = observed(&stream, vec![Constraint::condition("approved")]);
    activation.attach().unwrap();
    let context = ConstraintContext::at_hour(12).unwrap();
    assert!(activation.activate(&context, OwnerGeneration(1)).is_err());
    assert!(
        matches!(stream.snapshot().unwrap().records().last().unwrap().event(), CapabilityEvent::ConstraintViolation { constraint_type, .. } if constraint_type == "conditional")
    );
    assert!(activation.activate(&context, OwnerGeneration(2)).is_err());
    assert!(
        matches!(stream.snapshot().unwrap().records().last().unwrap().event(), CapabilityEvent::PolicyViolation { reason, .. } if reason == "generation_mismatch")
    );
}

#[test]
fn audited_zahir_observes_a_real_facade_refusal() {
    use crate::capability::facade::{Reference, ResolveError, Zahir};
    use crate::capability::zahir_facade::ZahirFacade;
    use crate::tools::registry::ToolHandlerRegistry;
    let stream = Arc::new(InMemoryEventStream::new(8).unwrap());
    let inner = ZahirFacade::new(ToolHandlerRegistry::new(), Arc::new(OwnershipTree::new()));
    let facade = AuditedZahir::new(inner, OwnerRef::Runtime, emitter(&stream));
    assert!(matches!(
        facade.resolve(Reference {
            id: id("missing"),
            visibility: VisibilityScope::default()
        }),
        Err(ResolveError::Unknown)
    ));
    let snapshot = stream.snapshot().unwrap();
    assert_eq!(snapshot.records().len(), 1);
    assert!(
        matches!(snapshot.records()[0].event(), CapabilityEvent::PolicyViolation { id: resource, reason } if resource == &id("missing") && reason == "Unknown")
    );
}

#[test]
fn sequence_exhaustion_does_not_change_the_buffer() {
    let stream = InMemoryEventStream::new(1).unwrap();
    stream.buffer.lock().unwrap().sequence = u64::MAX;
    assert!(matches!(
        stream.emit(OwnerRef::Runtime, revoked("one")),
        Err(EventError::SequenceExhausted)
    ));
    assert!(stream.snapshot().unwrap().records().is_empty());
}

#[test]
fn poisoned_event_storage_fails_closed_without_emission_panics() {
    let stream = Arc::new(InMemoryEventStream::new(1).unwrap());
    let writer = stream.clone();
    assert!(std::thread::spawn(move || {
        let _guard = writer.buffer.lock().unwrap();
        panic!("poison storage");
    })
    .join()
    .is_err());
    assert!(matches!(
        stream.emit(OwnerRef::Runtime, revoked("one")),
        Err(EventError::Poisoned)
    ));
    assert!(matches!(stream.snapshot(), Err(EventError::Poisoned)));
}

#[test]
fn failing_sink_preserves_the_original_policy_denial() {
    let stream = Arc::new(
        InMemoryEventStream::with_persistence(
            4,
            Some(Arc::new(Persistence(std::sync::Mutex::new(vec![]), true))),
        )
        .unwrap(),
    );
    let policy = PolicyEngine::new(OwnershipTree::new()).with_events(emitter(&stream));
    let desc = descriptor();
    assert_eq!(
        policy.resolve(&desc.id, &desc.owner, &desc.visibility, &desc),
        Err(crate::capability::policy::PolicyDenial::NotFound),
    );
    assert_eq!(stream.snapshot().unwrap().records().len(), 1);
}

#[test]
fn provider_activation_fault_is_failed_not_denied_in_compliance_report() {
    use crate::capability::audit::{AuditCollector, AuditQuery};
    use crate::capability::facade::{Reference, Zahir};
    use crate::capability::provider::{
        ProviderHandle, ProviderId, ProviderLifecycle, ProviderTransport,
    };
    use crate::capability::provider_registry::{
        ActivationError, ProviderActivator, ProviderRegistration, ProviderRegistry,
    };
    use crate::capability::zahir_facade::ZahirFacade;
    use crate::tools::registry::ToolHandlerRegistry;

    struct FailingActivator(ActivationError);
    impl ProviderActivator for FailingActivator {
        fn activate(&self) -> Result<(), ActivationError> {
            Err(self.0.clone())
        }
    }

    for (error, expected) in [
        (ActivationError::Failed, "failed"),
        (ActivationError::Rejected, "denied"),
        (ActivationError::NotVisible, "denied"),
        (ActivationError::Unknown, "denied"),
    ] {
        let stream = Arc::new(InMemoryEventStream::new(4).unwrap());
        let providers = ProviderRegistry::new();
        let desc = descriptor();
        let provider = ProviderHandle::new(ProviderLifecycle::new(
            ProviderId::new("test/audit-activation").unwrap(),
            ProviderTransport::InProcess,
        ));
        providers
            .register(
                ProviderRegistration::new(desc.clone(), provider)
                    .with_activator(Arc::new(FailingActivator(error.clone()))),
            )
            .unwrap();
        let inner = ZahirFacade::new_with_providers(
            ToolHandlerRegistry::new(),
            Arc::new(OwnershipTree::new()),
            providers,
        );
        let facade = AuditedZahir::new(inner, OwnerRef::Runtime, emitter(&stream));
        assert_eq!(
            facade.activate(Reference {
                id: desc.id.clone(),
                visibility: desc.visibility.clone()
            }),
            Err(error.clone())
        );
        let audit = AuditCollector::new(stream);
        let report = audit.report(&AuditQuery::default()).unwrap();
        assert_eq!(report.entries().len(), 1);
        assert_eq!(report.entries()[0].result(), expected);
        let json: Value = serde_json::from_str(&report.to_json().unwrap()).unwrap();
        assert_eq!(json["entries"][0]["result"], expected);
        if error == ActivationError::Failed {
            assert!(!matches!(
                report.entries()[0].event(),
                CapabilityEvent::PolicyViolation { .. }
            ));
            assert_eq!(json["entries"][0]["event_type"], "execution_failed");
            assert_eq!(json["entries"][0]["action"], "activate");
        } else {
            assert!(matches!(
                report.entries()[0].event(),
                CapabilityEvent::PolicyViolation { .. }
            ));
        }
    }
}
