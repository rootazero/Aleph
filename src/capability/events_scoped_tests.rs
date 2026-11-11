//! Same-source event transport tests; Gateway access remains a test adapter until Task7.
use crate::capability::audit::{AuditCollector, AuditError, AuditQuery};
use crate::capability::descriptor::CapabilityId;
use crate::capability::event_audience::{CapabilityEventAccess, EventAudience, TrustedEventScope};
use crate::capability::events::{
    AuditedZahir, CapabilityEvent, CapabilityEventEmitter, CapabilityEventRecord,
    CapabilityEventStream, CapabilityEventType, EventError, EventFilter, InMemoryEventStream,
};
use crate::capability::facade::{Scope, Zahir};
use crate::capability::ownership::{LifetimeScope, OwnerRef, OwnershipTree, VisibilityScope};
use crate::capability::policy_runtime::CapabilityCallContext;
use crate::gateway::event_scope::{scope_for_role, EventScopeGuard};
use crate::gateway::event_visibility::EventVisibilityIndex;
use crate::gateway::handlers::events::SubscriptionManager;
use crate::gateway::router::SessionKey;
use crate::gateway::server::connection::auth::wall_admits;
use crate::gateway::server::ConnectionState;
use crate::gateway::session_store::file_backend::{FileSessionStore, FileSessionStoreConfig};
use crate::gateway::session_store::SessionStore;
use crate::tools::registry::ToolHandlerRegistry;
use async_trait::async_trait;
use serde_json::json;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{Notify, RwLock};

fn id() -> CapabilityId {
    CapabilityId {
        namespace: "test/scoped-events".into(),
        name: "terminal".into(),
    }
}
fn revoked() -> CapabilityEvent {
    CapabilityEvent::CapabilityRevoked {
        id: id(),
        reason: "requested".into(),
    }
}
fn audience(
    user: Option<&str>,
    session: Option<&SessionKey>,
    incarnation: Option<&str>,
) -> EventAudience {
    EventAudience::from_call(
        &CapabilityCallContext::from_trusted_parts(
            OwnerRef::Runtime,
            user.map(str::to_owned),
            session.map(SessionKey::to_key_string),
            Some("workspace-a".into()),
            Some("producer-connection".into()),
            "correlation-a".into(),
        ),
        incarnation.map(str::to_owned),
    )
}

/// Uses existing connection identity/scope and SessionStore facts, not an allow-bit map.
/// The PTY topic gate is reused here; mounting a new capability topic is Task7.
struct GatewayAccess {
    connections: Arc<RwLock<HashMap<String, ConnectionState>>>,
    subscriptions: Arc<SubscriptionManager>,
    connection: String,
    store: Option<Arc<dyn SessionStore>>,
    incarnation: Option<String>,
}
#[async_trait]
impl CapabilityEventAccess for GatewayAccess {
    async fn admits(&self, record: &CapabilityEventRecord) -> bool {
        let (role, user, permissions) = {
            let connections = self.connections.read().await;
            let Some(connection) = connections.get(&self.connection) else {
                return false;
            };
            if connection.first_message {
                return false;
            }
            (
                connection.caller_role.clone(),
                connection.caller_user.clone(),
                connection.permissions.clone(),
            )
        };
        if !wall_admits(Some(&role), "")
            || !EventScopeGuard::default_rules().can_receive("pty.exit", &permissions)
            || !self
                .subscriptions
                .should_receive(&self.connection, "pty.exit", None)
                .await
        {
            return false;
        }
        let Some(store) = &self.store else {
            return false;
        };
        match record.audience() {
            EventAudience::OperatorOnly => {
                crate::tools::turn_context::role_is_operator(Some(&role))
            }
            EventAudience::Principal(principal) => {
                if Some(principal.user_id()) != user.as_deref()
                    || principal.source_incarnation() != self.incarnation.as_deref()
                    || principal.workspace() != Some("workspace-a")
                {
                    return false;
                }
                let Some(session) = principal.session_key() else {
                    return true;
                };
                let Some(key) = SessionKey::from_key_string(session) else {
                    return false;
                };
                // Current store presence matters even if an older visibility index cached it.
                if !matches!(store.get_metadata(&key).await, Ok(Some(_))) {
                    return false;
                }
                EventVisibilityIndex::new()
                    .event_admits_for(
                        "session.lifecycle.changed",
                        Some(&json!({"session_key": session})),
                        user.as_deref(),
                        Some(&role),
                        store,
                        None,
                    )
                    .await
            }
        }
    }
}
struct GatewayFixture {
    _temp: tempfile::TempDir,
    store: Arc<dyn SessionStore>,
    connections: Arc<RwLock<HashMap<String, ConnectionState>>>,
    subscriptions: Arc<SubscriptionManager>,
    session: SessionKey,
}
impl GatewayFixture {
    async fn new() -> Self {
        let temp = tempfile::TempDir::new().unwrap();
        let store: Arc<dyn SessionStore> = Arc::new(
            FileSessionStore::new(FileSessionStoreConfig {
                base_dir: temp.path().to_path_buf(),
                ..Default::default()
            })
            .unwrap(),
        );
        let session = SessionKey::main("task6-scoped");
        crate::scope::with_scope(
            Some(crate::scope::ScopeAttribution::personal("alice")),
            store.get_or_create(&session),
        )
        .await
        .unwrap();
        let connections = Arc::new(RwLock::new(HashMap::new()));
        for user in ["alice", "bob"] {
            let mut connection = ConnectionState::new("127.0.0.1".parse().unwrap(), true);
            connection.first_message = false;
            connection.caller_user = Some(user.into());
            connection.verified_user = Some(user.into());
            connection.caller_role = "operator".into();
            connection.permissions = scope_for_role("operator");
            connections.write().await.insert(user.into(), connection);
        }
        Self {
            _temp: temp,
            store,
            connections,
            subscriptions: Arc::new(SubscriptionManager::new()),
            session,
        }
    }
    fn scope(&self, connection: &str, incarnation: Option<&str>) -> TrustedEventScope {
        TrustedEventScope::from_access(Arc::new(GatewayAccess {
            connections: self.connections.clone(),
            subscriptions: self.subscriptions.clone(),
            connection: connection.into(),
            store: Some(self.store.clone()),
            incarnation: incarnation.map(str::to_owned),
        }))
    }
}

#[tokio::test]
async fn revoked_target_is_visible_but_demoted_subscriber_is_not() {
    let fixture = GatewayFixture::new().await;
    let stream = Arc::new(InMemoryEventStream::new(16).unwrap());
    let audit = AuditCollector::new(stream.clone());
    let tree = Arc::new(OwnershipTree::new());
    tree.register(
        id(),
        OwnerRef::Runtime,
        LifetimeScope::Runtime,
        VisibilityScope::default(),
    )
    .unwrap();
    let alice = fixture.scope("alice", Some("old-source"));
    let bob = fixture.scope("bob", Some("old-source"));
    let next_surface = fixture.scope("alice", Some("new-source"));
    let mut live = stream.subscribe_scoped(alice.clone(), EventFilter::default());
    let mut other_user = stream.subscribe_scoped(bob.clone(), EventFilter::default());
    let mut new_incarnation = stream.subscribe_scoped(next_surface.clone(), EventFilter::default());
    assert!(tree.revoke(&id()));
    assert!(tree.is_revoked(&id()));
    stream
        .emit_scoped(
            OwnerRef::Runtime,
            audience(Some("alice"), Some(&fixture.session), Some("old-source")),
            revoked(),
        )
        .unwrap();
    assert_eq!(live.recv().await.unwrap().event(), &revoked());
    assert_eq!(
        audit
            .query_scoped(&alice, &AuditQuery::default())
            .await
            .unwrap()
            .len(),
        1
    );
    assert!(
        audit
            .query_scoped(&bob, &AuditQuery::default())
            .await
            .unwrap()
            .is_empty(),
        "operator role must not cross creator audience"
    );
    assert!(
        audit
            .query_scoped(&next_surface, &AuditQuery::default())
            .await
            .unwrap()
            .is_empty(),
        "new incarnation cannot inherit old events"
    );
    let active_target = CapabilityId {
        name: "still-active".into(),
        ..id()
    };
    tree.register(
        active_target.clone(),
        OwnerRef::Runtime,
        LifetimeScope::Runtime,
        VisibilityScope::default(),
    )
    .unwrap();
    assert!(tree
        .resolve(&active_target, &VisibilityScope::default())
        .is_some());
    assert!(!tree.is_revoked(&active_target));
    {
        let mut connections = fixture.connections.write().await;
        let connection = connections.get_mut("alice").unwrap();
        connection.caller_role = "member".into();
        connection.permissions = scope_for_role("member");
    }
    assert!(audit
        .query_scoped(&alice, &AuditQuery::default())
        .await
        .unwrap()
        .is_empty());
    stream
        .emit_scoped(
            OwnerRef::Runtime,
            audience(Some("alice"), Some(&fixture.session), Some("old-source")),
            CapabilityEvent::PolicyViolation {
                id: active_target.clone(),
                reason: "denied".into(),
            },
        )
        .unwrap();
    assert!(!tree.is_revoked(&active_target));
    stream.close().unwrap();
    assert!(
        matches!(live.recv().await, Err(EventError::Closed)),
        "already subscribed receiver must recheck current role"
    );
    assert!(matches!(other_user.recv().await, Err(EventError::Closed)));
    assert!(matches!(
        new_incarnation.recv().await,
        Err(EventError::Closed)
    ));
    {
        let mut connections = fixture.connections.write().await;
        let connection = connections.get_mut("alice").unwrap();
        connection.caller_role = "operator".into();
        connection.permissions = scope_for_role("operator");
    }
    fixture
        .store
        .delete_session(&fixture.session)
        .await
        .unwrap();
    assert!(
        audit
            .query_scoped(&alice, &AuditQuery::default())
            .await
            .unwrap()
            .is_empty(),
        "withdrawn session fails closed"
    );
}

#[tokio::test]
async fn scoped_live_and_audit_share_exact_records() {
    let fixture = GatewayFixture::new().await;
    let stream = Arc::new(InMemoryEventStream::new(4).unwrap());
    let audit = AuditCollector::new(stream.clone());
    let scope = fixture.scope("alice", Some("source-a"));
    let mut live = stream.subscribe_scoped(scope.clone(), EventFilter::default());
    let observed = audience(Some("alice"), Some(&fixture.session), Some("source-a"));
    stream
        .emit_scoped(OwnerRef::Runtime, observed.clone(), revoked())
        .unwrap();
    let record = live.recv().await.unwrap();
    let logs = audit
        .query_scoped(&scope, &AuditQuery::default())
        .await
        .unwrap();
    assert_eq!(logs.len(), 1);
    assert_eq!(logs[0].sequence(), record.sequence());
    assert_eq!(logs[0].timestamp(), record.timestamp());
    assert_eq!(logs[0].audience(), record.audience());
    assert_eq!(logs[0].event(), record.event());
    assert_eq!(logs[0].actor(), record.actor());
    let encoded = serde_json::to_value(&record).unwrap();
    assert_eq!(
        encoded["audience"],
        json!({"kind":"principal","user_id":"alice","session_key": fixture.session.to_key_string(),"workspace":"workspace-a","source_incarnation":"source-a"})
    );
    assert_eq!(
        serde_json::to_value(&logs[0]).unwrap()["audience"],
        encoded["audience"]
    );
    let selected = EventFilter {
        event_type: Some(CapabilityEventType::PolicyViolation),
        ..Default::default()
    };
    let mut narrowed = stream.subscribe_scoped(scope.clone(), selected);
    stream
        .emit_scoped(OwnerRef::Runtime, observed.clone(), revoked())
        .unwrap();
    stream
        .emit_scoped(
            OwnerRef::Runtime,
            observed,
            CapabilityEvent::PolicyViolation {
                id: id(),
                reason: "denied".into(),
            },
        )
        .unwrap();
    let denial = narrowed.recv().await.unwrap();
    let query = AuditQuery {
        event_type: Some(CapabilityEventType::PolicyViolation),
        ..Default::default()
    };
    let filtered = audit.query_scoped(&scope, &query).await.unwrap();
    assert_eq!(filtered.len(), 1);
    assert_eq!(filtered[0].sequence(), denial.sequence());
    assert!(audit
        .query_scoped(
            &scope,
            &AuditQuery {
                from: Some(chrono::Utc::now()),
                through: Some(record.timestamp()),
                ..Default::default()
            }
        )
        .await
        .is_err());
    for _ in 0..2 {
        stream.emit(OwnerRef::Runtime, revoked()).unwrap();
    }
    assert!(matches!(
        audit.query_scoped(&scope, &query).await,
        Err(AuditError::IncompleteHistory { dropped: 1 })
    ));
    let json = serde_json::to_string(&record).unwrap();
    for absent in ["terminal_bytes", "raw_error", "text", "key_down", "secret"] {
        assert!(!json.contains(absent));
    }
}

struct All;
#[async_trait]
impl CapabilityEventAccess for All {
    async fn admits(&self, _: &CapabilityEventRecord) -> bool {
        true
    }
}
fn all() -> TrustedEventScope {
    TrustedEventScope::from_access(Arc::new(All))
}

#[tokio::test]
async fn producer_close_rejects_emit_and_receiver_drains_to_closed() {
    let stream = Arc::new(InMemoryEventStream::new(2).unwrap());
    let mut idle = stream.subscribe_scoped(all(), EventFilter::default());
    let mut idle_wait = Box::pin(idle.recv());
    assert!(futures_util::poll!(&mut idle_wait).is_pending());
    stream.close().unwrap();
    assert!(matches!(idle_wait.await, Err(EventError::Closed)));
    let retained_emitter = CapabilityEventEmitter::new(stream.clone());
    stream.close().unwrap();
    retained_emitter.emit(OwnerRef::Runtime, revoked());
    assert!(stream.snapshot().unwrap().records().is_empty());
    assert!(matches!(
        stream.emit(OwnerRef::Runtime, revoked()),
        Err(EventError::Closed)
    ));
    assert!(matches!(
        stream.subscribe(EventFilter::default()).recv().await,
        Err(EventError::Closed)
    ));
    let stream = Arc::new(InMemoryEventStream::new(2).unwrap());
    let mut queued = stream.subscribe_scoped(all(), EventFilter::default());
    let mut filtered = stream.subscribe_scoped(
        all(),
        EventFilter {
            event_type: Some(CapabilityEventType::Created),
            ..Default::default()
        },
    );
    for _ in 0..3 {
        stream.emit(OwnerRef::Runtime, revoked()).unwrap();
    }
    stream.close().unwrap();
    assert!(matches!(queued.recv().await, Err(EventError::Lagged(1))));
    assert_eq!(queued.recv().await.unwrap().sequence(), 2);
    assert_eq!(queued.recv().await.unwrap().sequence(), 3);
    assert!(matches!(queued.recv().await, Err(EventError::Closed)));
    assert!(matches!(filtered.recv().await, Err(EventError::Lagged(1))));
    assert!(matches!(filtered.recv().await, Err(EventError::Closed)));
    assert_eq!(stream.snapshot().unwrap().dropped(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn owned_receiver_is_send_and_atomic_close_race_never_loses_a_committed_record() {
    assert_eq!(
        tokio::runtime::Handle::current().runtime_flavor(),
        tokio::runtime::RuntimeFlavor::MultiThread,
        "emit/close race fixture requires a multi-thread runtime for cross-thread execution",
    );
    let stream = Arc::new(InMemoryEventStream::new(1).unwrap());
    let mut subscription = stream.subscribe_scoped(all(), EventFilter::default());
    let receiver = tokio::spawn(async move {
        let mut records = Vec::new();
        loop {
            match subscription.recv().await {
                Ok(record) => records.push(record),
                Err(EventError::Closed) => return records,
                Err(error) => panic!("unexpected receive error: {error}"),
            }
        }
    });
    let barrier = Arc::new(tokio::sync::Barrier::new(2));
    let producer = {
        let stream = stream.clone();
        let barrier = barrier.clone();
        tokio::spawn(async move {
            barrier.wait().await;
            stream.emit(OwnerRef::Runtime, revoked())
        })
    };
    barrier.wait().await;
    stream.close().unwrap();
    let emission = producer.await.unwrap();
    let received = receiver.await.unwrap();
    let snapshot = stream.snapshot().unwrap();
    match emission {
        Ok(()) => {
            assert_eq!(received.len(), 1);
            assert!(Arc::ptr_eq(&received[0], &snapshot.records()[0]));
        }
        Err(EventError::Closed) => {
            assert!(received.is_empty());
            assert!(snapshot.records().is_empty());
        }
        result => panic!("unexpected emission: {result:?}"),
    }
}

struct ReentrantAccess {
    stream: Arc<InMemoryEventStream>,
    entered: Notify,
    release: Notify,
}
#[async_trait]
impl CapabilityEventAccess for ReentrantAccess {
    async fn admits(&self, _: &CapabilityEventRecord) -> bool {
        self.stream.snapshot().unwrap();
        self.stream.close().unwrap();
        self.entered.notify_one();
        self.release.notified().await;
        true
    }
}
#[tokio::test]
async fn async_access_can_reenter_snapshot_and_close_without_a_buffer_lock() {
    let stream = Arc::new(InMemoryEventStream::new(2).unwrap());
    let access = Arc::new(ReentrantAccess {
        stream: stream.clone(),
        entered: Notify::new(),
        release: Notify::new(),
    });
    let scope = TrustedEventScope::from_access(access.clone());
    let mut receiver = stream.subscribe_scoped(scope.clone(), EventFilter::default());
    stream.emit(OwnerRef::Runtime, revoked()).unwrap();
    let worker = tokio::spawn(async move { receiver.recv().await });
    access.entered.notified().await;
    access.release.notify_one();
    assert_eq!(worker.await.unwrap().unwrap().sequence(), 1);
    let query = AuditCollector::new(stream);
    let criteria = AuditQuery::default();
    let future = query.query_scoped(&scope, &criteria);
    tokio::pin!(future);
    assert!(futures_util::poll!(&mut future).is_pending());
    access.entered.notified().await;
    access.release.notify_one();
    assert_eq!(future.await.unwrap().len(), 1);
}

#[tokio::test]
async fn missing_store_login_and_selection_never_widen_audience() {
    let fixture = GatewayFixture::new().await;
    let stream = Arc::new(InMemoryEventStream::new(8).unwrap());
    let audit = AuditCollector::new(stream.clone());
    stream
        .emit_scoped(
            OwnerRef::Runtime,
            audience(Some("alice"), Some(&fixture.session), Some("source-a")),
            revoked(),
        )
        .unwrap();
    let missing = TrustedEventScope::from_access(Arc::new(GatewayAccess {
        connections: fixture.connections.clone(),
        subscriptions: fixture.subscriptions.clone(),
        connection: "alice".into(),
        store: None,
        incarnation: Some("source-a".into()),
    }));
    assert!(audit
        .query_scoped(&missing, &AuditQuery::default())
        .await
        .unwrap()
        .is_empty());
    let scope = fixture.scope("alice", Some("source-a"));
    fixture
        .subscriptions
        .add_patterns("alice", vec!["different.topic".into()])
        .await;
    assert!(audit
        .query_scoped(&scope, &AuditQuery::default())
        .await
        .unwrap()
        .is_empty());
    fixture.subscriptions.remove_connection("alice").await;
    fixture
        .connections
        .write()
        .await
        .get_mut("alice")
        .unwrap()
        .first_message = true;
    assert!(audit
        .query_scoped(&scope, &AuditQuery::default())
        .await
        .unwrap()
        .is_empty());
    fixture.connections.write().await.remove("alice");
    assert!(audit
        .query_scoped(&scope, &AuditQuery::default())
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn operators_share_only_operator_records_not_each_others_creator_audience() {
    let fixture = GatewayFixture::new().await;
    let bob_session = SessionKey::main("task6-bob");
    crate::scope::with_scope(
        Some(crate::scope::ScopeAttribution::personal("bob")),
        fixture.store.get_or_create(&bob_session),
    )
    .await
    .unwrap();
    let stream = Arc::new(InMemoryEventStream::new(8).unwrap());
    let audit = AuditCollector::new(stream.clone());
    let alice = fixture.scope("alice", Some("source-a"));
    let bob = fixture.scope("bob", Some("source-a"));
    let mut alice_live = stream.subscribe_scoped(alice.clone(), EventFilter::default());
    let mut bob_live = stream.subscribe_scoped(bob.clone(), EventFilter::default());
    for captured in [
        audience(Some("alice"), Some(&fixture.session), Some("source-a")),
        audience(Some("bob"), Some(&bob_session), Some("source-a")),
        audience(None, None, Some("not-a-user")),
    ] {
        stream
            .emit_scoped(OwnerRef::Runtime, captured, revoked())
            .unwrap();
    }
    stream.close().unwrap();
    for (scope, receiver, expected) in [
        (&alice, &mut alice_live, [1, 3]),
        (&bob, &mut bob_live, [2, 3]),
    ] {
        for sequence in expected {
            assert_eq!(receiver.recv().await.unwrap().sequence(), sequence);
        }
        assert!(matches!(receiver.recv().await, Err(EventError::Closed)));
        let logs = audit
            .query_scoped(scope, &AuditQuery::default())
            .await
            .unwrap();
        assert_eq!(
            logs.iter().map(|log| log.sequence()).collect::<Vec<_>>(),
            expected
        );
        assert!(matches!(
            logs.last().unwrap().audience(),
            EventAudience::OperatorOnly
        ));
    }
}

#[test]
fn startup_composition_pin_names_the_same_hub_not_a_runtime_launch_claim() {
    let production = crate::utils::source_scan::code_keeping_literals(
        &crate::utils::source_scan::production_prefix(include_str!(
            "../bin/aleph-server/commands/start/mod.rs"
        )),
    );
    let normalized = production.split_whitespace().collect::<Vec<_>>().join(" ");
    let factory = "let capability_events = capability_init::build_capability_events();";
    let mount = "ProjectionHost::mount_with_runtime_and_events( (*tool_registry_phase2).clone(), Arc::clone(&ownership_tree), memory_runtime.clone(), special_registry, Arc::clone(&capability_policy), capability_events,";
    assert_eq!(normalized.matches(factory).count(), 1);
    assert_eq!(normalized.matches(mount).count(), 1);
    assert!(normalized.find(factory).unwrap() < normalized.find(mount).unwrap());
    let delegate = crate::utils::source_scan::code_keeping_literals(
        &crate::utils::source_scan::production_prefix(include_str!(
            "../bin/aleph-server/commands/start/capability_init.rs"
        )),
    );
    assert!(delegate
        .contains("Arc::new(alephcore::capability::events::InMemoryEventStream::default())"));
}

#[tokio::test]
async fn mounted_host_facade_and_audited_zahir_share_one_1024_record_hub() {
    use crate::capability::policy::PolicyEngine;
    use crate::capability::projection_host::ProjectionHost;
    use crate::capability::special::CapabilityHandlerRegistry;
    let temp = tempfile::TempDir::new().unwrap();
    let tree = Arc::new(OwnershipTree::new());
    let memory = crate::memory::runtime::MemoryRuntime::new(
        crate::memory::runtime::MemoryRuntimeConfig {
            metadata_path: temp.path().join("metadata.db"),
            note_memory_dir: temp.path().join("notes"),
        },
        tree.clone(),
    );
    memory.start().await.unwrap();
    let policy =
        Arc::new(PolicyEngine::with_shared_tree(tree.clone(), Default::default()).unwrap());
    let stream = Arc::new(InMemoryEventStream::default());
    let host = ProjectionHost::mount_with_runtime_and_events(
        ToolHandlerRegistry::new(),
        tree,
        memory.clone(),
        CapabilityHandlerRegistry::new(),
        policy,
        stream.clone(),
        false,
    );
    assert!(host.wait_until_ready().await);
    assert!(Arc::ptr_eq(&host.event_stream(), &stream));
    assert!(Arc::ptr_eq(&host.facade().event_stream(), &stream));
    let facade = host.facade();
    let metadata_before = facade.describe(Scope::default()).committed_cursor;
    let mut direct = facade
        .subscribe_events(all(), EventFilter::default())
        .unwrap();
    let decorated = AuditedZahir::new(
        facade,
        OwnerRef::Runtime,
        CapabilityEventEmitter::new(stream.clone()),
    );
    let mut audited = decorated
        .subscribe_events(all(), EventFilter::default())
        .unwrap();
    stream.emit(OwnerRef::Runtime, revoked()).unwrap();
    let record = direct.recv().await.unwrap();
    assert!(Arc::ptr_eq(&record, &audited.recv().await.unwrap()));
    assert!(Arc::ptr_eq(
        &record,
        &stream.snapshot().unwrap().records()[0]
    ));
    assert_eq!(
        host.audit_collector()
            .query_scoped(&all(), &AuditQuery::default())
            .await
            .unwrap()[0]
            .sequence(),
        record.sequence()
    );
    assert_eq!(
        decorated.describe(Scope::default()).committed_cursor,
        metadata_before
    );
    for _ in 0..1024 {
        stream.emit(OwnerRef::Runtime, revoked()).unwrap();
    }
    assert_eq!(stream.snapshot().unwrap().records().len(), 1024);
    assert_eq!(stream.snapshot().unwrap().dropped(), 1);
    stream.close().unwrap();
    memory.stop().await.unwrap();
    let outcome = host.close_and_await().await;
    assert!(outcome.source_joined && outcome.applier_joined);
    assert!(!outcome.source_failed && !outcome.applier_failed);
}
