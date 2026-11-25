use super::*;
use crate::capability::descriptor::{
    CapabilityDescriptor, CapabilityFacet, CapabilityFacets, CapabilityKind, CapabilityRevision,
    ProjectionMetadata, SchemaRef,
};
use crate::capability::ownership::{LifetimeScope, OwnershipTree, RunId};
use crate::capability::policy::PolicyEngine;
use serde_json::json;

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
fn provider_activation_fault_is_failed_not_denied() {
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

    for error in [
        ActivationError::Failed,
        ActivationError::Rejected,
        ActivationError::NotVisible,
        ActivationError::Unknown,
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
        let snapshot = stream.snapshot().unwrap();
        assert_eq!(snapshot.records().len(), 1);
        let event = snapshot.records()[0].event();
        if error == ActivationError::Failed {
            assert!(
                matches!(event, CapabilityEvent::ExecutionFailed { operation, .. } if operation == "activate")
            );
        } else {
            assert!(matches!(event, CapabilityEvent::PolicyViolation { .. }));
        }
    }
}
